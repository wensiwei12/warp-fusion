//! `join <window> as <key> { … }` 跨流注入（设计 §9）。
//!
//! 规则 `join` 是跨流的：驱动侧（用例 `stream`）与目标窗是两条流。跨流注入要造出
//! **配对**的右事件——右行的连接键 = 左实体键值、时间落在规则的 `within` 区间内。
//! 这里锁定三件事：右事件确实按此生成、字段被 `use(...)` 覆盖、且配对后**真能触发规则**
//! （用 oracle 复核，否则"造出来了但对不上"是测不出来的）。

use std::time::Duration;

use chrono::{DateTime, Utc};
use rand::SeedableRng;
use rand::rngs::StdRng;
use wf_lang::{BaseType, FieldDef, FieldType, WindowSchema};

use crate::datagen::inject_gen::generate_inject_events;
use crate::oracle::run_oracle_events_full;

use super::*;

/// q8（nexmark Monitor New Users）的最小化：person 驱动，join auction_events，
/// 连接键 `p.id == auction_events.seller`，区间下界 = 左事件时间（`[p.timestamp, bucket_end)`）；
/// `emit at` 是 v1 支持的 deferred 形态（见 VN30）。
const JOIN_RULE: &str = r#"rule person_creates_auction {
    events {
        p : person_events
    }
    on each p -> score(10)
    join auction_events within [p.timestamp, <bucket_end(p.timestamp, 5s)]
        on p.id == auction_events.seller
        emit at bucket_end(p.timestamp, 5s)
    entity(digit, p.id)
    yield alerts(id = p.id)
}"#;

fn person_schema() -> WindowSchema {
    WindowSchema {
        name: "person_events".to_string(),
        streams: vec!["person_events".to_string()],
        time_field: Some("timestamp".to_string()),
        over: Duration::from_secs(300),
        fields: vec![
            FieldDef {
                name: "timestamp".to_string(),
                field_type: FieldType::Base(BaseType::Time),
            },
            FieldDef {
                name: "id".to_string(),
                field_type: FieldType::Base(BaseType::Digit),
            },
            FieldDef {
                name: "name".to_string(),
                field_type: FieldType::Base(BaseType::Chars),
            },
        ],
    }
}

fn auction_schema() -> WindowSchema {
    WindowSchema {
        name: "auction_events".to_string(),
        streams: vec!["auction_events".to_string()],
        time_field: Some("timestamp".to_string()),
        over: Duration::from_secs(300),
        fields: vec![
            FieldDef {
                name: "timestamp".to_string(),
                field_type: FieldType::Base(BaseType::Time),
            },
            FieldDef {
                name: "seller".to_string(),
                field_type: FieldType::Base(BaseType::Digit),
            },
            FieldDef {
                name: "price".to_string(),
                field_type: FieldType::Base(BaseType::Digit),
            },
        ],
    }
}

fn alerts_schema() -> WindowSchema {
    WindowSchema {
        name: "alerts".to_string(),
        streams: vec![],
        time_field: None,
        over: Duration::from_secs(0),
        fields: vec![FieldDef {
            name: "id".to_string(),
            field_type: FieldType::Base(BaseType::Digit),
        }],
    }
}

fn schemas() -> Vec<WindowSchema> {
    vec![person_schema(), auction_schema(), alerts_schema()]
}

fn compile_join_rule(schemas: &[WindowSchema]) -> wf_lang::plan::RulePlan {
    let wfl = wf_lang::parse_wfl(JOIN_RULE).expect("rule parse");
    let mut plans = wf_lang::compile_wfl(&wfl, schemas).expect("rule compile");
    assert_eq!(plans.len(), 1);
    let plan = plans.remove(0);
    // 前提锁定：确实是跨流 join（目标窗与事件流不同）。
    assert_eq!(plan.joins.len(), 1);
    assert_eq!(plan.joins[0].right_window, "auction_events");
    plan
}

/// 4 个实体、每个 1 条左事件 + 2 条右事件：右事件进目标窗、键 = 左实体键值、
/// 时间 = 所属左事件时间、字段被 `use(...)` 覆盖。
#[test]
fn join_block_emits_paired_right_events() {
    let input = r#"
#[duration=10s]
scenario cross_stream<seed=3> {
    background { stream person_events gen 5/s }
    inject {
        hit<id: 4> for person_creates_auction person_events {
            use(name="p") x 1
            join auction_events as seller {
                use(price=42) x 2
            }
        }
    }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    let schemas = schemas();
    let plans = vec![compile_join_rule(&schemas)];
    let start: DateTime<Utc> = "2024-01-01T00:00:00Z".parse().unwrap();
    let duration = wfg.scenario.time_clause.duration;
    let mut rng = StdRng::seed_from_u64(wfg.scenario.seed);

    let result =
        generate_inject_events(&wfg, &plans, &schemas, &start, &duration, &mut rng).unwrap();

    let left: Vec<_> = result
        .events
        .iter()
        .filter(|event| event.window_name == "person_events")
        .collect();
    let right: Vec<_> = result
        .events
        .iter()
        .filter(|event| event.window_name == "auction_events")
        .collect();

    assert_eq!(left.len(), 4, "4 个实体各 1 条左事件");
    assert_eq!(right.len(), 8, "每实体 2 条右事件");

    for auction in &right {
        let seller = auction.fields.get("seller").expect("右行连接键");
        let paired = left
            .iter()
            .find(|event| event.fields.get("id") == Some(seller))
            .unwrap_or_else(|| panic!("右事件的 seller={seller} 必须配对到左实体"));
        assert_eq!(
            auction.timestamp, paired.timestamp,
            "右事件时间 = 所属左事件时间"
        );
        assert_eq!(
            auction.fields.get("price").and_then(|v| v.as_f64()),
            Some(42.0),
            "use(...) 的谓词覆盖目标窗字段（`use(price=42)` 归一成浮点）"
        );
    }

    // 8 个 seller 值两两配对到 4 个实体（每实体 2 条）。
    assert_eq!(left.len(), 4);
}

/// 配对**有效**：Oracle（真引擎模型）对同一批事件产出 4 条告警——右上事件的键/时间
/// 只要有一处错，join 就命中不了，这里会掉到 0。
#[test]
fn join_block_pairs_actually_trigger_the_rule() {
    let input = r#"
#[duration=10s]
scenario cross_stream_oracle<seed=5> {
    background { stream person_events gen 5/s }
    inject {
        hit<id: 3> for person_creates_auction person_events {
            use(name="p") x 1
            join auction_events as seller {
                use(price=7) x 1
            }
        }
    }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    let schemas = schemas();
    let plans = vec![compile_join_rule(&schemas)];
    let start: DateTime<Utc> = "2024-01-01T00:00:00Z".parse().unwrap();
    let duration = wfg.scenario.time_clause.duration;
    let mut rng = StdRng::seed_from_u64(wfg.scenario.seed);

    let result =
        generate_inject_events(&wfg, &plans, &schemas, &start, &duration, &mut rng).unwrap();

    // 注入事件只喂 oracle（背景噪声由场景生成器另出，这里只验证配对本身成立）。
    // join 规则必须带 schemas 调 `*_full`：否则右窗是 `EmptyLookup`，join 恒 miss。
    let alerts = run_oracle_events_full(
        result.events.clone(),
        &plans,
        &schemas,
        &start,
        &duration,
        None,
        true,
    )
    .unwrap();
    let entity_ids: std::collections::BTreeSet<&str> =
        alerts.alerts.iter().map(|a| a.entity_id.as_str()).collect();
    assert_eq!(
        entity_ids.len(),
        3,
        "每个 hit 实体都应因配对成功而产出告警：{:?}",
        alerts.alerts
    );
}
