//! 集成测试共享脚手架：把 `.wfg` 生成的事件灌进**真实引擎**（`wf_runtime::Reactor`），
//! 再用 `wfgen verify` 与 oracle 期望对拍（L3：引擎实际行为 == 期望）。
//!
//! 与 `e2e_datagen.rs` 同一条链路（生成 → oracle → Reactor → verify），区别是把场景、
//! 时长、变量**参数化**，供语料级 L3 复用；`windows.toml` 由场景 schema 生成，因此任何
//! 示例目录都能直接跑。
//!
//! 已知口径：batch/file 模式最后一批告警的 `origin` 是 `close:flush`，oracle 建模为
//! 场景末尾 `close:eos`；对拍前统一归一化（见 [`normalize_origin`]）。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Utc};

use wf_config::{ConfigVarContext, FusionConfigLoader, RawFusionConfigTree};
use wf_runtime::lifecycle::Reactor;
use wf_runtime::tracing_init::{DomainFormat, FileFields};
use wfgen::verify::{ActualAlert, VerifyReport};

/// Arrow 帧分块行数（与运行时接收侧约定一致）。
const FRAME_CHUNK_ROWS: usize = 2048;

/// 一次 L3 运行的结果。
pub struct EngineRun {
    pub report: VerifyReport,
    pub oracle_total: usize,
    pub actual_total: usize,
    /// 被归一化的 `close:flush` 告警数（> 0 说明走了收尾路径）。
    pub normalized_flush: usize,
    /// 产物目录（失败时可去看 `wfusion.toml` / `alerts/` / `input/`）。
    pub artifact_dir: PathBuf,
}

/// 跑一条场景的 L3 闭环。`case_rel` 形如 `<case>/scenarios/<file>.wfg`，其中 `<case>` 目录下
/// 需有 `schemas/` 与 `rules/`——引擎配置的 `schemas` / `rules` glob 都相对 `fixture_root`
/// （示例场景用 `examples/`，测试专用夹具用 `tests/fixtures/wfg_l3/`）。
///
/// `override_duration` 覆盖场景时长以控制耗时（背景条数按比例缩、注入条数不变）。
pub async fn engine_verify_in(
    fixture_root: &Path,
    case_rel: &str,
    override_duration: Duration,
    vars: &[(&str, &str)],
) -> EngineRun {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let wfg_path = fixture_root.join(case_rel);
    assert!(
        wfg_path.is_file(),
        "scenario not found: {}",
        wfg_path.display()
    );

    let example = case_rel
        .split('/')
        .next()
        .expect("case_rel must start with the case directory");
    let stem = wfg_path
        .file_stem()
        .and_then(|s| s.to_str())
        .expect("scenario file stem");
    let root_label = fixture_root
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("fixtures");
    let artifact_dir = manifest_dir
        .join("../../target/test-artifacts/wfg_l3")
        .join(format!("{root_label}_{example}_{stem}"));

    // 上一轮产物（尤其 sink 的 alerts/*.jsonl）会污染对拍：先清干净。
    let alert_dir = artifact_dir.join("alerts");
    let _ = std::fs::remove_dir_all(&alert_dir);
    std::fs::create_dir_all(&alert_dir).expect("create alert dir");
    let source_path = artifact_dir.join("input/events.arrow_framed");
    std::fs::create_dir_all(source_path.parent().expect("input parent")).expect("create input dir");
    let _ = std::fs::remove_file(&source_path);

    // 引擎日志落到产物目录（对拍失败时唯一的现场）：test 输出 1 份、文件 1 份。
    init_tracing(&artifact_dir, "engine.log");

    // ---- 场景 → 事件 → oracle ----
    let vars_map: HashMap<String, String> = vars
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let mut loaded = wfgen::loader::load_scenario(&wfg_path, &vars_map).expect("load scenario");
    wfgen::wfg_parser::override_duration(&mut loaded.wfg, override_duration);

    let errors =
        wfgen::validate::validate_wfg(&loaded.wfg, &loaded.schemas, &loaded.wfl_files, false);
    assert!(errors.is_empty(), "scenario validation failed: {errors:?}");

    let events = wfgen::datagen::generate(&loaded.wfg, &loaded.schemas, &loaded.rule_plans)
        .expect("event generation failed")
        .events;
    assert!(!events.is_empty(), "datagen produced zero events");

    let start: DateTime<Utc> = loaded
        .wfg
        .scenario
        .time_clause
        .start
        .parse()
        .expect("scenario start time");
    let duration = loaded.wfg.scenario.time_clause.duration;
    let injected_rules =
        wfgen::injection_targets::injected_rule_names(&loaded.wfg).expect("injected rules");
    // 用**带 schemas** 的变体（与 `cmd_gen` 同口径）：`run_oracle` 无 schema 时 join 一律不评估，
    // 跨流注入的期望告警会整片缺失，verify 必然报 missing。
    let oracle = wfgen::oracle::run_oracle_events_full(
        events.iter().cloned(),
        &loaded.rule_plans,
        &loaded.schemas,
        &start,
        &duration,
        Some(&injected_rules),
        true,
    )
    .expect("oracle evaluation failed");
    assert!(
        !oracle.alerts.is_empty(),
        "oracle produced zero alerts; injected_rules={injected_rules:?}"
    );

    // ---- 引擎配置（windows 由场景 schema 生成）----
    let windows_path = artifact_dir.join("models/windows.toml");
    std::fs::create_dir_all(windows_path.parent().expect("windows parent"))
        .expect("create windows dir");
    std::fs::write(&windows_path, windows_toml(&loaded.schemas)).expect("write windows.toml");

    let vars_toml: String = vars
        .iter()
        .map(|(k, v)| format!("{k} = \"{v}\"\n"))
        .collect();
    // sink 配置复用 `examples/sinks`（按 window 名路由的通用配置：`windows = ["*"]` 的
    // catch_all 会把任何场景的告警落到 `<work_root>/alerts/`），夹具根目录因此不必自带 sinks。
    let sinks_dir = manifest_dir.join("examples/sinks");
    let toml_str = format!(
        r#"
mode = "batch"
sinks = "{sinks}"
windows = "{windows}"
work_root = "{work_root}"

[[sources]]
type = "file"
name = "ingress"
path = "{source}"
data_format = "arrow_framed"
stream_tag = ""

[runtime]
executor_parallelism = 2
rule_exec_timeout = "30s"
schemas = "{example}/schemas/*.wfs"
rules   = "{example}/rules/*.wfl"

[vars]
{vars_toml}
"#,
        sinks = sinks_dir.display(),
        windows = windows_path.display(),
        work_root = artifact_dir.display(),
        source = source_path.display(),
        example = example,
    );
    let config_path = artifact_dir.join("wfusion.toml");
    std::fs::write(&config_path, &toml_str).expect("write wfusion.toml");
    let config = FusionConfigLoader::new(
        &config_path,
        &[],
        &ConfigVarContext::new(),
        Some(&artifact_dir),
    )
    .load()
    .expect("failed to parse config TOML");
    let raw = RawFusionConfigTree::from_toml_str(&toml_str, fixture_root).expect("raw config tree");

    write_arrow_framed(&events, &loaded.schemas, &source_path);

    // ---- 起引擎 → batch 模式自动退出 ----
    let reactor = Reactor::start(config, raw, fixture_root)
        .await
        .expect("Reactor::start failed");
    tokio::time::timeout(Duration::from_secs(60), reactor.wait())
        .await
        .unwrap_or_else(|_| {
            panic!(
                "batch reactor did not exit in 60s ({})",
                artifact_dir.display()
            )
        })
        .expect("reactor.wait failed");

    // ---- 对拍 ----
    let mut actual = read_alerts_from_sink_dir(&alert_dir)
        .unwrap_or_else(|e| panic!("reading alerts from {}: {e}", alert_dir.display()));
    let normalized_flush = normalize_origin(&mut actual);
    let tolerances = wfgen::oracle::OracleTolerances::default();
    let oracle_total = oracle.alerts.len();
    let actual_total = actual.len();
    let report = wfgen::verify::verify(
        &oracle.alerts,
        &actual,
        tolerances.score_tolerance,
        tolerances.time_tolerance_secs,
    );

    EngineRun {
        report,
        oracle_total,
        actual_total,
        normalized_flush,
        artifact_dir,
    }
}

/// 初始化 tracing（进程内**只装一次**：subscriber 是全局的，日志写到第一次调用的产物目录）。
///
/// 引擎的告警路由 / 规则编译失败只出现在日志里，因此产物必须留下日志才能排障。
fn init_tracing(artifact_dir: &Path, log_name: &str) {
    static INSTALLED: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    if INSTALLED.set(artifact_dir.to_path_buf()).is_err() {
        return; // 已有订阅者：日志在第一次调用的产物目录里（含本次运行）
    }

    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use tracing_subscriber::{EnvFilter, Layer, fmt};

    let _ = std::fs::remove_file(artifact_dir.join(log_name));
    let appender = tracing_appender::rolling::never(artifact_dir, log_name);
    let (non_blocking, guard) = tracing_appender::non_blocking(appender);
    // 日志线程要在整个测试进程里存活；泄漏这个 guard 是有意的（进程结束时回收）。
    Box::leak(Box::new(guard));
    let _ = tracing_subscriber::registry()
        .with(
            fmt::layer()
                .event_format(DomainFormat::new())
                .with_test_writer()
                .with_filter(EnvFilter::try_new("info").unwrap()),
        )
        .with(
            fmt::layer()
                .event_format(DomainFormat::new())
                .fmt_fields(FileFields::default())
                .with_ansi(false)
                .with_writer(non_blocking)
                .with_filter(EnvFilter::try_new("debug").unwrap()),
        )
        .try_init();
}

/// 由场景 schema 生成 `windows.toml`（每个 window 一条，`over_cap` 给足余量）。
fn windows_toml(schemas: &[wf_lang::WindowSchema]) -> String {
    let mut out = String::from(
        "[window_defaults]\n\
         evict_interval = \"30s\"\n\
         max_window_bytes = \"256MB\"\n\
         max_total_bytes = \"2GB\"\n\
         evict_policy = \"time_first\"\n\
         watermark = \"5s\"\n\
         allowed_lateness = \"0s\"\n\
         late_policy = \"drop\"\n",
    );
    for schema in schemas {
        // `over_cap` 是窗口跨度上限，必须 ≥ schema 的 `over`；测试里给 1h 余量即可。
        let over_cap = schema.over.as_secs().max(60) + 3600;
        out.push_str(&format!(
            "\n[window.{}]\nmode = \"local\"\nmax_window_bytes = \"256MB\"\nover_cap = \"{}s\"\n",
            schema.name, over_cap
        ));
    }
    out
}

/// 事件 → 类型化 Arrow 批 → `<len> <payload>` 帧（与 TCP sink / wf-runtime 接收侧一致）。
fn write_arrow_framed(
    events: &[wfgen::datagen::stream_gen::GenEvent],
    schemas: &[wf_lang::WindowSchema],
    path: &Path,
) {
    let batches = wfgen::output::arrow_ipc::events_to_typed_batches(
        events,
        schemas,
        wfgen::output::arrow_ipc::DEFAULT_MAX_FRAME_BYTES,
        wfgen::output::arrow_ipc::DEFAULT_MAX_FRAME_ROWS,
    )
    .expect("events_to_typed_batches failed");

    let mut framed = Vec::new();
    for (stream_name, batch) in &batches {
        for offset in (0..batch.num_rows()).step_by(FRAME_CHUNK_ROWS) {
            let len = (batch.num_rows() - offset).min(FRAME_CHUNK_ROWS);
            let chunk = batch.slice(offset, len);
            let ipc_payload = wp_arrow::ipc::encode_ipc(stream_name, &chunk)
                .unwrap_or_else(|e| panic!("encode_ipc failed for '{stream_name}': {e}"));
            framed.extend_from_slice(format!("{} ", ipc_payload.len()).as_bytes());
            framed.extend_from_slice(&ipc_payload);
        }
    }
    std::fs::write(path, framed)
        .unwrap_or_else(|e| panic!("failed to write {}: {e}", path.display()));
}

/// 读 sink 目录下所有 `*.jsonl`，按告警全字段去重（同一告警可能落进多个 sink 文件）。
fn read_alerts_from_sink_dir(alert_dir: &Path) -> std::io::Result<Vec<ActualAlert>> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(alert_dir)?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|ext| ext == "jsonl"))
        .collect();
    files.sort();

    let mut alerts = Vec::new();
    for path in files {
        let parsed = wfgen::output::jsonl::read_alerts_jsonl(&path).unwrap_or_else(|e| {
            panic!("reading alerts {}: {}", path.display(), e.report().render())
        });
        alerts.extend(parsed);
    }

    let mut seen = HashSet::new();
    alerts.retain(|alert| {
        seen.insert(format!(
            "{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}",
            alert.rule_name,
            alert.score,
            alert.entity_type,
            alert.entity_id,
            alert.origin,
            alert.fired_at
        ))
    });
    Ok(alerts)
}

/// 收尾口径归一化：`close:flush`（引擎 batch 收尾）→ `close:eos`（oracle 的场景末尾）。
fn normalize_origin(alerts: &mut [ActualAlert]) -> usize {
    let mut normalized = 0;
    for alert in alerts.iter_mut() {
        if alert.origin == "close:flush" {
            alert.origin = "close:eos".to_string();
            normalized += 1;
        }
    }
    normalized
}
