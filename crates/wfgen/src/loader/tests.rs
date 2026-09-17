//! `use from "…"` 值文件的解析：object / 数组 / NDJSON / 各类错误。

use std::path::Path;

use super::*;

/// 造一个只含一个注入用例的 `.wfg`（`use_form` 是 `use …` 之后的原文）。
fn wfg_with_use(use_form: &str) -> WfgFile {
    let source = format!(
        r#"
#[duration=10s]
scenario use_from_probe<seed=42> {{
  background {{
    stream LoginWindow gen 10/s
  }}
  inject {{
    hit<sip: 3> for some_rule LoginWindow {{
      {use_form}
    }}
  }}
}}
"#
    );
    parse_wfg(&source).expect("parse wfg")
}

fn value_source(wfg: &WfgFile) -> &ValueSource {
    &wfg.syntax
        .as_ref()
        .and_then(|syntax| syntax.injection.as_ref())
        .expect("injection block")
        .cases[0]
        .groups[0]
        .source
}

/// 顶层 object → 就地变成 `Json(object)`（与 `use({...})` 同构）。
#[test]
fn object_file_is_inlined() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("evt.json");
    std::fs::write(&file, r#"{"sip": "10.0.0.9", "dport": 22}"#).unwrap();

    let mut wfg = wfg_with_use(&format!("use from \"{}\" x 2", file.display()));
    resolve_inject_files(&mut wfg, &dir.path().join("scenario.wfg")).unwrap();

    match value_source(&wfg) {
        ValueSource::Json(serde_json::Value::Object(map)) => {
            assert_eq!(map.get("sip"), Some(&serde_json::json!("10.0.0.9")));
            assert_eq!(map.get("dport"), Some(&serde_json::json!(22)));
        }
        other => panic!("期望内联 object，实际 {other:?}"),
    }
}

/// 顶层数组 → 原地保留为 `Json(array)`（多条记录，生成时循环取用）。
#[test]
fn array_file_keeps_records() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("evt.json");
    std::fs::write(&file, r#"[{"sip": "10.0.0.1"}, {"sip": "10.0.0.2"}]"#).unwrap();

    let mut wfg = wfg_with_use(&format!("use from \"{}\" x 5", file.display()));
    resolve_inject_files(&mut wfg, &dir.path().join("scenario.wfg")).unwrap();

    match value_source(&wfg) {
        ValueSource::Json(serde_json::Value::Array(items)) => assert_eq!(items.len(), 2),
        other => panic!("期望内联 array，实际 {other:?}"),
    }
}

/// 相对路径按 `.wfg` 所在目录解析；`_` 前缀键留给 loader 之后的展开逻辑（不在这里过滤）。
#[test]
fn relative_path_resolves_against_scenario_dir() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("raw")).unwrap();
    std::fs::write(
        dir.path().join("raw/evt.json"),
        r#"{"sip": "10.0.0.1", "_stream": "syslog"}"#,
    )
    .unwrap();

    let mut wfg = wfg_with_use("use from \"raw/evt.json\" x 1");
    resolve_inject_files(&mut wfg, &dir.path().join("scenario.wfg")).unwrap();

    match value_source(&wfg) {
        ValueSource::Json(serde_json::Value::Object(map)) => {
            assert!(
                map.contains_key("_stream"),
                "过滤留给 json_top_level_entries"
            );
        }
        other => panic!("期望内联 object，实际 {other:?}"),
    }
}

/// NDJSON（每行一个 object，空行与 `//` 注释忽略）→ 多条记录。
#[test]
fn ndjson_file_becomes_records() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("raw.ndjson");
    std::fs::write(
        &file,
        "{\"sip\": \"10.0.0.1\"}\n\n// 注释行\n{\"sip\": \"10.0.0.2\"}\n",
    )
    .unwrap();

    let mut wfg = wfg_with_use(&format!("use from \"{}\" x 4", file.display()));
    resolve_inject_files(&mut wfg, &dir.path().join("scenario.wfg")).unwrap();

    match value_source(&wfg) {
        ValueSource::Json(serde_json::Value::Array(items)) => {
            assert_eq!(items.len(), 2);
            assert_eq!(items[1].get("sip"), Some(&serde_json::json!("10.0.0.2")));
        }
        other => panic!("期望内联 array，实际 {other:?}"),
    }
}

/// 文件不存在 → Io 错误（含解析后的路径），不静默生成空字段。
#[test]
fn missing_file_errors_with_resolved_path() {
    let dir = tempfile::tempdir().unwrap();
    let mut wfg = wfg_with_use("use from \"raw/missing.json\" x 1");

    let err = resolve_inject_files(&mut wfg, &dir.path().join("scenario.wfg")).unwrap_err();
    let detail = err.detail().as_deref().unwrap_or_default();
    assert!(detail.contains("missing.json"), "获取到: {detail}");
    assert!(
        detail.contains("raw/missing.json"),
        "错误里应带原始写法: {detail}"
    );
}

/// 顶层是标量 → 明确报错。
#[test]
fn scalar_top_level_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("scalar.json");
    std::fs::write(&file, "42").unwrap();

    let mut wfg = wfg_with_use(&format!("use from \"{}\" x 1", file.display()));
    let err = resolve_inject_files(&mut wfg, &dir.path().join("scenario.wfg")).unwrap_err();
    let detail = err.detail().as_deref().unwrap_or_default();
    assert!(
        detail.contains("顶层必须是 JSON object 或 object 数组"),
        "获取到: {detail}"
    );
}

/// 数组元素不是 object（或数组为空）→ 明确报错。
#[test]
fn non_object_records_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("mixed.json");
    std::fs::write(&file, r#"[{"sip": "10.0.0.1"}, "oops"]"#).unwrap();

    let mut wfg = wfg_with_use(&format!("use from \"{}\" x 1", file.display()));
    let err = resolve_inject_files(&mut wfg, &dir.path().join("scenario.wfg")).unwrap_err();
    let detail = err.detail().as_deref().unwrap_or_default();
    assert!(
        detail.contains("元素必须是 JSON object"),
        "获取到: {detail}"
    );

    let empty = dir.path().join("empty.json");
    std::fs::write(&empty, "[]").unwrap();
    let mut wfg = wfg_with_use(&format!("use from \"{}\" x 1", empty.display()));
    let err = resolve_inject_files(&mut wfg, &dir.path().join("scenario.wfg")).unwrap_err();
    assert!(
        err.detail()
            .as_deref()
            .unwrap_or_default()
            .contains("数组为空"),
        "获取到: {}",
        err.detail().as_deref().unwrap_or_default()
    );
}

/// 既不是 JSON 也不是 NDJSON → 报错里同时给出两种口径的失败原因。
#[test]
fn unparsable_file_reports_both_attempts() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("broken.json");
    std::fs::write(&file, "not json at all").unwrap();

    let mut wfg = wfg_with_use(&format!("use from \"{}\" x 1", file.display()));
    let err = resolve_inject_files(&mut wfg, &dir.path().join("scenario.wfg")).unwrap_err();
    let detail = err.detail().as_deref().unwrap_or_default();
    assert!(
        detail.contains("不是合法 JSON，也不是合法的 NDJSON"),
        "获取到: {detail}"
    );
}

/// `--no-wfl`（`skip_wfl`）不解析值文件：整条规则/注入链路都被跳过，不因缺文件报错。
#[test]
fn skip_wfl_does_not_resolve_value_files() {
    let dir = tempfile::tempdir().unwrap();
    let scenario = dir.path().join("scenario.wfg");
    let mut wfg = wfg_with_use("use from \"raw/missing.json\" x 1");

    load_from_uses(&mut wfg, &scenario, &HashMap::new(), true)
        .expect("skip_wfl 不该去读注入值文件");
    assert!(matches!(value_source(&wfg), ValueSource::File(_)));

    let err = load_from_uses(&mut wfg, &scenario, &HashMap::new(), false).unwrap_err();
    assert!(
        err.detail()
            .as_deref()
            .unwrap_or_default()
            .contains("missing.json")
    );
}

/// 没有注入块的场景：解析是空操作（不 panic、不改任何东西）。
#[test]
fn scenario_without_injection_is_untouched() {
    let source = r#"
#[duration=10s]
scenario no_inject<seed=1> {
  background { stream LoginWindow gen 10/s }
}
"#;
    let mut wfg = parse_wfg(source).expect("parse");
    resolve_inject_files(&mut wfg, Path::new("/nonexistent/scenario.wfg")).unwrap();
}
