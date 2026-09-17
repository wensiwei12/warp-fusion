use std::collections::HashSet;

use crate::error::WfgenResult;
use crate::wfg_ast::WfgFile;

/// 注入用例覆盖到的规则名：生成期断言与 oracle 只跑这些规则。
///
/// `for RULE` 在新语法里必填，因此这里不存在「推不出目标规则」的分支
/// （旧语法由 `expect` 反推目标规则，已随 `expect` 块删除）。
pub fn injected_rule_names(wfg: &WfgFile) -> WfgenResult<HashSet<String>> {
    let Some(injection) = wfg
        .syntax
        .as_ref()
        .and_then(|syntax| syntax.injection.as_ref())
    else {
        return Ok(HashSet::new());
    };

    Ok(injection
        .cases
        .iter()
        .map(|case| case.target_rule.clone())
        .collect())
}
