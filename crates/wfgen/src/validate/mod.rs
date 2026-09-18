mod syntax;

#[cfg(test)]
mod tests;

use wf_lang::WindowSchema;
use wf_lang::ast::WflFile;

use crate::wfg_ast::WfgFile;

/// A validation error found in a `.wfg` file.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidationError {
    pub code: &'static str,
    pub message: String,
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)
    }
}

/// Validate a parsed `.wfg` file against schemas and WFL rules.
///
/// Returns a list of validation errors (empty if valid).
///
/// 只有 stream-first 语法存在（`wfg_parser` 只实现它，`WfgFile::syntax` 恒为 `Some`），
/// 所以这里只有一条校验路径；手搓出来、没有 `syntax` 段的 `WfgFile` 没有可校验的内容。
///
/// When `skip_wfl` is true, validations that require WFL rules to exist
/// (injection cases referencing rules) are skipped, so a scenario can be
/// generated as baseline events without any rule files.
pub fn validate_wfg(
    wfg: &WfgFile,
    schemas: &[WindowSchema],
    wfl_files: &[WflFile],
    skip_wfl: bool,
) -> Vec<ValidationError> {
    let all_rules: Vec<_> = wfl_files.iter().flat_map(|f| f.rules.iter()).collect();
    syntax::validate_syntax(wfg, schemas, &all_rules, skip_wfl)
}
