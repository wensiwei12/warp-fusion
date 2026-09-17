use super::*;

#[test]
fn test_syntax_extra_use_step_fails_when_rule_has_no_matching_step() {
    let input = r#"
#[duration=5s]
scenario inject_extra_use<seed=42> {
    traffic {
        stream LoginWindow gen 20/s
    }
    injection {
        hit<50%> LoginWindow {
            src_ip seq {
                use(success=false) with(5)
                use(success=true) with(1)
            }
        }
    }
    expect {
        hit(auth_fail_rule) >= 0%
    }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    let schemas = vec![make_login_schema()];
    let plans = vec![make_auth_fail_plan()];

    let err = match generate(&wfg, &schemas, &plans) {
        Ok(_) => panic!("generation should fail when use(...) count exceeds rule steps"),
        Err(err) => err,
    };
    let rendered = err.report().render().to_string();
    assert!(
        rendered.contains("exceeds rule step count"),
        "unexpected error: {rendered}"
    );
}

#[test]
fn test_syntax_distinct_close_rejects_multiple_use_steps_for_one_rule_step() {
    let input = r#"
#[duration=10m]
scenario inject_distinct_close<seed=42> {
    traffic {
        stream LoginWindow gen 20/s
    }
    injection {
        hit<25%> LoginWindow {
            src_ip seq {
                use(success=false, dport=80) with(1)
                use(success=false, dport=443) with(1)
                use(success=false, dport=8080) with(1)
            }
        }
    }
    expect {
        hit(distinct_close) >= 0%
    }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    let schemas = vec![make_login_schema()];
    let plans = vec![make_distinct_close_plan()];

    let err = match generate(&wfg, &schemas, &plans) {
        Ok(_) => panic!("generation should fail when use(...) count exceeds rule steps"),
        Err(err) => err,
    };
    let rendered = err.report().render().to_string();
    assert!(
        rendered.contains("exceeds rule step count"),
        "unexpected error: {rendered}"
    );
}

#[test]
fn test_syntax_use_steps_match_multi_step_bind_filters() {
    let input = r#"
#[duration=10m]
scenario inject_chain_attack<seed=42> {
    traffic {
        stream LoginWindow gen 20/s
    }
    injection {
        hit<25%> LoginWindow {
            src_ip seq {
                use(success=false, dport=80) with(5)
                use(success=true, dport=22) with(3)
            }
        }
    }
    expect {
        hit(chain_attack) >= 0%
    }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    let schemas = vec![make_login_schema()];
    let plans = vec![make_chain_attack_plan()];

    let result = generate(&wfg, &schemas, &plans).unwrap();

    let mut counts_by_entity: std::collections::HashMap<String, (usize, usize)> =
        std::collections::HashMap::new();
    for event in &result.events {
        let Some(entity) = event.fields.get("src_ip").and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(success) = event.fields.get("success").and_then(|v| v.as_bool()) else {
            continue;
        };
        let entry = counts_by_entity.entry(entity.to_string()).or_default();
        if success {
            entry.1 += 1;
        } else {
            entry.0 += 1;
        }
    }

    assert!(
        counts_by_entity
            .values()
            .any(|(scan_count, login_count)| *scan_count >= 5 && *login_count >= 3),
        "expected one injected entity with 5 scan and 3 login events, got {counts_by_entity:?}"
    );

    let start = "2024-01-01T00:00:00Z".parse().unwrap();
    let duration = Duration::from_secs(600);
    let oracle = run_oracle(&result.events, &plans, &start, &duration, None).unwrap();
    assert!(
        !oracle.alerts.is_empty(),
        "multi-step bind-filter hit should trigger oracle"
    );
}

#[test]
fn test_syntax_seq_entity_is_preserved_as_cluster_field() {
    let input = r#"
#[duration=5s]
scenario inject_entity<seed=42> {
    traffic {
        stream LoginWindow gen 20/s
    }
    injection {
        hit<50%> LoginWindow {
            username seq {
                use(success=false) with(5)
            }
        }
    }
    expect {
        hit(brute_force) >= 0%
    }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    assert!(wfg.scenario.injects.is_empty());

    let schemas = vec![make_login_schema()];
    let plans = vec![make_brute_force_plan()];
    let result = generate(&wfg, &schemas, &plans).unwrap();

    let entity_values: Vec<_> = result
        .events
        .iter()
        .filter_map(|event| event.fields.get("username").and_then(|v| v.as_str()))
        .filter(|username| username.starts_with("hit_username_"))
        .collect();

    assert!(
        entity_values.len() >= 50,
        "expected syntax seq entity field to be generated on inject events"
    );
}

#[test]
fn test_syntax_injection_case_targets_multiple_rules() {
    let input = r#"
#[duration=5s]
scenario inject_targets<seed=42> {
    traffic {
        stream LoginWindow gen 40/s
    }
    injection {
        hit<25%> for brute_force LoginWindow {
            src_ip seq {
                use(success=false) with(5)
            }
        }
        hit<25%> for bool_chain LoginWindow {
            username seq {
                use(success=false) with(1)
                then use(success=true) with(1)
            }
        }
    }
    expect {
        hit(brute_force) >= 0%
        hit(bool_chain) >= 0%
    }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    let schemas = vec![make_login_schema()];
    let plans = vec![make_brute_force_plan(), make_bool_chain_plan()];

    let result = generate(&wfg, &schemas, &plans).unwrap();

    let ip_targeted = result.events.iter().any(|event| {
        event
            .fields
            .get("src_ip")
            .and_then(|v| v.as_str())
            .is_some_and(|src_ip| src_ip.starts_with("10."))
    });
    let user_targeted = result.events.iter().any(|event| {
        event
            .fields
            .get("username")
            .and_then(|v| v.as_str())
            .is_some_and(|username| username.starts_with("hit_username_"))
    });

    assert!(ip_targeted, "expected brute_force-targeted inject events");
    assert!(user_targeted, "expected bool_chain-targeted inject events");
}

#[test]
fn test_syntax_injection_multi_rule_expect_without_target_fails_generation() {
    let input = r#"
#[duration=5s]
scenario inject_targets<seed=42> {
    traffic {
        stream LoginWindow gen 40/s
    }
    injection {
        hit<25%> LoginWindow {
            src_ip seq {
                use(success=false) with(5)
            }
        }
    }
    expect {
        hit(brute_force) >= 0%
        hit(bool_chain) >= 0%
    }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    let schemas = vec![make_login_schema()];
    let plans = vec![make_brute_force_plan()];

    let err = match generate(&wfg, &schemas, &plans) {
        Ok(_) => panic!("generation should fail without explicit target rule"),
        Err(err) => err,
    };
    let rendered = err.report().render().to_string();
    assert!(
        rendered.contains("requires 'for RULE'"),
        "unexpected error: {rendered}"
    );
}

#[test]
fn test_syntax_near_miss_uses_explicit_use_step_count() {
    let input = r#"
#[duration=10s]
scenario inject_nm_syntax<seed=42> {
    traffic {
        stream LoginWindow gen 100/s
    }
    injection {
        near_miss<20%> LoginWindow {
            username seq {
                use(success=false) with(2)
            }
        }
    }
    expect {
        hit(brute_force) >= 0%
    }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    let schemas = vec![make_login_schema()];
    let plans = vec![make_brute_force_plan()];

    let result = generate(&wfg, &schemas, &plans).unwrap();
    assert_eq!(result.events.len(), 1000);

    let mut by_entity: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for event in &result.events {
        let Some(username) = event.fields.get("username").and_then(|v| v.as_str()) else {
            continue;
        };
        if !username.starts_with("nm_username_") {
            continue;
        }
        assert_eq!(
            event.fields.get("success").and_then(|v| v.as_bool()),
            Some(false),
            "near_miss use(...) predicates should be applied to generated events"
        );
        *by_entity.entry(username.to_string()).or_default() += 1;
    }

    assert!(
        !by_entity.is_empty(),
        "expected near_miss inject entities with generated username prefix"
    );
    assert!(
        by_entity.values().all(|count| *count == 2),
        "near_miss syntax should honor with(2) instead of rule threshold - 1, got {by_entity:?}"
    );

    let start = "2024-01-01T00:00:00Z".parse().unwrap();
    let duration = Duration::from_secs(3600);
    let oracle = run_oracle(&result.events, &plans, &start, &duration, None).unwrap();
    assert_eq!(
        oracle.alerts.len(),
        0,
        "near_miss clusters with 2 events must not trigger threshold 5 rule"
    );
}

#[test]
fn test_syntax_miss_uses_explicit_use_step_count_and_predicates() {
    let input = r#"
#[duration=10s]
scenario inject_miss_syntax<seed=42> {
    traffic {
        stream LoginWindow gen 100/s
    }
    injection {
        miss<20%> LoginWindow {
            username seq {
                use(success=true) with(5)
            }
        }
    }
    expect {
        hit(brute_force) >= 0%
    }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    let schemas = vec![make_login_schema()];
    let plans = vec![make_brute_force_plan()];

    let result = generate(&wfg, &schemas, &plans).unwrap();
    assert_eq!(result.events.len(), 1000);

    let mut by_entity: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for event in &result.events {
        let Some(username) = event.fields.get("username").and_then(|v| v.as_str()) else {
            continue;
        };
        if !username.starts_with("miss_username_") {
            continue;
        }
        assert_eq!(
            event.fields.get("success").and_then(|v| v.as_bool()),
            Some(true),
            "miss use(...) predicates should be applied to generated events"
        );
        *by_entity.entry(username.to_string()).or_default() += 1;
    }

    assert!(
        !by_entity.is_empty(),
        "expected miss inject entities with generated username prefix"
    );
    assert!(
        by_entity.values().all(|count| *count == 1),
        "miss syntax should give each generated event a unique entity, got {by_entity:?}"
    );

    let start = "2024-01-01T00:00:00Z".parse().unwrap();
    let duration = Duration::from_secs(3600);
    let oracle = run_oracle(&result.events, &plans, &start, &duration, None).unwrap();
    assert_eq!(
        oracle.alerts.len(),
        0,
        "miss events should use unique entities and must not trigger alerts even when with(5)"
    );
}

#[test]
fn test_syntax_miss_allows_predicates_to_override_rule_filter() {
    let input = r#"
#[duration=10s]
scenario inject_miss_filter_override<seed=42> {
    traffic {
        stream LoginWindow gen 100/s
    }
    injection {
        miss<20%> LoginWindow {
            username seq {
                use(success=true) with(5)
            }
        }
    }
    expect {
        hit(auth_fail_rule) >= 0%
    }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    let schemas = vec![make_login_schema()];
    let plans = vec![make_auth_fail_plan()];

    let result = generate(&wfg, &schemas, &plans).unwrap();
    assert_eq!(result.events.len(), 1000);

    let miss_events: Vec<_> = result
        .events
        .iter()
        .filter(|event| {
            event
                .fields
                .get("username")
                .and_then(|v| v.as_str())
                .is_some_and(|username| username.starts_with("miss_username_"))
        })
        .collect();

    assert!(
        !miss_events.is_empty(),
        "expected miss inject events with generated username prefix"
    );
    assert!(
        miss_events
            .iter()
            .all(|event| event.fields.get("success").and_then(|v| v.as_bool()) == Some(true)),
        "miss predicates should override auth_fail_rule success=false filter"
    );

    let start = "2024-01-01T00:00:00Z".parse().unwrap();
    let duration = Duration::from_secs(3600);
    let oracle = run_oracle(&result.events, &plans, &start, &duration, None).unwrap();
    assert_eq!(
        oracle.alerts.len(),
        0,
        "miss filter overrides should not trigger auth_fail_rule alerts"
    );
}

// ---------------------------------------------------------------------------
// 新语法：显式实体数量（hit<[field:]N> ... use ... x N）
// ---------------------------------------------------------------------------

/// `hit<N> ... x M` 生成的事件数恰好是 `N × M`——不再由「配额 × 比例 ÷ 每实体条数」推导。
#[test]
fn test_explicit_entity_count_is_honored() {
    // LoginWindow 100/s × 1s = 100 条配额；hit<10> x 5 = 10 实体 × 5 = 50 条注入
    let input = r#"
#[duration=1s]
scenario explicit_count<seed=42> {
    traffic {
        stream LoginWindow gen 100/s
    }
    injection {
        hit<10> for auth_fail_rule LoginWindow {
            use(success=false) x 5
        }
    }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    let schemas = vec![make_login_schema()];
    let plans = vec![make_auth_fail_plan()];

    let result = generate(&wfg, &schemas, &plans).unwrap();

    let injected: Vec<_> = result
        .events
        .iter()
        .filter(|e| {
            e.fields
                .get("src_ip")
                .and_then(|v| v.as_str())
                .map(|s| s.starts_with("10.") && s.len() <= 15)
                .unwrap_or(false)
        })
        .collect();
    let entities: std::collections::HashSet<&str> = injected
        .iter()
        .filter_map(|e| e.fields.get("src_ip").and_then(|v| v.as_str()))
        .collect();

    assert_eq!(
        injected.len(),
        50,
        "10 个实体 × 每个 5 条 = 50 条注入事件；实际 {}",
        injected.len()
    );
    assert_eq!(entities.len(), 10, "实体数应恰好为 hit<10> 写的 10");

    let syntax = wfg.syntax.as_ref().unwrap();
    let case = &syntax.injection.as_ref().unwrap().cases[0];
    let crate::wfg_ast::InjectCase::Explicit(explicit) = case else {
        panic!("新语法应解析为 InjectCase::Explicit");
    };
    assert_eq!(explicit.entity_count, 10);
    assert_eq!(explicit.entity_field, None, "实体键省略时从规则推断");
    assert_eq!(explicit.groups.len(), 1);
    assert_eq!(explicit.groups[0].count, 5);
}

/// 显式实体键（`hit<sip: N>`）被记入 AST（多 key 规则或需要消歧时使用）。
#[test]
fn test_explicit_entity_field_parsed() {
    let input = r#"
#[duration=1s]
scenario explicit_field<seed=1> {
    traffic { stream LoginWindow gen 100/s }
    injection {
        hit<src_ip: 7> for auth_fail_rule LoginWindow { use(success=false) x 1 }
    }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    let case = &wfg
        .syntax
        .as_ref()
        .unwrap()
        .injection
        .as_ref()
        .unwrap()
        .cases[0];
    let crate::wfg_ast::InjectCase::Explicit(explicit) = case else {
        panic!("Explicit");
    };
    assert_eq!(explicit.entity_field.as_deref(), Some("src_ip"));
    assert_eq!(explicit.entity_count, 7);
}
