use super::*;

/// 取断言失败的错误明细（`detail` 必填，缺失即测试自身有问题）。
fn detail_of(err: &crate::error::WfgenError) -> String {
    err.detail().as_deref().unwrap_or_default().to_string()
}

fn entity(
    rule: &str,
    mode: InjectCaseMode,
    index: u64,
    value: serde_json::Value,
) -> InjectEntityKey {
    InjectEntityKey {
        rule: rule.to_string(),
        mode,
        index,
        field: "sip".to_string(),
        value,
        steps: vec![InjectStepCount {
            bind_alias: "fail".to_string(),
            count: 3,
            threshold: 3,
        }],
    }
}

fn alert(rule: &str, entity_id: &str, origin: &str) -> OracleAlert {
    OracleAlert {
        rule_name: rule.to_string(),
        score: 70.0,
        entity_type: "ip".to_string(),
        entity_id: entity_id.to_string(),
        origin: origin.to_string(),
        emit_time: "2024-01-01T00:00:00+00:00".to_string(),
        fields: Vec::new(),
        intermediate: false,
    }
}

/// 实体字段值渲染必须与 oracle 的 `entity_id` 同口径（整数不带 `.0`）。
#[test]
fn entity_id_of_value_matches_oracle_rendering() {
    assert_eq!(
        entity_id_of_value(&serde_json::json!("10.0.0.1")),
        "10.0.0.1"
    );
    assert_eq!(entity_id_of_value(&serde_json::json!(5)), "5");
    assert_eq!(entity_id_of_value(&serde_json::json!(5.0)), "5");
    assert_eq!(entity_id_of_value(&serde_json::json!(2.5)), "2.5");
    assert_eq!(entity_id_of_value(&serde_json::json!(true)), "true");
    assert_eq!(entity_id_of_value(&serde_json::json!(null)), "");
    assert_eq!(entity_id_of_value(&serde_json::json!(["a"])), "[array]");
    assert_eq!(entity_id_of_value(&serde_json::json!({"a": 1})), "[object]");
}

/// hit 实体都报了警、near_miss/miss 都没报警 → 通过，并回报纳管实体数。
#[test]
fn all_modes_satisfied_passes() {
    let entities = vec![
        entity("r", InjectCaseMode::Hit, 1, serde_json::json!("10.0.0.1")),
        entity("r", InjectCaseMode::Hit, 2, serde_json::json!(5.0)),
        entity(
            "r",
            InjectCaseMode::NearMiss,
            1,
            serde_json::json!("10.0.0.9"),
        ),
        entity("r", InjectCaseMode::Miss, 1, serde_json::json!("10.0.0.8")),
    ];
    let alerts = vec![
        alert("r", "10.0.0.1", "event"),
        alert("r", "5", "close:eos"),
    ];

    assert_eq!(assert_inject_modes(&entities, &alerts).unwrap(), 4);
}

/// 没有注入用例（背景流量场景）不做任何断言。
#[test]
fn no_entities_passes() {
    assert_eq!(
        assert_inject_modes(&[], &[alert("r", "10.0.0.1", "event")]).unwrap(),
        0
    );
}

/// hit 实体未产出告警 → INJ1，消息含实体值与条数/阈值诊断。
#[test]
fn hit_without_alert_reports_inj1() {
    let entities = vec![entity(
        "r",
        InjectCaseMode::Hit,
        7,
        serde_json::json!("10.0.0.6"),
    )];
    let err = assert_inject_modes(&entities, &[]).unwrap_err();
    let detail = detail_of(&err);

    assert!(detail.contains("INJ1: 1 hit"), "获取到: {detail}");
    assert!(
        detail.contains("第 7 个实体（sip=10.0.0.6）"),
        "获取到: {detail}"
    );
    assert!(detail.contains("fail 3/3"), "条数/阈值必须摊开: {detail}");
}

/// hit 实体未报警且条数低于阈值时，消息点名未达阈值的 bind。
#[test]
fn inj1_reason_names_unmet_bind() {
    let mut e = entity("r", InjectCaseMode::Hit, 1, serde_json::json!(1));
    e.steps = vec![InjectStepCount {
        bind_alias: "scan".to_string(),
        count: 2,
        threshold: 5,
    }];

    let err = assert_inject_modes(&[e], &[]).unwrap_err();
    assert!(
        detail_of(&err).contains("scan 2/5（scan 未达阈值）"),
        "获取到: {}",
        detail_of(&err)
    );
}

/// near_miss 实体产出了告警 → INJ2，消息含触发路径与时间。
#[test]
fn near_miss_with_alert_reports_inj2() {
    let entities = vec![entity(
        "data_exfil",
        InjectCaseMode::NearMiss,
        3,
        serde_json::json!("10.0.0.2"),
    )];
    let alerts = vec![alert("data_exfil", "10.0.0.2", "close:timeout")];

    let err = assert_inject_modes(&entities, &alerts).unwrap_err();
    let detail = detail_of(&err);
    assert!(
        detail.contains("INJ2: 1 near_miss/miss"),
        "获取到: {detail}"
    );
    assert!(
        detail.contains("near_miss 用例第 3 个实体（sip=10.0.0.2）"),
        "获取到: {detail}"
    );
    assert!(
        detail.contains("命中 close:timeout 路径"),
        "获取到: {detail}"
    );
}

/// 另一条规则同名实体必然报警，不影响本规则的断言口径。
#[test]
fn alerts_are_scoped_per_rule() {
    let entities = vec![entity(
        "r_a",
        InjectCaseMode::NearMiss,
        1,
        serde_json::json!("10.0.0.5"),
    )];
    let alerts = vec![alert("r_b", "10.0.0.5", "event")];

    assert_eq!(assert_inject_modes(&entities, &alerts).unwrap(), 1);
}

/// 语料级失败（上万个实体）只列前若干条明细 + 总数，避免不可读。
#[test]
fn failures_are_capped_with_total() {
    let entities: Vec<InjectEntityKey> = (0..50)
        .map(|i| entity("r", InjectCaseMode::Hit, i + 1, serde_json::json!(i)))
        .collect();

    let err = assert_inject_modes(&entities, &[]).unwrap_err();
    let detail = detail_of(&err);
    assert!(detail.contains("INJ1: 50 hit"), "获取到: {detail}");
    assert_eq!(detail.matches("不会触发规则").count(), MAX_EXAMPLES);
    assert!(detail.contains("…同码合计 50 个实体"), "获取到: {detail}");
}
