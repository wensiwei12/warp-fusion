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

// ---------------------------------------------------------------------------
// snapshot（无 `within`）：q3/q20 形态
// ---------------------------------------------------------------------------

/// `snapshot` 点查：`join auction_events snapshot on b.auction == auction_events.id`。
/// 右事件必须**提前 1ms**——snapshot 在驱动事件被处理时查找右窗，同刻右行还没进去。
const SNAPSHOT_RULE: &str = r#"rule bid_expands_auction {
    events {
        b : bid_events
    }
    on each b -> score(10)
    join auction_events snapshot on b.auction == auction_events.id
    entity(digit, b.auction)
    yield alerts(id = b.auction)
}"#;

fn snapshot_schemas() -> Vec<WindowSchema> {
    vec![
        WindowSchema {
            name: "bid_events".to_string(),
            streams: vec!["bid_events".to_string()],
            time_field: Some("timestamp".to_string()),
            over: Duration::from_secs(300),
            fields: vec![
                FieldDef {
                    name: "timestamp".to_string(),
                    field_type: FieldType::Base(BaseType::Time),
                },
                FieldDef {
                    name: "auction".to_string(),
                    field_type: FieldType::Base(BaseType::Digit),
                },
                FieldDef {
                    name: "price".to_string(),
                    field_type: FieldType::Base(BaseType::Digit),
                },
            ],
        },
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
                    name: "id".to_string(),
                    field_type: FieldType::Base(BaseType::Digit),
                },
                FieldDef {
                    name: "category".to_string(),
                    field_type: FieldType::Base(BaseType::Digit),
                },
            ],
        },
        alerts_schema(),
    ]
}

/// snapshot 形态：右事件提前 1ms、键 = 左实体键值、字段被 `use(...)` 覆盖。
#[test]
fn snapshot_join_puts_right_event_earlier() {
    let input = r#"
#[duration=10s]
scenario snap<seed=11> {
    background { stream bid_events gen 5/s }
    inject {
        hit<auction: 3> for bid_expands_auction bid_events {
            use(price=5) x 1
            join auction_events as id {
                use(category=10) x 1
            }
        }
    }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    let schemas = snapshot_schemas();
    let wfl = wf_lang::parse_wfl(SNAPSHOT_RULE).expect("rule parse");
    let mut plans = wf_lang::compile_wfl(&wfl, &schemas).expect("rule compile");
    let plan = plans.remove(0);
    let start: DateTime<Utc> = "2024-01-01T00:00:00Z".parse().unwrap();
    let duration = wfg.scenario.time_clause.duration;
    let mut rng = StdRng::seed_from_u64(wfg.scenario.seed);

    let result = generate_inject_events(
        &wfg,
        std::slice::from_ref(&plan),
        &schemas,
        &start,
        &duration,
        &mut rng,
    )
    .unwrap();

    let bids: Vec<_> = result
        .events
        .iter()
        .filter(|e| e.window_name == "bid_events")
        .collect();
    let auctions: Vec<_> = result
        .events
        .iter()
        .filter(|e| e.window_name == "auction_events")
        .collect();
    assert_eq!(bids.len(), 3);
    assert_eq!(auctions.len(), 3);

    for auction in &auctions {
        let id = auction.fields.get("id").expect("右行连接键");
        let paired = bids
            .iter()
            .find(|b| b.fields.get("auction") == Some(id))
            .unwrap_or_else(|| panic!("右事件 id={id} 必须配对到左实体"));
        assert_eq!(
            paired.timestamp - auction.timestamp,
            chrono::Duration::milliseconds(1),
            "snapshot 形态：右事件应比左事件早 1ms（保证驱动事件处理时已可见，且毫秒精度的下游也看得出先后）"
        );
        assert_eq!(
            auction.fields.get("category").and_then(|v| v.as_f64()),
            Some(10.0),
            "use(category=10) 归一成浮点"
        );
    }
}

/// 配对**有效**：snapshot 形态下 oracle 也应产出 3 条告警（右行提前才可见）。
#[test]
fn snapshot_join_pairs_actually_trigger_the_rule() {
    let input = r#"
#[duration=10s]
scenario snap_oracle<seed=13> {
    background { stream bid_events gen 5/s }
    inject {
        hit<auction: 3> for bid_expands_auction bid_events {
            use(price=5) x 1
            join auction_events as id {
                use(category=10) x 1
            }
        }
    }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    let schemas = snapshot_schemas();
    let wfl = wf_lang::parse_wfl(SNAPSHOT_RULE).expect("rule parse");
    let mut plans = wf_lang::compile_wfl(&wfl, &schemas).expect("rule compile");
    let plan = plans.remove(0);
    let start: DateTime<Utc> = "2024-01-01T00:00:00Z".parse().unwrap();
    let duration = wfg.scenario.time_clause.duration;
    let mut rng = StdRng::seed_from_u64(wfg.scenario.seed);

    let result = generate_inject_events(
        &wfg,
        std::slice::from_ref(&plan),
        &schemas,
        &start,
        &duration,
        &mut rng,
    )
    .unwrap();

    let alerts = run_oracle_events_full(
        result.events.clone(),
        &[plan],
        &schemas,
        &start,
        &duration,
        None,
        true,
    )
    .unwrap();
    let ids: std::collections::BTreeSet<&str> =
        alerts.alerts.iter().map(|a| a.entity_id.as_str()).collect();
    assert_eq!(
        ids.len(),
        3,
        "每个 hit 实体都应因 snapshot 配对命中而产出告警：{:?}",
        alerts.alerts
    );
}

// ---------------------------------------------------------------------------
// join-then-key（nexmark q6 形态）：match 键取自 join 侧
// ---------------------------------------------------------------------------

/// `match<seller:10m>` 的键 `seller` 不在驱动事件（bid）上，而在 join 侧（auction）：
/// 引擎先 snapshot join 拿到 `seller`，再按它分组。实体仍是驱动侧的 `b.auction`。
const JOIN_THEN_KEY_RULE: &str = r#"rule avg_price_by_seller {
    events {
        b : bid_events
    }
    match<seller:10m> {
        on event { b.price | avg >= 200; }
    } -> score(20)
    join auction_events snapshot on b.auction == auction_events.id
    entity(digit, b.auction)
    yield alerts(id = b.auction)
}"#;

fn join_then_key_schemas() -> Vec<WindowSchema> {
    let mut schemas = snapshot_schemas();
    // auction_events 补上 `seller`（join 侧的分组键）。
    let auction = schemas
        .iter_mut()
        .find(|s| s.name == "auction_events")
        .expect("auction_events");
    auction.fields.push(FieldDef {
        name: "seller".to_string(),
        field_type: FieldType::Base(BaseType::Digit),
    });
    schemas
}

/// join 侧键（`seller`）必须写到**右行**上，且与背景噪声（digit `0..100_000`）所在的带分开
/// ——否则注入实例会和背景事件并到同一条窗口实例，阈值口径被稀释。
#[test]
fn join_then_key_writes_join_side_key_on_the_right_row() {
    let input = r#"
#[duration=10s]
scenario jtk<seed=17> {
    background { stream bid_events gen 5/s }
    inject {
        hit<auction: 3> for avg_price_by_seller bid_events {
            use(price=250) x 2
            join auction_events as id {
                use({}) x 1
            }
        }
    }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    let schemas = join_then_key_schemas();
    let wfl = wf_lang::parse_wfl(JOIN_THEN_KEY_RULE).expect("rule parse");
    let mut plans = wf_lang::compile_wfl(&wfl, &schemas).expect("rule compile");
    let plan = plans.remove(0);
    // 前提锁定：规则确实是 join-then-key（键 `seller` 不在 bid 上）。
    let key_join = plan
        .match_plan
        .key_join
        .as_ref()
        .expect("规则应是 join-then-key（key_join 有值）");
    assert_eq!(key_join.right_field, "seller");
    assert_eq!(key_join.right_window, "auction_events");

    let start: DateTime<Utc> = "2024-01-01T00:00:00Z".parse().unwrap();
    let duration = wfg.scenario.time_clause.duration;
    let mut rng = StdRng::seed_from_u64(wfg.scenario.seed);
    let result = generate_inject_events(
        &wfg,
        std::slice::from_ref(&plan),
        &schemas,
        &start,
        &duration,
        &mut rng,
    )
    .unwrap();

    let auctions: Vec<_> = result
        .events
        .iter()
        .filter(|e| e.window_name == "auction_events")
        .collect();
    let bids: Vec<_> = result
        .events
        .iter()
        .filter(|e| e.window_name == "bid_events")
        .collect();
    // `x 2` = 每实体 2 条左事件；`join` 块按**左事件**逐条补发，因此右事件也是 6 条。
    assert_eq!(bids.len(), 6);
    assert_eq!(auctions.len(), 6);

    let mut sellers = std::collections::BTreeSet::new();
    for auction in &auctions {
        let seller = auction
            .fields
            .get("seller")
            .and_then(|v| v.as_i64())
            .expect("右行必须带 join 侧键 seller");
        assert!(
            seller >= 1 << 22,
            "join 侧键值应落在与背景噪声分开的带里（≥ 1<<22），实际 {seller}"
        );
        sellers.insert(seller);
        // 连接键：右行 id = 驱动侧 auction（两侧指向同一实体）
        assert!(
            bids.iter()
                .any(|b| b.fields.get("auction") == auction.fields.get("id")),
            "右行 id 必须与某条驱动 bid 的 auction 相同"
        );
    }
    assert_eq!(
        sellers.len(),
        3,
        "每个实体一个独立的 join 侧键值（实例不合并）"
    );
}

/// join-then-key 的配对**有效**：oracle 应产出 3 条告警（引擎按 join 侧键分组）。
#[test]
fn join_then_key_pairs_actually_trigger_the_rule() {
    let input = r#"
#[duration=10s]
scenario jtk_oracle<seed=19> {
    background { stream bid_events gen 5/s }
    inject {
        hit<auction: 3> for avg_price_by_seller bid_events {
            use(price=250) x 2
            join auction_events as id {
                use({}) x 1
            }
        }
    }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    let schemas = join_then_key_schemas();
    let wfl = wf_lang::parse_wfl(JOIN_THEN_KEY_RULE).expect("rule parse");
    let mut plans = wf_lang::compile_wfl(&wfl, &schemas).expect("rule compile");
    let plan = plans.remove(0);
    let start: DateTime<Utc> = "2024-01-01T00:00:00Z".parse().unwrap();
    let duration = wfg.scenario.time_clause.duration;
    let mut rng = StdRng::seed_from_u64(wfg.scenario.seed);
    let result = generate_inject_events(
        &wfg,
        std::slice::from_ref(&plan),
        &schemas,
        &start,
        &duration,
        &mut rng,
    )
    .unwrap();

    let alerts = run_oracle_events_full(
        result.events.clone(),
        &[plan],
        &schemas,
        &start,
        &duration,
        None,
        true,
    )
    .unwrap();
    let ids: std::collections::BTreeSet<&str> =
        alerts.alerts.iter().map(|a| a.entity_id.as_str()).collect();
    assert_eq!(
        ids.len(),
        3,
        "每个 hit 实体都应因 join-then-key 配对命中而产出告警：{:?}",
        alerts.alerts
    );
}

// ---------------------------------------------------------------------------
// 轮 3 / 轮 5：断言口径、确定性与不变量
// ---------------------------------------------------------------------------

/// 阈值 3 的 **snapshot** join 规则：给「miss + join」一个**语义上成立**的形状——1 条驱动
/// 事件达不到阈值，必然不报警（与 join 是否命中无关）。
///
/// 注意：引擎的 deferred（`emit at`）目前只支持 `on each` 驱动形态，`match` + deferred 会在
/// 编译期报错，所以这里用 snapshot。（设计 §9.4 已记该边界。）
const THRESHOLD3_RULE: &str = r#"rule b3 {
    events {
        b : bid_events
    }
    match<auction: 5m> {
        on event { b | count >= 3; }
    } -> score(10)
    join auction_events snapshot on b.auction == auction_events.id
    entity(digit, b.auction)
    yield alerts(id = b.auction)
}"#;

fn fingerprint(events: &[crate::datagen::stream_gen::GenEvent]) -> Vec<String> {
    events
        .iter()
        .map(|e| {
            format!(
                "{}|{}|{}|{:?}",
                e.window_name, e.timestamp, e.stream_name, e.fields
            )
        })
        .collect()
}

/// `miss` 用例同样支持 `join` 块（`non_hit.rs` 那条路径）：右事件按左事件补发，
/// 且整批事件**不触发规则**（阈值 3 > 1 条驱动事件）——INJ2 的口径不被 join 破坏。
#[test]
fn miss_case_with_join_block_emits_right_events() {
    let input = r#"
#[duration=10s]
scenario miss_join<seed=23> {
    background { stream bid_events gen 5/s }
    inject {
        miss<auction: 2> for b3 bid_events {
            use(price=3) x 1
            join auction_events as id {
                use(category=10) x 2
            }
        }
    }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    let schemas = snapshot_schemas();
    let wfl = wf_lang::parse_wfl(THRESHOLD3_RULE).expect("rule parse");
    let mut plans = wf_lang::compile_wfl(&wfl, &schemas).expect("rule compile");
    let plan = plans.remove(0);
    let start: DateTime<Utc> = "2024-01-01T00:00:00Z".parse().unwrap();
    let duration = wfg.scenario.time_clause.duration;
    let mut rng = StdRng::seed_from_u64(wfg.scenario.seed);

    let result = generate_inject_events(
        &wfg,
        std::slice::from_ref(&plan),
        &schemas,
        &start,
        &duration,
        &mut rng,
    )
    .unwrap();

    let left: Vec<_> = result
        .events
        .iter()
        .filter(|e| e.window_name == "bid_events")
        .collect();
    let right: Vec<_> = result
        .events
        .iter()
        .filter(|e| e.window_name == "auction_events")
        .collect();
    assert_eq!(left.len(), 2, "2 个 miss 实体各 1 条驱动事件");
    assert_eq!(right.len(), 4, "每条驱动事件补 2 条右事件");
    for auction in &right {
        let id = auction.fields.get("id").expect("右行连接键");
        assert!(
            left.iter().any(|b| b.fields.get("auction") == Some(id)),
            "右行 id 必须配对到某个驱动实体"
        );
    }

    // `miss` 的口径：整批（含右事件）不触发规则。
    let alerts = run_oracle_events_full(
        result.events.clone(),
        &[plan],
        &schemas,
        &start,
        &duration,
        None,
        true,
    )
    .unwrap();
    assert!(
        alerts.alerts.is_empty(),
        "miss 用例不应产出告警: {:?}",
        alerts.alerts
    );
}

/// 注入生成是**确定性**的：同 seed 两次生成逐条一致（含 join 右事件的键与时间）。
#[test]
fn join_generation_is_deterministic() {
    let input = r#"
#[duration=10s]
scenario det_join<seed=29> {
    background { stream person_events gen 5/s }
    inject {
        hit<id: 3> for person_creates_auction person_events {
            use(name="p") x 1
            join auction_events as seller { use(price=7) x 2 }
        }
    }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    let schemas = schemas();
    let plans = vec![compile_join_rule(&schemas)];
    let start: DateTime<Utc> = "2024-01-01T00:00:00Z".parse().unwrap();
    let duration = wfg.scenario.time_clause.duration;

    let run = || {
        let mut rng = StdRng::seed_from_u64(wfg.scenario.seed);
        generate_inject_events(&wfg, &plans, &schemas, &start, &duration, &mut rng).unwrap()
    };
    let a = fingerprint(&run().events);
    let b = fingerprint(&run().events);
    assert_eq!(a, b, "同 seed 两次生成应逐条一致");
    assert!(!a.is_empty());
}

/// 值域不变量（**INJ1/INJ2 的前提**）：驱动侧连接键值（实体段，低位）与 join 侧键值
/// （`1<<22` 起的分离带）互不重叠。
#[test]
fn join_side_key_band_is_disjoint_from_driver_entity_segment() {
    let input = r#"
#[duration=10s]
scenario disjoint<seed=31> {
    background { stream bid_events gen 5/s }
    inject {
        hit<auction: 3> for avg_price_by_seller bid_events {
            use(price=250) x 1
            join auction_events as id {
                use({}) x 1
            }
        }
    }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    let schemas = join_then_key_schemas();
    let wfl = wf_lang::parse_wfl(JOIN_THEN_KEY_RULE).expect("rule parse");
    let mut plans = wf_lang::compile_wfl(&wfl, &schemas).expect("rule compile");
    let plan = plans.remove(0);
    let start: DateTime<Utc> = "2024-01-01T00:00:00Z".parse().unwrap();
    let duration = wfg.scenario.time_clause.duration;
    let mut rng = StdRng::seed_from_u64(wfg.scenario.seed);

    let result =
        generate_inject_events(&wfg, &[plan], &schemas, &start, &duration, &mut rng).unwrap();

    let driver_values: std::collections::BTreeSet<i64> = result
        .events
        .iter()
        .filter(|e| e.window_name == "bid_events")
        .filter_map(|e| e.fields.get("auction").and_then(|v| v.as_i64()))
        .collect();
    let join_side_values: std::collections::BTreeSet<i64> = result
        .events
        .iter()
        .filter(|e| e.window_name == "auction_events")
        .filter_map(|e| e.fields.get("seller").and_then(|v| v.as_i64()))
        .collect();

    assert!(!driver_values.is_empty() && !join_side_values.is_empty());
    assert!(
        driver_values.iter().all(|v| *v < (1 << 22)),
        "驱动实体值应在低位实体段：{driver_values:?}"
    );
    assert!(
        join_side_values.iter().all(|v| *v >= (1 << 22)),
        "join 侧键值应在分离带：{join_side_values:?}"
    );
    assert!(
        driver_values
            .intersection(&join_side_values)
            .next()
            .is_none(),
        "两段必须不重叠（否则注入实例会与背景/其他实例合并）"
    );
}
