//! `replay` 的端到端测试（设计 §8）：照单发货 + 平移进场景时间轴。

use std::collections::BTreeMap;

use crate::datagen::replay_gen::{ReplayRecord, generate_replay_events, plan_replay_timeline};
use crate::datagen::stream_gen::GenEvent;
use crate::loader::resolve_replay_files;
use crate::validate::validate_wfg;

use super::*;

/// 在仓外临时目录里造一份 ndjson 记录文件，返回 (目录, 场景文件路径)。
fn write_scenario(records: &str, scenario: &str) -> (tempfile_dir::Dir, std::path::PathBuf) {
    let dir = tempfile_dir::Dir::new("wfgen-replay");
    dir.write("raw.ndjson", records);
    dir.write("scenario.wfg", scenario);
    let path = dir.path().join("scenario.wfg");
    (dir, path)
}

/// 极简临时目录（避免为一个测试引入 tempfile 依赖）。
mod tempfile_dir {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQ: AtomicU64 = AtomicU64::new(0);

    pub(super) struct Dir {
        path: PathBuf,
    }

    impl Dir {
        pub(super) fn new(prefix: &str) -> Self {
            let seq = SEQ.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!("{prefix}-{}-{seq}", std::process::id()));
            std::fs::create_dir_all(&path).expect("create temp dir");
            Self { path }
        }

        pub(super) fn path(&self) -> &Path {
            &self.path
        }

        pub(super) fn write(&self, name: &str, content: &str) {
            std::fs::write(self.path.join(name), content).expect("write temp file");
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

/// 只跑「加载 + 校验」：错误路径的用例会卡在生成期（同一份计划算不出来），
/// 因此不能用 `run`（它 `unwrap` 生成结果）。
fn validate_only(records: &str, scenario: &str) -> Vec<String> {
    let (_dir, path) = write_scenario(records, scenario);
    let mut wfg = parse_wfg(&std::fs::read_to_string(&path).unwrap()).unwrap();
    resolve_replay_files(&mut wfg, &path).unwrap();
    let schemas = vec![make_login_schema()];
    validate_wfg(&wfg, &schemas, &[], true)
        .into_iter()
        .map(|error| format!("{}: {}", error.code, error.message))
        .collect()
}

/// 跑一遍「加载 → 校验 → 生成」，返回事件与校验错误。
fn run(records: &str, scenario: &str) -> (Vec<GenEvent>, Vec<String>) {
    let (_dir, path) = write_scenario(records, scenario);
    let mut wfg = parse_wfg(&std::fs::read_to_string(&path).unwrap()).unwrap();
    resolve_replay_files(&mut wfg, &path).unwrap();

    let schemas = vec![make_login_schema()];
    let errors = validate_wfg(&wfg, &schemas, &[], true)
        .into_iter()
        .map(|error| format!("{}: {}", error.code, error.message))
        .collect();

    let start = wfg
        .scenario
        .time_clause
        .start
        .parse::<chrono::DateTime<chrono::Utc>>()
        .unwrap();
    let events = generate_replay_events(&wfg, &schemas, &start).unwrap();
    (events, errors)
}

/// 每条 replay 事件相对场景起点的偏移（秒）。
fn offsets_secs(events: &[GenEvent], start: &str) -> Vec<f64> {
    let start_nanos = start
        .parse::<chrono::DateTime<chrono::Utc>>()
        .unwrap()
        .timestamp_nanos_opt()
        .unwrap();
    events
        .iter()
        .map(|event| (event.timestamp.timestamp_nanos_opt().unwrap() - start_nanos) as f64 / 1e9)
        .collect()
}

fn scenario_with(records_file: &str, duration: &str) -> String {
    format!(
        r#"
#[duration={duration}]
scenario replay_case<seed=1> {{
    background {{ stream LoginWindow gen 1/s }}

    replay LoginWindow {{ use from "{records_file}" }}
}}
"#
    )
}

/// 文件里最早的时间戳被平移到场景起点，同文件内的相对间隔保持。
#[test]
fn replay_rebases_file_timestamps_onto_scenario_start() {
    // 秒级时间戳（按位宽归一化）：最早 → 场景起点，其余各 +1s / +2.5s。
    let records = r#"{"_timestamp": 1712345678, "username": "a"}
{"_timestamp": 1712345679, "username": "b"}
{"_timestamp": 1712345680.5, "username": "c"}
"#;
    let (events, errors) = run(records, &scenario_with("raw.ndjson", "10s"));
    assert!(errors.is_empty(), "不应有校验错误：{errors:?}");
    assert_eq!(events.len(), 3);
    assert_eq!(
        offsets_secs(&events, "2026-01-01T00:00:00Z"),
        vec![0.0, 1.0, 2.5]
    );
}

/// 文件没有时间字段 → 按序号在 `duration` 内均匀落下（与 `miss` 同策略）。
#[test]
fn replay_without_time_field_spreads_evenly() {
    let records = "{\"username\": \"a\"}\n{\"username\": \"b\"}\n{\"username\": \"c\"}\n{\"username\": \"d\"}\n";
    let (events, errors) = run(records, &scenario_with("raw.ndjson", "8s"));
    assert!(errors.is_empty(), "不应有校验错误：{errors:?}");
    assert_eq!(events.len(), 4);
    // 8s × i / 4 → 0 / 2 / 4 / 6
    assert_eq!(
        offsets_secs(&events, "2026-01-01T00:00:00Z"),
        vec![0.0, 2.0, 4.0, 6.0]
    );
}

/// 记录的字段照抄，`_` 前缀的内部键不进事件字段；时间列写成 schema 的 `time_field`。
#[test]
fn replay_copies_record_fields_and_writes_time_column() {
    let records = "{\"_timestamp\": 10, \"_stream\": \"x\", \"username\": \"a\"}\n";
    let (events, errors) = run(records, &scenario_with("raw.ndjson", "10s"));
    assert!(errors.is_empty(), "不应有校验错误：{errors:?}");

    let fields = &events[0].fields;
    assert_eq!(fields.get("username").and_then(|v| v.as_str()), Some("a"));
    assert!(
        !fields.contains_key("_stream"),
        "内部键不进字段：{fields:?}"
    );
    assert!(
        fields
            .get("timestamp")
            .is_some_and(|value| value.is_number()),
        "时间列应写入数字纳秒：{fields:?}"
    );
}

/// 文件的时间跨度超过 `#[duration]` → VN25（平移后必然溢出，不截断）。
#[test]
fn replay_span_over_duration_is_vn25() {
    let records = "{\"_timestamp\": 100}\n{\"_timestamp\": 400}\n";
    let (_events, errors) = run(records, &scenario_with("raw.ndjson", "1m"));
    assert!(
        errors.iter().any(|error| error.starts_with("VN25")),
        "errors: {errors:?}"
    );
}

/// 时间字段回退（非单调）→ 报错，lint (VN26) 与生成期同一口。
///
/// 记录按文件顺序发货，而下游（合并排序 / oracle 窗口推进 / 引擎水位）全部假定
/// 事件按时间有序：回退的记录会被当成“未来事件”，两侧窗口收口就此分叉。
/// 重排是静默改用户数据，因此选择报错。
#[test]
fn replay_time_field_must_be_non_decreasing() {
    let records = "{\"_timestamp\": 200}\n{\"_timestamp\": 100}\n";
    let errors = validate_only(records, &scenario_with("raw.ndjson", "10m"));
    let vn26: Vec<&String> = errors
        .iter()
        .filter(|error| error.starts_with("VN26"))
        .collect();
    assert_eq!(vn26.len(), 1, "errors: {errors:?}");
    assert!(
        vn26[0].contains("单调不减"),
        "消息要点明单调性: {}",
        vn26[0]
    );

    // 同刻（相等）是合法的：单调**不减**，不是严格递增。
    let equal = "{\"_timestamp\": 100}\n{\"_timestamp\": 100}\n";
    let errors = validate_only(equal, &scenario_with("raw.ndjson", "10m"));
    assert!(errors.is_empty(), "同刻不该报错: {errors:?}");
}

/// 时间字段只出现在部分记录里 → VN26（落时间口径必须唯一）。
#[test]
fn replay_partial_time_field_is_vn26() {
    let records = "{\"_timestamp\": 100}\n{\"username\": \"a\"}\n";
    let errors = validate_only(records, &scenario_with("raw.ndjson", "10m"));
    assert!(
        errors.iter().any(|error| error.starts_with("VN26")),
        "errors: {errors:?}"
    );
}

/// 文件为空 → VN26，且消息里点名文件。
#[test]
fn replay_empty_file_is_vn26() {
    let (_events, errors) = run("[]\n", &scenario_with("raw.ndjson", "10m"));
    let vn26: Vec<&String> = errors
        .iter()
        .filter(|error| error.starts_with("VN26"))
        .collect();
    assert_eq!(vn26.len(), 1, "errors: {errors:?}");
    assert!(vn26[0].contains("raw.ndjson"), "{}", vn26[0]);
}

/// 目标 stream 不在已加载 schema 里 → VN3。
#[test]
fn replay_unknown_window_is_vn3() {
    let scenario = r#"
#[duration=10m]
scenario replay_unknown<seed=1> {
    background { stream LoginWindow gen 1/s }

    replay NoSuchWindow { use from "raw.ndjson" }
}
"#;
    let (_events, errors) = run("{\"username\": \"a\"}\n", scenario);
    assert!(
        errors.iter().any(|error| error.starts_with("VN3")),
        "errors: {errors:?}"
    );
}

/// 多条 `replay` 各自独立平移（都从场景起点开始）。
#[test]
fn multiple_replays_rebase_independently() {
    let scenario = r#"
#[duration=10m]
scenario replay_two<seed=1> {
    background { stream LoginWindow gen 1/s }

    replay LoginWindow { use from "raw.ndjson" }
    replay LoginWindow { use from "later.ndjson" }
}
"#;
    let dir = tempfile_dir::Dir::new("wfgen-replay2");
    dir.write("raw.ndjson", "{\"_timestamp\": 5}\n");
    dir.write("later.ndjson", "{\"_timestamp\": 900}\n");
    dir.write("scenario.wfg", scenario);
    let path = dir.path().join("scenario.wfg");

    let mut wfg = parse_wfg(&std::fs::read_to_string(&path).unwrap()).unwrap();
    resolve_replay_files(&mut wfg, &path).unwrap();
    let schemas = vec![make_login_schema()];
    let events = generate_replay_events(
        &wfg,
        &schemas,
        &wfg.scenario.time_clause.start.parse().unwrap(),
    )
    .unwrap();

    assert_eq!(events.len(), 2);
    assert_eq!(
        offsets_secs(&events, "2026-01-01T00:00:00Z"),
        vec![0.0, 0.0],
        "两条 replay 各自把自己最早的记录摆在场景起点"
    );
}

/// 记录的字段名按 schema 写出时不做校验（照单发货），但窗口名要对得上。
#[test]
fn replay_events_carry_the_target_window() {
    let records = "{\"username\": \"a\"}\n";
    let (events, errors) = run(records, &scenario_with("raw.ndjson", "1m"));
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].window_name, "LoginWindow");
    assert_eq!(events[0].stream_name, "login_events");
}

/// 汇总一下字段映射：便于人读地核对「记录 → 事件」。
#[test]
fn replay_field_map_is_record_plus_time_column() {
    let records = "{\"_timestamp\": 1, \"username\": \"u\", \"attempts\": 3}\n";
    let (events, _) = run(records, &scenario_with("raw.ndjson", "1m"));
    let mapped: BTreeMap<&str, String> = events[0]
        .fields
        .iter()
        .map(|(key, value)| (key.as_str(), value.to_string()))
        .collect();
    assert_eq!(mapped.get("username").map(String::as_str), Some("\"u\""));
    assert_eq!(mapped.get("attempts").map(String::as_str), Some("3"));
    assert!(mapped.contains_key("timestamp"), "{mapped:?}");
}

// ---------------------------------------------------------------------------
// `without(...)` × `replay`：replay 事件是“排不掉”的来源（设计 §8.3 附注）
// ---------------------------------------------------------------------------

/// `scan → (不含 login) → scan`：用来验证 replay 数据落进 `without` 窗口时的行为。
const NOT_STEP_RULE: &str = r#"rule probe_rule {
    events {
        scan  : LoginWindow && success == false
        login : LoginWindow && success == true
    }
    match<src_ip : 5s> {
        on event seq {
            has scan;
            not has login;
            has scan;
        }
    } -> score(1)
    entity(ip, scan.src_ip)
    yield alerts()
}"#;

fn make_alerts_schema() -> WindowSchema {
    WindowSchema {
        name: "alerts".to_string(),
        streams: vec![],
        time_field: None,
        over: Duration::from_secs(300),
        fields: vec![],
    }
}

/// replay 记录命中 `without` 谓词 → 生成期报错（不能默默删用户的数据、也不能无视）。
#[test]
fn replay_events_hitting_without_predicate_are_a_generation_error() {
    // 两条记录：第一条只用来把锚点摆在场景起点，第二条落在 guard 窗口（[3s, 7s]）里。
    let records = r#"{"_timestamp": 1000, "src_ip": "10.9.9.9", "success": false}
{"_timestamp": 1005, "src_ip": "10.0.0.0", "success": true}
"#;
    let scenario = r#"
#[duration=10s]
scenario replay_without_conflict<seed=1> {
    background { stream LoginWindow gen 1/s }

    inject {
        hit<src_ip: 1> for probe_rule LoginWindow {
            use(success=false) x 1
            then use(success=false) x 1
            without(success=true) within 4s
        }
    }

    replay LoginWindow { use from "raw.ndjson" }
}
"#;
    let (_dir, path) = write_scenario(records, scenario);
    let mut wfg = parse_wfg(&std::fs::read_to_string(&path).unwrap()).unwrap();
    resolve_replay_files(&mut wfg, &path).unwrap();

    let schemas = vec![make_login_schema(), make_alerts_schema()];
    let wfl = wf_lang::parse_wfl(NOT_STEP_RULE).unwrap();
    let plans = wf_lang::compile_wfl(&wfl, &schemas).unwrap();

    let err = match generate(&wfg, &schemas, &plans) {
        Ok(_) => panic!("replay 事件命中 without 谓词时必须报错"),
        Err(err) => err,
    };
    let detail = err.detail().clone().unwrap_or_default();
    assert!(detail.contains("replay"), "获取到: {detail}");
    assert!(detail.contains("without"), "获取到: {detail}");
    assert!(detail.contains("success=true"), "获取到: {detail}");
}

/// 同一条 replay（实体键对不上 guard）不该误报：guard 只认「属于该实体」的事件。
#[test]
fn replay_events_outside_the_guard_entity_are_fine() {
    let records = r#"{"_timestamp": 1000, "src_ip": "10.9.9.9", "success": false}
{"_timestamp": 1005, "src_ip": "10.9.9.9", "success": true}
"#;
    let scenario = r#"
#[duration=10s]
scenario replay_without_ok<seed=1> {
    background { stream LoginWindow gen 1/s }

    inject {
        hit<src_ip: 1> for probe_rule LoginWindow {
            use(success=false) x 1
            then use(success=false) x 1
            without(success=true) within 4s
        }
    }

    replay LoginWindow { use from "raw.ndjson" }
}
"#;
    let (_dir, path) = write_scenario(records, scenario);
    let mut wfg = parse_wfg(&std::fs::read_to_string(&path).unwrap()).unwrap();
    resolve_replay_files(&mut wfg, &path).unwrap();

    let schemas = vec![make_login_schema(), make_alerts_schema()];
    let wfl = wf_lang::parse_wfl(NOT_STEP_RULE).unwrap();
    let plans = wf_lang::compile_wfl(&wfl, &schemas).unwrap();

    let result = generate(&wfg, &schemas, &plans).expect("别的实体不受 guard 约束");
    assert!(
        result
            .events
            .iter()
            .any(|event| event.fields.get("src_ip").and_then(|v| v.as_str()) == Some("10.9.9.9")),
        "replay 事件应在输出里"
    );
}

/// 没有时间字段时按序号均匀落下，偏移用 `u128` 计算：`total × index` 在 `i64` 里会溢出
/// （旧实现：`#[duration=2d]` + 10 万条记录的 `replay` 会让 `wfgen lint` panic；release 下
/// 静默回绕为负偏移，事件落到场景起点之前）。
#[test]
fn no_time_field_timeline_over_huge_duration_does_not_overflow() {
    let records: Vec<ReplayRecord> = (0..3)
        .map(|_| ReplayRecord {
            fields: std::collections::HashMap::new(),
            internal_timestamp: None,
        })
        .collect();
    // 7.2e9 秒 = 7.2e18 纳秒；对 3 条记录，旧实现算到 index=2 时是 1.44e19 > i64::MAX。
    let duration = std::time::Duration::from_secs(7_200_000_000);

    let timeline = plan_replay_timeline(&records, None, duration).expect("按序号落时间");

    assert_eq!(timeline.time_field, None);
    assert_eq!(timeline.offsets_nanos.len(), 3);
    assert!(
        timeline.offsets_nanos.windows(2).all(|w| w[0] <= w[1]),
        "偏移应单调不减：{:?}",
        timeline.offsets_nanos
    );
    assert!(
        timeline
            .offsets_nanos
            .iter()
            .all(|&offset| offset >= 0 && (offset as u128) < duration.as_nanos()),
        "偏移应落在场景时长内：{:?}",
        timeline.offsets_nanos
    );
}
