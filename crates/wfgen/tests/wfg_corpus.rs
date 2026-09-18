//! 仓内 `.wfg` 语料回归：每个场景都走**用户可见的同一条路径**（`wfgen gen`）。
//!
//! 覆盖的是此前"只在文档里存在"的场景文件——`crates/wfgen/examples/*/scenarios/*.wfg`
//! （文档里让人"直接照抄"的样本）与 `crates/wfadm/templates/models/scenarios/*.wfg`
//! （`wfadm init` 模板）；`docker/default_setting/` 下的同构副本一并纳入，这样两份拷贝
//! 漂移时会被抓到。
//!
//! 每条用例同时覆盖三层：
//!
//! - **L0 静态校验**：`VN*`（字段 / 规则 / 实体 / 注解 / 单例块 …）；
//! - **L1 生成期硬断言**：`INJ1` / `INJ2`（`hit` 必报、`near_miss`·`miss` 必不报）——
//!   `cmd_gen::run` 在写期望文件**之前**断言，失败即 `Err`；
//! - **L2 期望文件**：`.except.jsonl` 必须非空。
//!
//! 额外断言**事件条数 = 背景配额 + 注入条数**（两条口径完全分离、互不挤压）：
//! 期望值由 `.wfg` 的 AST 独立算出（`scenario.total` + `实体数 × ΣN`），不复制 datagen 的实现。
//! 带 `replay` / `join` 的场景没有这条断言——它们的条数由文件与左事件派生，公式不同。
//!
//! 统一 `--duration 1m` 压时长以控制耗时：背景条数按比例缩，注入条数不受影响
//! （`conv/top_scanners.wfg` 有 `spread 1m`，1m 已是下界）。
//!
//! 本地只跑一部分：`WFC_CORPUS_ONLY=<路径子串> cargo test -p wfgen --test wfg_corpus`

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use wfgen::cmd_gen::{self, Args};
use wfgen::wfg_ast::WfgFile;

/// 语料清单。**显式列出**（不用 glob）：新增场景必须显式登记，避免"文件在但没被测"。
const CORPUS: &[&str] = &[
    // `wfgen` 示例：文档里"可直接照抄"的 6 个样本。
    "crates/wfgen/examples/avg/scenarios/dns_tunnel.wfg",
    "crates/wfgen/examples/conv/scenarios/top_scanners.wfg",
    "crates/wfgen/examples/count/scenarios/brute_force.wfg",
    "crates/wfgen/examples/distinct/scenarios/port_scan.wfg",
    "crates/wfgen/examples/multi_step/scenarios/chain_attack.wfg",
    "crates/wfgen/examples/sum/scenarios/data_exfil.wfg",
    // `wfadm init` 模板（4 个）。
    "crates/wfadm/templates/models/scenarios/port_scan.wfg",
    "crates/wfadm/templates/models/scenarios/port_scan_quick.wfg",
    "crates/wfadm/templates/models/scenarios/ssh_brute_force.wfg",
    "crates/wfadm/templates/models/scenarios/ssh_brute_quick.wfg",
    // docker 示例里的同构副本（4 个）——与模板内容应保持一致，漂移即失败。
    "docker/default_setting/models/scenarios/port_scan.wfg",
    "docker/default_setting/models/scenarios/port_scan_quick.wfg",
    "docker/default_setting/models/scenarios/ssh_brute_force.wfg",
    "docker/default_setting/models/scenarios/ssh_brute_quick.wfg",
];

/// 语料统一压到的场景时长（见文件头注释）。
const CORPUS_DURATION: &str = "1m";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("canonicalize repo root")
}

fn selected(rel: &str) -> bool {
    match std::env::var("WFC_CORPUS_ONLY") {
        Ok(filter) => rel.contains(&filter),
        Err(_) => true,
    }
}

/// 注入事件条数 = `Σ 用例(实体数 × Σ 事件组 x N)`（`inject` 与场景时长无关）。
fn inject_event_count(wfg: &WfgFile) -> Option<u64> {
    let syntax = wfg.syntax.as_ref()?;
    // `replay`（按文件条数）与 `join`（右事件随左事件派生）不适用这个公式。
    if !syntax.replays.is_empty()
        || syntax
            .injection
            .as_ref()
            .is_some_and(|i| i.cases.iter().any(|c| !c.joins.is_empty()))
    {
        return None;
    }
    let injection = syntax.injection.as_ref()?;
    Some(
        injection
            .cases
            .iter()
            .map(|c| {
                let per_entity: u64 = c.groups.iter().map(|g| g.count).sum();
                c.entity_count * per_entity
            })
            .sum(),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn corpus_scenarios_generate_and_pass_their_own_assertions() {
    let root = repo_root();
    let cases: Vec<&str> = CORPUS.iter().copied().filter(|rel| selected(rel)).collect();
    assert!(!cases.is_empty(), "WFC_CORPUS_ONLY filtered out every case");

    let mut failures = Vec::new();
    for rel in cases {
        if let Err(msg) = run_one(&root, rel).await {
            failures.push(format!("[{rel}] {msg}"));
        }
    }

    assert!(
        failures.is_empty(),
        "{} corpus case(s) failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

async fn run_one(root: &Path, rel: &str) -> Result<(), String> {
    let path = root.join(rel);
    assert!(path.is_file(), "corpus entry is missing on disk: {rel}");

    // 期望条数：从 AST 独立算（`load_scenario` 会用**文件里**的时长解析，所以先覆盖到测试时长）。
    let mut loaded = wfgen::loader::load_scenario(&path, &HashMap::new())
        .map_err(|e| format!("load_scenario failed: {}", e.report().render()))?;
    wfgen::wfg_parser::override_duration(&mut loaded.wfg, Duration::from_secs(60));
    let background = loaded.wfg.scenario.total;
    let inject = inject_event_count(&loaded.wfg);
    drop(loaded);

    let out = tempfile::tempdir().map_err(|e| format!("tempdir: {e}"))?;
    cmd_gen::run(Args {
        scenario: path.clone(),
        format: "jsonl".to_string(),
        out: Some(out.path().to_path_buf()),
        ws: Vec::new(),
        wfl: Vec::new(),
        no_wfl: false,
        no_oracle: false,
        send: false,
        addr: "127.0.0.1:1".to_string(),
        duration: Some(CORPUS_DURATION.to_string()),
    })
    .await
    .map_err(|e| format!("wfgen gen failed: {}", e.report().render()))?;

    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| format!("scenario has no file stem: {}", path.display()))?;
    let events_path = out.path().join(format!("{stem}.jsonl"));
    let events = wfgen::output::jsonl::read_events_jsonl(&events_path)
        .map_err(|e| format!("reading generated events failed: {}", e.report().render()))?;
    assert!(
        !events.is_empty(),
        "[{rel}] generated zero events — scenario generated nothing"
    );

    // L2：期望告警文件必须存在且非空（`hit` 用例至少要产出期望告警）。
    let expected_path = out.path().join(format!("{stem}.except.jsonl"));
    let expected = std::fs::read_to_string(&expected_path)
        .map_err(|e| format!("expected alerts file {expected_path:?} missing: {e}"))?;
    let expected_rows = expected.lines().filter(|l| !l.trim().is_empty()).count();
    assert!(
        expected_rows > 0,
        "[{rel}] expected-alert file is empty — oracle produced nothing"
    );

    if let Some(inject) = inject {
        let want = background + inject;
        assert_eq!(
            events.len() as u64,
            want,
            "[{rel}] event count must be background({background}) + inject({inject}); \
             generated {}",
            events.len()
        );
    }

    Ok(())
}
