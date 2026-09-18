//! 生成期硬断言（INJ1/INJ2）的端到端覆盖：`generate` → `run_oracle` →
//! `assert_inject_modes`，用真实生成的事件与 oracle 告警验证口径。

use std::collections::HashSet;

use crate::error::WfgenResult;
use crate::inject_assert::assert_inject_modes;

use super::*;

/// 跑一遍「生成 + oracle + 断言」，返回断言结果。
fn assert_scenario(wfg_src: &str, plans: &[RulePlan]) -> WfgenResult<usize> {
    let wfg = parse_wfg(wfg_src).unwrap();
    let schemas = vec![make_login_schema()];
    let result = generate(&wfg, &schemas, plans).unwrap();

    let start = "2024-01-01T00:00:00Z".parse().unwrap();
    let duration = wfg.scenario.time_clause.duration;
    let oracle = run_oracle(&result.events, plans, &start, &duration, None).unwrap();

    assert_inject_modes(&result.inject_entities, &oracle.alerts)
}

/// hit 用例的实体确实都产出了告警 → 断言通过（阈值 5，每个实体 5 条）。
#[test]
fn hit_entities_all_alert_pass() {
    let input = r#"
#[duration=5s]
scenario hit_ok<seed=42> {
    background { stream LoginWindow gen 100/s }
    inject {
        hit<src_ip: 8> for auth_fail_rule LoginWindow {
            use(success=false) x 5
        }
    }
}
"#;

    let plans = vec![make_auth_fail_plan()];
    assert_eq!(assert_scenario(input, &plans).unwrap(), 8);
}

/// hit 用例的 `x N` 达不到阈值 → 每个实体一条 INJ1 明细。
#[test]
fn hit_below_threshold_reports_inj1() {
    let input = r#"
#[duration=5s]
scenario hit_low<seed=42> {
    background { stream LoginWindow gen 100/s }
    inject {
        hit<src_ip: 4> for auth_fail_rule LoginWindow {
            use(success=false) x 2
        }
    }
}
"#;

    let plans = vec![make_auth_fail_plan()];
    let err = assert_scenario(input, &plans).unwrap_err();
    let detail = err.detail().as_deref().unwrap_or_default();

    assert!(detail.contains("INJ1: 4 hit"), "获取到: {detail}");
    assert!(
        detail.contains("不会触发规则 auth_fail_rule"),
        "获取到: {detail}"
    );
    assert!(
        detail.contains("auth_fail 2/5"),
        "条数/阈值必须摊开: {detail}"
    );
}

/// near_miss 用 `use(...)` 满足了 filter 但达到阈值 → 产出告警即 INJ2。
#[test]
fn near_miss_reaching_threshold_reports_inj2() {
    let input = r#"
#[duration=5s]
scenario nm_over<seed=42> {
    background { stream LoginWindow gen 100/s }
    inject {
        near_miss<src_ip: 3> for auth_fail_rule LoginWindow {
            use(success=false) x 5
        }
    }
}
"#;

    let plans = vec![make_auth_fail_plan()];
    let err = assert_scenario(input, &plans).unwrap_err();
    let detail = err.detail().as_deref().unwrap_or_default();

    assert!(detail.contains("INJ2: 3"), "获取到: {detail}");
    assert!(
        detail.contains("near_miss 用例第 1 个实体"),
        "获取到: {detail}"
    );
    assert!(
        detail.contains("会触发规则 auth_fail_rule"),
        "获取到: {detail}"
    );
}

/// near_miss / miss 都达不到阈值 → 断言通过。
///
/// `miss` 的 `x N` 是「N 条各自独立键」：声明 6 个实体的用例生成 6×5 条事件、
/// 每个事件一个实体（这是 `miss` 能构造出来的前提——同一实体成簇就会报警），
/// 因此纳入断言的实体数是 3 + 30 而不是 3 + 6。
#[test]
fn near_miss_and_miss_below_threshold_pass() {
    let input = r#"
#[duration=5s]
scenario nm_ok<seed=42> {
    background { stream LoginWindow gen 100/s }
    inject {
        near_miss<src_ip: 3> for auth_fail_rule LoginWindow {
            use(success=false) x 4
        }
        miss<src_ip: 6> for auth_fail_rule LoginWindow {
            use(success=false) x 5
        }
    }
}
"#;

    let plans = vec![make_auth_fail_plan()];
    assert_eq!(assert_scenario(input, &plans).unwrap(), 3 + 30);
}

/// 规则的 `entity(...)` 不是单一字段（复合表达式）时无法与告警 `entity_id` 对齐：
/// 该用例整体不计入断言，但必须如实计入 `unasserted`（`gen` 据此打 Warning），
/// 而不是静默当作通过。
#[test]
fn composite_entity_expression_is_reported_as_unasserted() {
    let input = r#"
#[duration=5s]
scenario composite_entity<seed=42> {
    background { stream LoginWindow gen 100/s }
    inject {
        hit<src_ip: 4> for auth_fail_rule LoginWindow {
            use(success=false) x 5
        }
    }
}
"#;

    let wfg = parse_wfg(input).unwrap();
    let schemas = vec![make_login_schema()];
    let mut plans = vec![make_auth_fail_plan()];
    plans[0].entity_plan.entity_id_expr = Expr::BinOp {
        op: BinOp::Add,
        left: Box::new(Expr::Field(FieldRef::Simple("src_ip".to_string()))),
        right: Box::new(Expr::Number(1.0)),
    };

    let result = generate(&wfg, &schemas, &plans).unwrap();
    assert!(result.inject_entities.is_empty());
    assert_eq!(result.unasserted_inject_entities, 4);

    let oracle = run_oracle(
        &result.events,
        &plans,
        &"2024-01-01T00:00:00Z".parse().unwrap(),
        &wfg.scenario.time_clause.duration,
        None,
    )
    .unwrap();
    assert_eq!(
        assert_inject_modes(&result.inject_entities, &oracle.alerts).unwrap(),
        0
    );
}

/// 不同用例的实体键空间互不重叠。
///
/// `hit` 与 `near_miss` 若指向同一个实体，两个模式的口径就互相污染（一个要求
/// 报警、一个要求不报警）——这是本语料在断言落地后暴露的主要坑。
#[test]
fn inject_cases_use_disjoint_entity_values() {
    let input = r#"
#[duration=5s]
scenario disjoint<seed=42> {
    background { stream LoginWindow gen 100/s }
    inject {
        hit<src_ip: 6> for auth_fail_rule LoginWindow {
            use(success=false) x 5
        }
        near_miss<src_ip: 4> for auth_fail_rule LoginWindow {
            use(success=false) x 2
        }
        miss<src_ip: 3> for auth_fail_rule LoginWindow {
            use(success=false) x 1
        }
    }
}
"#;

    let wfg = parse_wfg(input).unwrap();
    let schemas = vec![make_login_schema()];
    let plans = vec![make_auth_fail_plan()];
    let result = generate(&wfg, &schemas, &plans).unwrap();

    // 段内不重叠：三种模式各自的实体值全局唯一。
    let values: Vec<&str> = result
        .inject_entities
        .iter()
        .map(|entity| entity.value.as_str().expect("Ip 实体值是字符串"))
        .collect();
    let unique: HashSet<&str> = values.iter().copied().collect();
    assert_eq!(unique.len(), values.len(), "实体值必须互不重叠: {values:?}");
    assert_eq!(result.inject_entities.len(), 6 + 4 + 3);

    // 且模式断言通过（hit 5 条 = 阈值 5；near_miss 2 条 / miss 1 条 < 5）。
    let oracle = run_oracle(
        &result.events,
        &plans,
        &"2024-01-01T00:00:00Z".parse().unwrap(),
        &wfg.scenario.time_clause.duration,
        None,
    )
    .unwrap();
    assert_eq!(
        assert_inject_modes(&result.inject_entities, &oracle.alerts).unwrap(),
        6 + 4 + 3
    );
}

/// 无注入用例时断言空转（背景流量场景不受影响）。
#[test]
fn background_only_scenario_is_not_asserted() {
    let input = r#"
#[duration=5s]
scenario bg_only<seed=42> {
    background { stream LoginWindow gen 100/s }
}
"#;

    let plans = vec![make_brute_force_plan()];
    assert_eq!(assert_scenario(input, &plans).unwrap(), 0);
}
