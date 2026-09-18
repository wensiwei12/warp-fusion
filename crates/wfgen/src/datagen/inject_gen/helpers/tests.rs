use std::collections::HashMap;

use wf_lang::ast::Measure;

use super::*;

use crate::datagen::inject_gen::structures::{InjectOverrides, InjectUseStepOverrides, StepInfo};

#[test]
fn matched_use_predicates_are_capped_to_step_event_count() {
    let steps = vec![StepInfo {
        bind_alias: "auth_fail".to_string(),
        scenario_alias: "LoginWindow".to_string(),
        window_name: "LoginWindow".to_string(),
        measure: Measure::Count,
        threshold: 5,
        filter_overrides: HashMap::from([("success".to_string(), serde_json::Value::Bool(false))]),
    }];
    let use_steps = vec![InjectUseStepOverrides::single(
        1_000,
        HashMap::from([("success".to_string(), serde_json::Value::Bool(false))]),
    )];

    let mapped = map_use_predicates_to_rule_steps(&steps, &use_steps, &[4], true).unwrap();

    assert_eq!(mapped.len(), 1);
    assert_eq!(
        mapped[0].len(),
        4,
        "matched filter predicates must not allocate beyond generated event count"
    );
    assert!(
        mapped[0]
            .iter()
            .all(|predicates| predicates.get("success") == Some(&serde_json::Value::Bool(false)))
    );
}

#[test]
fn use_step_counts_return_empty_for_empty_steps() {
    let use_steps = vec![InjectUseStepOverrides::single(
        1,
        HashMap::from([("success".to_string(), serde_json::Value::Bool(false))]),
    )];

    let counts = compute_use_step_counts(&[], &use_steps).unwrap();

    assert!(counts.is_empty());
}

#[test]
fn planned_use_steps_bind_by_declaration_order() {
    let steps = vec![
        StepInfo {
            bind_alias: "auth_fail".to_string(),
            scenario_alias: "LoginWindow".to_string(),
            window_name: "LoginWindow".to_string(),
            measure: Measure::Count,
            threshold: 1,
            filter_overrides: HashMap::from([(
                "success".to_string(),
                serde_json::Value::Bool(false),
            )]),
        },
        StepInfo {
            bind_alias: "followup".to_string(),
            scenario_alias: "LoginWindow".to_string(),
            window_name: "LoginWindow".to_string(),
            measure: Measure::Count,
            threshold: 1,
            filter_overrides: HashMap::new(),
        },
    ];
    let use_steps = vec![
        InjectUseStepOverrides::single(
            1,
            HashMap::from([("success".to_string(), serde_json::Value::Bool(false))]),
        ),
        InjectUseStepOverrides::single(
            1,
            HashMap::from([("dport".to_string(), serde_json::json!(22))]),
        ),
    ];

    let counts = compute_use_step_counts(&steps, &use_steps).unwrap();
    let mapped = map_use_predicates_to_rule_steps(&steps, &use_steps, &[1, 1], true).unwrap();

    assert_eq!(counts, vec![1, 1]);
    assert_eq!(
        mapped[0][0].get("success"),
        Some(&serde_json::Value::Bool(false))
    );
    assert_eq!(mapped[1][0].get("dport"), Some(&serde_json::json!(22)));
}

#[test]
fn one_use_step_does_not_spill_across_rule_steps() {
    let steps = vec![
        StepInfo {
            bind_alias: "first".to_string(),
            scenario_alias: "LoginWindow".to_string(),
            window_name: "LoginWindow".to_string(),
            measure: Measure::Count,
            threshold: 1,
            filter_overrides: HashMap::new(),
        },
        StepInfo {
            bind_alias: "second".to_string(),
            scenario_alias: "LoginWindow".to_string(),
            window_name: "LoginWindow".to_string(),
            measure: Measure::Count,
            threshold: 1,
            filter_overrides: HashMap::new(),
        },
    ];
    let use_steps = vec![InjectUseStepOverrides::single(
        2,
        HashMap::from([("dport".to_string(), serde_json::json!(22))]),
    )];

    let counts = compute_use_step_counts(&steps, &use_steps).unwrap();
    let mapped = map_use_predicates_to_rule_steps(&steps, &use_steps, &[2, 1], true).unwrap();

    assert_eq!(counts, vec![2, 0]);
    assert_eq!(mapped[0].len(), 2);
    assert!(
        mapped[1][0].is_empty(),
        "one use(...) clause must not spill predicates into the next rule step"
    );
}

#[test]
fn extra_use_step_errors_when_rule_steps_exhausted() {
    let steps = vec![StepInfo {
        bind_alias: "auth_fail".to_string(),
        scenario_alias: "LoginWindow".to_string(),
        window_name: "LoginWindow".to_string(),
        measure: Measure::Count,
        threshold: 5,
        filter_overrides: HashMap::from([("success".to_string(), serde_json::Value::Bool(false))]),
    }];
    let use_steps = vec![
        InjectUseStepOverrides::single(
            5,
            HashMap::from([("success".to_string(), serde_json::Value::Bool(false))]),
        ),
        InjectUseStepOverrides::single(
            1,
            HashMap::from([("success".to_string(), serde_json::Value::Bool(true))]),
        ),
    ];

    let err = compute_use_step_counts(&steps, &use_steps).unwrap_err();
    let rendered = err.report().render().to_string();

    assert!(
        rendered.contains("exceeds rule step count"),
        "unexpected error: {rendered}"
    );
}

#[test]
fn zero_count_use_step_errors() {
    let steps = vec![StepInfo {
        bind_alias: "auth_fail".to_string(),
        scenario_alias: "LoginWindow".to_string(),
        window_name: "LoginWindow".to_string(),
        measure: Measure::Count,
        threshold: 5,
        filter_overrides: HashMap::new(),
    }];
    let use_steps = vec![InjectUseStepOverrides::single(0, HashMap::new())];

    let err = compute_use_step_counts(&steps, &use_steps).unwrap_err();
    let rendered = err.report().render().to_string();

    assert!(
        rendered.contains("count must be greater than 0"),
        "unexpected error: {rendered}"
    );
}

#[test]
fn conflicting_use_step_predicates_error() {
    let steps = vec![StepInfo {
        bind_alias: "auth_fail".to_string(),
        scenario_alias: "LoginWindow".to_string(),
        window_name: "LoginWindow".to_string(),
        measure: Measure::Count,
        threshold: 5,
        filter_overrides: HashMap::from([("success".to_string(), serde_json::Value::Bool(false))]),
    }];
    let use_steps = vec![InjectUseStepOverrides::single(
        5,
        HashMap::from([("success".to_string(), serde_json::Value::Bool(true))]),
    )];

    let err = compute_use_step_counts(&steps, &use_steps).unwrap_err();
    let rendered = err.report().render().to_string();

    assert!(
        rendered.contains("conflicts with rule step filter"),
        "unexpected error: {rendered}"
    );
}

#[test]
fn near_miss_counts_are_written_counts_not_clamped() {
    let steps = vec![
        StepInfo {
            bind_alias: "step0".to_string(),
            scenario_alias: "LoginWindow".to_string(),
            window_name: "LoginWindow".to_string(),
            measure: Measure::Count,
            threshold: 1,
            filter_overrides: HashMap::from([("stage".to_string(), serde_json::json!("first"))]),
        },
        StepInfo {
            bind_alias: "step1".to_string(),
            scenario_alias: "LoginWindow".to_string(),
            window_name: "LoginWindow".to_string(),
            measure: Measure::Count,
            threshold: 2,
            filter_overrides: HashMap::new(),
        },
        StepInfo {
            bind_alias: "step2".to_string(),
            scenario_alias: "LoginWindow".to_string(),
            window_name: "LoginWindow".to_string(),
            measure: Measure::Count,
            threshold: 1,
            filter_overrides: HashMap::from([("stage".to_string(), serde_json::json!("after"))]),
        },
    ];
    let overrides = InjectOverrides {
        entity_count: Some(7),
        entity_field: None,
        within: None,
        use_steps: vec![
            InjectUseStepOverrides::single(
                3,
                HashMap::from([("stage".to_string(), serde_json::json!("first"))]),
            ),
            InjectUseStepOverrides::single(
                4,
                HashMap::from([("stage".to_string(), serde_json::json!("after"))]),
            ),
        ],
    };

    let counts = compute_near_miss_counts(&steps, &overrides).unwrap();

    assert_eq!(
        counts,
        vec![3, 4, 0],
        "near_miss 的条数就是 `use ... x N` 写的数：不补全、不按 `阈值 - 1` 夹取"
    );
}

/// 实体个数是写下来的，不做任何隐式除法；没写就是 0 条。
#[test]
fn cluster_count_is_the_written_entity_count() {
    let written = InjectOverrides {
        entity_count: Some(500),
        entity_field: Some("sip".to_string()),
        within: None,
        use_steps: Vec::new(),
    };
    assert_eq!(resolve_cluster_count(&written), 500);

    let absent = InjectOverrides {
        entity_count: None,
        entity_field: None,
        within: None,
        use_steps: Vec::new(),
    };
    assert_eq!(
        resolve_cluster_count(&absent),
        0,
        "没有实体个数就什么都不生成，不再回退到「配额 × 比例」"
    );
}

/// hit 与 near_miss 共用同一套条数口径：**模式不改数字**。
#[test]
fn hit_and_near_miss_share_the_same_counts() {
    let steps = vec![StepInfo {
        bind_alias: "step0".to_string(),
        scenario_alias: "LoginWindow".to_string(),
        window_name: "LoginWindow".to_string(),
        measure: Measure::Count,
        threshold: 10,
        filter_overrides: HashMap::new(),
    }];
    let overrides = InjectOverrides {
        entity_count: Some(5),
        entity_field: None,
        within: None,
        use_steps: vec![InjectUseStepOverrides::single(12, HashMap::new())],
    };

    let hit = compute_hit_counts(&steps, &overrides).unwrap();
    let near_miss = compute_near_miss_counts(&steps, &overrides).unwrap();

    assert_eq!(hit, vec![12], "hit 条数不被阈值 10 改写");
    assert_eq!(near_miss, hit, "两模式条数口径必须一致");
}

// ---------------------------------------------------------------------------
// 时间铺开：簇起点等距（设计 §4.7）
// ---------------------------------------------------------------------------

#[test]
fn uniform_cluster_start_is_evenly_spaced() {
    // duration 100s、窗口 10s → span 90s；5 个簇 → 0 / 22.5 / 45 / 67.5 / 90。
    let starts: Vec<f64> = (0..5).map(|i| uniform_cluster_start(i, 5, 90.0)).collect();
    assert_eq!(starts, vec![0.0, 22.5, 45.0, 67.5, 90.0]);

    // 首簇贴 0、末簇贴 span，整段 duration 被均匀覆盖、两端不留空档。
    assert_eq!(starts[0], 0.0);
    assert_eq!(starts[4], 90.0);

    // 等距：相邻差恒为 span / (count - 1)。
    let step = 90.0 / 4.0;
    for pair in starts.windows(2) {
        assert!((pair[1] - pair[0] - step).abs() < 1e-9, "{starts:?}");
    }
}

#[test]
fn uniform_cluster_start_degenerate_cases() {
    // 单个簇取中点（与 miss 单条事件居中同风格）。
    assert_eq!(uniform_cluster_start(0, 1, 90.0), 45.0);

    // 窗口不短于 duration（span 0）：无法错开，退回 0（保持旧行为）。
    assert_eq!(uniform_cluster_start(3, 5, 0.0), 0.0);
    assert_eq!(uniform_cluster_start(0, 1, 0.0), 0.0);
    assert_eq!(uniform_cluster_start(0, 1, -1.0), 0.0);

    // 0 个簇（生成侧不会走到，防御性）同样取中点而非除零。
    assert_eq!(uniform_cluster_start(0, 0, 90.0), 45.0);
}
