//! `wfgen lint` 的**规则语义检查**（`[WFL]` 族）回归。
//!
//! 背景：`lint` 此前只看 `.wfg` 结构，规则能否编译要等 `gen`（`compile_wfl`）或引擎
//! 加载才知道 —— 于是「`lint` 说 OK、`gen` 才报错」成为常态；更糟的一类（写了能过
//! 编译、运行期却永远不生效的表达式，见 warp-fusion#101）连 `gen` 也不一定报。
//! 现在 `lint` 复用同一份 checker，两边的结论必须一致。
//!
//! 三条口径各有一条用例（夹具见 `tests/fixtures/wfg_lint/`）：
//!
//! 1. 规则语义错误 → 退出码非 0，stderr 带 `[WFL]`（#101 的阈值非常量）；
//! 2. 语义干净 → stdout 恰好是 `OK`、退出码 0（调用方按「整行 == `OK`」判定）；
//! 3. 只有 warning 的规则 → 仍然 `OK`（warning 不是失败，否则所有场景会莫名变红）。
//!
//! 走真实二进制（`CARGO_BIN_EXE_wfgen`）而不是 `cmd_lint::run`：后者出错时直接
//! `process::exit(1)`，会把测试进程一起带走。

use std::path::{Path, PathBuf};
use std::process::Command;

fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wfg_lint")
}

struct LintOut {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

fn lint(scenario: &str) -> LintOut {
    let out = Command::new(env!("CARGO_BIN_EXE_wfgen"))
        .arg("lint")
        .arg(
            fixture_root()
                .join("scenarios")
                .join(format!("{scenario}.wfg")),
        )
        .output()
        .expect("spawn wfgen lint");
    LintOut {
        code: out.status.code(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

#[test]
fn clean_scenario_prints_ok_and_exits_zero() {
    let out = lint("ok");
    assert_eq!(out.code, Some(0), "stderr: {}", out.stderr);
    assert_eq!(
        out.stdout.trim(),
        "OK",
        "调用方按「整行 == OK」判定，stdout 不能有别的内容；stderr: {}",
        out.stderr
    );
}

#[test]
fn rule_semantic_error_fails_lint_with_wfl_tag() {
    let out = lint("threshold_dynamic");
    assert_ne!(out.code, Some(0), "语义错误必须让 lint 失败");
    assert_eq!(out.stdout.trim(), "", "失败时不应打印 OK");
    // #101：阈值里写 instance 收集函数 —— 编译期曾放行、运行期分支恒不触发。
    assert!(
        out.stderr.contains("[WFL]") && out.stderr.contains("first()"),
        "stderr 应带 `[WFL]` 标签并点名违规函数，实际：{}",
        out.stderr
    );
    assert!(
        out.stderr.contains("threshold"),
        "错误信息要点明是阈值位置，实际：{}",
        out.stderr
    );
}

#[test]
fn warning_only_rule_does_not_fail_lint() {
    let out = lint("warn_only");
    assert_eq!(
        out.code,
        Some(0),
        "只有 warning 的规则不能让 lint 失败；stderr: {}",
        out.stderr
    );
    assert_eq!(out.stdout.trim(), "OK", "stderr: {}", out.stderr);
}

/// 上一条用例只有在「该规则**确实**会产生 warning」时才证明得了「warning 不致命」。
/// 这里直接对同一份 `.wfl` 跑 checker，钉住这一点（避免夹具悄悄退化成零诊断）。
#[test]
fn warn_only_fixture_actually_warns_without_errors() {
    let root = fixture_root();
    let wfl =
        std::fs::read_to_string(root.join("rules/warn_only.wfl")).expect("read warn_only.wfl");
    let wfs = std::fs::read_to_string(root.join("schemas/login.wfs")).expect("read login.wfs");
    let file = wf_lang::parse_wfl(&wfl).expect("parse warn_only.wfl");
    let schemas = wf_lang::parse_wfs(&wfs).expect("parse login.wfs");

    let diags = wf_lang::check_wfl(&file, &schemas);
    let errors: Vec<_> = diags
        .iter()
        .filter(|d| d.severity == wf_lang::Severity::Error)
        .collect();
    assert!(errors.is_empty(), "夹具不应有 error，实际：{errors:?}");
    assert!(
        diags
            .iter()
            .any(|d| d.severity == wf_lang::Severity::Warning && d.message.contains("redundant")),
        "夹具必须真的产出 `within` 冗余 warning，否则本用例空转：{diags:?}"
    );
}
