use super::*;
use crate::wfg_parser::parse_wfg;

#[test]
fn test_syntax_valid_minimal() {
    let input = r#"
use "schemas/security.wfs"
use "rules/brute_force.wfl"

#[duration=10m]
scenario brute_force_detect<seed=42> {
    background {
        stream auth_events gen 100/s
    }
    inject {
        hit<sip: 500> for brute_force_then_scan auth_events {
            use(login="failed") x 3
        }
        near_miss<sip: 200> for brute_force_then_scan auth_events {
            use(login="failed") x 2
        }
        miss<sip: 100> for brute_force_then_scan auth_events {
            use(login="success") x 1
        }
    }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    let schemas = vec![make_schema(
        "auth_events",
        vec![
            ("sip", BaseType::Ip),
            ("login", BaseType::Chars),
            ("action", BaseType::Chars),
        ],
    )];
    let wfl = make_wfl("brute_force_then_scan", vec![("fail", "auth_events")]);
    let errors = validate_wfg(&wfg, &schemas, &[wfl], false);
    assert!(
        !errors.iter().any(|e| e.code.starts_with("VN")),
        "unexpected VN errors: {:?}",
        errors
    );
}

/// VN20：旧的按比例形式在**解析期**就被拒绝，`lint` / `gen` 都拦得住。
#[test]
fn test_syntax_legacy_percent_is_rejected_at_parse_time() {
    let input = r#"
#[duration=10m]
scenario s<seed=1> {
    background { stream auth_events gen 100/s }
    inject {
        hit<30%> for rule_a auth_events {
            user seq {
                use(login="failed") with(3)
            }
        }
    }
}
"#;
    let err = parse_wfg(input).unwrap_err().report().render();
    assert!(err.contains("VN20"), "unexpected error: {err}");
}

#[test]
fn test_syntax_stream_missing_in_schema() {
    let input = r#"
#[duration=10m]
scenario s<seed=1> {
    background { stream missing_window gen 100/s }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    let schemas = vec![make_schema("auth_events", vec![])];
    let errors = validate_wfg(&wfg, &schemas, &[], false);
    assert!(
        errors.iter().any(|e| e.code == "VN3"),
        "errors: {:?}",
        errors
    );
}

#[test]
fn test_syntax_use_step_duplicate_field_rejected() {
    let input = r#"
#[duration=10m]
scenario s<seed=1> {
    background { stream auth_events gen 100/s }
    inject {
        hit<user: 50> for rule_a auth_events {
            use(login="failed", login="success") x 1
        }
    }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    let schemas = vec![make_schema("auth_events", vec![])];
    let errors = validate_wfg(&wfg, &schemas, &[], false);
    assert!(
        errors.iter().any(|e| e.code == "VN9"),
        "errors: {:?}",
        errors
    );
}

#[test]
fn test_syntax_injection_stream_must_be_declared_in_background() {
    let input = r#"
#[duration=10m]
scenario s<seed=1> {
    background { stream auth_events gen 100/s }
    inject {
        hit<user: 50> for rule_a typo_events {
            use(login="failed") x 1
        }
    }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    let schemas = vec![
        make_schema(
            "auth_events",
            vec![("user", BaseType::Chars), ("login", BaseType::Chars)],
        ),
        make_schema(
            "typo_events",
            vec![("user", BaseType::Chars), ("login", BaseType::Chars)],
        ),
    ];
    let errors = validate_wfg(&wfg, &schemas, &[], false);
    assert!(
        errors.iter().any(|e| e.code == "VN10"),
        "errors: {:?}",
        errors
    );
}

#[test]
fn test_syntax_injection_fields_must_exist_in_schema() {
    let input = r#"
#[duration=10m]
scenario s<seed=1> {
    background { stream auth_events gen 100/s }
    inject {
        hit<missing_user: 50> for rule_a auth_events {
            use(missing_login="failed") x 1
        }
    }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    let schemas = vec![make_schema("auth_events", vec![("login", BaseType::Chars)])];
    let errors = validate_wfg(&wfg, &schemas, &[], false);
    assert!(
        errors.iter().any(|e| e.code == "VN11"),
        "errors: {:?}",
        errors
    );
}

#[test]
fn test_syntax_entity_field_must_not_be_redeclared_in_use() {
    let input = r#"
#[duration=10m]
scenario s<seed=1> {
    background { stream auth_events gen 100/s }
    inject {
        hit<user: 50> for rule_a auth_events {
            use(user="alice", login="failed") x 1
        }
    }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    let schemas = vec![make_schema(
        "auth_events",
        vec![("user", BaseType::Chars), ("login", BaseType::Chars)],
    )];
    let errors = validate_wfg(&wfg, &schemas, &[], false);
    assert!(
        errors.iter().any(|e| e.code == "VN12"),
        "errors: {:?}",
        errors
    );
}

/// VN22：显式实体字段不在该 stream 的 schema 里。生成器对拿不到类型的字段只会用
/// 字符串兜底，注入会静默指向一个"看似实体"的字段。
#[test]
fn test_syntax_explicit_entity_field_must_exist_in_schema() {
    let input = r#"
#[duration=10m]
scenario s<seed=1> {
    background { stream auth_events gen 100/s }
    inject {
        hit<sip: 50> for rule_a auth_events {
            use(login="failed") x 1
        }
    }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    // schema 里没有 sip
    let schemas = vec![make_schema("auth_events", vec![("login", BaseType::Chars)])];
    let wfl = make_wfl("rule_a", vec![("a", "auth_events")]);
    let errors = validate_wfg(&wfg, &schemas, &[wfl], false);
    assert!(
        errors.iter().any(|e| e.code == "VN22"),
        "errors: {:?}",
        errors
    );
    assert!(
        !errors.iter().any(|e| e.code == "VN23"),
        "显式字段与推断一致时不该报 VN23：{:?}",
        errors
    );
}

/// VN23：单 key `match` 规则的实体就是该 key；显式写成别的字段会使"逐实体变化的字段"
/// 与"规则聚合的 key"不是同一个。
#[test]
fn test_syntax_explicit_entity_field_must_match_match_key() {
    let input = r#"
#[duration=10m]
scenario s<seed=1> {
    background { stream auth_events gen 100/s }
    inject {
        hit<user: 50> for rule_a auth_events {
            use(login="failed") x 1
        }
    }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    let schemas = vec![make_schema(
        "auth_events",
        vec![
            ("user", BaseType::Chars),
            ("sip", BaseType::Ip),
            ("login", BaseType::Chars),
        ],
    )];
    // `match<sip:1m>` → 推断实体字段 = sip
    let wfl = make_wfl("rule_a", vec![("a", "auth_events")]);
    let errors = validate_wfg(&wfg, &schemas, &[wfl], false);
    assert!(
        errors.iter().any(|e| e.code == "VN23"),
        "errors: {:?}",
        errors
    );
    assert!(
        !errors.iter().any(|e| e.code == "VN22"),
        "字段在 schema 里就不该报 VN22：{:?}",
        errors
    );
}

/// VN23（`on each`）：实体字段由 `entity(...)` 的单字段推断，显式写另一个字段同样不一致。
#[test]
fn test_syntax_explicit_entity_field_must_match_each_entity_expr() {
    let input = r#"
#[duration=10m]
scenario s<seed=1> {
    background { stream LoginWindow gen 20/s }
    inject {
        hit<username: 3> for each_alert LoginWindow {
            use(attempts=500) x 1
        }
    }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    let schemas = vec![make_schema(
        "LoginWindow",
        vec![
            ("username", BaseType::Chars),
            ("sip", BaseType::Ip),
            ("attempts", BaseType::Digit),
        ],
    )];
    // `entity(ip, e.sip)` → 推断实体字段 = sip
    let wfl = make_wfl_each("each_alert", "LoginWindow", "sip");
    let errors = validate_wfg(&wfg, &schemas, &[wfl], false);
    assert!(
        errors.iter().any(|e| e.code == "VN23"),
        "errors: {:?}",
        errors
    );
    assert!(
        !errors.iter().any(|e| e.code == "VN22"),
        "字段在 schema 里就不该报 VN22：{:?}",
        errors
    );
}

/// 多 key 规则的实体是 key 元组（设计 §3.7 第二行），显式写字段是**消歧**用法 → 不报 VN23。
#[test]
fn test_syntax_explicit_entity_field_allowed_on_multi_key_rule() {
    let input = r#"
#[duration=10m]
scenario s<seed=1> {
    background { stream auth_events gen 100/s }
    inject {
        hit<sip: 20> for rule_a auth_events {
            use(login="failed") x 1
        }
    }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    let schemas = vec![make_schema(
        "auth_events",
        vec![
            ("sip", BaseType::Ip),
            ("dport", BaseType::Digit),
            ("login", BaseType::Chars),
        ],
    )];
    let wfl = make_wfl_match("rule_a", vec![("a", "auth_events")], "sip, dport", None);
    let errors = validate_wfg(&wfg, &schemas, &[wfl], false);
    assert!(
        !errors.iter().any(|e| e.code == "VN22" || e.code == "VN23"),
        "多 key 下显式字段是消歧用法：{:?}",
        errors
    );
}

/// 多个用例各自 `for RULE`（`for` 现在是必填，不再从 `expect` 反推）。
#[test]
fn test_syntax_injection_multi_rule_cases_are_allowed() {
    let input = r#"
#[duration=10m]
scenario s<seed=1> {
    background { stream auth_events gen 100/s }
    inject {
        hit<user: 50> for rule_a auth_events {
            use(login="failed") x 1
        }
        hit<user: 20> for rule_b auth_events {
            use(login="failed") x 1
        }
    }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    let schemas = vec![make_schema(
        "auth_events",
        vec![("login", BaseType::Chars), ("sip", BaseType::Ip)],
    )];
    let wfl_a = make_wfl("rule_a", vec![("a", "auth_events")]);
    let wfl_b = make_wfl("rule_b", vec![("b", "auth_events")]);
    let errors = validate_wfg(&wfg, &schemas, &[wfl_a, wfl_b], false);
    assert!(
        !errors.iter().any(|e| e.code == "VN14"),
        "errors: {:?}",
        errors
    );
}

#[test]
fn test_syntax_injection_target_rule_must_exist() {
    let input = r#"
#[duration=10m]
scenario s<seed=1> {
    background { stream auth_events gen 100/s }
    inject {
        hit<user: 50> for missing_rule auth_events {
            use(login="failed") x 1
        }
    }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    let schemas = vec![make_schema(
        "auth_events",
        vec![("login", BaseType::Chars), ("sip", BaseType::Ip)],
    )];
    let errors = validate_wfg(&wfg, &schemas, &[], false);
    assert!(
        errors.iter().any(|e| e.code == "VN14"),
        "errors: {:?}",
        errors
    );
}

/// VN25：`spread` 不得超过场景 `#[duration]`。
#[test]
fn test_syntax_spread_over_duration_rejected() {
    for (spread, expect_vn25) in [("20m", true), ("10m", false)] {
        let input = format!(
            r#"
#[duration=10m]
scenario spread_case<seed=1> {{
    background {{ stream auth_events gen 100/s }}
    inject {{
        hit<user: 5> for rule_a auth_events {{
            use(login="failed") x 1
            spread {spread}
        }}
    }}
}}
"#
        );
        let wfg = parse_wfg(&input).unwrap();
        let schemas = vec![make_schema(
            "auth_events",
            vec![("login", BaseType::Chars), ("sip", BaseType::Ip)],
        )];
        let errors = validate_wfg(&wfg, &schemas, &[], false);
        assert_eq!(
            errors.iter().any(|e| e.code == "VN25"),
            expect_vn25,
            "spread {spread} 的 VN25 判定不符，errors: {:?}",
            errors
        );
    }
}

/// 合法的显式数量用例不产生任何 VN 错误（不受旧的 percent/seq 检查影响）。
#[test]
fn test_syntax_explicit_counts_valid() {
    let input = r#"
#[duration=10m]
scenario explicit_ok<seed=1> {
    background { stream auth_events gen 100/s }
    inject {
        hit<500> for brute_force_then_scan auth_events {
            use(login="failed") x 12
        }
    }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    let schemas = vec![make_schema(
        "auth_events",
        vec![
            ("sip", BaseType::Ip),
            ("login", BaseType::Chars),
            ("action", BaseType::Chars),
        ],
    )];
    let wfl = make_wfl("brute_force_then_scan", vec![("fail", "auth_events")]);
    let errors = validate_wfg(&wfg, &schemas, &[wfl], false);
    assert!(
        !errors.iter().any(|e| e.code.starts_with("VN")),
        "unexpected VN errors: {:?}",
        errors
    );
}

/// VN21：实体个数为 0、或某个事件组 `x 0`，都必须报错（而不是静默不生成）。
#[test]
fn test_syntax_explicit_zero_counts_rejected() {
    for (snippet, expect) in [
        (
            "hit<0> for brute_force_then_scan auth_events { use(login=\"failed\") x 1 }",
            "实体个数必须大于 0",
        ),
        (
            "hit<5> for brute_force_then_scan auth_events { use(login=\"failed\") x 0 }",
            "x 0",
        ),
    ] {
        let input = format!(
            r#"
#[duration=10m]
scenario zero_count<seed=1> {{
    background {{ stream auth_events gen 100/s }}
    inject {{ {snippet} }}
}}
"#
        );
        let wfg = parse_wfg(&input).unwrap();
        let schemas = vec![make_schema(
            "auth_events",
            vec![
                ("sip", BaseType::Ip),
                ("login", BaseType::Chars),
                ("action", BaseType::Chars),
            ],
        )];
        let wfl = make_wfl("brute_force_then_scan", vec![("fail", "auth_events")]);
        let errors = validate_wfg(&wfg, &schemas, &[wfl], false);
        assert!(
            errors
                .iter()
                .any(|e| e.code == "VN21" && e.message.contains(expect)),
            "期望 VN21 含 {expect:?}，实际: {:?}",
            errors
        );
    }
}

/// VN21：没有任何事件组的用例同样报错（数量写不下来就没有意义）。
#[test]
fn test_syntax_empty_groups_rejected() {
    let input = r#"
#[duration=10m]
scenario empty_groups<seed=1> {
    background { stream auth_events gen 100/s }
    inject {
        hit<5> for rule_a auth_events { }
    }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    let schemas = vec![make_schema("auth_events", vec![])];
    let errors = validate_wfg(&wfg, &schemas, &[], false);
    assert!(
        errors
            .iter()
            .any(|e| e.code == "VN21" && e.message.contains("至少需要一个")),
        "errors: {:?}",
        errors
    );
}

/// `--no-wfl` 下不校验规则存在性（没有规则可参照）。
#[test]
fn test_syntax_target_rule_check_skipped_when_skip_wfl() {
    let input = r#"
#[duration=10m]
scenario s<seed=1> {
    background { stream auth_events gen 100/s }
    inject {
        hit<user: 5> for missing_rule auth_events {
            use(login="failed") x 1
        }
    }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    let schemas = vec![make_schema(
        "auth_events",
        vec![("login", BaseType::Chars), ("sip", BaseType::Ip)],
    )];

    let errs_normal = validate_wfg(&wfg, &schemas, &[], false);
    assert!(
        errs_normal.iter().any(|e| e.code == "VN14"),
        "VN14 must be reported when the WFL pipeline is active: {:?}",
        errs_normal
    );

    let errs_skipped = validate_wfg(&wfg, &schemas, &[], true);
    assert!(
        !errs_skipped.iter().any(|e| e.code == "VN14"),
        "VN14 must be skipped under skip_wfl: {:?}",
        errs_skipped
    );
}

// ---------------------------------------------------------------------------
// `use from` 的记录形态（loader 解析后与 `use({...})` 同构）
// ---------------------------------------------------------------------------

/// 造一个注入用例，并把它的值来源替换成给定的 JSON（模拟 `use from` 解析结果）。
fn wfg_with_inject_json(json: serde_json::Value) -> WfgFile {
    let input = r#"
#[duration=10m]
scenario s<seed=1> {
    background { stream auth_events gen 100/s }
    inject {
        hit<login: 2> for rule_a auth_events {
            use(login="failed") x 3
        }
    }
}
"#;
    let mut wfg = parse_wfg(input).unwrap();
    let group = &mut wfg
        .syntax
        .as_mut()
        .and_then(|syntax| syntax.injection.as_mut())
        .expect("injection block")
        .cases[0]
        .groups[0];
    group.source = ValueSource::Json(json);
    wfg
}

/// 数组（多记录）不再触发 VN17——它是 `use from` 解析后的正常形态。
#[test]
fn test_use_records_array_is_accepted() {
    let wfg = wfg_with_inject_json(serde_json::json!([
        {"action": "failed"},
        {"action": "failed"}
    ]));
    let schemas = vec![make_schema(
        "auth_events",
        vec![("action", BaseType::Chars), ("login", BaseType::Chars)],
    )];
    let errors = validate_wfg(&wfg, &schemas, &[], true);
    assert!(
        !errors.iter().any(|e| e.code.starts_with("VN")),
        "记录数组应被接受: {:?}",
        errors
    );
}

/// 多记录里重复出现同一字段是正常的（每条记录都有实体键），不得报 VN9。
#[test]
fn test_use_records_repeating_a_field_is_not_vn9() {
    let wfg = wfg_with_inject_json(serde_json::json!([
        {"action": "a", "sip": "10.0.0.1"},
        {"action": "a", "sip": "10.0.0.2"},
        {"action": "a", "sip": "10.0.0.3"}
    ]));
    let schemas = vec![make_schema(
        "auth_events",
        vec![("action", BaseType::Chars), ("sip", BaseType::Ip)],
    )];
    let errors = validate_wfg(&wfg, &schemas, &[], true);
    assert!(
        !errors.iter().any(|e| e.code == "VN9"),
        "记录之间重复字段不是 VN9: {:?}",
        errors
    );
}

/// 某条记录出现 schema 之外的字段 → VN11（每条记录都要各自检查）。
#[test]
fn test_use_records_field_outside_schema_is_vn11() {
    let wfg = wfg_with_inject_json(serde_json::json!([
        {"action": "failed"},
        {"action": "failed", "nope": 1}
    ]));
    let schemas = vec![make_schema(
        "auth_events",
        vec![("action", BaseType::Chars)],
    )];
    let errors = validate_wfg(&wfg, &schemas, &[], true);
    assert!(
        errors.iter().any(|e| e.code == "VN11"),
        "记录里 schema 之外的字段应报 VN11: {:?}",
        errors
    );
}

/// 非 object 记录 / 空数组 / 标量顶层 → VN17。
#[test]
fn test_use_records_shape_errors_are_vn17() {
    let schemas = vec![make_schema(
        "auth_events",
        vec![("action", BaseType::Chars)],
    )];
    for json in [
        serde_json::json!([{"action": "failed"}, "oops"]),
        serde_json::json!([]),
        serde_json::json!(42),
    ] {
        let wfg = wfg_with_inject_json(json.clone());
        let errors = validate_wfg(&wfg, &schemas, &[], true);
        assert!(
            errors.iter().any(|e| e.code == "VN17"),
            "形态非法应报 VN17（json={json}）: {:?}",
            errors
        );
    }
}
