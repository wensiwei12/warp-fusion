//! 语料级 **L3 对拍**：`.wfg` → 生成 → oracle → 真实引擎（`Reactor`）→ `wfgen verify`。
//!
//! 与 `wfg_corpus.rs`（L0–L2：校验 + 生成期硬断言 + 期望文件）配套：这里补的是
//! **引擎实际行为 == 期望** 这一段——生成的数据真的能进引擎、类型/时间/流路由都对得上。
//!
//! 只挑代表场景（全量跑 L3 太贵）：三个聚合家族的规则 + 三套不同的 schema/窗口。
//! `count/brute_force.wfg` 已由 `e2e_datagen.rs` 覆盖，这里不重复。
//!
//! 统一把场景时长压到 60s：背景条数按比例缩，注入条数不变，断言口径不变。

mod common;

use std::time::Duration;

/// L3 语料：(示例目录/场景文件, 说明, 时长覆盖秒数)。
///
/// `None` = 用场景文件里声明的 `#[duration]`（当覆盖会改变规则语义时用）。
const L3_CORPUS: &[(&str, &str, Option<u64>)] = &[
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
];

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn l3_corpus_matches_the_real_engine() {
    let mut failures = Vec::new();
    let mut summary = Vec::new();

    for (rel, what, duration_secs) in L3_CORPUS {
        let duration = Duration::from_secs(duration_secs.unwrap_or(60));
        let run = common::engine_verify(rel, duration, &[]).await;
        let line = format!(
            "{rel} ({what}): oracle={} actual={} matched={} flush_normalized={} artifact={}",
            run.oracle_total,
            run.actual_total,
            run.report.summary.matched,
            run.normalized_flush,
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
