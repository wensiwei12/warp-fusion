use super::*;
use crate::wfg_ast::ValueSource;
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

/// VN1：`background` 至少要有一条 stream（没有背景就没有流量可言）。
#[test]
fn test_vn1_empty_background_rejected() {
    let input = r#"
#[duration=10s]
scenario s<seed=1> {
    background { }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    let errors = validate_wfg(&wfg, &[], &[], false);
    assert!(
        errors.iter().any(|e| e.code == "VN1"),
        "空 background 应报 VN1: {errors:?}"
    );
}

/// VN2：stream 速率必须大于 0。
#[test]
fn test_vn2_nonpositive_rate_rejected() {
    let input = r#"
#[duration=10s]
scenario s<seed=1> {
    background { stream LoginWindow gen 0/s }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    let errors = validate_wfg(&wfg, &[], &[], false);
    assert!(
        errors.iter().any(|e| e.code == "VN2"),
        "rate = 0 应报 VN2: {errors:?}"
    );
}

/// VN30 的**形状**错误：空 `join` 块、块内 `x 0` → VN21（与主体事件组同口径）。
#[test]
fn test_vn30_join_block_shape_errors() {
    let rule = "
rule p_joins_a {
    events { p : person_events }
    on each p -> score(10)
    join auction_events within [p.timestamp, <bucket_end(p.timestamp, 5s)] on p.id == auction_events.seller emit at bucket_end(p.timestamp, 5s)
    entity(digit, p.id)
    yield alerts(id = p.id)
}";
    let schemas = vec![
        make_schema("person_events", vec![("id", BaseType::Digit)]),
        make_schema(
            "auction_events",
            vec![("seller", BaseType::Digit), ("price", BaseType::Digit)],
        ),
    ];
    let wfl = wf_lang::parse_wfl(rule).unwrap();

    for (join_body, why) in [
        ("join auction_events as seller { }", "空 join 块"),
        (
            "join auction_events as seller { use(price=1) x 0 }",
            "join 块内 x 0",
        ),
    ] {
        let input = format!(
            r#"
#[duration=10s]
scenario s<seed=1> {{
    background {{ stream person_events gen 5/s }}
    inject {{
        hit<id: 2> for p_joins_a person_events {{
            use(id=1) x 1
            {join_body}
        }}
    }}
}}
"#
        );
        let wfg = parse_wfg(&input).unwrap();
        let errors = validate_wfg(&wfg, &schemas, std::slice::from_ref(&wfl), false);
        assert!(
            errors.iter().any(|e| e.code == "VN21"),
            "{why}: 应报 VN21: {errors:?}"
        );
    }
}

/// `--no-wfl`（`skip_wfl`）：规则相关检查让位——join 块不再按规则 join 校验，
/// 但 schema 相关的检查（目标窗存在）仍然做。
#[test]
fn test_vn30_join_skips_rule_checks_when_no_wfl() {
    let schemas = vec![
        make_schema("person_events", vec![("id", BaseType::Digit)]),
        make_schema("auction_events", vec![("seller", BaseType::Digit)]),
    ];
    let input = r#"
#[duration=10s]
scenario s<seed=1> {
    background { stream person_events gen 5/s }
    inject {
        hit<id: 2> for whatever_rule person_events {
            use(id=1) x 1
            join auction_events as seller { use(seller=1) x 1 }
        }
    }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    // 没有 WFL 文件：VN14/VN30 的规则匹配检查跳过，不报错。
    let errors = validate_wfg(&wfg, &schemas, &[], true);
    assert!(
        !errors.iter().any(|e| e.code == "VN30" || e.code == "VN14"),
        "skip_wfl 下不应报规则相关错误: {errors:?}"
    );

    // 目标窗不在 schema 里仍要报（与 WFL 无关）。
    let bad = r#"
#[duration=10s]
scenario s<seed=1> {
    background { stream person_events gen 5/s }
    inject {
        hit<id: 2> for whatever_rule person_events {
            use(id=1) x 1
            join nope_events as seller { use(seller=1) x 1 }
        }
    }
}
"#;
    let wfg = parse_wfg(bad).unwrap();
    let errors = validate_wfg(&wfg, &schemas, &[], true);
    assert!(
        errors.iter().any(|e| e.code == "VN30"),
        "目标窗不在 schema 应报 VN30: {errors:?}"
    );
}

/// VN31：`entity <window>.<field> zipf(...)`（设计 §10）的静态一致性——窗口/字段存在、
/// 类型可承载实体、参数取值、重复声明、以及与注入实体的**值域预算**。
#[test]
fn test_vn31_entity_dist_checks() {
    let schemas = vec![make_schema(
        "LoginWindow",
        vec![("src_ip", BaseType::Ip), ("ok", BaseType::Bool)],
    )];
    // `background` 块内容 → 校验结果
    let check = |background_body: &str| {
        let src = format!(
            r#"
#[duration=10s]
scenario s<seed=1> {{
    background {{ {background_body} }}
}}
"#
        );
        let wfg = parse_wfg(&src).unwrap();
        validate_wfg(&wfg, &schemas, &[], false)
    };

    // 合法：不报 VN31
    let errs = check("stream LoginWindow gen 10/s  entity LoginWindow.src_ip zipf(pool=8)");
    assert!(
        !errs.iter().any(|e| e.code == "VN31"),
        "合法声明不应报 VN31: {errs:?}"
    );

    // 各类非法
    for (decl, why) in [
        (
            "stream LoginWindow gen 10/s  entity Nope.src_ip zipf(pool=8)",
            "目标窗不在 schema",
        ),
        (
            "stream LoginWindow gen 10/s  entity LoginWindow.nope zipf(pool=8)",
            "字段不在 schema",
        ),
        (
            "stream LoginWindow gen 10/s  entity LoginWindow.ok zipf(pool=8)",
            "字段类型不支持（bool）",
        ),
        (
            "stream LoginWindow gen 10/s  entity LoginWindow.src_ip zipf(pool=0)",
            "pool = 0",
        ),
        (
            "stream LoginWindow gen 10/s  entity LoginWindow.src_ip zipf(pool=8, fresh=1.5)",
            "fresh 越界",
        ),
        (
            "stream LoginWindow gen 10/s  entity LoginWindow.src_ip zipf(pool=8)  entity LoginWindow.src_ip zipf(pool=4)",
            "重复声明",
        ),
        (
            "stream LoginWindow gen 10/s  entity LoginWindow.src_ip zipf(pool=8388609)",
            "值域预算超出 24 位空间",
        ),
        (
            "stream OtherWindow gen 10/s  entity LoginWindow.src_ip zipf(pool=8)",
            "该窗口没有 background stream",
        ),
    ] {
        let errs = check(decl);
        assert!(
            errs.iter().any(|e| e.code == "VN31"),
            "{why}: 应报 VN31，实际 {errs:?}"
        );
    }
}

/// VN30：`join <window> as <key>` 必须能匹配到规则的 join 子句（目标窗 + 右侧连接键），
/// 且形态是缺省 inner。
#[test]
fn test_vn30_join_must_match_rule_join_clause() {
    // 规则里 join 的右窗是 auction_events、连接键是 seller。
    let rule = "
rule person_creates_auction {
    events { p : person_events }
    on each p -> score(10)
    join auction_events within [p.timestamp, <bucket_end(p.timestamp, 5s)]
        on p.id == auction_events.seller
        emit at bucket_end(p.timestamp, 5s)
    entity(digit, p.id)
    yield alerts(id = p.id)
}";
    let schemas = vec![
        make_schema("person_events", vec![("id", BaseType::Digit)]),
        make_schema(
            "auction_events",
            vec![("seller", BaseType::Digit), ("price", BaseType::Digit)],
        ),
    ];
    let wfl = wf_lang::parse_wfl(rule).unwrap();

    let ok = r#"
#[duration=10s]
scenario s<seed=1> {
    background { stream person_events gen 5/s }
    inject {
        hit<id: 2> for person_creates_auction person_events {
            use(id=1) x 1
            join auction_events as seller { use(price=1) x 1 }
        }
    }
}
"#;
    let wfg = parse_wfg(ok).unwrap();
    let errors = validate_wfg(&wfg, &schemas, std::slice::from_ref(&wfl), false);
    assert!(
        !errors.iter().any(|e| e.code == "VN30"),
        "匹配上的 join 不应报 VN30: {errors:?}"
    );

    for (decl, why) in [
        (
            "join auction_events as wrong_key { use(price=1) x 1 }",
            "连接键字段写错",
        ),
        (
            "join bid_events as seller { use(price=1) x 1 }",
            "目标窗写错",
        ),
    ] {
        let input = format!(
            r#"
#[duration=10s]
scenario s<seed=1> {{
    background {{ stream person_events gen 5/s }}
    inject {{
        hit<id: 2> for person_creates_auction person_events {{
            use(id=1) x 1
            {decl}
        }}
    }}
}}
"#
        );
        let wfg = parse_wfg(&input).unwrap();
        let errors = validate_wfg(&wfg, &schemas, std::slice::from_ref(&wfl), false);
        let vn30: Vec<_> = errors.iter().filter(|e| e.code == "VN30").collect();
        assert!(!vn30.is_empty(), "{why}: 应报 VN30: {errors:?}");
    }
}

/// VN30：v1 只支持 **deferred**（`emit at`）join。即时 inner join 要求右行在驱动事件
/// 被处理时就已可见（右事件必须更早），而 `within` 下界常常就是左事件时间——两者冲突，
/// 造出来的数据会时好时坏，因此明确拒绝。
#[test]
fn test_vn30_join_requires_deferred_emit_at() {
    let rule = "
rule p_joins_a {
    events { p : person_events }
    on each p -> score(10)
    join auction_events within [p.timestamp, <bucket_end(p.timestamp, 5s)] on p.id == auction_events.seller
    entity(digit, p.id)
    yield alerts(id = p.id)
}";
    let schemas = vec![
        make_schema("person_events", vec![("id", BaseType::Digit)]),
        make_schema(
            "auction_events",
            vec![("seller", BaseType::Digit), ("price", BaseType::Digit)],
        ),
    ];
    let wfl = wf_lang::parse_wfl(rule).unwrap();
    let input = r#"
#[duration=10s]
scenario s<seed=1> {
    background { stream person_events gen 5/s }
    inject {
        hit<id: 2> for p_joins_a person_events {
            use(id=1) x 1
            join auction_events as seller { use(price=1) x 1 }
        }
    }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    let errors = validate_wfg(&wfg, &schemas, &[wfl], false);
    assert!(
        errors
            .iter()
            .any(|e| e.code == "VN30" && e.message.contains("emit at")),
        "非 deferred join 应报 VN30: {errors:?}"
    );
}

/// VN30：`snapshot`（无 `within`）形态**放行**——右事件前挪 1ns 即可在驱动事件处理时可见
/// （q3/q20 形状）；但 `snapshot` + `within`、以及 `asof` 仍拒绝。
#[test]
fn test_vn30_snapshot_join_is_supported() {
    let schemas = vec![
        make_schema("bid_events", vec![("auction", BaseType::Digit)]),
        make_schema(
            "auction_events",
            vec![("id", BaseType::Digit), ("category", BaseType::Digit)],
        ),
    ];
    let check = |join_clause: &str| {
        let rule = format!(
            "
rule bid_expands {{
    events {{ b : bid_events }}
    on each b -> score(10)
    {join_clause}
    entity(digit, b.auction)
    yield alerts(id = b.auction)
}}"
        );
        let wfl = wf_lang::parse_wfl(&rule).expect("rule parse");
        let input = "
#[duration=10s]
scenario s<seed=1> {
    background { stream bid_events gen 5/s }
    inject {
        hit<auction: 2> for bid_expands bid_events {
            use(auction=1) x 1
            join auction_events as id { use(category=10) x 1 }
        }
    }
}
";
        let wfg = parse_wfg(input).unwrap();
        let errors = validate_wfg(&wfg, &schemas, &[wf_lang::parse_wfl(&rule).unwrap()], false);
        (wfl, errors)
    };

    // snapshot（无 within）→ 放行
    let (_wfl, errors) = check("join auction_events snapshot on b.auction == auction_events.id");
    assert!(
        !errors.iter().any(|e| e.code == "VN30"),
        "snapshot join 应被支持：{errors:?}"
    );

    // asof → 拒绝（形态未支持）
    let (_wfl, errors) =
        check("join auction_events asof within 5s on b.auction == auction_events.id");
    assert!(
        errors
            .iter()
            .any(|e| e.code == "VN30" && e.message.contains("形态不支持")),
        "asof 应报 VN30：{errors:?}"
    );

    // 即时 inner（无 emit at）→ 拒绝
    let (_wfl, errors) = check("join auction_events within 5s on b.auction == auction_events.id");
    assert!(
        errors.iter().any(|e| e.code == "VN30"),
        "无 emit at 的 inner 应报 VN30：{errors:?}"
    );

    // 占位（保持 asof 分支后的断言结构）
}

/// VN30 的字段检查落在**目标窗** schema 上；连接键由生成器写，重复声明按 VN12 报。
#[test]
fn test_vn30_join_fields_checked_against_target_window() {
    let rule = "
rule p_joins_a {
    events { p : person_events }
    on each p -> score(10)
    join auction_events within [p.timestamp, <bucket_end(p.timestamp, 5s)]
        on p.id == auction_events.seller
        emit at bucket_end(p.timestamp, 5s)
    entity(digit, p.id)
    yield alerts(id = p.id)
}";
    let schemas = vec![
        make_schema("person_events", vec![("id", BaseType::Digit)]),
        make_schema("auction_events", vec![("seller", BaseType::Digit)]),
    ];
    let wfl = wf_lang::parse_wfl(rule).unwrap();

    // 字段不在目标窗 schema → VN11；在 use 里重复连接键 → VN12。
    let input = r#"
#[duration=10s]
scenario s<seed=1> {
    background { stream person_events gen 5/s }
    inject {
        hit<id: 2> for p_joins_a person_events {
            use(id=1) x 1
            join auction_events as seller { use(nope=1) x 1 }
        }
    }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    let errors = validate_wfg(&wfg, &schemas, std::slice::from_ref(&wfl), false);
    assert!(
        errors.iter().any(|e| e.code == "VN11"),
        "目标窗里没有的字段应报 VN11: {errors:?}"
    );

    let input = r#"
#[duration=10s]
scenario s<seed=1> {
    background { stream person_events gen 5/s }
    inject {
        hit<id: 2> for p_joins_a person_events {
            use(id=1) x 1
            join auction_events as seller { use(seller=1) x 1 }
        }
    }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    let errors = validate_wfg(&wfg, &schemas, &[wfl], false);
    assert!(
        errors.iter().any(|e| e.code == "VN12"),
        "在 use 里重复 join 连接键应报 VN12: {errors:?}"
    );
}

/// VN29：注解键白名单——`#[...]` 只认 `duration`，`<...>` 只认 `seed`。
/// 注解列表是泛化解析的，其余键此前被**静默忽略**（文档里声明「未实现」的
/// `tick` / `rows` / `emit` 正是这一类）。
#[test]
fn test_vn29_unknown_annotation_keys_rejected() {
    let input = r#"
#[duration=10m, tick=1s]
scenario s<seed=1, rows=10> {
    background { stream auth_events gen 100/s }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    let schemas = vec![make_schema("auth_events", vec![("sip", BaseType::Ip)])];
    let errors = validate_wfg(&wfg, &schemas, &[], false);
    let vn29: Vec<_> = errors.iter().filter(|e| e.code == "VN29").collect();
    assert_eq!(vn29.len(), 2, "期望 tick 与 rows 各报一条：{errors:?}");
    assert!(vn29.iter().any(|e| e.message.contains("'tick'")));
    assert!(vn29.iter().any(|e| e.message.contains("'rows'")));
}

/// VN29 的值类型检查：写错值类型此前会静默退回默认值（`duration` 60s / `seed` 0）。
#[test]
fn test_vn29_annotation_value_types_are_checked() {
    for (decl, want_kind) in [
        ("#[duration=10]", "数字"), // `10` 不是时长字面量 → 曾静默退回 60s
        ("#[duration=abc]", "字符串"),
    ] {
        let input = format!(
            r#"
{decl}
scenario s<seed=1> {{
    background {{ stream auth_events gen 100/s }}
}}
"#
        );
        let wfg = parse_wfg(&input).unwrap();
        let schemas = vec![make_schema("auth_events", vec![("sip", BaseType::Ip)])];
        let errors = validate_wfg(&wfg, &schemas, &[], false);
        let vn29: Vec<_> = errors.iter().filter(|e| e.code == "VN29").collect();
        assert_eq!(vn29.len(), 1, "{decl}: {errors:?}");
        assert!(
            vn29[0].message.contains(want_kind),
            "{decl}: 消息应点出值类型 {want_kind}，实际 {}",
            vn29[0].message
        );
    }

    // `seed` 必须是数字（字符串此前静默退回 0）。
    let input = r#"
#[duration=10m]
scenario s<seed="abc"> {
    background { stream auth_events gen 100/s }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    let schemas = vec![make_schema("auth_events", vec![("sip", BaseType::Ip)])];
    let errors = validate_wfg(&wfg, &schemas, &[], false);
    let vn29: Vec<_> = errors.iter().filter(|e| e.code == "VN29").collect();
    assert_eq!(vn29.len(), 1, "{errors:?}");
    assert!(vn29[0].message.contains("非负数字"), "{}", vn29[0].message);
}

/// VN29 的反面：`#[duration=10m]` + `<seed=42>` 放行。
#[test]
fn test_vn29_valid_annotations_pass() {
    let input = r#"
#[duration=2m]
scenario s<seed=42> {
    background { stream auth_events gen 100/s }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    let schemas = vec![make_schema("auth_events", vec![("sip", BaseType::Ip)])];
    let errors = validate_wfg(&wfg, &schemas, &[], false);
    assert!(
        !errors.iter().any(|e| e.code == "VN29"),
        "合法注解不应报 VN29: {errors:?}"
    );
    assert_eq!(wfg.scenario.seed, 42);
    assert_eq!(
        wfg.scenario.time_clause.duration,
        std::time::Duration::from_secs(120)
    );
}

/// VN28：`wave` / `burst` / `timeline` 三个速率形态语法已定、**语义未实现**——降级时只取
/// `base=`（`timeline` 取第一段）当常量速率，生成结果与写法不符且不报错（压测强度静默失效）。
/// 三个形态各拦一次，消息要点名 stream 与形态。
#[test]
fn test_vn28_unimplemented_rate_shapes_are_rejected() {
    for (decl, kind) in [
        ("wave(base=80/s, amp=40/s, period=2m)", "wave(...)"),
        (
            "burst(base=40/s, peak=300/s, every=3m, hold=20s)",
            "burst(...)",
        ),
        (
            "timeline { 0m..2m=20/s\n            2m..4m=60/s\n        }",
            "timeline",
        ),
    ] {
        let input = format!(
            r#"
#[duration=10m]
scenario s<seed=1> {{
    background {{ stream auth_events gen {decl} }}
}}
"#
        );
        let wfg = parse_wfg(&input).unwrap_or_else(|e| panic!("{kind}: 解析应通过: {e:?}"));
        let schemas = vec![make_schema(
            "auth_events",
            vec![("sip", BaseType::Ip), ("login", BaseType::Chars)],
        )];
        let errors = validate_wfg(&wfg, &schemas, &[], false);
        let vn28: Vec<_> = errors.iter().filter(|e| e.code == "VN28").collect();
        assert_eq!(vn28.len(), 1, "{kind}: 期望 1 条 VN28，实际 {errors:?}");
        assert!(
            vn28[0].message.contains("auth_events") && vn28[0].message.contains(kind),
            "{kind}: 消息应点名 stream 与形态，实际 {}",
            vn28[0].message
        );
    }
}

/// VN28 的反面：常量速率不受影响（含 `/m`、`/h` 单位）。
#[test]
fn test_vn28_constant_rate_passes() {
    for decl in ["100/s", "6000/m", "360000/h"] {
        let input = format!(
            r#"
#[duration=10m]
scenario s<seed=1> {{
    background {{ stream auth_events gen {decl} }}
}}
"#
        );
        let wfg = parse_wfg(&input).unwrap();
        let schemas = vec![make_schema("auth_events", vec![("sip", BaseType::Ip)])];
        let errors = validate_wfg(&wfg, &schemas, &[], false);
        assert!(
            !errors.iter().any(|e| e.code == "VN28"),
            "{decl}: 常量速率不应报 VN28: {errors:?}"
        );
    }
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

/// VN12（`on each` 推断出的实体键字段）：用例头**没写**实体字段时，`use` 里写实体键
/// 同样是静默失效 —— 生成器 `build_event_fields_with_predicates` 里 key_overrides 优先级
/// 最高，会把 `use` 给的值覆盖成实体 id 派生值，数据里根本不是作者写的那个数。
#[test]
fn test_syntax_inferred_entity_key_field_must_not_be_redeclared_in_use() {
    let input = r#"
#[duration=10m]
scenario s<seed=1> {
    background { stream bid_events gen 100/s }
    inject {
        hit<1> for q2_mod_123 bid_events {
            use(auction=123, price=100) x 1
        }
    }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    let schemas = vec![make_schema(
        "bid_events",
        vec![("auction", BaseType::Digit), ("price", BaseType::Digit)],
    )];
    // `on each b -> …` + `entity(digit, b.auction)` → 推断实体字段 = auction
    let wfl = make_wfl_each("q2_mod_123", "bid_events", "auction");
    let errors = validate_wfg(&wfg, &schemas, &[wfl], false);
    assert!(
        errors.iter().any(|e| e.code == "VN12"),
        "推断实体字段在 use 里重复应报 VN12: {:?}",
        errors
    );
}

/// VN12（非实体键字段不拦）：`use` 里写**非**实体键字段照旧放行。
#[test]
fn test_syntax_non_key_field_in_use_is_allowed() {
    let input = r#"
#[duration=10m]
scenario s<seed=1> {
    background { stream bid_events gen 100/s }
    inject {
        hit<1> for q2_mod_123 bid_events {
            use(price=100, channel="G") x 1
        }
    }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    let schemas = vec![make_schema(
        "bid_events",
        vec![
            ("auction", BaseType::Digit),
            ("price", BaseType::Digit),
            ("channel", BaseType::Chars),
        ],
    )];
    let wfl = make_wfl_each("q2_mod_123", "bid_events", "auction");
    let errors = validate_wfg(&wfg, &schemas, &[wfl], false);
    assert!(
        !errors.iter().any(|e| e.code == "VN12"),
        "非实体字段不该报 VN12: {:?}",
        errors
    );
}

/// VN12（`match` 规则的实体键字段）：生成器无条件把规则的实体键字段写进事件，`use` 里再写
/// 它同样是被覆盖的静默失效——注意这里被拦的是**规则的 key**，不是 `entity(...)`。
#[test]
fn test_syntax_match_key_must_not_be_redeclared_in_use() {
    let input = r#"
#[duration=10m]
scenario s<seed=1> {
    background { stream auth_events gen 100/s }
    inject {
        hit<1> for rule_a auth_events {
            use(sip=1, dport=22) x 1
        }
    }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    let schemas = vec![make_schema(
        "auth_events",
        vec![("sip", BaseType::Ip), ("dport", BaseType::Digit)],
    )];
    // match<sip, dport> 双 key；`use(sip=…)` 会被生成器覆盖 → VN12
    let wfl = make_wfl_match("rule_a", vec![("a", "auth_events")], "sip, dport", None);
    let errors = validate_wfg(&wfg, &schemas, &[wfl], false);
    assert!(
        errors.iter().any(|e| e.code == "VN12"),
        "use 里重复 match key 应报 VN12: {:?}",
        errors
    );
}

/// VN12 的**误报护栏**（join-then-key）：`match<seller>` 而 `entity(…, b.auction)` 时，
/// 生成器覆盖的是规则 key `seller`，`entity(...)` 的 `auction` 并不在覆盖之列——
/// 拿"推断的实体字段"当口径会把本来能生效的 `use(auction=…)` 误拒。
#[test]
fn test_syntax_entity_field_not_in_rule_keys_is_allowed_in_use() {
    let input = r#"
#[duration=10m]
scenario s<seed=1> {
    background { stream bid_events gen 100/s }
    inject {
        hit<1> for rule_j bid_events {
            use(auction=123, price=100) x 1
        }
    }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    let schemas = vec![make_schema(
        "bid_events",
        vec![
            ("auction", BaseType::Digit),
            ("seller", BaseType::Digit),
            ("price", BaseType::Digit),
        ],
    )];
    // match<seller>（规则 key）+ entity(ip, b.auction)（实体字段）：两者不同
    let wfl = make_wfl_match(
        "rule_j",
        vec![("b", "bid_events")],
        "seller",
        Some("auction"),
    );
    let errors = validate_wfg(&wfg, &schemas, &[wfl], false);
    assert!(
        !errors.iter().any(|e| e.code == "VN12"),
        "entity(...) 字段不在规则 key 里时不该报 VN12（生成器不覆盖它）: {:?}",
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

/// VN24：`use` 事件组数超过规则的事件步骤数（每个 `use ... x N` 对应一个步骤）。
#[test]
fn test_syntax_use_groups_must_not_exceed_rule_steps() {
    let input = r#"
#[duration=10m]
scenario s<seed=1> {
    background { stream auth_events gen 100/s }
    inject {
        hit<sip: 5> for rule_a auth_events {
            use(login="failed") x 1
            use(login="failed") x 1
        }
    }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    let schemas = vec![make_schema(
        "auth_events",
        vec![("sip", BaseType::Ip), ("login", BaseType::Chars)],
    )];
    // `match<sip:1m> { on event { a | count >= 1; } }` → 1 个事件步骤
    let wfl = make_wfl("rule_a", vec![("a", "auth_events")]);
    let errors = validate_wfg(&wfg, &schemas, &[wfl], false);
    assert!(
        errors.iter().any(|e| e.code == "VN24"),
        "errors: {:?}",
        errors
    );
}

/// VN24 的「步骤数」口径必须与编译产物一致（跱模块不变量）：用编译器推导的
/// `event_steps`（`on each` 则按生成器合成的 1 步）当 oracle，对每种规则形态验证边界
/// ——组数 == 步骤数放行、+1 报 VN24。链形态的 `not` 步骤不计入（编译器把它交给 L2 的
/// `SeqPlan`），这条由第三个用例锁定。
#[test]
fn vn24_step_count_matches_compiled_event_steps() {
    const WIN: &str = "auth_events";
    let cases: &[(&str, &str)] = &[
        (
            "match 单步",
            r#"rule probe_rule {
    events { a : auth_events }
    match<sip : 1m> { on event { a | count >= 1; } }
    -> score(1)
    entity(ip, a.sip)
    yield alerts()
}"#,
        ),
        (
            "match 三步",
            r#"rule probe_rule {
    events { a : auth_events }
    match<sip : 1m> { on event { a | count >= 1; a | count >= 1; a | count >= 1; } }
    -> score(1)
    entity(ip, a.sip)
    yield alerts()
}"#,
        ),
        (
            "seq 链含 not（not 不计入）",
            r#"rule probe_rule {
    events { a : auth_events }
    match<sip : 1m> { on event seq { has a; not has a; has a within 10m; } }
    -> score(1)
    entity(ip, a.sip)
    yield alerts()
}"#,
        ),
        (
            "on each（生成器合成 1 步）",
            r#"rule probe_rule {
    events { a : auth_events }
    on each a -> score(1)
    entity(ip, a.sip)
    yield alerts()
}"#,
        ),
    ];

    for (name, rule_src) in cases {
        let schemas = vec![
            make_schema(
                WIN,
                vec![
                    ("sip", BaseType::Ip),
                    ("retry", BaseType::Digit),
                    ("login", BaseType::Chars),
                ],
            ),
            make_schema("alerts", vec![]),
        ];
        let wfl = wf_lang::parse_wfl(rule_src).unwrap_or_else(|e| panic!("{name}: {e}"));
        let plans = wf_lang::compile_wfl(&wfl, &schemas)
            .unwrap_or_else(|e| panic!("{name} 编译失败: {e:?}"));
        // oracle：编译器的事件步骤数；`on each` 的步骤由生成器合成，故为 1。
        let step_count = plans
            .iter()
            .find(|plan| plan.name == "probe_rule")
            .map(|plan| {
                if plan.each_plan.is_some() {
                    1
                } else {
                    plan.match_plan.event_steps.len()
                }
            })
            .unwrap_or_else(|| panic!("{name}: 找不到 probe_rule"));

        assert!(
            !vn24_probe(&schemas, rule_src, step_count),
            "{name}: use 组数 == 步骤数 {step_count} 时不该报 VN24"
        );
        assert!(
            vn24_probe(&schemas, rule_src, step_count + 1),
            "{name}: use 组数 > 步骤数 {step_count} 时必须报 VN24"
        );
    }
}

/// 造一个「`for probe_rule` + 指定个数 `use` 组」的场景，返回是否报了 VN24。
fn vn24_probe(schemas: &[wf_lang::WindowSchema], rule_src: &str, groups: usize) -> bool {
    let win = schemas[0].name.as_str();
    let groups_src: String = (0..groups)
        .map(|i| format!("            use(retry={i}) x 1\n"))
        .collect();
    let wfg_src = format!(
        "#[duration=10m]\nscenario s<seed=1> {{\n    background {{ stream {win} gen 100/s }}\n    inject {{\n        hit<sip: 5> for probe_rule {win} {{\n{groups_src}        }}\n    }}\n}}\n"
    );
    let wfg = parse_wfg(&wfg_src).unwrap();
    let wfl = wf_lang::parse_wfl(rule_src).unwrap();
    let errors = validate_wfg(&wfg, schemas, &[wfl], false);
    errors.iter().any(|e| e.code == "VN24")
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

/// 多记录里重复出现同一字段是正常的（每条记录都带实体键字段），不得报 VN9。
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

// ---------------------------------------------------------------------------
// `without(...)` 构造约束的校验（设计 §3.8）
// ---------------------------------------------------------------------------

/// 造一个「1 个 `use` 组 + 指定条 `without` 子句」的场景，返回校验错误。
fn without_probe(
    schemas: &[wf_lang::WindowSchema],
    rules: &[wf_lang::ast::WflFile],
    withouts: &str,
) -> Vec<String> {
    let src = format!(
        "#[duration=10m]\nscenario s<seed=1> {{\n    background {{ stream auth_events gen 100/s }}\n    inject {{\n        hit<user: 5> for probe_rule auth_events {{\n            use(login=\"failed\") x 1\n{withouts}        }}\n    }}\n}}\n"
    );
    let wfg = parse_wfg(&src).unwrap();
    validate_wfg(&wfg, schemas, rules, false)
        .into_iter()
        .map(|e| format!("{}: {}", e.code, e.message))
        .collect()
}

/// VN25：`without ... within` 不得超过场景 `#[duration]`。
#[test]
fn test_syntax_without_within_over_duration_rejected() {
    let schemas = vec![make_schema(
        "auth_events",
        vec![("user", BaseType::Chars), ("action", BaseType::Chars)],
    )];
    for (within, expect_vn25) in [("20m", true), ("10m", false)] {
        let errors = without_probe(
            &schemas,
            &[],
            &format!("            without(action=\"scan\") within {within}\n"),
        );
        assert_eq!(
            errors.iter().any(|e| e.starts_with("VN25")),
            expect_vn25,
            "without within {within} 的 VN25 判定不符，errors: {errors:?}"
        );
    }
}

/// `without` 的谓词写错字段名 → VN11（否则会静默变成“删不掉该事件”）。
#[test]
fn test_syntax_without_field_outside_schema_is_vn11() {
    let schemas = vec![make_schema(
        "auth_events",
        vec![("user", BaseType::Chars), ("action", BaseType::Chars)],
    )];
    let errors = without_probe(&schemas, &[], "            without(nope=\"x\")\n");
    assert!(
        errors
            .iter()
            .any(|e| e.starts_with("VN11") && e.contains("nope")),
        "errors: {errors:?}"
    );
}

/// `without` 的谓词重复同一字段 → VN9；重复实体键字段 → VN12。
#[test]
fn test_syntax_without_duplicate_and_entity_field_rejected() {
    let schemas = vec![make_schema(
        "auth_events",
        vec![("user", BaseType::Chars), ("action", BaseType::Chars)],
    )];
    let errors = without_probe(
        &schemas,
        &[],
        "            without(action=\"a\", action=\"b\", user=\"u\")\n",
    );
    assert!(
        errors.iter().any(|e| e.starts_with("VN9")),
        "重复字段应报 VN9: {errors:?}"
    );
    assert!(
        errors.iter().any(|e| e.starts_with("VN12")),
        "重复实体键字段应报 VN12: {errors:?}"
    );
}

/// `without` 不占 use 步骤位：组数在步数内、但 `without` 写多条也不报 VN24。
#[test]
fn test_syntax_without_not_counted_toward_rule_steps() {
    let schemas = vec![
        make_schema(
            "auth_events",
            vec![("sip", BaseType::Ip), ("login", BaseType::Chars)],
        ),
        make_schema("alerts", vec![]),
    ];
    let wfl = wf_lang::parse_wfl(
        r#"rule probe_rule {
    events { a : auth_events }
    match<sip : 1m> { on event { a | count >= 1; } }
    -> score(1)
    entity(ip, a.sip)
    yield alerts()
}"#,
    )
    .unwrap();
    let src = "#[duration=10m]\nscenario s<seed=1> {\n    background { stream auth_events gen 100/s }\n    inject {\n        hit<sip: 5> for probe_rule auth_events {\n            use(login=\"failed\") x 1\n            without(login=\"ok\")\n            without(login=\"ok2\")\n            without(login=\"ok3\")\n        }\n    }\n}\n";
    let wfg = parse_wfg(src).unwrap();
    let errors = validate_wfg(&wfg, &schemas, &[wfl], false);
    assert!(
        !errors.iter().any(|e| e.code == "VN24"),
        "`without` 不得计入 VN24 的步骤数: {:?}",
        errors
    );
}

// ---------------------------------------------------------------------------
// VN27：实体 id 总数不得超过 24 位地址空间
// ---------------------------------------------------------------------------

/// 造一个只带注入、规则与 schema 极简的场景（VN27 与 WFL 无关，传空规则即可）。
fn entity_id_budget_probe(uses: &str) -> Vec<String> {
    let src = format!(
        "#[duration=10m]\nscenario s<seed=1> {{\n    background {{ stream auth_events gen 1/s }}\n    inject {{\n{uses}    }}\n}}\n"
    );
    let wfg = parse_wfg(&src).unwrap();
    let schemas = vec![make_schema(
        "auth_events",
        vec![("sip", BaseType::Ip), ("login", BaseType::Chars)],
    )];
    validate_wfg(&wfg, &schemas, &[], true)
        .into_iter()
        .map(|e| format!("{}: {}", e.code, e.message))
        .collect()
}

/// `hit` / `near_miss` 每个实体一个 id：总数 = 实体个数。边界取上限两侧（含相等）。
#[test]
fn test_syntax_entity_id_budget_hit_boundary() {
    const LIMIT: u64 = 1 << 24;
    for (count, expect_vn27) in [(LIMIT - 1, false), (LIMIT, true), (LIMIT + 1, true)] {
        let errors = entity_id_budget_probe(&format!(
            "        hit<sip: {count}> for rule_a auth_events {{\n            use(login=\"x\") x 1\n        }}\n"
        ));
        assert_eq!(
            errors.iter().any(|e| e.starts_with("VN27")),
            expect_vn27,
            "hit<sip: {count}> 的 VN27 判定不符，errors: {errors:?}"
        );
    }
}

/// `miss` 每个事件一个独立键：总数 = 实体个数 × ΣN。
///
/// 这里单个用例的实体个数只占上限的四分之一，乘上 `x 4` 后刚好触顶——口径写错（漏乘）就抓不住。
#[test]
fn test_syntax_entity_id_budget_miss_multiplies_by_events() {
    const LIMIT: u64 = 1 << 24;
    for (count, per_entity, expect_vn27) in [(LIMIT / 4, 3, false), (LIMIT / 4, 4, true)] {
        let errors = entity_id_budget_probe(&format!(
            "        miss<sip: {count}> for rule_a auth_events {{\n            use(login=\"x\") x {per_entity}\n        }}\n"
        ));
        assert_eq!(
            errors.iter().any(|e| e.starts_with("VN27")),
            expect_vn27,
            "miss<sip: {count}> x {per_entity} 的 VN27 判定不符，errors: {errors:?}"
        );
    }
}

/// 多用例**累加**（实体 id 分段是场景级的，不是用例级的）：单看任何一个都不超，合起来超。
#[test]
fn test_syntax_entity_id_budget_accumulates_across_cases() {
    const LIMIT: u64 = 1 << 24;
    let case = |mode: &str| {
        format!(
            "        {mode}<sip: {}> for rule_a auth_events {{\n            use(login=\"x\") x 1\n        }}\n",
            LIMIT / 2
        )
    };

    let one = entity_id_budget_probe(&case("hit"));
    assert!(
        !one.iter().any(|e| e.starts_with("VN27")),
        "单个用例只占一半，不该报 VN27: {one:?}"
    );

    let two = entity_id_budget_probe(&format!("{}{}", case("hit"), case("near_miss")));
    assert!(
        two.iter().any(|e| e.starts_with("VN27")),
        "两个用例各占一半、合计触顶，必须报 VN27: {two:?}"
    );
}
