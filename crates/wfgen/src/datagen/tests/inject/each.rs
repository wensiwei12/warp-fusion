//! `on each` 规则（无状态、逐事件告警）作为注入目标。
//!
//! `on each` 没有窗口与阈值：命中**一条**事件即产出告警，因此
//! `hit` = 每个实体的注入事件都要满足 each 过滤条件，`near_miss` / `miss` =
//! 一个都不许命中（在 `on each` 上两者同义，设计 §3.2）。

use std::collections::HashSet;

use crate::error::WfgenResult;
use crate::inject_assert::assert_inject_modes;

use super::*;

/// 跑一遍「生成 + oracle + 断言」，返回断言结果。
fn assert_scenario(wfg_src: &str) -> WfgenResult<usize> {
    let wfg = parse_wfg(wfg_src).unwrap();
    let schemas = vec![make_login_schema()];
    let plans = vec![make_each_plan()];
    let result = generate(&wfg, &schemas, &plans).unwrap();

    let start = "2024-01-01T00:00:00Z".parse().unwrap();
    let duration = wfg.scenario.time_clause.duration;
    let oracle = run_oracle(&result.events, &plans, &start, &duration, None).unwrap();

    assert_inject_modes(&result.inject_entities, &oracle.alerts)
}

/// `hit<field: N>`：每个实体一条命中过滤条件的事件 → 每个实体都产出告警。
#[test]
fn hit_on_each_rule_alerts_per_entity() {
    let input = r#"
#[duration=10s]
scenario each_hit<seed=42> {
    background { stream LoginWindow gen 20/s }
    inject {
        hit<username: 3> for each_alert LoginWindow {
            use(attempts=500) x 1
        }
    }
}
"#;

    assert_eq!(assert_scenario(input).unwrap(), 3);
}

/// 实体字段可省：`on each` 形态从 `entity(...)` 的单一字段推断（设计 §3.7）——
/// 推断成功才有 3 个**互不相同**的实体。
#[test]
fn entity_field_is_inferred_from_entity_expr_for_each_rules() {
    let input = r#"
#[duration=10s]
scenario each_infer<seed=42> {
    background { stream LoginWindow gen 20/s }
    inject {
        hit<3> for each_alert LoginWindow {
            use(attempts=500) x 1
        }
    }
}
"#;

    let wfg = parse_wfg(input).unwrap();
    let schemas = vec![make_login_schema()];
    let plans = vec![make_each_plan()];
    let result = generate(&wfg, &schemas, &plans).unwrap();

    assert_eq!(result.inject_entities.len(), 3);
    let values: HashSet<&str> = result
        .inject_entities
        .iter()
        .filter_map(|entity| entity.value.as_str())
        .collect();
    assert_eq!(values.len(), 3, "每个实体必须拿到不同的推断键值");
    assert!(result.unasserted_inject_entities.is_empty());

    // 断言本身通过（3 个实体都命中 attempts >= 100）。
    let start = "2024-01-01T00:00:00Z".parse().unwrap();
    let duration = wfg.scenario.time_clause.duration;
    let oracle = run_oracle(&result.events, &plans, &start, &duration, None).unwrap();
    assert_eq!(
        assert_inject_modes(&result.inject_entities, &oracle.alerts).unwrap(),
        3
    );
}

/// `hit` 但注入值不满足 each 过滤条件 → INJ1（断言在 `on each` 上同样有效）。
#[test]
fn hit_below_each_filter_reports_inj1() {
    let input = r#"
#[duration=10s]
scenario each_hit_low<seed=42> {
    background { stream LoginWindow gen 20/s }
    inject {
        hit<username: 2> for each_alert LoginWindow {
            use(attempts=1) x 1
        }
    }
}
"#;

    let err = assert_scenario(input).unwrap_err();
    let detail = err.detail().as_deref().unwrap_or_default();
    assert!(detail.contains("INJ1: 2 hit"), "获取到: {detail}");
    assert!(
        detail.contains("不会触发规则 each_alert"),
        "获取到: {detail}"
    );
    assert!(detail.contains("第 1 个实体"), "获取到: {detail}");
}

/// `near_miss` / `miss`：注入值不满足过滤条件 → 一个都不告警（断言通过）。
#[test]
fn near_miss_and_miss_on_each_rule_do_not_alert() {
    let input = r#"
#[duration=10s]
scenario each_negative<seed=42> {
    background { stream LoginWindow gen 20/s }
    inject {
        near_miss<username: 2> for each_alert LoginWindow {
            use(attempts=5) x 1
        }
        miss<username: 3> for each_alert LoginWindow {
            use(attempts=5) x 2
        }
    }
}
"#;

    assert_eq!(assert_scenario(input).unwrap(), 2 + 6);
}

/// `on each` 的规则窗口是编译器占位（`Sliding(1s)`）：未写 `spread` 时簇铺在它之内
/// （3 条事件不打在同一时刻）。`spread D` 可显式覆盖。
#[test]
fn each_rule_cluster_is_spread_within_its_placeholder_window() {
    let input = r#"
#[duration=10s]
scenario each_spread<seed=42> {
    background { stream LoginWindow gen 20/s }
    inject {
        hit<username: 1> for each_alert LoginWindow {
            use(attempts=500) x 3
        }
    }
}
"#;

    let wfg = parse_wfg(input).unwrap();
    let schemas = vec![make_login_schema()];
    let plans = vec![make_each_plan()];
    let result = generate(&wfg, &schemas, &plans).unwrap();

    let entity = result.inject_entities[0]
        .value
        .as_str()
        .unwrap()
        .to_string();
    let mut timestamps: Vec<_> = result
        .events
        .iter()
        .filter(|event| {
            event
                .fields
                .get("username")
                .and_then(|v| v.as_str())
                .is_some_and(|name| name == entity)
        })
        .map(|event| event.timestamp)
        .collect();
    timestamps.sort();
    assert_eq!(timestamps.len(), 3);
    assert!(
        timestamps[2] > timestamps[0],
        "3 条事件必须铺开（默认铺在占位窗口 1s 内），实际都落在 {:?}",
        timestamps[0]
    );
}
