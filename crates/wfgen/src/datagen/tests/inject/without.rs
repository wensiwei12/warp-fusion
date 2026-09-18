//! `without(...)` 构造约束（设计 §3.8）。
//!
//! 带否定步骤（`on event seq { has scan; not has login; has scan; }`）的规则，
//! 只注入正向事件是不够的：该实体的窗口里只要落进**一条**匹配 `not` 步骤的背景噪声，
//! 规则就不会触发。`without(...)` 就是这条"该实体窗口内不得出现匹配事件"的构造声明，
//! 生成期把它落成对背景噪声的剔除（[`WithoutGuard`]）。

use std::collections::HashSet;
use std::time::Duration;

use chrono::{DateTime, Utc};
use rand::SeedableRng;
use rand::rngs::StdRng;

use crate::datagen::inject_gen::{WithoutGuard, generate_inject_events};
use crate::datagen::stream_gen::GenEvent;
use crate::inject_assert::assert_inject_modes;
use crate::oracle::run_oracle;

use super::*;

/// `scan → (不含 login) → scan`：第二条 `not has login` 是把"窗口里混进成功登录"
/// 变成"不触发"的那一步。
const NOT_STEP_RULE: &str = r#"rule probe_rule {
    events {
        scan  : LoginWindow && success == false
        login : LoginWindow && success == true
    }
    match<src_ip : 5s> {
        on event seq {
            has scan;
            not has login;
            has scan;
        }
    } -> score(1)
    entity(ip, scan.src_ip)
    yield alerts()
}"#;

fn make_alerts_schema() -> WindowSchema {
    WindowSchema {
        name: "alerts".to_string(),
        streams: vec![],
        time_field: None,
        over: Duration::from_secs(300),
        fields: vec![],
    }
}

fn schemas() -> Vec<WindowSchema> {
    vec![make_login_schema(), make_alerts_schema()]
}

/// 用真实 WFL 编译器产出规则计划：`event_steps` 里不含 `not` 步骤（设计 §4.1 VN24），
/// 所以两个 `use` 组都落在 `scan` 上。
fn compile_not_step_rule(schemas: &[WindowSchema]) -> RulePlan {
    let wfl = wf_lang::parse_wfl(NOT_STEP_RULE).expect("rule parse");
    let mut plans = wf_lang::compile_wfl(&wfl, schemas).expect("rule compile");
    assert_eq!(plans.len(), 1);
    let plan = plans.remove(0);

    // 前提锁定：链里确实有否定步骤（且它不在 event_steps 里）——否则上面两个用例
    // 就退化成“普通多步规则”，测不到 `without` 的意义。
    let seq = plan.match_plan.seq.as_ref().expect("链形态应有 SeqPlan");
    assert_eq!(seq.steps.iter().filter(|step| step.neg).count(), 1);
    assert_eq!(plan.match_plan.event_steps.len(), 2);
    plan
}

// ---------------------------------------------------------------------------
// 执行清单（注入侧）
// ---------------------------------------------------------------------------

/// 每个实体一条 guard：窗口起点 = 该实体**首条注入事件**，窗长 = `within`。
#[test]
fn without_guards_cover_every_entity_window() {
    let input = r#"
#[duration=10s]
scenario guards<seed=7> {
    background { stream LoginWindow gen 20/s }
    inject {
        hit<src_ip: 3> for probe_rule LoginWindow {
            use(success=false) x 1
            then use(success=false) x 1
            without(success=true) within 4s
        }
    }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    let schemas = schemas();
    let plans = vec![compile_not_step_rule(&schemas)];
    let start: DateTime<Utc> = "2024-01-01T00:00:00Z".parse().unwrap();
    let duration = wfg.scenario.time_clause.duration;
    let mut rng = StdRng::seed_from_u64(wfg.scenario.seed);

    let result =
        generate_inject_events(&wfg, &plans, &schemas, &start, &duration, &mut rng).unwrap();

    assert_eq!(result.without_guards.len(), 3, "每个实体一条 guard");
    let values: HashSet<String> = result
        .without_guards
        .iter()
        .map(|guard| guard.value.to_string())
        .collect();
    assert_eq!(values.len(), 3, "三个实体互不相同");

    for guard in &result.without_guards {
        assert_eq!(guard.window_name, "LoginWindow");
        assert_eq!(guard.field, "src_ip");
        assert_eq!(guard.window, Duration::from_secs(4));
        let first = result
            .events
            .iter()
            .filter(|event| event.fields.get("src_ip") == Some(&guard.value))
            .map(|event| event.timestamp)
            .min()
            .expect("该实体有注入事件");
        assert_eq!(guard.start, first, "窗口起点取首条注入事件");
    }
}

/// `within` 省略 → 窗长取目标规则 `match` 的窗口（这里是 5s）。
#[test]
fn without_within_defaults_to_rule_match_window() {
    let input = r#"
#[duration=10s]
scenario guards_default<seed=7> {
    background { stream LoginWindow gen 20/s }
    inject {
        hit<src_ip: 2> for probe_rule LoginWindow {
            use(success=false) x 1
            then use(success=false) x 1
            without(success=true)
        }
    }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    let schemas = schemas();
    let plans = vec![compile_not_step_rule(&schemas)];
    let start: DateTime<Utc> = "2024-01-01T00:00:00Z".parse().unwrap();
    let duration = wfg.scenario.time_clause.duration;
    let mut rng = StdRng::seed_from_u64(wfg.scenario.seed);

    let result =
        generate_inject_events(&wfg, &plans, &schemas, &start, &duration, &mut rng).unwrap();

    assert_eq!(result.without_guards.len(), 2);
    for guard in &result.without_guards {
        assert_eq!(guard.window, Duration::from_secs(5));
    }
}

/// 没写 `without` 就不产生清单 —— 既有场景的输出逐条不变（后向兼容）。
#[test]
fn no_without_means_no_guards() {
    let input = r#"
#[duration=10s]
scenario no_without<seed=7> {
    background { stream LoginWindow gen 20/s }
    inject {
        hit<src_ip: 2> for probe_rule LoginWindow {
            use(success=false) x 1
            then use(success=false) x 1
        }
    }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    let schemas = schemas();
    let plans = vec![compile_not_step_rule(&schemas)];
    let start: DateTime<Utc> = "2024-01-01T00:00:00Z".parse().unwrap();
    let duration = wfg.scenario.time_clause.duration;
    let mut rng = StdRng::seed_from_u64(wfg.scenario.seed);

    let result =
        generate_inject_events(&wfg, &plans, &schemas, &start, &duration, &mut rng).unwrap();

    assert!(result.without_guards.is_empty());
}

// ---------------------------------------------------------------------------
// 背景剔除（背景侧）
// ---------------------------------------------------------------------------

fn bg_event(window: &str, src_ip: &str, success: bool, secs: i64) -> GenEvent {
    let start: DateTime<Utc> = "2024-01-01T00:00:00Z".parse().unwrap();
    let mut fields = serde_json::Map::new();
    fields.insert("src_ip".to_string(), serde_json::json!(src_ip));
    fields.insert("success".to_string(), serde_json::json!(success));
    GenEvent {
        stream_name: "login_events".to_string(),
        window_name: window.to_string(),
        timestamp: start + chrono::Duration::seconds(secs),
        fields,
    }
}

/// 只剔「属于该实体 + 窗内（闭区间）+ 命中谓词」的背景事件；别的实体、别的 stream、
/// 窗外、以及不命中谓词的噪声都不动。
#[test]
fn suppress_without_guards_drops_only_matching_events_of_the_entity_window() {
    let start: DateTime<Utc> = "2024-01-01T00:00:00Z".parse().unwrap();
    let guards = vec![WithoutGuard {
        window_name: "LoginWindow".to_string(),
        field: "src_ip".to_string(),
        value: serde_json::json!("10.0.0.0"),
        start,
        window: Duration::from_secs(4),
        predicates: vec![("success".to_string(), serde_json::json!(true))],
    }];

    let events = vec![
        bg_event("LoginWindow", "10.0.0.0", true, 1), // 目标实体 + 窗内 + 命中 → 剔
        bg_event("LoginWindow", "10.0.0.0", true, 4), // 边界（闭区间）→ 剔
        bg_event("LoginWindow", "10.0.0.0", false, 2), // 窗内但不命中谓词 → 留
        bg_event("LoginWindow", "10.0.0.0", true, 5), // 窗外 → 留
        bg_event("LoginWindow", "10.0.0.1", true, 1), // 别的实体 → 留
        bg_event("OtherWindow", "10.0.0.0", true, 1), // 别的 stream → 留
    ];

    let kept = crate::datagen::suppress_without_guards(events, &guards);
    assert_eq!(kept.len(), 4, "只应剔掉 2 条命中的目标实体窗内事件");
    assert_eq!(
        kept.iter()
            .filter(|event| event.fields.get("success") == Some(&serde_json::json!(true)))
            .count(),
        3,
        "剩下的 success=true 噪声都属于别的实体 / stream / 窗外"
    );
}

/// 数值谓词按数值比较：`without(dport=22)` 要能认出手写 JSON 浮点与生成器整数
/// 两种形态的同一个值。
#[test]
fn suppress_without_guards_compares_numbers_by_value() {
    let start: DateTime<Utc> = "2024-01-01T00:00:00Z".parse().unwrap();
    let guards = vec![WithoutGuard {
        window_name: "LoginWindow".to_string(),
        field: "src_ip".to_string(),
        value: serde_json::json!("10.0.0.0"),
        start,
        window: Duration::from_secs(4),
        // `without(dport=22)` 解析出来是浮点 22.0
        predicates: vec![("dport".to_string(), serde_json::json!(22.0))],
    }];

    let mut event = bg_event("LoginWindow", "10.0.0.0", false, 1);
    event
        .fields
        .insert("dport".to_string(), serde_json::json!(22));

    assert!(crate::datagen::suppress_without_guards(vec![event], &guards).is_empty());
}

/// 没有 guard 时输入原样返回（零开销、零行为变化）。
#[test]
fn suppress_without_guards_is_a_noop_without_guards() {
    let events = vec![bg_event("LoginWindow", "10.0.0.0", true, 1)];
    let kept = crate::datagen::suppress_without_guards(events, &[]);
    assert_eq!(kept.len(), 1);
}

// ---------------------------------------------------------------------------
// 端到端
// ---------------------------------------------------------------------------

/// `hit` + `without`：带否定步骤的规则被可靠触发（INJ1 通过）。
#[test]
fn hit_with_without_triggers_not_step_rule() {
    let input = r#"
#[duration=10s]
scenario not_step_hit<seed=42> {
    background { stream LoginWindow gen 20/s }
    inject {
        hit<src_ip: 5> for probe_rule LoginWindow {
            use(success=false) x 1
            then use(success=false) x 1
            without(success=true) within 4s
        }
    }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    let schemas = schemas();
    let plans = vec![compile_not_step_rule(&schemas)];
    let result = generate(&wfg, &schemas, &plans).unwrap();

    let start = "2024-01-01T00:00:00Z".parse().unwrap();
    let duration = wfg.scenario.time_clause.duration;
    let oracle = run_oracle(&result.events, &plans, &start, &duration, None).unwrap();

    // 5 个 hit 实体都产出告警 → 断言返回实体总数。
    assert_eq!(
        assert_inject_modes(&result.inject_entities, &oracle.alerts).unwrap(),
        5,
        "带 `not` 步骤的规则在 `without` 保护下应逐实体触发"
    );
}

/// `near_miss` 走不完链（只注入第一步）→ 一个实体都不触发（INJ2 通过）。
#[test]
fn near_miss_incomplete_chain_does_not_alert() {
    let input = r#"
#[duration=10s]
scenario not_step_near_miss<seed=42> {
    background { stream LoginWindow gen 20/s }
    inject {
        near_miss<src_ip: 5> for probe_rule LoginWindow {
            use(success=false) x 1
        }
    }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    let schemas = schemas();
    let plans = vec![compile_not_step_rule(&schemas)];
    let result = generate(&wfg, &schemas, &plans).unwrap();

    let start = "2024-01-01T00:00:00Z".parse().unwrap();
    let duration = wfg.scenario.time_clause.duration;
    let oracle = run_oracle(&result.events, &plans, &start, &duration, None).unwrap();

    assert_eq!(
        assert_inject_modes(&result.inject_entities, &oracle.alerts).unwrap(),
        5
    );
}

/// 注入事件自己命中 `without(...)` 谓词 → 窗口里确实会出现匹配事件，排不掉，
/// 必须报生成期错误（而不是静默产出一个永远不触发的 `hit`）。
#[test]
fn injected_event_matching_without_predicate_is_a_generation_error() {
    let input = r#"
#[duration=10s]
scenario not_step_conflict<seed=42> {
    background { stream LoginWindow gen 20/s }
    inject {
        hit<src_ip: 2> for probe_rule LoginWindow {
            use(success=false, dport=22) x 1
            then use(success=false) x 1
            without(dport=22) within 4s
        }
    }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    let schemas = schemas();
    let plans = vec![compile_not_step_rule(&schemas)];

    let err = match generate(&wfg, &schemas, &plans) {
        Ok(_) => panic!("注入事件命中 without 谓词时必须报错（否则 hit 永远不触发）"),
        Err(err) => err,
    };
    let detail = err.detail().as_deref().unwrap_or_default();
    assert!(detail.contains("命中谓词"), "获取到: {detail}");
    assert!(detail.contains("dport=22"), "获取到: {detail}");
    assert!(detail.contains("src_ip"), "获取到: {detail}");
}
