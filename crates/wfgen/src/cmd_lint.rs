use std::collections::HashMap;
use std::path::PathBuf;

use orion_error::conversion::SourceErr;
use wf_lang::ast::WflFile;
use wf_lang::{Severity, WindowSchema};

use crate::error::{WfgenReason, WfgenResult};
use crate::loader::load_from_uses;
use crate::validate::validate_wfg;
use crate::wfg_parser::parse_wfg;

use crate::cmd_helpers::{load_wfl_files, load_ws_files};

/// `wfgen lint` 参数：校验 .wfg scenario 文件。
#[derive(clap::Args)]
pub struct Args {
    /// Path to the .wfg scenario file
    pub scenario: PathBuf,

    /// Additional .wfs schema files (beyond those in `use` declarations)
    #[arg(long)]
    pub ws: Vec<PathBuf>,

    /// Additional .wfl rule files (beyond those in `use` declarations)
    #[arg(long)]
    pub wfl: Vec<PathBuf>,
}

pub fn run(args: Args) -> WfgenResult<()> {
    let scenario = args.scenario;
    let ws = args.ws;
    let wfl = args.wfl;
    let wfg_content = std::fs::read_to_string(&scenario).source_err(
        WfgenReason::Io,
        format!("reading .wfg file: {}", scenario.display()),
    )?;
    let mut wfg = parse_wfg(&wfg_content)?;

    let (mut schemas, mut wfl_files) = load_from_uses(&mut wfg, &scenario, &HashMap::new(), false)?;
    schemas.extend(load_ws_files(&ws)?);
    wfl_files.extend(load_wfl_files(&wfl)?);

    let errors: Vec<String> = validate_wfg(&wfg, &schemas, &wfl_files, false)
        .into_iter()
        .map(|e| e.to_string())
        .chain(check_rule_errors(&wfl_files, &schemas))
        .collect();
    if errors.is_empty() {
        println!("OK");
    } else {
        for e in &errors {
            eprintln!("{e}");
        }
        std::process::exit(1);
    }
    Ok(())
}

/// `.wfl` 的**规则语义检查**（`[WFL]` 族），与 [`.wfg`](crate::validate) 的 `VN` 同一套
/// 输出。
///
/// 此前 `lint` 只看 `.wfg` 结构，规则能不能编译要等到 `gen`（`compile_wfl`）或引擎加载
/// 才知道 —— 于是「`lint` 说 OK、`gen` 才报错」成为常态，而更糟的那一类（写了能过编译、
/// 运行期却永远不生效的表达式，见 warp-fusion#101）连 `gen` 也不一定报。这里直接用同一份
/// checker，让 `lint` 与 `gen` 的结论一致。
///
/// 三点口径：
/// - **只报 `error`**：`lint` 的输出被脚本按「整行 == `OK`」判定（如 `verify_wfg.sh` 的
///   L0 列），混进 warning 会让所有场景莫名变成 FAIL；
/// - checker 干净后再跑一次 `compile_wfl`：checker 之后的**装配阶段**仍可能失败
///   （`gen` 走的就是这条路径），漏了它 `lint` 仍可能领先 `gen` 报 OK；
/// - 一个文件里有多条错误就全报（不“只报第一条”），与 VN 错误一致。
fn check_rule_errors(wfl_files: &[WflFile], schemas: &[WindowSchema]) -> Vec<String> {
    let mut out = Vec::new();
    for file in wfl_files {
        let semantic: Vec<String> = wf_lang::check_wfl(file, schemas)
            .into_iter()
            .filter(|e| e.severity == Severity::Error)
            .map(|e| format!("[WFL] {e}"))
            .collect();
        if !semantic.is_empty() {
            out.extend(semantic);
            continue;
        }
        if let Err(err) = wf_lang::compile_wfl(file, schemas) {
            out.push(format!(
                "[WFL] {}",
                err.detail()
                    .clone()
                    .unwrap_or_else(|| err.to_string())
                    .trim()
            ));
        }
    }
    out
}
