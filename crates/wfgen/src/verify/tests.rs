use crate::oracle::OracleAlert;
use crate::verify::{ActualAlert, verify};

#[test]
fn exact_match_passes() {
    let expected = vec![OracleAlert {
        rule_name: "r1".to_string(),
        score: 85.0,
        entity_type: "ip".to_string(),
        entity_id: "10.0.0.1".to_string(),
        origin: "event".to_string(),
        emit_time: "2024-01-01T00:05:00Z".to_string(),
        fields: vec![],
        intermediate: false,
    }];

    let actual = vec![ActualAlert {
        rule_name: "r1".to_string(),
        score: 85.0,
        entity_type: "ip".to_string(),
        entity_id: "10.0.0.1".to_string(),
        origin: "event".to_string(),
        fired_at: "2024-01-01T00:05:00Z".to_string(),
    }];

    let report = verify(&expected, &actual, 0.01, 1.0);
    assert_eq!(report.status, "pass");
    assert_eq!(report.summary.matched, 1);
    assert_eq!(report.summary.missing, 0);
    assert_eq!(report.summary.unexpected, 0);
    assert_eq!(report.summary.field_mismatch, 0);
}

#[test]
fn missing_alert_fails() {
    let expected = vec![OracleAlert {
        rule_name: "r1".to_string(),
        score: 85.0,
        entity_type: "ip".to_string(),
        entity_id: "10.0.0.1".to_string(),
        origin: "event".to_string(),
        emit_time: "2024-01-01T00:05:00Z".to_string(),
        fields: vec![],
        intermediate: false,
    }];

    let actual = vec![];

    let report = verify(&expected, &actual, 0.01, 1.0);
    assert_eq!(report.status, "fail");
    assert_eq!(report.summary.missing, 1);
}

#[test]
fn unexpected_alert_fails() {
    let expected = vec![];

    let actual = vec![ActualAlert {
        rule_name: "r1".to_string(),
        score: 85.0,
        entity_type: "ip".to_string(),
        entity_id: "10.0.0.1".to_string(),
        origin: "event".to_string(),
        fired_at: "2024-01-01T00:05:00Z".to_string(),
    }];

    let report = verify(&expected, &actual, 0.01, 1.0);
    assert_eq!(report.status, "fail");
    assert_eq!(report.summary.unexpected, 1);
}

#[test]
fn score_mismatch_fails() {
    let expected = vec![OracleAlert {
        rule_name: "r1".to_string(),
        score: 85.0,
        entity_type: "ip".to_string(),
        entity_id: "10.0.0.1".to_string(),
        origin: "event".to_string(),
        emit_time: "2024-01-01T00:05:00Z".to_string(),
        fields: vec![],
        intermediate: false,
    }];

    let actual = vec![ActualAlert {
        rule_name: "r1".to_string(),
        score: 50.0,
        entity_type: "ip".to_string(),
        entity_id: "10.0.0.1".to_string(),
        origin: "event".to_string(),
        fired_at: "2024-01-01T00:05:00Z".to_string(),
    }];

    let report = verify(&expected, &actual, 0.01, 1.0);
    assert_eq!(report.status, "fail");
    assert_eq!(report.summary.field_mismatch, 1);
}

#[test]
fn score_within_tolerance_passes() {
    let expected = vec![OracleAlert {
        rule_name: "r1".to_string(),
        score: 85.0,
        entity_type: "ip".to_string(),
        entity_id: "10.0.0.1".to_string(),
        origin: "event".to_string(),
        emit_time: "2024-01-01T00:05:00Z".to_string(),
        fields: vec![],
        intermediate: false,
    }];

    let actual = vec![ActualAlert {
        rule_name: "r1".to_string(),
        score: 85.005,
        entity_type: "ip".to_string(),
        entity_id: "10.0.0.1".to_string(),
        origin: "event".to_string(),
        fired_at: "2024-01-01T00:05:00Z".to_string(),
    }];

    let report = verify(&expected, &actual, 0.01, 1.0);
    assert_eq!(report.status, "pass");
    assert_eq!(report.summary.matched, 1);
}

#[test]
fn empty_both_passes() {
    let report = verify(&[], &[], 0.01, 1.0);
    assert_eq!(report.status, "pass");
    assert_eq!(report.summary.matched, 0);
}

#[test]
fn missing_alert_has_details() {
    let expected = vec![OracleAlert {
        rule_name: "r1".to_string(),
        score: 85.0,
        entity_type: "ip".to_string(),
        entity_id: "10.0.0.1".to_string(),
        origin: "event".to_string(),
        emit_time: "2024-01-01T00:05:00Z".to_string(),
        fields: vec![],
        intermediate: false,
    }];

    let report = verify(&expected, &[], 0.01, 1.0);
    assert_eq!(report.missing_details.len(), 1);
    assert_eq!(report.missing_details[0].rule_name, "r1");
    assert_eq!(report.missing_details[0].entity_id, "10.0.0.1");
}

#[test]
fn unexpected_alert_has_details() {
    let actual = vec![ActualAlert {
        rule_name: "r1".to_string(),
        score: 85.0,
        entity_type: "ip".to_string(),
        entity_id: "10.0.0.2".to_string(),
        origin: "event".to_string(),
        fired_at: "2024-01-01T00:05:00Z".to_string(),
    }];

    let report = verify(&[], &actual, 0.01, 1.0);
    assert_eq!(report.unexpected_details.len(), 1);
    assert_eq!(report.unexpected_details[0].entity_id, "10.0.0.2");
}

#[test]
fn score_mismatch_has_details() {
    let expected = vec![OracleAlert {
        rule_name: "r1".to_string(),
        score: 85.0,
        entity_type: "ip".to_string(),
        entity_id: "10.0.0.1".to_string(),
        origin: "event".to_string(),
        emit_time: "2024-01-01T00:05:00Z".to_string(),
        fields: vec![],
        intermediate: false,
    }];

    let actual = vec![ActualAlert {
        rule_name: "r1".to_string(),
        score: 50.0,
        entity_type: "ip".to_string(),
        entity_id: "10.0.0.1".to_string(),
        origin: "event".to_string(),
        fired_at: "2024-01-01T00:05:00Z".to_string(),
    }];

    let report = verify(&expected, &actual, 0.01, 1.0);
    assert_eq!(report.mismatch_details.len(), 1);
    assert_eq!(report.mismatch_details[0].expected_score, 85.0);
    assert_eq!(report.mismatch_details[0].actual_score, 50.0);
}

#[test]
fn test_markdown_report_format() {
    let expected = vec![OracleAlert {
        rule_name: "brute_force".to_string(),
        score: 85.0,
        entity_type: "ip".to_string(),
        entity_id: "10.0.0.1".to_string(),
        origin: "event".to_string(),
        emit_time: "2024-01-01T00:05:00Z".to_string(),
        fields: vec![],
        intermediate: false,
    }];

    let actual = vec![ActualAlert {
        rule_name: "brute_force".to_string(),
        score: 50.0,
        entity_type: "ip".to_string(),
        entity_id: "10.0.0.1".to_string(),
        origin: "event".to_string(),
        fired_at: "2024-01-01T00:05:00Z".to_string(),
    }];

    let report = verify(&expected, &actual, 0.01, 1.0);
    let md = report.to_markdown();

    assert!(md.contains("## wfgen Verify Report"), "should have header");
    assert!(md.contains("**Status**: FAIL"), "should show FAIL status");
    assert!(md.contains("### Summary"), "should have summary section");
    assert!(
        md.contains("| Metric | Count |"),
        "should have summary table"
    );
    assert!(
        md.contains("### Field Mismatches"),
        "should have mismatch section"
    );
    assert!(md.contains("brute_force"), "should contain rule name");
    assert!(md.contains("85.00"), "should contain expected score");
    assert!(md.contains("50.00"), "should contain actual score");
}

#[test]
fn test_markdown_pass_report() {
    let report = verify(&[], &[], 0.01, 1.0);
    let md = report.to_markdown();

    assert!(md.contains("**Status**: PASS"));
    // No details sections for pass
    assert!(!md.contains("### Missing"));
    assert!(!md.contains("### Unexpected"));
    assert!(!md.contains("### Field Mismatches"));
}

#[test]
fn time_mismatch_beyond_tolerance_fails() {
    let expected = vec![OracleAlert {
        rule_name: "r1".to_string(),
        score: 85.0,
        entity_type: "ip".to_string(),
        entity_id: "10.0.0.1".to_string(),
        origin: "event".to_string(),
        emit_time: "2024-01-01T00:05:00Z".to_string(),
        fields: vec![],
        intermediate: false,
    }];

    let actual = vec![ActualAlert {
        rule_name: "r1".to_string(),
        score: 85.0, // score matches
        entity_type: "ip".to_string(),
        entity_id: "10.0.0.1".to_string(),
        origin: "event".to_string(),
        fired_at: "2024-01-01T00:05:05Z".to_string(), // 5s later
    }];

    // With 1s tolerance → mismatch
    let report = verify(&expected, &actual, 0.01, 1.0);
    assert_eq!(report.status, "fail");
    assert_eq!(report.summary.field_mismatch, 1);
    assert_eq!(report.summary.matched, 0);

    // With 10s tolerance → pass
    let report = verify(&expected, &actual, 0.01, 10.0);
    assert_eq!(report.status, "pass");
    assert_eq!(report.summary.matched, 1);
}

/// 热实体（同一 `entity_id` 落几千条告警）的配对必须**不退化**。
///
/// 匹配的分组键含 `entity_id` ⇒ 单组规模 = 该实体的告警数；组内是「逐期望找最近未用」的
/// 扫描（Σ(n²)）。`parse_time_approx` 一旦被放回**内层循环**（历史实现），成本会再乘上
/// 一次 chrono 日期解析：实测 q21 语料（单组 8050）据此从 0.28s 涨到 5.37s。
/// 本用例取 3000×3000（9e6 次比较，debug 下 ~0.1s）：若解析回归进内层，它会涨到**秒级**
/// （debug 下每对一次 chrono 解析），在 CI 墙上时间上非常显眼（同时也在断言配对计数正确）。
#[test]
fn hot_group_matching_stays_fast_and_correct() {
    const N: usize = 3_000;
    let expected: Vec<OracleAlert> = (0..N)
        .map(|i| OracleAlert {
            rule_name: "hot".to_string(),
            score: 1.0,
            entity_type: "digit".to_string(),
            entity_id: "42".to_string(), // 同一个实体 → 全部落进同一组
            origin: "event".to_string(),
            emit_time: format!("2024-01-01T00:00:{:02}Z", i % 60),
            fields: vec![],
            intermediate: false,
        })
        .collect();
    let actual: Vec<ActualAlert> = (0..N)
        .map(|i| ActualAlert {
            rule_name: "hot".to_string(),
            score: 1.0,
            entity_type: "digit".to_string(),
            entity_id: "42".to_string(),
            origin: "event".to_string(),
            fired_at: format!("2024-01-01T00:00:{:02}Z", i % 60),
        })
        .collect();

    let report = verify(&expected, &actual, 0.01, 1.0);
    assert_eq!(report.summary.expected_total, N);
    assert_eq!(report.summary.matched, N, "同分布的两侧应逐条配上");
    assert_eq!(report.summary.missing + report.summary.unexpected, 0);
}
