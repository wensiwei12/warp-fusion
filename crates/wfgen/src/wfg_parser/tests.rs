use super::parse_wfg;
use crate::wfg_ast::*;

#[test]
fn test_parse_minimal_syntax_scenario() {
    let input = r#"
#[duration=10m]
scenario brute_force_detect<seed=42> {
  traffic {
    stream auth_events gen 100/s
  }
}
"#;

    let wfg = parse_wfg(input).unwrap();
    assert_eq!(wfg.scenario.name, "brute_force_detect");
    assert_eq!(wfg.scenario.seed, 42);
    assert!(wfg.syntax.is_some());
    let syntax = wfg.syntax.as_ref().unwrap();
    assert_eq!(syntax.traffic.streams.len(), 1);
    assert_eq!(syntax.traffic.streams[0].stream, "auth_events");
    assert!(matches!(
        syntax.traffic.streams[0].rate,
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
  traffic { stream auth_events gen 50/s }
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
  traffic {
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
    let t = &wfg.syntax.as_ref().unwrap().traffic.streams;
    assert!(matches!(t[0].rate, RateExpr::Wave { .. }));
    assert!(matches!(t[1].rate, RateExpr::Burst { .. }));
    assert!(matches!(t[2].rate, RateExpr::Timeline(_)));
}

#[test]
fn test_parse_injection_and_expect_extensions() {
    let input = r#"
#[duration=30m]
scenario brute_force_detect<seed=7> {
  traffic {
    stream auth_events gen 200/s
  }

  injection {
    hit<30%> auth_events {
      user seq {
        use(login="failed") with(3)
        then use(action="port_scan") with(1)
      }
    }
    near_miss<10%> auth_events {
      user seq {
        use(login="failed") with(2)
        not(action="port_scan") within(1m)
      }
    }
    miss<60%> auth_events {
      user seq {
        use(login="success") with(1)
      }
    }
  }

  expect {
    hit(brute_force_then_scan) >= 95%
    near_miss(brute_force_then_scan) <= 1%
    miss(brute_force_then_scan) <= 0.1%
    precision(brute_force_then_scan) >= 99%
    recall(brute_force_then_scan) >= 95%
    fpr(brute_force_then_scan) <= 0.5%
    latency_p95(brute_force_then_scan) <= 2s
  }
}
"#;

    let wfg = parse_wfg(input).unwrap();
    let syntax = wfg.syntax.as_ref().unwrap();
    let inj = syntax.injection.as_ref().unwrap();
    assert!(
        wfg.scenario.injects.is_empty(),
        "new syntax injection must not be converted into ScenarioDecl.injects"
    );
    assert_eq!(inj.cases.len(), 3);
    assert_eq!(inj.cases[0].mode(), InjectCaseMode::Hit);
    assert_eq!(inj.cases[1].mode(), InjectCaseMode::NearMiss);
    assert_eq!(inj.cases[2].mode(), InjectCaseMode::Miss);
    assert_eq!(inj.cases[0].target_rule(), None);

    let InjectCase::Legacy(legacy) = &inj.cases[0] else {
        panic!("按比例的旧形态应解析为 InjectCase::Legacy");
    };
    assert_eq!(legacy.percent, 30.0);
    let steps = &legacy.seq.steps;
    assert!(matches!(steps[0], SeqStep::Use { .. }));
    assert!(matches!(steps[1], SeqStep::Use { .. }));
    let InjectCase::Legacy(near_miss) = &inj.cases[1] else {
        panic!("legacy");
    };
    assert!(matches!(near_miss.seq.steps[1], SeqStep::Not { .. }));

    let expect = syntax.expect.as_ref().unwrap();
    assert_eq!(expect.checks.len(), 7);
    assert!(matches!(expect.checks[6].metric, ExpectMetric::LatencyP95));
    assert!(matches!(expect.checks[6].value, ExpectValue::Duration(_)));
}

#[test]
fn test_parse_then_only_allows_use_step() {
    let input = r#"
#[duration=10m]
scenario invalid_then_not<seed=1> {
  traffic { stream auth_events gen 100/s }
  injection {
    near_miss<10%> auth_events {
      user seq {
        use(login="failed") with(1)
        then not(action="port_scan") within(1m)
      }
    }
  }
}
"#;

    let err = parse_wfg(input).unwrap_err().to_string();
    assert!(
        err.contains("'use' after 'then'"),
        "unexpected parse error: {err}"
    );
}

#[test]
fn test_parse_injection_case_target_rule() {
    let input = r#"
#[duration=10m]
scenario targeted<seed=1> {
  traffic { stream auth_events gen 100/s }
  injection {
    hit<30%> for brute_force auth_events {
      user seq {
        use(login="failed") with(3)
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
    assert_eq!(case.target_rule(), Some("brute_force"));
    assert_eq!(case.stream(), "auth_events");
}

#[test]
fn test_parse_comments_and_optional_semicolon() {
    let input = r#"
// header
#[duration=10m]
scenario s<seed=1> {
  traffic {
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
  traffic { stream sdm_event gen 100/s }
  injection {
    hit<100%> sdm_event {
      sip seq {
        use({
          "tenant_id": "tenant02",
          "source_finding_obj": {
            "title": "自定义威胁情报",
            "rule": { "label": "账号攻击" }
          },
          "tags": ["a", "b"],
          "unmapped": null,
          "_stream": "ignored"
        }) with(2)
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
    let InjectCase::Legacy(legacy) = &case else {
        panic!("legacy 形态");
    };
    let SeqStep::UseJson { json, count } = &legacy.seq.steps[0] else {
        panic!("应为 UseJson，实际 {:?}", legacy.seq.steps[0]);
    };
    assert_eq!(*count, 2);

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
  traffic { stream sdm_event gen 100/s }
  injection {
    hit<100%> sdm_event {
      sip seq {
        use(
          tenant_id="tenant02",
          event_id="evt-1"
        ) with(1)
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
    let InjectCase::Legacy(legacy) = &case else {
        panic!("legacy 形态");
    };
    let SeqStep::Use { predicates, count } = &legacy.seq.steps[0] else {
        panic!("应为 Use");
    };
    assert_eq!(*count, 1);
    assert_eq!(predicates.len(), 2);
}

/// 字段值为 object / array / null 时走 `AttrValue::Json`（不再被当成字符串）。
#[test]
fn test_parse_predicate_structured_values() {
    let input = r#"
#[duration=1s]
scenario structured<seed=1> {
  traffic { stream sdm_event gen 100/s }
  injection {
    hit<100%> sdm_event {
      sip seq {
        use(
          obj={"k": {"n": 1}},
          arr=[1, 2, 3],
          none=null
        ) with(1)
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
    let InjectCase::Legacy(legacy) = &case else {
        panic!("legacy 形态");
    };
    let SeqStep::Use { predicates, .. } = &legacy.seq.steps[0] else {
        panic!("应为 Use");
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
  traffic { stream sdm_event gen 100/s }
  injection {
    hit<100%> sdm_event { sip seq { use([1, 2]) with(1) } }
  }
}
"#;
    // 数组不是合法 predicate 列表、也不是合法的内联 JSON 顶层 → 解析失败
    assert!(parse_wfg(input).is_err());
}
