use super::*;
use crate::oracle::json_to_core_value;
use wf_engine::match_engine::Value;

#[test]
fn hop_oracle_closes_every_covered_window() {
    // hop(10s, 2s) + `and close` count：每覆盖窗口收口输出一条。
    // 事件 t=0/4/8s → 覆盖窗口并集 k=-4..4（9 个，窗口末 2/4/6/8/10/12/14/16/18s）。
    // 收尾水位 = **数据末尾**（8s，对齐引擎 `final_wm`，见 oracle/mod.rs 的
    // `sweep_nanos`）：只有窗口末 ≤ 8s 的 4 个在收尾扫描时到收口；末 10..18s 的
    // 到期点在数据末尾之后，引擎没有事件把水位推过去 → 由 close_all 收口，
    // 而 close_all 只发射完整窗口（w_end ≤ 最终事件时间 8s），即上面那 4 个。
    let mut plan = make_simple_rule_plan();
    // 默认 on-event 阈值为 3，改为 1（单事件即达标）。
    plan.match_plan.event_steps[0].branches[0].agg.threshold = Expr::Number(1.0);
    plan.match_plan.window_spec = WindowSpec::Hop {
        size: Duration::from_secs(10),
        slide: Duration::from_secs(2),
    };
    plan.match_plan.close_steps = vec![StepPlan {
        branches: vec![BranchPlan {
            label: Some("n".to_string()),
            source: "fail".to_string(),
            field: None,
            guard: None,
            agg: AggPlan {
                transforms: vec![],
                measure: Measure::Count,
                cmp: CmpOp::Ge,
                threshold: Expr::Number(1.0),
            },
        }],
    }];
    plan.match_plan.close_mode = CloseMode::And;
    let start: chrono::DateTime<Utc> = "2024-01-01T00:00:00Z".parse().unwrap();
    let duration = Duration::from_secs(12);
    let events = vec![
        make_event("s1", "LoginWindow", "10.0.0.1", "2024-01-01T00:00:00Z"),
        make_event("s1", "LoginWindow", "10.0.0.1", "2024-01-01T00:00:04Z"),
        make_event("s1", "LoginWindow", "10.0.0.1", "2024-01-01T00:00:08Z"),
    ];
    let result = run_oracle(&events, &[plan], &start, &duration, None).unwrap();
    // 2026-08-23 close_all 对齐 oracle/Flink 后：close_all 只收口**完整**窗口
    // （w_end ≤ 最终事件时间 8s）——尾部未完整窗口释放实例但不发射（q5 修复
    // 同源）。故最终只有收尾扫描已收口的 4 个完整窗口输出。
    assert_eq!(
        result.alerts.len(),
        4,
        "只有收尾水位（数据末尾 8s）之内的完整窗口各输出一条"
    );
}

#[test]
fn hit_cluster_triggers_alert() {
    let plan = make_simple_rule_plan();
    let start: chrono::DateTime<Utc> = "2024-01-01T00:00:00Z".parse().unwrap();
    let duration = Duration::from_secs(3600);

    // 3 events with same key → should trigger
    let events = vec![
        make_event("s1", "LoginWindow", "10.0.0.1", "2024-01-01T00:01:00Z"),
        make_event("s1", "LoginWindow", "10.0.0.1", "2024-01-01T00:02:00Z"),
        make_event("s1", "LoginWindow", "10.0.0.1", "2024-01-01T00:03:00Z"),
    ];

    let result = run_oracle(&events, &[plan], &start, &duration, None).unwrap();
    assert_eq!(result.alerts.len(), 1);
    assert_eq!(result.alerts[0].rule_name, "brute_force");
    assert_eq!(result.alerts[0].entity_id, "10.0.0.1");
    assert!((result.alerts[0].score - 85.0).abs() < f64::EPSILON);
    assert_eq!(result.alerts[0].origin, "event");
}

#[test]
fn near_miss_no_alert() {
    let plan = make_simple_rule_plan();
    let start: chrono::DateTime<Utc> = "2024-01-01T00:00:00Z".parse().unwrap();
    let duration = Duration::from_secs(3600);

    // 2 events (threshold is 3) → should NOT trigger
    let events = vec![
        make_event("s1", "LoginWindow", "10.0.0.1", "2024-01-01T00:01:00Z"),
        make_event("s1", "LoginWindow", "10.0.0.1", "2024-01-01T00:02:00Z"),
    ];

    let result = run_oracle(&events, &[plan], &start, &duration, None).unwrap();
    assert_eq!(result.alerts.len(), 0);
}

#[test]
fn different_keys_isolated() {
    let plan = make_simple_rule_plan();
    let start: chrono::DateTime<Utc> = "2024-01-01T00:00:00Z".parse().unwrap();
    let duration = Duration::from_secs(3600);

    // 2 events each for two different IPs → neither triggers (threshold=3)
    let events = vec![
        make_event("s1", "LoginWindow", "10.0.0.1", "2024-01-01T00:01:00Z"),
        make_event("s1", "LoginWindow", "10.0.0.2", "2024-01-01T00:01:30Z"),
        make_event("s1", "LoginWindow", "10.0.0.1", "2024-01-01T00:02:00Z"),
        make_event("s1", "LoginWindow", "10.0.0.2", "2024-01-01T00:02:30Z"),
    ];

    let result = run_oracle(&events, &[plan], &start, &duration, None).unwrap();
    assert_eq!(result.alerts.len(), 0);
}

#[test]
fn bind_filter_is_applied_during_oracle_eval() {
    let plan = make_filtered_rule_plan();
    let start: chrono::DateTime<Utc> = "2024-01-01T00:00:00Z".parse().unwrap();
    let duration = Duration::from_secs(3600);

    let events = vec![
        make_action_event(
            "s1",
            "LoginWindow",
            "10.0.0.1",
            "failed",
            "2024-01-01T00:01:00Z",
        ),
        make_action_event(
            "s1",
            "LoginWindow",
            "10.0.0.1",
            "success",
            "2024-01-01T00:02:00Z",
        ),
        make_action_event(
            "s1",
            "LoginWindow",
            "10.0.0.1",
            "success",
            "2024-01-01T00:03:00Z",
        ),
    ];

    let result = run_oracle(&events, &[plan], &start, &duration, None).unwrap();
    assert_eq!(
        result.alerts.len(),
        0,
        "oracle must honor bind filters instead of counting all same-window events"
    );
}

#[test]
fn close_all_eos_fires_and_close_rule_at_scenario_end() {
    let plan = make_and_close_rule_plan();
    let start: chrono::DateTime<Utc> = "2024-01-01T00:00:00Z".parse().unwrap();
    let duration = Duration::from_secs(60);

    // The 5m match window has not expired by scenario end. The oracle must
    // still model finite replay EOF and close active instances.
    let events = vec![
        make_event("s1", "LoginWindow", "10.0.0.1", "2024-01-01T00:00:01Z"),
        make_event("s1", "LoginWindow", "10.0.0.1", "2024-01-01T00:00:02Z"),
        make_event("s1", "LoginWindow", "10.0.0.1", "2024-01-01T00:00:03Z"),
    ];

    let result = run_oracle(&events, &[plan], &start, &duration, None).unwrap();

    assert_eq!(result.alerts.len(), 1);
    assert_eq!(result.alerts[0].rule_name, "close_rule");
    assert_eq!(result.alerts[0].entity_id, "10.0.0.1");
    assert_eq!(result.alerts[0].origin, "close:eos");
}

/// batch 收尾水位 = **数据末尾**（P0）：5m 窗口的到期点（~303s）落在数据末尾
/// （3s）之后时，收尾**不得**走 `close:timeout`。
///
/// 旧口径按场景边界扫（`eos_nanos` = 600s > 303s）会误判为窗口已到期并输出
/// `close:timeout`；引擎的水位是 `final_wm = max(窗口 max_event_time, 机器水位)`
/// = 数据末尾 3s，此时没有事件把水位推到到期点 → 走收尾 `close:flush`，
/// `close_reason == "timeout"` 的守卫不命中。
#[test]
fn timeout_guard_does_not_fire_at_data_end_without_expiry() {
    let plan = make_timeout_guard_close_rule_plan();
    let start: chrono::DateTime<Utc> = "2024-01-01T00:00:00Z".parse().unwrap();
    let duration = Duration::from_secs(600);

    let events = vec![
        make_event("s1", "LoginWindow", "10.0.0.1", "2024-01-01T00:00:01Z"),
        make_event("s1", "LoginWindow", "10.0.0.1", "2024-01-01T00:00:02Z"),
        make_event("s1", "LoginWindow", "10.0.0.1", "2024-01-01T00:00:03Z"),
    ];

    let result = run_oracle(&events, &[plan], &start, &duration, None).unwrap();

    assert_eq!(
        result.alerts.len(),
        0,
        "到期点在数据末尾之后 → 收尾是 close:flush，timeout 守卫不应命中"
    );
}

/// 同一实例若**真的**在数据末尾之前到期（后续事件把水位推过到期点），
/// `close:timeout` 守卫仍按事件驱动的收尾扫描命中。
#[test]
fn timeout_guard_fires_when_instance_expires_before_data_end() {
    let plan = make_timeout_guard_close_rule_plan();
    let start: chrono::DateTime<Utc> = "2024-01-01T00:00:00Z".parse().unwrap();
    let duration = Duration::from_secs(600);

    let events = vec![
        make_event("s1", "LoginWindow", "10.0.0.1", "2024-01-01T00:00:01Z"),
        make_event("s1", "LoginWindow", "10.0.0.1", "2024-01-01T00:00:02Z"),
        make_event("s1", "LoginWindow", "10.0.0.1", "2024-01-01T00:00:03Z"),
        // 同一 sip 的后续事件：推进水位越过 5m 窗口到期点（~303s），实例按
        // `close:timeout` 收口；它自身开启的新实例只有 1 条事件，不满足
        // on-event 阈值（3），且到期点 700s > 数据末尾 400s → 不输出。
        make_event("s1", "LoginWindow", "10.0.0.1", "2024-01-01T00:06:40Z"),
    ];

    let result = run_oracle(&events, &[plan], &start, &duration, None).unwrap();

    assert_eq!(result.alerts.len(), 1);
    assert_eq!(result.alerts[0].rule_name, "timeout_close_rule");
    assert_eq!(result.alerts[0].entity_id, "10.0.0.1");
    assert_eq!(result.alerts[0].origin, "close:timeout");
}

/// batch 收尾水位 = **数据末尾**（P0）的正面锁定：到期点在数据末尾之后的实例，
/// 由收尾 `close_all` 以 `close:eos` 收口，且 `emit_time` = **该实例最后一条
/// 事件**（4s），不是场景边界（600s）。
#[test]
fn batch_sweep_uses_data_end_not_scenario_end() {
    let plan = make_and_close_rule_plan();
    let start: chrono::DateTime<Utc> = "2024-01-01T00:00:00Z".parse().unwrap();
    let duration = Duration::from_secs(600);

    let events = vec![
        make_event("s1", "LoginWindow", "10.0.0.1", "2024-01-01T00:00:01Z"),
        make_event("s1", "LoginWindow", "10.0.0.1", "2024-01-01T00:00:02Z"),
        make_event("s1", "LoginWindow", "10.0.0.1", "2024-01-01T00:00:03Z"),
        make_event("s1", "LoginWindow", "10.0.0.1", "2024-01-01T00:00:04Z"),
    ];

    let result = run_oracle(&events, &[plan], &start, &duration, None).unwrap();

    assert_eq!(result.alerts.len(), 1);
    assert_eq!(result.alerts[0].origin, "close:eos");
    assert_eq!(result.alerts[0].emit_time, "2024-01-01T00:00:04.000Z");
}

#[test]
fn empty_events_no_alerts() {
    let plan = make_simple_rule_plan();
    let start: chrono::DateTime<Utc> = "2024-01-01T00:00:00Z".parse().unwrap();
    let duration = Duration::from_secs(3600);

    let result = run_oracle(&[], &[plan], &start, &duration, None).unwrap();
    assert_eq!(result.alerts.len(), 0);
}

#[test]
fn multi_alias_same_window_both_receive_events() {
    // Rule with two binds on the same window: "a" and "b" both reference LoginWindow.
    // Step 1 uses "a" (count >= 2), step 2 uses "b" (count >= 2).
    // All events come from LoginWindow, so both aliases must receive them.
    let plan = RulePlan {
        name: "multi_bind".to_string(),
        binds: vec![
            BindPlan {
                alias: "a".to_string(),
                window: "LoginWindow".to_string(),
                filter: None,
            },
            BindPlan {
                alias: "b".to_string(),
                window: "LoginWindow".to_string(),
                filter: None,
            },
        ],
        lets: Vec::new(),
        match_plan: MatchPlan {
            key_exprs: Vec::new(),
            keys: vec![FieldRef::Simple("sip".to_string())],
            key_map: None,
            key_join: None,
            window_spec: WindowSpec::Sliding(Duration::from_secs(300)),
            event_steps: vec![
                StepPlan {
                    branches: vec![BranchPlan {
                        label: Some("step_a".to_string()),
                        source: "a".to_string(),
                        field: None,
                        guard: None,
                        agg: AggPlan {
                            transforms: vec![],
                            measure: Measure::Count,
                            cmp: CmpOp::Ge,
                            threshold: Expr::Number(2.0),
                        },
                    }],
                },
                StepPlan {
                    branches: vec![BranchPlan {
                        label: Some("step_b".to_string()),
                        source: "b".to_string(),
                        field: None,
                        guard: None,
                        agg: AggPlan {
                            transforms: vec![],
                            measure: Measure::Count,
                            cmp: CmpOp::Ge,
                            threshold: Expr::Number(2.0),
                        },
                    }],
                },
            ],
            close_steps: vec![],
            close_mode: CloseMode::Or,
            match_mode: MatchMode::Seq,
            accu: false,
            seq: None,
            tracked_bind_aliases: std::collections::HashSet::new(),
            tracked_bind_fields: std::collections::HashMap::new(),
            tracked_plain_fields: std::collections::HashSet::new(),
            needs_field_history: false,
            trigger_event_needed: false,
        },
        each_plan: None,
        joins: vec![],
        r#where: None,
        entity_plan: EntityPlan {
            entity_type: "ip".to_string(),
            entity_id_expr: Expr::Field(FieldRef::Simple("sip".to_string())),
        },
        yield_plan: YieldPlan {
            target: "alerts".to_string(),
            version: None,
            fields: vec![],
        },
        score_plan: ScorePlan {
            expr: Expr::Number(90.0),
        },
        pattern_origin: None,
        conv_plan: None,
        limits_plan: None,
        conv_window: None,
        stats_plan: None,
    };

    let start: chrono::DateTime<Utc> = "2024-01-01T00:00:00Z".parse().unwrap();
    let duration = Duration::from_secs(3600);

    // 4 events to LoginWindow → alias "a" gets 4, alias "b" gets 4.
    // Step 1 (a >= 2) triggers after event 2, step 2 (b >= 2) triggers after event 4.
    let events = vec![
        make_event("s1", "LoginWindow", "10.0.0.1", "2024-01-01T00:01:00Z"),
        make_event("s1", "LoginWindow", "10.0.0.1", "2024-01-01T00:02:00Z"),
        make_event("s1", "LoginWindow", "10.0.0.1", "2024-01-01T00:03:00Z"),
        make_event("s1", "LoginWindow", "10.0.0.1", "2024-01-01T00:04:00Z"),
    ];

    let result = run_oracle(&events, &[plan], &start, &duration, None).unwrap();

    // With the old single-alias map, alias "b" would never receive events
    // and the rule would never fully match. With the fix, both aliases
    // receive events and the multi-step rule completes.
    assert!(
        !result.alerts.is_empty(),
        "multi-alias same-window rule should trigger when both aliases receive events"
    );
    assert_eq!(result.alerts[0].rule_name, "multi_bind");
}

#[test]
fn sc7_uninjected_rule_skipped() {
    let plan = make_simple_rule_plan(); // name = "brute_force"
    let start: chrono::DateTime<Utc> = "2024-01-01T00:00:00Z".parse().unwrap();
    let duration = Duration::from_secs(3600);

    // 3 events that would trigger the rule
    let events = vec![
        make_event("s1", "LoginWindow", "10.0.0.1", "2024-01-01T00:01:00Z"),
        make_event("s1", "LoginWindow", "10.0.0.1", "2024-01-01T00:02:00Z"),
        make_event("s1", "LoginWindow", "10.0.0.1", "2024-01-01T00:03:00Z"),
    ];

    // With injected_rules containing "brute_force" → alert generated
    let injected: std::collections::HashSet<String> =
        ["brute_force".to_string()].into_iter().collect();
    let result = run_oracle(
        &events,
        std::slice::from_ref(&plan),
        &start,
        &duration,
        Some(&injected),
    )
    .unwrap();
    assert_eq!(result.alerts.len(), 1);

    // With injected_rules NOT containing "brute_force" → no alert (SC7)
    let other: std::collections::HashSet<String> =
        ["some_other_rule".to_string()].into_iter().collect();
    let result = run_oracle(&events, &[plan], &start, &duration, Some(&other)).unwrap();
    assert_eq!(result.alerts.len(), 0);
}

// ---------------------------------------------------------------------------
// 结构化字段（object / array）在 GenEvent → 引擎 Value 的转换
// ---------------------------------------------------------------------------

/// object / array 必须**递归**保留：旧实现把它们整段丢掉，读嵌套字段的规则在
/// oracle 侧恒不命中（与引擎侧不一致）。
#[test]
fn json_to_core_value_keeps_structured_values() {
    let value = serde_json::json!({
        "action": "syn",
        "nested": {"sev": 10, "flag": true, "none": null},
        "tags": ["a", 22, {"deep": 1}],
        "empty_array": [],
        "empty_object": {}
    });

    let Some(Value::Object(map)) = json_to_core_value(&value) else {
        panic!("顶层 object 必须保留");
    };
    assert_eq!(map.get("action"), Some(&Value::Str("syn".into())));

    let Some(Value::Object(nested)) = map.get("nested") else {
        panic!("嵌套 object 必须保留");
    };
    assert_eq!(nested.get("sev"), Some(&Value::Float(10.0)));
    assert_eq!(nested.get("flag"), Some(&Value::Bool(true)));
    assert!(!nested.contains_key("none"), "null 成员与引擎一致地丢弃");

    let Some(Value::Array(tags)) = map.get("tags") else {
        panic!("array 必须保留");
    };
    assert_eq!(tags[0], Value::Str("a".into()));
    assert_eq!(tags[1], Value::Float(22.0));
    let Value::Object(deep) = &tags[2] else {
        panic!("数组里的 object 必须保留");
    };
    assert_eq!(deep.get("deep"), Some(&Value::Float(1.0)));

    assert_eq!(map.get("empty_array"), Some(&Value::Array(Vec::new())));
    assert_eq!(
        map.get("empty_object"),
        Some(&Value::Object(Default::default()))
    );
}

// ---------------------------------------------------------------------------
// 时间列字段的值域口径（oracle ↔ 引擎列式读取）
// ---------------------------------------------------------------------------

/// 回归（2026-09-19）：**时间列字段**必须落 [`Value::Int`]，与引擎的列式口径一致。
///
/// 引擎的数据面是**列式**的：wfgen 把 `BaseType::Time` 写成箭头 `Timestamp(Nanosecond)`
/// 列，引擎 `extract_field_value` 读列即 `Value::Int(i64)`（逐位精确）。oracle 只有
/// JSON 来源，若时间字段也跟着 [`json_to_core_value`] 的「JSON 数字不猜整型」落
/// `Float`，两侧对同一份数据就不同口径；`within` 的下界（`p.timestamp`）经 f64 把
/// epoch-ns（≈1.77e18 > 2^53）量化到 ~256ns，同刻时右行正压在下界上，约一半的
/// `row_ts >= lo` 翻转 → deferred join 静默漏配（实测：50 个实体只命中 26 个，
/// 时间字段改走 `Int` 后 50/50）。
#[test]
fn time_typed_fields_are_exact_int_columns_in_oracle_events() {
    use super::super::{gen_event_to_core, time_columns};
    use wf_lang::{BaseType, FieldDef, FieldType, WindowSchema};

    // ulp=256 → 非对齐，经 f64 往返必变（对齐值恰好可精确表示，测不出差异）。
    let ns: i64 = 1_767_225_600_000_000_001;
    assert_ne!(ns as f64 as i64, ns, "前提：该值经 f64 必丢精度");

    let schema = WindowSchema {
        name: "conn_events".into(),
        streams: vec!["conn_events".into()],
        time_field: Some("ts".into()),
        over: Duration::from_secs(300),
        fields: vec![
            FieldDef {
                name: "ts".into(),
                field_type: FieldType::Base(BaseType::Time),
            },
            FieldDef {
                name: "id".into(),
                field_type: FieldType::Base(BaseType::Digit),
            },
        ],
    };
    let cols = time_columns(&[schema]);

    let mut fields = serde_json::Map::new();
    fields.insert("ts".into(), serde_json::json!(ns));
    fields.insert("id".into(), serde_json::json!(7));
    let ev = GenEvent {
        stream_name: "conn_events".into(),
        window_name: "conn_events".into(),
        timestamp: Utc::now(),
        fields,
    };

    let core = gen_event_to_core(&ev, &cols);
    assert_eq!(
        core.fields["ts"],
        Value::Int(ns),
        "时间列必须逐位精确（不得经 f64）"
    );
    // 已知边界（**本片未对齐**）：`Digit`/`Int64` 列在引擎里同样是 `Value::Int`，
    // 但 oracle 仍按 JSON 口径落 `Float`。对 `|i| < 2^53`（id / price 等）两者在
    // 比较、同一性键、`format_f64` 输出上恰好一致，所以暂时无害；若将来要全面对齐
    // 列式口径，本断言会失败——那是**故意**的提醒点，不是需保的契约。
    assert_eq!(core.fields["id"], Value::Float(7.0));
}
