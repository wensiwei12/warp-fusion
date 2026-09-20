//! 生成器写进事件的**实体键字段**口径（设计 §3.7 / §9.5）：
//!
//! 1. 显式 `key { … }` 映射 → 写**来源字段**（引擎按来源字段取值，写逻辑名会落在一个
//!    schema 里不存在的列上、被静默丢弃）；
//! 2. `entity(...)` 的单一字段**总是**写（不写就无法与告警 `entity_id` 对齐：断言真空）；
//! 3. join 驱动侧连接键镜像成实体标识值（两侧同值，连接条件才成立）；
//! 4. 注入占用的数值键值域由背景**避让**（否则背景噪声会替负样本把阈值顶过）。
//!
//! 这四条都是“写错也不报错”的地方，所以每条都用**引擎可观测的后果**来钉
//! （数据里那个字段的值 / 配对事件 / oracle 是否真报警），而不是只断言内部字段列表。

use std::time::Duration;

use rand::SeedableRng;
use rand::rngs::StdRng;
use wf_lang::{BaseType, FieldDef, FieldType, WindowSchema};

use crate::datagen::inject_gen::generate_inject_events;
use crate::inject_assert::assert_inject_modes;
use crate::oracle::run_oracle_events_full;

use super::*;

fn digit(name: &str) -> FieldDef {
    FieldDef {
        name: name.to_string(),
        field_type: FieldType::Base(BaseType::Digit),
    }
}

fn time(name: &str) -> FieldDef {
    FieldDef {
        name: name.to_string(),
        field_type: FieldType::Base(BaseType::Time),
    }
}

fn window(name: &str, streams: &[&str], fields: Vec<FieldDef>) -> WindowSchema {
    // 只有真的有时刻列时才声明 `time_field`（产出窗没有时刻列）。
    let time_field = fields
        .iter()
        .any(|field| field.name == "timestamp")
        .then(|| "timestamp".to_string());
    WindowSchema {
        name: name.to_string(),
        streams: streams.iter().map(|s| s.to_string()).collect(),
        time_field,
        over: Duration::from_secs(300),
        fields,
    }
}

/// 显式 `key { user = e.sip }`：生成器必须写 **source 字段 `sip`**。
///
/// 引擎按 `key_map` 的来源字段取值（`wf-cep::extract_key`：先查 (逻辑名, 本别名) 的
/// `source_field`，找不到才回退逻辑名字段）。写逻辑名 `user` 会落在一个 schema 里
/// 不存在的列上——`build_event_fields_with_predicates` 按 schema 逐字段套覆盖，
/// 那条覆盖**静默丢弃**：数据里 `sip` 还是随机值，注入实体根本没被注入，
/// 而 `record_entity` 只好把它计成 `unasserted`（断言真空）。
#[test]
fn key_mapping_rule_writes_the_source_field() {
    let schemas = vec![
        window(
            "login_events",
            &["login_events"],
            vec![time("timestamp"), digit("attempts")],
        ),
        window("out", &[], vec![digit("id")]),
    ];
    // sip 是 Ip 在下面单独补（window() 只给 digit/time 提供便捷构造）。
    let mut schemas = schemas;
    schemas[0].fields.insert(
        1,
        FieldDef {
            name: "sip".to_string(),
            field_type: FieldType::Base(BaseType::Ip),
        },
    );

    let rule = r#"rule keymap_rule {
        events { e : login_events }
        match<:5m> {
            key { user = e.sip; }
            on event { e | count >= 1; }
        } -> score(10.0)
        entity(ip, e.sip)
        yield out (id = 1)
    }"#;
    let wfl = wf_lang::parse_wfl(rule).expect("rule parse");
    let mut plans = wf_lang::compile_wfl(&wfl, &schemas).expect("rule compile");
    let plan = plans.remove(0);
    // 前提锁定：key 是**逻辑名**、来源字段另存 key_map。
    assert_eq!(plan.match_plan.key_map.as_ref().map(Vec::len), Some(1));

    let input = r#"
#[duration=10s]
scenario keymap<seed=1> {
    background { stream login_events gen 5/s }
    inject {
        hit<1> for keymap_rule login_events {
            use(attempts=500) x 1
        }
    }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    let start = "2024-01-01T00:00:00Z".parse().unwrap();
    let mut rng = StdRng::seed_from_u64(1);
    let result = generate_inject_events(
        &wfg,
        &[plan],
        &schemas,
        &start,
        &wfg.scenario.time_clause.duration,
        &mut rng,
    )
    .unwrap();

    assert!(
        result.unasserted_entities.is_empty(),
        "实体标识字段必须被写进事件：{:?}",
        result.unasserted_entities
    );
    let entity = &result.entity_keys[0];
    assert_eq!(entity.field, "sip");
    assert_eq!(entity.value, serde_json::json!("10.0.0.0"));
    // 数据里确实写着来源字段的值（不是随机 sip、也没有逻辑名 `user` 这一列）。
    let event = result
        .events
        .iter()
        .find(|event| event.window_name == "login_events")
        .expect("inject event");
    assert_eq!(
        event.fields.get("sip"),
        Some(&serde_json::json!("10.0.0.0"))
    );
    assert_eq!(event.fields.get("user"), None);
}

/// join-then-key（`match<seller>` 取自 join 侧、实体是驱动侧 `b.auction`）的
/// **推断**形态（用例头不写实体字段）：三个后果一次钉住——
/// 驱动事件写 `auction`、配对右行写同一个值、oracle 真报出这条 hit。
///
/// 历史实现在 `keys` 非空时返回 `None`（“多 key = 实体是 key 元组，不代它”），于是
/// `auction` 只用随机值：join 对不上 → hit 根本不开火，而该实体被静默计入 `unasserted`。
#[test]
fn join_then_key_inferred_entity_field_is_injected_and_pairs() {
    let schemas = vec![
        window(
            "bid_events",
            &["bid_events"],
            vec![time("timestamp"), digit("auction"), digit("price")],
        ),
        window(
            "auction_events",
            &["auction_events"],
            vec![time("timestamp"), digit("id"), digit("seller")],
        ),
        window("out", &[], vec![digit("id")]),
    ];
    let rule = r#"rule jtk {
        events { b : bid_events }
        match<seller:10m> {
            on event { b | count >= 1; }
        } -> score(10.0)
        join auction_events snapshot on b.auction == auction_events.id
        entity(digit, b.auction)
        yield out (id = b.auction)
    }"#;
    let wfl = wf_lang::parse_wfl(rule).expect("rule parse");
    let mut plans = wf_lang::compile_wfl(&wfl, &schemas).expect("rule compile");
    let plan = plans.remove(0);
    let plan_for_oracle = plan.clone();

    let input = r#"
#[duration=10s]
scenario jtk<seed=5> {
    background { stream bid_events gen 1/s }
    inject {
        hit<1> for jtk bid_events {
            use(price=250) x 1
            join auction_events as id { use({}) x 1 }
        }
    }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    let start = "2024-01-01T00:00:00Z".parse().unwrap();
    let mut rng = StdRng::seed_from_u64(5);
    let result = generate_inject_events(
        &wfg,
        &[plan],
        &schemas,
        &start,
        &wfg.scenario.time_clause.duration,
        &mut rng,
    )
    .unwrap();

    assert!(
        result.unasserted_entities.is_empty(),
        "join-then-key 的实体标识字段必须被写进事件：{:?}",
        result.unasserted_entities
    );
    let entity = &result.entity_keys[0];
    assert_eq!(entity.field, "auction");
    let entity_value = entity.value.clone();
    // 实体值取自注入用的**低位带**（`entity_base + i`），不是背景随机值。
    assert!(
        entity_value.as_u64().is_some_and(|value| value < 8),
        "实体值应在注入值带内: {entity_value}"
    );

    let bid = result
        .events
        .iter()
        .find(|event| event.window_name == "bid_events")
        .expect("driver event");
    assert_eq!(bid.fields.get("auction"), Some(&entity_value));
    let right = result
        .events
        .iter()
        .find(|event| event.window_name == "auction_events")
        .expect("paired right event");
    // 连接键两侧同值：驱动侧 `b.auction` → 配对右行 `id` 必须是同一个值。
    assert_eq!(right.fields.get("id"), Some(&entity_value));

    // 场景边界：snapshot 右行前挪 1ms，首簇左事件正好落在场景起点——不能跑到起点之前。
    for event in &result.events {
        assert!(
            event.timestamp >= start,
            "注入事件不能早于场景起点：{} < {start}（{:?}）",
            event.timestamp,
            event.fields
        );
    }

    // INJ1：这条 hit 真的报出来了（不是“造出来了但对不上”）。
    let oracle = run_oracle_events_full(
        result.events.clone(),
        &[plan_for_oracle],
        &schemas,
        &start,
        &wfg.scenario.time_clause.duration,
        None,
        true,
    )
    .unwrap();
    assert_eq!(
        assert_inject_modes(&result.entity_keys, &oracle.alerts).unwrap(),
        1,
        "hit 实体必须报出告警（join 配对成立）"
    );
}

/// 注入占用的数值键值域由**背景避让**：背景 `digit` 字段不再取到注入实体的键值。
///
/// 撞值会让规则把背景事件算到注入实体头上（阈值被噪声顶过、否定步骤被噪声满足），
/// INJ2「near_miss / miss 必不报警」于是变成概率性的——q5 实测踩过，q6 语料靠
/// `entity … zipf(...)` 池规避。本用例刻意**不声明池**，验证生成器自己兜住。
#[test]
fn background_values_avoid_the_injected_key_band() {
    let schemas = vec![
        window(
            "bid_events",
            &["bid_events"],
            vec![time("timestamp"), digit("auction"), digit("price")],
        ),
        window("out", &[], vec![digit("id")]),
    ];
    let rule = r#"rule passthrough {
        events { b : bid_events }
        on each b -> score(10.0)
        entity(digit, b.auction)
        yield out (id = b.auction)
    }"#;
    let wfl = wf_lang::parse_wfl(rule).expect("rule parse");
    let mut plans = wf_lang::compile_wfl(&wfl, &schemas).expect("rule compile");
    let plan = plans.remove(0);

    // 300 个实体 → 注入占用 [0, 300)；背景量级 50k 事件 → 不避让时必然撞值。
    let input = r#"
#[duration=10s]
scenario bg_avoid<seed=7> {
    background { stream bid_events gen 5000/s }
    inject {
        hit<300> for passthrough bid_events {
            use(price=1) x 1
        }
    }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    let result = generate(&wfg, &schemas, &[plan]).unwrap();

    let injected = 300_u64;
    let low: u64 = result
        .events
        .iter()
        .filter(|event| {
            event
                .fields
                .get("auction")
                .and_then(serde_json::Value::as_u64)
                .is_some_and(|value| value < injected)
        })
        .count() as u64;
    assert_eq!(
        low, injected,
        "低于注入值带的背景事件必须为 0（只允许 {injected} 条注入事件）"
    );
}
