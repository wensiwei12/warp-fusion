//! `use from "…"` 值文件的生成期行为：object 单记录 / 数组与 NDJSON 多记录轮转。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::datagen::GenResult;
use crate::loader::resolve_inject_files;

use super::*;

/// 写一份值文件（`TempDir` 必须存活到测试结束，故一并返回）。
fn value_file(content: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("values.json");
    std::fs::write(&path, content).expect("write value file");
    (dir, path)
}

/// 解析 → 解析值文件（绝对路径，与 base_dir 无关）→ 生成。
fn generate_with_value_file(source: &str) -> GenResult {
    let mut wfg = parse_wfg(source).expect("parse wfg");
    resolve_inject_files(&mut wfg, Path::new("/wfg/scenario.wfg")).expect("resolve inject files");
    let schemas = vec![make_login_schema()];
    let plans = vec![make_auth_fail_plan()];
    generate(&wfg, &schemas, &plans).expect("generate")
}

/// 注入实体的 `src_ip` 集合（背景事件的 IP 随机生成，必须排除）。
fn injected_ips(result: &GenResult) -> HashSet<&str> {
    result
        .inject_entities
        .iter()
        .filter_map(|entity| entity.value.as_str())
        .collect()
}

/// 按实体（`src_ip`）取**注入的**事件字段序列（时间序）。
fn per_entity_fields(result: &GenResult, field: &str) -> Vec<Vec<serde_json::Value>> {
    let injected = injected_ips(result);
    let mut by_entity: std::collections::BTreeMap<
        String,
        Vec<(chrono::DateTime<chrono::Utc>, serde_json::Value)>,
    > = std::collections::BTreeMap::new();
    for event in &result.events {
        let Some(entity) = event.fields.get("src_ip").and_then(|v| v.as_str()) else {
            continue;
        };
        if !injected.contains(entity) {
            continue;
        }
        let value = event
            .fields
            .get(field)
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        by_entity
            .entry(entity.to_string())
            .or_default()
            .push((event.timestamp, value));
    }
    by_entity
        .into_values()
        .map(|mut events| {
            events.sort_by_key(|(ts, _)| *ts);
            events.into_iter().map(|(_, value)| value).collect()
        })
        .collect()
}

/// 顶层 object → 该步骤所有事件共用这一份值。
#[test]
fn object_file_applies_same_values_to_every_event() {
    let (_dir, path) = value_file(r#"{"attempts": 7}"#);
    let result = generate_with_value_file(&format!(
        r#"
#[duration=5s]
scenario use_from_object<seed=42> {{
    background {{ stream LoginWindow gen 100/s }}
    inject {{
        hit<src_ip: 2> for auth_fail_rule LoginWindow {{
            use from "{path}" x 4
        }}
    }}
}}
"#,
        path = path.display()
    ));

    assert_eq!(result.inject_entities.len(), 2);
    let per_entity = per_entity_fields(&result, "attempts");
    assert_eq!(per_entity.len(), 2);
    for attempts in &per_entity {
        assert_eq!(
            attempts,
            &vec![serde_json::json!(7); 4],
            "单条记录应作用于该步骤的每一条事件"
        );
    }
}

/// 顶层数组 → 按事件序号在记录间循环取用（`N > 记录数` 回绕）。
#[test]
fn array_file_cycles_records_over_events() {
    let (_dir, path) = value_file(r#"[{"attempts": 1}, {"attempts": 2}, {"attempts": 3}]"#);
    let result = generate_with_value_file(&format!(
        r#"
#[duration=5s]
scenario use_from_array<seed=42> {{
    background {{ stream LoginWindow gen 100/s }}
    inject {{
        hit<src_ip: 2> for auth_fail_rule LoginWindow {{
            use from "{path}" x 4
        }}
    }}
}}
"#,
        path = path.display()
    ));

    assert_eq!(result.inject_entities.len(), 2);
    for attempts in per_entity_fields(&result, "attempts") {
        assert_eq!(
            attempts,
            vec![
                serde_json::json!(1),
                serde_json::json!(2),
                serde_json::json!(3),
                serde_json::json!(1),
            ],
            "3 条记录 / 4 条事件：第 4 条回绕到第 1 条记录"
        );
    }
}

/// NDJSON（每行一个 object）与数组同构：多记录轮转。
#[test]
fn ndjson_file_cycles_records_over_events() {
    let (_dir, path) = value_file("{\"attempts\": 1}\n{\"attempts\": 2}\n");
    let result = generate_with_value_file(&format!(
        r#"
#[duration=5s]
scenario use_from_ndjson<seed=42> {{
    background {{ stream LoginWindow gen 100/s }}
    inject {{
        hit<src_ip: 1> for auth_fail_rule LoginWindow {{
            use from "{path}" x 3
        }}
    }}
}}
"#,
        path = path.display()
    ));

    let per_entity = per_entity_fields(&result, "attempts");
    assert_eq!(
        per_entity,
        vec![vec![
            serde_json::json!(1),
            serde_json::json!(2),
            serde_json::json!(1)
        ]]
    );
}

/// `_` 前缀的内部键忽略：整份原始日志粘进来时不该污染事件字段。
#[test]
fn underscore_keys_are_ignored_for_file_records() {
    let (_dir, path) = value_file(r#"{"attempts": 5, "_stream": "syslog", "_window": "w"}"#);
    let result = generate_with_value_file(&format!(
        r#"
#[duration=5s]
scenario use_from_raw_log<seed=42> {{
    background {{ stream LoginWindow gen 100/s }}
    inject {{
        hit<src_ip: 1> for auth_fail_rule LoginWindow {{
            use from "{path}" x 2
        }}
    }}
}}
"#,
        path = path.display()
    ));

    let injected = injected_ips(&result);
    let mut seen = 0;
    for event in &result.events {
        let Some(sip) = event.fields.get("src_ip").and_then(|v| v.as_str()) else {
            continue;
        };
        if !injected.contains(sip) {
            continue;
        }
        seen += 1;
        assert!(!event.fields.contains_key("_stream"));
        assert!(!event.fields.contains_key("_window"));
        assert_eq!(event.fields.get("attempts"), Some(&serde_json::json!(5)));
    }
    assert_eq!(seen, 2, "1 个实体 × 2 条事件");
}

/// 多记录里任何一条与规则 filter 冲突都要报错，并点出是哪一条记录。
#[test]
fn conflicting_record_is_rejected_with_record_index() {
    let (_dir, path) = value_file(r#"[{"success": false}, {"success": true}]"#);
    // auth_fail_rule 的 bind filter 是 `success == false`
    let source = format!(
        r#"
#[duration=5s]
scenario use_from_conflict<seed=42> {{
    background {{ stream LoginWindow gen 100/s }}
    inject {{
        hit<src_ip: 1> for auth_fail_rule LoginWindow {{
            use from "{path}" x 2
        }}
    }}
}}
"#,
        path = path.display()
    );
    let mut wfg = parse_wfg(&source).expect("parse wfg");
    resolve_inject_files(&mut wfg, Path::new("/wfg/scenario.wfg")).expect("resolve");

    let schemas = vec![make_login_schema()];
    let plans = vec![make_auth_fail_plan()];
    let err = match generate(&wfg, &schemas, &plans) {
        Ok(_) => panic!("冲突记录必须报错"),
        Err(err) => err,
    };
    let detail = err.detail().as_deref().unwrap_or_default();
    assert!(
        detail.contains("conflicts with rule step filter"),
        "获取到: {detail}"
    );
    assert!(
        detail.contains("记录 #2/2"),
        "必须点出冲突的记录序号: {detail}"
    );
}
