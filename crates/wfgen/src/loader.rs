use std::collections::HashMap;
use std::path::{Path, PathBuf};

use orion_error::OperationContext;
use orion_error::conversion::SourceErr;

use wf_config::ConfigVarContext;
use wf_config::load_wfl_with_context;

use crate::error::{self, WfgenReason, WfgenResult, WfgenStructExt};
use crate::prelude;
use crate::wfg_ast::{ValueSource, WfgFile};
use crate::wfg_parser::parse_wfg;

#[cfg(test)]
mod tests;

/// Everything loaded and compiled from a `.wfg` scenario and its `use` declarations.
pub struct LoadedScenario {
    pub wfg: WfgFile,
    pub schemas: Vec<wf_lang::WindowSchema>,
    pub wfl_files: Vec<wf_lang::ast::WflFile>,
    pub rule_plans: Vec<wf_lang::plan::RulePlan>,
}

/// Load a `.wfg` scenario file, resolve `use` declarations, and compile rules.
///
/// `.wfl` files are preprocessed with `vars` before parsing, falling back to
/// environment variables for any undefined references.
pub fn load_scenario(
    wfg_path: &Path,
    vars: &HashMap<String, String>,
) -> WfgenResult<LoadedScenario> {
    let wfg_content = std::fs::read_to_string(wfg_path).source_err(
        WfgenReason::Io,
        format!("reading .wfg file: {}", wfg_path.display()),
    )?;
    let mut wfg = parse_wfg(&wfg_content)
        .map_err(|err| err.with_context(OperationContext::at(wfg_path.display().to_string())))?;

    let (schemas, wfl_files) = load_from_uses(&mut wfg, wfg_path, vars, false)?;

    let mut rule_plans = Vec::new();
    for wfl_file in &wfl_files {
        let plans = wf_lang::compile_wfl(wfl_file, &schemas).wfgen()?;
        rule_plans.extend(plans);
    }

    Ok(LoadedScenario {
        wfg,
        schemas,
        wfl_files,
        rule_plans,
    })
}

/// Load `.wfs` schemas and `.wfl` rule files referenced by `use` declarations,
/// and resolve the injection `use from "…"` value files.
///
/// Paths in `use` declarations **and** in injection `use from` are resolved
/// relative to `wfg_path`'s directory (absolute paths are used as-is).
/// `.wfl` sources are preprocessed with `vars` (and environment variable
/// fallback) via [`wf_lang::preprocess_vars_with_env`] before parsing.
///
/// `wfg` is taken by `&mut` because [`resolve_inject_files`] rewrites the
/// injection value sources in place: validation and generation then only ever
/// see the resolved form, and neither has to read files itself.
///
/// When `skip_wfl` is true, `.wfl` entries in `use` declarations are skipped
/// (no parsing, no `_global.wfl` / yield-preset evaluation), and injection
/// value files are **not** resolved either — `--no-wfl` drops the whole rule /
/// injection pipeline, so those files are irrelevant.
pub fn load_from_uses(
    wfg: &mut WfgFile,
    wfg_path: &Path,
    vars: &HashMap<String, String>,
    skip_wfl: bool,
) -> WfgenResult<(Vec<wf_lang::WindowSchema>, Vec<wf_lang::ast::WflFile>)> {
    let base_dir = wfg_path.parent().unwrap_or_else(|| Path::new("."));
    if !skip_wfl {
        resolve_inject_files(wfg, wfg_path)?;
    }
    let mut wfl_vars = vars.clone();
    wfl_vars
        .entry("WORK_DIR".to_string())
        .or_insert_with(|| base_dir.to_string_lossy().to_string());
    let wfl_ctx = ConfigVarContext::from_explicit_vars(wfl_vars);

    let mut schemas = Vec::new();
    let mut wfl_files = Vec::new();

    for use_decl in &wfg.uses {
        let resolved = base_dir.join(&use_decl.path);
        let ext = resolved.extension().and_then(|e| e.to_str()).unwrap_or("");

        match ext {
            "wfs" => {
                let content = std::fs::read_to_string(&resolved).source_err(
                    WfgenReason::Io,
                    format!("reading .wfs from use declaration: {}", resolved.display()),
                )?;
                let parsed = wf_lang::parse_wfs(&content).wfgen()?;
                schemas.extend(parsed);
            }
            "wfl" => {
                if skip_wfl {
                    continue;
                }
                let source = load_wfl_with_context(&resolved, &wfl_ctx, Some(base_dir)).wfgen()?;
                let mut parsed = wf_lang::parse_wfl(&source).wfgen()?;
                // Merge `_global.wfl` yield presets next to the rule file, matching
                // wf-runtime's project prelude convention. Skip when the file
                // being loaded is the prelude itself.
                if let Some(prelude_path) = prelude::prelude_path_for(&resolved)
                    && !prelude::is_prelude_file(&resolved, &prelude_path)
                {
                    let prelude = prelude::load_rule_prelude(&prelude_path, &wfl_ctx, base_dir)?;
                    prelude::validate_rule_prelude_conflicts(&parsed, &resolved, &prelude)?;
                    prelude::apply_rule_prelude(&mut parsed, &prelude);
                }
                wfl_files.push(parsed);
            }
            other => {
                return error::fail(
                    WfgenReason::Validation,
                    format!(
                        "unsupported file extension '{}' in use declaration: {}",
                        other, use_decl.path
                    ),
                );
            }
        }
    }

    Ok((schemas, wfl_files))
}

/// 就地解析注入用例里 `use from "path"` 的值文件（设计 §3.3）。
///
/// 路径相对 `.wfg` 所在目录（与 `use "x.wfs"` 同一基准），绝对路径按原样用。
/// 文件内容解析成 [`ValueSource::Json`]，与 `use({...})` 的内联形态完全同构：
///
/// - 顶层 **object** → 一条记录，顶层键展开为字段（`_` 前缀键忽略）；
/// - 顶层 **object 数组** → 多条记录，生成时按事件序号循环取用（`N > 记录数` 回绕）；
/// - 其余（每行一个 object 的 NDJSON）→ 多条记录。
///
/// 解析失败、文件不存在、顶层不是 object / object 数组、数组为空都在这里报错——
/// 不做"静默产出空字段"的兜底。
pub fn resolve_inject_files(wfg: &mut WfgFile, wfg_path: &Path) -> WfgenResult<()> {
    let base_dir = wfg_path.parent().unwrap_or_else(|| Path::new("."));
    let Some(injection) = wfg
        .syntax
        .as_mut()
        .and_then(|syntax| syntax.injection.as_mut())
    else {
        return Ok(());
    };

    for case in &mut injection.cases {
        for group in &mut case.groups {
            let ValueSource::File(path) = &group.source else {
                continue;
            };
            let resolved = resolve_relative(base_dir, path);
            let content = std::fs::read_to_string(&resolved).source_err(
                WfgenReason::Io,
                format!(
                    "reading inject value file: {} (from `use from \"{path}\"`)",
                    resolved.display()
                ),
            )?;
            group.source = ValueSource::Json(parse_inject_value_file(&content, &resolved)?);
        }
    }

    Ok(())
}

/// `base_dir` 下的相对路径；绝对路径原样返回。
fn resolve_relative(base_dir: &Path, path: &str) -> PathBuf {
    let path = Path::new(path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base_dir.join(path)
    }
}

/// 值文件内容 → 记录形态的 JSON（object 或 object 数组）。
fn parse_inject_value_file(content: &str, path: &Path) -> WfgenResult<serde_json::Value> {
    // 单份 JSON（object / array）优先；失败再按 NDJSON 逐行解析。
    let value = match serde_json::from_str::<serde_json::Value>(content) {
        Ok(value) => value,
        Err(single_err) => parse_ndjson_records(content).map_err(|ndjson_err| {
            error::error(
                WfgenReason::Validation,
                format!(
                    "inject value file {} 不是合法 JSON，也不是合法的 NDJSON：单份解析失败（{}）；逐行解析失败（{}）",
                    path.display(),
                    single_err,
                    ndjson_err
                ),
            )
        })?,
    };

    validate_inject_records(value, path)
}

/// NDJSON：每行一个 JSON object（空行与 `//` 注释行忽略）。
fn parse_ndjson_records(content: &str) -> Result<serde_json::Value, String> {
    let mut records = Vec::new();
    for (idx, line) in content.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("//") {
            continue;
        }
        let record: serde_json::Value = serde_json::from_str(line)
            .map_err(|err| format!("第 {} 行不是合法 JSON: {}", idx + 1, err))?;
        records.push(record);
    }
    Ok(serde_json::Value::Array(records))
}

/// 记录形态检查：顶层 object，或非空的 object 数组。
fn validate_inject_records(
    value: serde_json::Value,
    path: &Path,
) -> WfgenResult<serde_json::Value> {
    match value {
        serde_json::Value::Object(_) => Ok(value),
        serde_json::Value::Array(items) => {
            if items.is_empty() {
                return error::fail(
                    WfgenReason::Validation,
                    format!("inject value file {} 的记录数组为空", path.display()),
                );
            }
            if let Some(non_object) = items.iter().find(|item| !item.is_object()) {
                return error::fail(
                    WfgenReason::Validation,
                    format!(
                        "inject value file {} 的记录数组元素必须是 JSON object，实际有 {}",
                        path.display(),
                        json_type_name(non_object)
                    ),
                );
            }
            Ok(serde_json::Value::Array(items))
        }
        other => error::fail(
            WfgenReason::Validation,
            format!(
                "inject value file {} 的顶层必须是 JSON object 或 object 数组，实际有 {}",
                path.display(),
                json_type_name(&other)
            ),
        ),
    }
}

fn json_type_name(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "bool",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}
