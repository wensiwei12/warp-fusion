//! 语料级 **L3 对拍**：`.wfg` → 生成 → oracle → 真实引擎（`Reactor`）→ `wfgen verify`。
//!
//! 与 `wfg_corpus.rs`（L0–L2：校验 + 生成期硬断言 + 期望文件）配套：这里补的是
//! **引擎实际行为 == 期望** 这一段——生成的数据真的能进引擎、类型/时间/流路由都对得上。
//!
//! 两组语料：
//! - `examples/` 下的示例场景（只挑代表：三个聚合家族的规则 + 三套 schema）；
//!   `count/brute_force.wfg` 已由 `e2e_datagen.rs` 覆盖，这里不重复。
//! - `tests/fixtures/wfg_l3/` 的测试专用夹具：**新特性**的端到端对拍——跨流注入 `join`、
//!   否定约束 `without(...)`、背景实体分布 `entity … zipf(...)`。这三者此前只在生成层
//!   （L2）有测试，没走过真引擎。
//!
//! 统一把场景时长压到 60s：背景条数按比例缩，注入条数不变，断言口径不变。

mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

/// 一条 L3 用例：夹具根目录 + 场景相对路径 + 说明 + 时长覆盖秒数。
///
/// `None` = 用场景文件里声明的 `#[duration]`（当覆盖会改变规则语义时用）。
type Case<'a> = (&'a Path, &'a str, &'a str, Option<u64>);

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn l3_corpus_matches_the_real_engine() {
    let examples: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples");
    let fixtures: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wfg_l3");

    let mut cases: Vec<Case> = Vec::new();
    for (scenario, what, secs) in [
        // 聚合家族的代表场景（三套不同 schema / 窗口）。
        (
            "distinct/scenarios/port_scan.wfg",
            "distinct 聚合",
            Some(60),
        ),
        (
            "sum/scenarios/data_exfil.wfg",
            "sum 聚合 + 多步骤",
            Some(60),
        ),
        (
            "avg/scenarios/dns_tunnel.wfg",
            "avg 聚合 + 另一套 schema",
            Some(60),
        ),
    ] {
        cases.push((examples.as_path(), scenario, what, secs));
    }
    for (scenario, what, secs) in [
        (
            "join/scenarios/pair.wfg",
            "跨流注入 join（snapshot 形态，右行提前 1ms）",
            Some(60),
        ),
        (
            "deferred/scenarios/probe.wfg",
            "跨流注入 join（deferred 形态，右行与左行同刻 = within 下界边界）",
            Some(60),
        ),
        (
            "without/scenarios/probe.wfg",
            "否定约束 without(...)",
            Some(60),
        ),
        (
            "zipf/scenarios/hot.wfg",
            "背景实体分布 entity … zipf(...)",
            Some(60),
        ),
        (
            "object_fields/scenarios/on_each_object.wfg",
            "嵌套 object 注入 + `on each` + 嵌套路径 yield（issue #72）",
            Some(60),
        ),
    ] {
        cases.push((fixtures.as_path(), scenario, what, secs));
    }

    let mut failures = Vec::new();
    let mut summary = Vec::new();

    for (root, scenario_rel, what, secs) in cases {
        let duration = Duration::from_secs(secs.unwrap_or(60));
        let run = common::engine_verify_in(root, scenario_rel, duration, &[]).await;
        let line = format!(
            "{scenario_rel} ({what}): oracle={} actual={} matched={} artifact={}",
            run.oracle_total,
            run.actual_total,
            run.report.summary.matched,
            run.artifact_dir.display()
        );
        summary.push(line.clone());
        if run.report.status != "pass" {
            failures.push(format!("{line}\n{}", run.report.to_markdown()));
        }
    }

    assert!(
        failures.is_empty(),
        "{} L3 case(s) failed (oracle ≠ engine):\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
    // 通过时也把口径打出来，便于人肉核对"确实对拍过、不是空跑"。
    for line in summary {
        println!("L3 OK  {line}");
    }
}
