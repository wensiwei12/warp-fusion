use super::parse_wfg;
use crate::wfg_ast::*;

#[test]
fn test_parse_minimal_syntax_scenario() {
    let input = r#"
#[duration=10m]
scenario brute_force_detect<seed=42> {
  background {
    stream auth_events gen 100/s
  }
}
"#;

    let wfg = parse_wfg(input).unwrap();
    assert_eq!(wfg.scenario.name, "brute_force_detect");
    assert_eq!(wfg.scenario.seed, 42);
    assert!(wfg.syntax.is_some());
    let syntax = wfg.syntax.as_ref().unwrap();
    assert_eq!(syntax.background.streams.len(), 1);
    assert_eq!(syntax.background.streams[0].stream, "auth_events");
    assert!(matches!(
        syntax.background.streams[0].rate,
        RateExpr::Constant(_)
    ));
}

#[test]
fn test_parse_use_declarations() {
    let input = r#"
use "../schemas/security.wfs"
use "../rules/brute_force.wfl"

#[duration=10m]
scenario s<seed=1> {
  background { stream auth_events gen 50/s }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    assert_eq!(wfg.uses.len(), 2);
    assert_eq!(wfg.uses[0].path, "../schemas/security.wfs");
    assert_eq!(wfg.uses[1].path, "../rules/brute_force.wfl");
}

#[test]
fn test_parse_rate_expressions_wave_burst_timeline() {
    let input = r#"
#[duration=10m]
scenario rates<seed=2> {
  background {
    stream s1 gen wave(base=80/s, amp=20/s, period=2m, shape=triangle)
    stream s2 gen burst(base=40/s, peak=300/s, every=3m, hold=20s)
    stream s3 gen timeline {
      0m..2m=20/s
      2m..4m=60/s
    }
  }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    let t = &wfg.syntax.as_ref().unwrap().background.streams;
    assert!(matches!(t[0].rate, RateExpr::Wave { .. }));
    assert!(matches!(t[1].rate, RateExpr::Burst { .. }));
    assert!(matches!(t[2].rate, RateExpr::Timeline(_)));
}

/// `entity <window>.<field> zipf(...)`（设计 §10 实体分布）进 AST。
#[test]
fn test_parse_entity_distribution() {
    let input = r#"
#[duration=10s]
scenario dist<seed=7> {
    background {
        stream LoginWindow gen 400/s
        entity LoginWindow.src_ip zipf(pool=8, exponent=1.5, fresh=0.2)
    }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    let block = &wfg.syntax.as_ref().unwrap().background;
    assert_eq!(block.streams.len(), 1, "分布声明不占 stream");
    assert_eq!(block.entities.len(), 1);
    let dist = &block.entities[0];
    assert_eq!(dist.window, "LoginWindow");
    assert_eq!(dist.field, "src_ip");
    assert_eq!(dist.pool, 8);
    assert!((dist.exponent - 1.5).abs() < 1e-9);
    assert!((dist.fresh - 0.2).abs() < 1e-9);

    // 只写 `pool` 时 `exponent` / `fresh` 取默认（1.0 / 0.0）。
    let input = r#"
#[duration=10s]
scenario dist<seed=7> {
    background {
        stream LoginWindow gen 10/s
        entity LoginWindow.src_ip zipf(pool=4)
    }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    let dist = &wfg.syntax.as_ref().unwrap().background.entities[0];
    assert_eq!(dist.pool, 4);
    assert!((dist.exponent - 1.0).abs() < 1e-9);
    assert!(dist.fresh.abs() < 1e-9);

    // 缺 `pool` → 解析期报错（必填参数）。
    let bad = r#"
#[duration=10s]
scenario dist<seed=7> {
    background {
        stream LoginWindow gen 10/s
        entity LoginWindow.src_ip zipf(exponent=1.5)
    }
}
"#;
    assert!(parse_wfg(bad).is_err(), "缺 pool 应报错");
}

/// `join <window> as <key> { use … x N }`（设计 §9 跨流注入）进 AST。
#[test]
fn test_parse_join_block() {
    let input = r#"
#[duration=10s]
scenario cross<seed=3> {
    background { stream person_events gen 5/s }
    inject {
        hit<id: 4> for person_creates_auction person_events {
            use(name="p") x 1
            join auction_events as seller {
                use(price=42) x 2
                then use(price=7) x 1
            }
        }
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
    assert_eq!(case.groups.len(), 1, "join 块不占事件步骤位");
    assert_eq!(case.joins.len(), 1);
    let join = &case.joins[0];
    assert_eq!(join.window, "auction_events");
    assert_eq!(join.key_field, "seller");
    assert_eq!(join.groups.len(), 2);
    assert_eq!(join.groups[0].count, 2);
    assert_eq!(join.groups[1].count, 1);
}

#[test]
fn test_parse_injection_extensions() {
    let input = r#"
#[duration=30m]
scenario brute_force_detect<seed=7> {
  background {
    stream auth_events gen 200/s
  }

  inject {
    hit<user: 500> for brute_force_then_scan auth_events {
      use(login="failed") x 3
      then use(action="port_scan") x 1
    }
    near_miss<user: 200> for brute_force_then_scan auth_events {
      use(login="failed") x 2
    }
    miss<user: 100> for brute_force_then_scan auth_events {
      use(login="success") x 1
    }
  }
}
"#;

    let wfg = parse_wfg(input).unwrap();
    let syntax = wfg.syntax.as_ref().unwrap();
    let inj = syntax.injection.as_ref().unwrap();
    assert_eq!(inj.cases.len(), 3);
    assert_eq!(inj.cases[0].mode, InjectCaseMode::Hit);
    assert_eq!(inj.cases[1].mode, InjectCaseMode::NearMiss);
    assert_eq!(inj.cases[2].mode, InjectCaseMode::Miss);

    let hit = &inj.cases[0];
    assert_eq!(hit.target_rule, "brute_force_then_scan");
    assert_eq!(hit.stream, "auth_events");
    assert_eq!(hit.entity_field.as_deref(), Some("user"));
    assert_eq!(hit.entity_count, 500);
    assert_eq!(hit.groups.len(), 2);
    assert_eq!(hit.groups[0].count, 3);
    assert_eq!(hit.groups[1].count, 1);
    assert!(matches!(hit.groups[0].source, ValueSource::Predicates(_)));
    assert_eq!(hit.spread, None);
}

/// `then` 后面必须跟一个 `use ... x N` 事件组（或 `without(...)` 约束），
/// 不能是任意 token。
#[test]
fn test_parse_then_requires_event_group() {
    let input = r#"
#[duration=10m]
scenario invalid_then<seed=1> {
  background { stream auth_events gen 100/s }
  inject {
    near_miss<user: 10> for rule_a auth_events {
      use(login="failed") x 1
      then 42
    }
  }
}
"#;

    let err = parse_wfg(input).unwrap_err().to_string();
    assert!(err.contains("event group"), "unexpected parse error: {err}");
}

/// VN20：旧的否定步骤 `not(...) within(...)` 报错并指名 `without(...)`。
#[test]
fn test_legacy_not_step_is_rejected_with_without_hint() {
    let input = r#"
#[duration=10m]
scenario invalid_then_not<seed=1> {
  background { stream auth_events gen 100/s }
  inject {
    near_miss<user: 10> for rule_a auth_events {
      use(login="failed") x 1
      then not(action="port_scan") within(1m)
    }
  }
}
"#;

    let err = parse_wfg(input).unwrap_err().to_string();
    assert!(err.contains("VN20"), "unexpected parse error: {err}");
    assert!(
        err.contains("without"),
        "错误信息应指明新写法 `without(...)`: {err}"
    );
}

#[test]
fn test_parse_injection_case_target_rule() {
    let input = r#"
#[duration=10m]
scenario targeted<seed=1> {
  background { stream auth_events gen 100/s }
  inject {
    hit<user: 30> for brute_force auth_events {
      use(login="failed") x 3
    }
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
    assert_eq!(case.target_rule, "brute_force");
    assert_eq!(case.stream, "auth_events");
}

#[test]
fn test_parse_comments_and_optional_semicolon() {
    let input = r#"
// header
#[duration=10m]
scenario s<seed=1> {
  background {
    stream auth_events gen 100/s; // optional semicolon
  }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    assert_eq!(wfg.scenario.name, "s");
}

#[test]
fn test_legacy_syntax_rejected() {
    let input = r#"
scenario legacy seed 1 {
  time "2024-01-01T00:00:00Z" duration 1h
  total 100
  stream s1 : W 10/s
}
"#;
    assert!(parse_wfg(input).is_err());
}

// ---------------------------------------------------------------------------
// 结构化值 / 整份 JSON 内联（use({...})）与 use(...) 内空白
// ---------------------------------------------------------------------------

/// `use({...})`：整份 JSON 内联，顶层对象即整组字段；值可多层嵌套。
#[test]
fn test_parse_use_whole_json_inline() {
    let input = r#"
#[duration=1s]
scenario obj_inline<seed=1> {
  background { stream sdm_event gen 100/s }
  inject {
    hit<sip: 5> for sdm_rule sdm_event {
      use({
        "tenant_id": "tenant02",
        "source_finding_obj": {
          "title": "自定义威胁情报",
          "rule": { "label": "账号攻击" }
        },
        "tags": ["a", "b"],
        "unmapped": null,
        "_stream": "ignored"
      }) x 2
    }
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
    let ValueSource::Json(json) = &case.groups[0].source else {
        panic!("应为 ValueSource::Json，实际 {:?}", case.groups[0].source);
    };
    assert_eq!(case.groups[0].count, 2);

    // `_` 前缀的内部字段被忽略
    let entries = json_top_level_entries(json).expect("顶层应为 object");
    let keys: Vec<&str> = entries.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(keys.len(), 4, "实际: {keys:?}");
    assert!(!keys.contains(&"_stream"));

    // 多层嵌套与数组/ null 原样保留
    let by_key = |k: &str| entries.iter().find(|(n, _)| n == k).map(|(_, v)| v);
    assert_eq!(
        by_key("source_finding_obj").and_then(|v| v.pointer("/rule/label")),
        Some(&serde_json::json!("账号攻击"))
    );
    assert_eq!(by_key("tags"), Some(&serde_json::json!(["a", "b"])));
    assert_eq!(by_key("unmapped"), Some(&serde_json::Value::Null));
}

/// `use(` 之后允许换行与缩进（整份 JSON 内联必然是多行排版）。
#[test]
fn test_parse_use_allows_newline_after_paren() {
    let input = r#"
#[duration=1s]
scenario multi_line<seed=1> {
  background { stream sdm_event gen 100/s }
  inject {
    hit<sip: 1> for sdm_rule sdm_event {
      use(
        tenant_id="tenant02",
        event_id="evt-1"
      ) x 1
    }
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
    let ValueSource::Predicates(predicates) = &case.groups[0].source else {
        panic!("应为 Predicates");
    };
    assert_eq!(case.groups[0].count, 1);
    assert_eq!(predicates.len(), 2);
}

/// 字段值为 object / array / null 时走 `AttrValue::Json`（不再被当成字符串）。
#[test]
fn test_parse_predicate_structured_values() {
    let input = r#"
#[duration=1s]
scenario structured<seed=1> {
  background { stream sdm_event gen 100/s }
  inject {
    hit<sip: 1> for sdm_rule sdm_event {
      use(
        obj={"k": {"n": 1}},
        arr=[1, 2, 3],
        none=null
      ) x 1
    }
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
    let ValueSource::Predicates(predicates) = &case.groups[0].source else {
        panic!("应为 Predicates");
    };
    let value_of = |name: &str| {
        predicates
            .iter()
            .find(|p| p.field == name)
            .map(|p| &p.value)
    };
    assert_eq!(
        value_of("obj"),
        Some(&AttrValue::Json(serde_json::json!({"k": {"n": 1}})))
    );
    assert_eq!(
        value_of("arr"),
        Some(&AttrValue::Json(serde_json::json!([1, 2, 3])))
    );
    assert_eq!(
        value_of("none"),
        Some(&AttrValue::Json(serde_json::Value::Null))
    );
}

/// 顶层不是 object 的 `use([...])` 必须被解析层拒绝（避免静默无字段）。
#[test]
fn test_reject_use_json_array_toplevel() {
    let input = r#"
#[duration=1s]
scenario arr<seed=1> {
  background { stream sdm_event gen 100/s }
  inject {
    hit<sip: 1> for sdm_rule sdm_event { use([1, 2]) x 1 }
  }
}
"#;
    // 数组不是合法 predicate 列表、也不是合法的内联 JSON 顶层 → 解析失败
    assert!(parse_wfg(input).is_err());
}

/// `spread <duration>` 与 `use from "<file>"` 进入 AST（文件读取由 loader 负责）。
#[test]
fn test_parse_spread_and_use_from_file() {
    let input = r#"
#[duration=10m]
scenario spread_from<seed=1> {
  background { stream sdm_event gen 100/s }
  inject {
    hit<sip: 20> for sdm_rule sdm_event {
      use from "raw/big.ndjson" x 3
      spread 5m
    }
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
    assert_eq!(case.groups[0].count, 3);
    assert_eq!(
        case.groups[0].source,
        ValueSource::File("raw/big.ndjson".to_string())
    );
    assert_eq!(case.spread, Some(std::time::Duration::from_secs(300)));
}

// ---------------------------------------------------------------------------
// VN20：旧的按比例注入语法在解析期就被拒绝
// ---------------------------------------------------------------------------

/// 新关键字 `inject { … }`（与 `background` 成对）正常解析。
#[test]
fn test_inject_block_keyword_is_accepted() {
    let input = r#"
#[duration=10m]
scenario inject_keyword<seed=1> {
  background { stream auth_events gen 100/s }
  inject {
    hit<sip: 3> for rule_a auth_events {
      use(action="failed") x 2
    }
  }
}
"#;

    let wfg = parse_wfg(input).expect("`inject` 块应被接受");
    let cases = &wfg
        .syntax
        .as_ref()
        .and_then(|syntax| syntax.injection.as_ref())
        .expect("injection block")
        .cases;
    assert_eq!(cases.len(), 1);
}

/// VN20：旧的块关键字 `traffic` 已改名为 `background`，必须报错并指出改写方向。
#[test]
fn test_legacy_traffic_keyword_is_rejected_with_vn20() {
    let input = r#"
#[duration=10m]
scenario legacy_traffic<seed=1> {
  traffic { stream auth_events gen 100/s }
}
"#;

    let err = parse_wfg(input).unwrap_err().to_string();
    assert!(err.contains("VN20"), "unexpected parse error: {err}");
    assert!(
        err.contains("`background`"),
        "错误信息应指明新关键字: {err}"
    );
}

/// VN20：旧的块关键字 `injection` 已改名为 `inject`，必须报错并指出改写方向。
#[test]
fn test_legacy_injection_keyword_is_rejected_with_vn20() {
    let input = r#"
#[duration=10m]
scenario legacy_keyword<seed=1> {
  background { stream auth_events gen 100/s }
  injection {
    hit<sip: 3> for rule_a auth_events {
      use(action="failed") x 2
    }
  }
}
"#;

    let err = parse_wfg(input).unwrap_err().to_string();
    assert!(err.contains("VN20"), "unexpected parse error: {err}");
    assert!(err.contains("`inject`"), "错误信息应指明新关键字: {err}");
}

/// VN20：`hit<20%> ... with(N)` 已移除，必须报错并给出改写方向。
#[test]
fn test_legacy_percent_form_is_rejected_with_vn20() {
    let input = r#"
#[duration=10m]
scenario legacy_percent<seed=1> {
  background { stream auth_events gen 100/s }
  inject {
    hit<20%> auth_events {
      user seq {
        use(login="failed") with(3)
      }
    }
  }
}
"#;

    let err = parse_wfg(input).unwrap_err().to_string();
    assert!(err.contains("VN20"), "unexpected parse error: {err}");
    assert!(
        err.contains("hit<sip: 500>"),
        "错误信息应给出改写形态: {err}"
    );
}

/// 三个模式走同一条判定——旧语法不能只在 `hit` 上报错。
#[test]
fn test_legacy_percent_form_is_rejected_for_all_modes() {
    for mode in ["hit", "near_miss", "miss"] {
        let input = format!(
            r#"
#[duration=10m]
scenario legacy_{mode}<seed=1> {{
  background {{ stream auth_events gen 100/s }}
  inject {{
    {mode}<20%> auth_events {{ user seq {{ use(login="failed") with(3) }} }}
  }}
}}
"#
        );
        let err = parse_wfg(&input).unwrap_err().to_string();
        assert!(err.contains("VN20"), "[{mode}] unexpected error: {err}");
    }
}

// ---------------------------------------------------------------------------
// `without(...)`：构造约束（设计 §3.8）
// ---------------------------------------------------------------------------

/// `without(...)` 进 AST：谓词与 `within` 都保留，且**不占** `groups` 的位。
#[test]
fn test_parse_without_step_into_ast() {
    let input = r#"
#[duration=10m]
scenario has_not_step<seed=1> {
  background { stream auth_events gen 100/s }
  inject {
    hit<sip: 20> for scan_then_xfer auth_events {
      use(action="scan") x 3
      without(action="login") within 30s
    }
  }
}
"#;
    let case = &wfg_case(input);
    assert_eq!(case.groups.len(), 1, "`without` 不应占 use 步骤位");
    assert_eq!(case.withouts.len(), 1);
    let wa = &case.withouts[0];
    assert_eq!(wa.predicates.len(), 1);
    assert_eq!(wa.predicates[0].field, "action");
    assert_eq!(
        wa.predicates[0].value,
        AttrValue::String("login".to_string())
    );
    assert_eq!(wa.within, Some(std::time::Duration::from_secs(30)));
}

/// `within` 省略 → 判定窗取目标规则 `match` 的窗口（此处只验证解析为 `None`）。
#[test]
fn test_parse_without_within_defaults_to_none() {
    let input = r#"
#[duration=10m]
scenario has_not_step<seed=1> {
  background { stream auth_events gen 100/s }
  inject {
    hit<sip: 20> for scan_then_xfer auth_events {
      use(action="scan") x 3
      without(action="login")
    }
  }
}
"#;
    let case = &wfg_case(input);
    assert_eq!(case.withouts.len(), 1);
    assert_eq!(case.withouts[0].within, None);
}

/// 可带可选 `then`；且 `without` 与 `use` 的相对位置无语义（两者都收进各自的列表）。
#[test]
fn test_parse_without_accepts_optional_then_and_any_position() {
    let input = r#"
#[duration=10m]
scenario has_not_step<seed=1> {
  background { stream auth_events gen 100/s }
  inject {
    hit<sip: 20> for scan_then_xfer auth_events {
      without(action="login") within 1m
      use(action="scan") x 3
      then use(action="xfer") x 1
      then without(action="logout")
    }
  }
}
"#;
    let case = &wfg_case(input);
    assert_eq!(case.groups.len(), 2);
    assert_eq!(case.withouts.len(), 2);
    assert_eq!(case.withouts[0].predicates[0].field, "action");
    assert_eq!(case.withouts[1].predicates[0].field, "action");
    assert_eq!(case.withouts[1].within, None);
}

/// VN20：旧的 `<field> seq { … }` 块必须被指名拒绝。
#[test]
fn test_legacy_field_seq_block_is_rejected_with_vn20() {
    let input = r#"
#[duration=10m]
scenario legacy_seq<seed=1> {
  background { stream auth_events gen 100/s }
  inject {
    hit<sip: 5> for rule_a auth_events {
      user seq {
        use(login="failed") x 3
      }
    }
  }
}
"#;
    let err = parse_wfg(input).unwrap_err().to_string();
    assert!(err.contains("VN20"), "unexpected parse error: {err}");
    assert!(err.contains("seq"), "错误信息应指明旧的 `seq` 块: {err}");
}

/// VN20：旧的条数写法 `with(N)` 必须被指名拒绝（新写法是 `x N`）。
#[test]
fn test_legacy_with_count_is_rejected_with_vn20() {
    let input = r#"
#[duration=10m]
scenario legacy_with<seed=1> {
  background { stream auth_events gen 100/s }
  inject {
    hit<sip: 5> for rule_a auth_events {
      use(login="failed") with(3)
    }
  }
}
"#;
    let err = parse_wfg(input).unwrap_err().to_string();
    assert!(err.contains("VN20"), "unexpected parse error: {err}");
    assert!(err.contains("x N"), "错误信息应指明新写法 `x N`: {err}");
}

/// 取第一个 injection case 的便捷函数。
fn wfg_case(input: &str) -> InjectCase {
    parse_wfg(input)
        .expect("scenario should parse")
        .syntax
        .and_then(|syntax| syntax.injection)
        .expect("inject block")
        .cases
        .into_iter()
        .next()
        .expect("at least one inject case")
}

// ---------------------------------------------------------------------------
// `replay <window> { use from "<file>" }`（设计 §8）
// ---------------------------------------------------------------------------

/// `replay` 进 AST：窗口名 + 文件路径；可写多条。
#[test]
fn test_parse_replay_stmts_into_ast() {
    let input = r#"
#[duration=10m]
scenario replay_case<seed=1> {
  background { stream auth_events gen 100/s }
  replay auth_events { use from "raw/a.ndjson" }
  replay auth_events { use from "raw/b.ndjson"; }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    let syntax = wfg.syntax.as_ref().unwrap();
    assert_eq!(syntax.replays.len(), 2);
    assert_eq!(syntax.replays[0].window, "auth_events");
    assert_eq!(syntax.replays[0].file, "raw/a.ndjson");
    assert!(syntax.replays[0].records.is_none(), "解析期不读文件");
    assert_eq!(syntax.replays[1].file, "raw/b.ndjson");
    assert!(syntax.injection.is_none(), "replay 与 inject 互相独立");
}

/// `replay` 可与 `inject` 并存，顺序无关。
#[test]
fn test_replay_coexists_with_inject_block() {
    let input = r#"
#[duration=10m]
scenario both<seed=1> {
  background { stream auth_events gen 100/s }
  replay auth_events { use from "raw/a.ndjson" }
  inject {
    hit<sip: 3> for rule_a auth_events {
      use(action="failed") x 1
    }
  }
}
"#;
    let wfg = parse_wfg(input).unwrap();
    let syntax = wfg.syntax.as_ref().unwrap();
    assert_eq!(syntax.replays.len(), 1);
    assert_eq!(syntax.injection.as_ref().unwrap().cases.len(), 1);
}

/// `replay` 不写条数：写成 `x N` 报 VN20 并指向 `inject`。
#[test]
fn test_replay_count_is_rejected_with_vn20() {
    let input = r#"
#[duration=10m]
scenario replay_count<seed=1> {
  background { stream auth_events gen 100/s }
  replay auth_events { use from "raw/a.ndjson" x 3 }
}
"#;
    let err = parse_wfg(input).unwrap_err().to_string();
    assert!(err.contains("VN20"), "unexpected parse error: {err}");
    assert!(err.contains("条数"), "错误信息应说明条数：{err}");
}

/// `replay` 的值来源只能是文件：内联值报 VN20 并指向 `inject`。
#[test]
fn test_replay_inline_value_is_rejected_with_vn20() {
    let input = r#"
#[duration=10m]
scenario replay_inline<seed=1> {
  background { stream auth_events gen 100/s }
  replay auth_events { use(action="failed") }
}
"#;
    let err = parse_wfg(input).unwrap_err().to_string();
    assert!(err.contains("VN20"), "unexpected parse error: {err}");
    assert!(err.contains("use from"), "错误信息应指明写法：{err}");
}
