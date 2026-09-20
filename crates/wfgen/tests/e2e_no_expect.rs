//! Functional tests for `--no-expect` / `--no-wfl` (issue #58).
//!
//! `--no-wfl` opts out of the whole WFL pipeline: no rule loading
//! (`_global.wfl` / yield-preset evaluation), no compilation, no injection, so
//! generation falls back to baseline random events. `--no-expect` keeps the
//! pipeline (injection `use()` fixed values still apply) and only skips the
//! expected-output sidecars. These tests use the `examples/count` fixture, which
//! declares an `expect` block (so normal mode would emit `.except.jsonl`
//! sidecars).

use std::collections::HashMap;
use std::path::PathBuf;

use serde_json::Value as Json;

use wfgen::cmd_gen::{self, run};
use wfgen::datagen::generate;
use wfgen::loader::load_from_uses;
use wfgen::validate::validate_wfg;
use wfgen::wfg_parser::parse_wfg;

const WFG_REL: &str = "examples/count/scenarios/brute_force.wfg";

/// 构造 `wfgen gen` 参数（fixture 固定：brute_force.wfg / jsonl / 不发送）。
fn gen_args(out: PathBuf, wfl: Vec<PathBuf>, no_wfl: bool, no_expect: bool) -> cmd_gen::Args {
    cmd_gen::Args {
        scenario: manifest().join(WFG_REL),
        format: "jsonl".to_string(),
        out: Some(out),
        ws: Vec::new(),
        wfl,
        no_wfl,
        no_expect,
        send: false,
        addr: "127.0.0.1:1".to_string(),
        duration: None,
    }
}

fn manifest() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn skip_wfl_loads_schemas_but_no_rules() {
    let wfg_path = manifest().join(WFG_REL);
    let content = std::fs::read_to_string(&wfg_path).expect("read wfg");
    let mut wfg = parse_wfg(&content).expect("parse wfg");

    let (schemas, wfl_files) =
        load_from_uses(&mut wfg, &wfg_path, &HashMap::new(), true).expect("load with skip_wfl");
    assert!(
        !schemas.is_empty(),
        "schemas must still load under skip_wfl"
    );
    assert!(
        wfl_files.is_empty(),
        "WFL files must not load under skip_wfl"
    );
}

#[test]
fn skip_wfl_validation_passes_without_rules() {
    let wfg_path = manifest().join(WFG_REL);
    let content = std::fs::read_to_string(&wfg_path).expect("read wfg");
    let mut wfg = parse_wfg(&content).expect("parse wfg");

    let (schemas, wfl_files) =
        load_from_uses(&mut wfg, &wfg_path, &HashMap::new(), true).expect("load with skip_wfl");
    let errors = validate_wfg(&wfg, &schemas, &wfl_files, true);
    assert!(
        errors.is_empty(),
        "validation must pass under skip_wfl (no rule-not-found errors): {:?}",
        errors
    );
}

#[test]
fn skip_wfl_generates_baseline_events() {
    let wfg_path = manifest().join(WFG_REL);
    let content = std::fs::read_to_string(&wfg_path).expect("read wfg");
    let mut wfg = parse_wfg(&content).expect("parse wfg");

    let (schemas, _wfl_files) =
        load_from_uses(&mut wfg, &wfg_path, &HashMap::new(), true).expect("load with skip_wfl");
    let result = generate(&wfg, &schemas, &[]).expect("generate with empty rule_plans");
    assert!(
        !result.events.is_empty(),
        "baseline events must be generated without rule plans"
    );
}

#[tokio::test]
async fn no_expect_run_writes_events_without_sidecars() {
    let tmp = std::env::temp_dir().join("wfgen-e2e-no-expect");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create temp out dir");
    let out = tmp.clone();

    let res = run(gen_args(out.clone(), Vec::new(), false, true)).await;
    assert!(res.is_ok(), "--no-expect run failed: {:?}", res.err());

    let files: Vec<String> = std::fs::read_dir(&out)
        .expect("read out dir")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        files
            .iter()
            .any(|f| f.ends_with(".jsonl") && !f.contains(".except.")),
        "events file written: {files:?}"
    );
    assert!(
        !files.iter().any(|f| f.contains(".except.")),
        "no expected sidecars under --no-expect: {files:?}"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn no_wfl_run_writes_events_without_sidecars() {
    // `--no-wfl` skips the whole WFL pipeline (no compilation, no injection, no
    // expected output); confirm it still writes baseline events with no sidecars.
    let tmp = std::env::temp_dir().join("wfgen-e2e-no-wfl");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create temp out dir");
    let out = tmp.clone();

    let res = run(gen_args(out.clone(), Vec::new(), true, false)).await;
    assert!(res.is_ok(), "--no-wfl run failed: {:?}", res.err());

    let files: Vec<String> = std::fs::read_dir(&out)
        .expect("read out dir")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        files
            .iter()
            .any(|f| f.ends_with(".jsonl") && !f.contains(".except.")),
        "events file written: {files:?}"
    );
    assert!(
        !files.iter().any(|f| f.contains(".except.")),
        "no expected sidecars under --no-wfl: {files:?}"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn skip_wfl_generation_drops_injected_fixed_values() {
    // The fixture injects `use(action="failed")` / `use(action="success")` fixed
    // values. Under skip_wfl (empty rule_plans → no inject-aware generation),
    // those fixed values must not dominate: events are pure baseline with random
    // `action`.
    let wfg_path = manifest().join(WFG_REL);
    let content = std::fs::read_to_string(&wfg_path).expect("read wfg");
    let mut wfg = parse_wfg(&content).expect("parse wfg");

    let (schemas, _wfl_files) =
        load_from_uses(&mut wfg, &wfg_path, &HashMap::new(), true).expect("load with skip_wfl");
    let result = generate(&wfg, &schemas, &[]).expect("generate with empty rule_plans");

    let total = result.events.len();
    assert!(total > 0, "baseline events must be generated");
    let failed = result
        .events
        .iter()
        .filter(|e| e.fields.get("action").and_then(|v| v.as_str()) == Some("failed"))
        .count();
    assert!(
        failed < total / 20,
        "injected fixed values must not appear under skip_wfl (failed={failed}/{total})"
    );
}

#[tokio::test]
async fn no_wfl_run_skips_cli_wfl_files() {
    // A .wfl passed via the CLI `--wfl` flag is skipped under `--no-wfl`: it
    // must not be loaded or compiled, so the run still succeeds with no WFL
    // errors and no sidecars.
    let tmp = std::env::temp_dir().join("wfgen-e2e-no-wfl-cli");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create temp out dir");
    let out = tmp.clone();

    let wfl_file = manifest().join("examples/count/rules/brute_force.wfl");
    let res = run(gen_args(out.clone(), vec![wfl_file], true, false)).await;
    assert!(
        res.is_ok(),
        "--no-wfl with CLI --wfl failed: {:?}",
        res.err()
    );

    let files: Vec<String> = std::fs::read_dir(&out)
        .expect("read out dir")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        !files.iter().any(|f| f.contains(".except.")),
        "no expected sidecars: {files:?}"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn no_expect_run_removes_stale_expectation_sidecars() {
    // 期望文件路径按用例名（= 场景文件 stem）固定：这次不写，上一次的产物就留在原地。
    // 下游 `wfgen verify --expected <out>/brute_force.except.jsonl` 会拿**旧期望**去比
    // 新数据——两侧都非空，于是静默给出一个看起来有证据、实际对错了版本的结论。
    // 夹具 brute_force.wfg 有注入用例，因此这三种文件本来都会被写出来。
    let tmp = std::env::temp_dir().join("wfgen-e2e-no-expect-stale");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create temp out dir");

    let stale = [
        "brute_force.except.jsonl",
        "brute_force.except.meta.jsonl",
        "brute_force.faulted-except.jsonl",
    ];
    for name in stale {
        std::fs::write(tmp.join(name), "{\"stale\":true}\n").expect("write stale sidecar");
    }

    let out = tmp.clone();
    let res = run(gen_args(out.clone(), Vec::new(), false, true)).await;
    assert!(res.is_ok(), "--no-expect run failed: {:?}", res.err());

    for name in stale {
        assert!(
            !tmp.join(name).exists(),
            "stale expectation sidecar must be removed under --no-expect: {name}"
        );
    }

    let _ = std::fs::remove_dir_all(&tmp);
}

/// `--no-expect` 把 INJ1/INJ2 一起关掉了（断言吃的是**期望**那份 oracle 结果）：
/// 注入照旧执行、数据照旧产出，但 hit/near_miss/miss 的口径没有任何验证。
/// 这件事必须出声——否则 `--no-expect` 看起来只是“不写期望文件”。
#[test]
fn no_expect_warns_that_inject_assertions_did_not_run() {
    let tmp = std::env::temp_dir().join("wfgen-e2e-no-expect-warn");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create temp out dir");

    let stderr = |extra: &[&str]| -> String {
        let out_dir =
            std::env::temp_dir().join(format!("wfgen-e2e-no-expect-warn-{}", extra.join("-")));
        let _ = std::fs::remove_dir_all(&out_dir);
        std::fs::create_dir_all(&out_dir).expect("create temp out dir");
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_wfgen"))
            .arg("gen")
            .arg("--scenario")
            .arg(manifest().join(WFG_REL))
            .arg("--out")
            .arg(&out_dir)
            .args(extra)
            .output()
            .expect("run wfgen gen");
        assert!(
            output.status.success(),
            "wfgen gen {extra:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let text = String::from_utf8_lossy(&output.stderr).into_owned();
        let _ = std::fs::remove_dir_all(&out_dir);
        text
    };

    let warned = stderr(&["--no-expect"]);
    assert!(
        warned.contains("INJ1/INJ2") && warned.contains("--no-expect"),
        "--no-expect 必须警告注入断言未跑，实际 stderr:\n{warned}"
    );

    // 反向对照：正常跑（写期望 = 跑断言）不该出现这条警告。
    let normal = stderr(&[]);
    assert!(
        !normal.contains("INJ1/INJ2"),
        "正常路径不应警告断言未跑，实际 stderr:\n{normal}"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn no_expect_run_keeps_injected_fixed_values() {
    // `--no-expect` keeps the WFL pipeline, so injection `use()` fixed values
    // still apply: the generated events must include the injected
    // `action="failed"` values rather than pure random baseline. Only
    // `--no-wfl` produces random events (see skip_wfl_generation_drops_*).
    let tmp = std::env::temp_dir().join("wfgen-e2e-no-expect-inject");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create temp out dir");
    let out = tmp.clone();

    let res = run(gen_args(out.clone(), Vec::new(), false, true)).await;
    assert!(res.is_ok(), "--no-expect run failed: {:?}", res.err());

    // output_case = the scenario file stem → brute_force.jsonl.
    let events_file = out.join("brute_force.jsonl");
    let content = std::fs::read_to_string(&events_file).expect("read events file");
    let mut total = 0;
    let mut failed = 0;
    for line in content.lines() {
        total += 1;
        let json: Json = serde_json::from_str(line).expect("json line");
        if json.get("action").and_then(Json::as_str) == Some("failed") {
            failed += 1;
        }
    }
    assert!(total > 0, "events file written");
    assert!(
        failed > total / 20,
        "injection fixed values must be present under --no-expect (failed={failed}/{total})"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn normal_mode_loads_rules() {
    // `--no-expect` keeps the WFL pipeline (skip_wfl = false), so rules load
    // from `use` declarations and compile — the inverse of the `--no-wfl` skip.
    let wfg_path = manifest().join(WFG_REL);
    let content = std::fs::read_to_string(&wfg_path).expect("read wfg");
    let mut wfg = parse_wfg(&content).expect("parse wfg");

    let (schemas, wfl_files) =
        load_from_uses(&mut wfg, &wfg_path, &HashMap::new(), false).expect("load skip_wfl=false");
    assert!(!schemas.is_empty());
    assert!(
        !wfl_files.is_empty(),
        "rules must load when the WFL pipeline is active (--no-expect / normal)"
    );

    let plans = wfl_files
        .iter()
        .flat_map(|f| wf_lang::compile_wfl(f, &schemas).expect("compile"))
        .collect::<Vec<_>>();
    assert!(!plans.is_empty(), "rules compile to non-empty rule plans");
}

#[test]
fn normal_mode_generation_applies_injected_fixed_values() {
    // With the WFL pipeline active (skip_wfl = false, as under --no-expect),
    // compiled rule plans drive injection: the fixture's `use(action="failed")`
    // fixed values appear in a large fraction of generated events.
    let wfg_path = manifest().join(WFG_REL);
    let content = std::fs::read_to_string(&wfg_path).expect("read wfg");
    let mut wfg = parse_wfg(&content).expect("parse wfg");

    let (schemas, wfl_files) =
        load_from_uses(&mut wfg, &wfg_path, &HashMap::new(), false).expect("load");
    let plans: Vec<_> = wfl_files
        .iter()
        .flat_map(|f| wf_lang::compile_wfl(f, &schemas).expect("compile"))
        .collect();
    let result = generate(&wfg, &schemas, &plans).expect("generate with rule plans");

    let total = result.events.len();
    assert!(total > 0);
    let failed = result
        .events
        .iter()
        .filter(|e| e.fields.get("action").and_then(|v| v.as_str()) == Some("failed"))
        .count();
    assert!(
        failed > total / 20,
        "injection fixed values must be present with rule plans (failed={failed}/{total})"
    );
}
