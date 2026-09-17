//! 生成期注入断言（INJ1 / INJ2，设计 §4.2）。
//!
//! - `hit` 用例的每个实体**必须**产出至少一条告警（INJ1）；
//! - `near_miss` / `miss` 用例的每个实体**必须不**产出告警（INJ2）。
//!
//! 断言以**注入实体**为单位（背景事件不参与），判定直接复用 `gen` 为写
//! `.except.jsonl` 而跑的 oracle 结果——把「最后由 `wfgen verify` 红一行百分比」
//! 提前到生成期，并精确定位到实体。

use std::collections::HashMap;

use crate::datagen::inject_gen::{InjectEntityKey, InjectStepCount};
use crate::error::{self, WfgenReason, WfgenResult};
use crate::oracle::OracleAlert;
use crate::wfg_ast::InjectCaseMode;

#[cfg(test)]
mod tests;

/// 每类失败最多列出的明细条数：语料级失败可能涉及上万个实体，全量列出不可读。
const MAX_EXAMPLES: usize = 5;

/// 按实体逐条断言 hit / near_miss / miss。
///
/// 返回纳入断言的实体个数；无失败时 `Ok`，有失败时一次性汇总报错（含前
/// [`MAX_EXAMPLES`] 条明细 + 总数）。
pub fn assert_inject_modes(
    entities: &[InjectEntityKey],
    alerts: &[OracleAlert],
) -> WfgenResult<usize> {
    if entities.is_empty() {
        return Ok(0);
    }

    // 规则 → 实体 id → 该实体触发的告警（一个实体可能命中多条路径 / 多个窗口）。
    let mut fired: HashMap<&str, HashMap<&str, Vec<&OracleAlert>>> = HashMap::new();
    for alert in alerts {
        fired
            .entry(alert.rule_name.as_str())
            .or_default()
            .entry(alert.entity_id.as_str())
            .or_default()
            .push(alert);
    }

    let mut missing: Vec<&InjectEntityKey> = Vec::new();
    let mut spurious: Vec<(&InjectEntityKey, &OracleAlert)> = Vec::new();

    for entity in entities {
        let entity_id = entity_id_of_value(&entity.value);
        let hits = fired
            .get(entity.rule.as_str())
            .and_then(|by_id| by_id.get(entity_id.as_str()));

        match (entity.mode, hits) {
            (InjectCaseMode::Hit, None) => missing.push(entity),
            (InjectCaseMode::NearMiss | InjectCaseMode::Miss, Some(alerts)) => {
                spurious.push((entity, alerts[0]));
            }
            _ => {}
        }
    }

    if missing.is_empty() && spurious.is_empty() {
        return Ok(entities.len());
    }

    error::fail(
        WfgenReason::Generation,
        render_failures(&missing, &spurious),
    )
}

/// 失败汇总：按 INJ1 / INJ2 分组，各列前若干条明细。
fn render_failures(
    missing: &[&InjectEntityKey],
    spurious: &[(&InjectEntityKey, &OracleAlert)],
) -> String {
    let mut detail = String::from("injection mode assertion failed (§4.2):\n");

    if !missing.is_empty() {
        detail.push_str(&format!(
            "INJ1: {} hit 实体未产出告警（hit 用例每个实体都必须报警）\n",
            missing.len()
        ));
        for entity in missing.iter().take(MAX_EXAMPLES) {
            detail.push_str(&format!(
                "  hit 用例第 {} 个实体（{}={}）不会触发规则 {}：{}\n",
                entity.index,
                entity.field,
                entity_id_of_value(&entity.value),
                entity.rule,
                missing_reason(&entity.steps),
            ));
        }
        append_more(&mut detail, missing.len());
    }

    if !spurious.is_empty() {
        detail.push_str(&format!(
            "INJ2: {} near_miss/miss 实体产出了告警（必须一条都不报）\n",
            spurious.len()
        ));
        for (entity, alert) in spurious.iter().take(MAX_EXAMPLES) {
            detail.push_str(&format!(
                "  {} 用例第 {} 个实体（{}={}）会触发规则 {}：命中 {} 路径（emit_time={}）\n",
                mode_name(entity.mode),
                entity.index,
                entity.field,
                entity_id_of_value(&entity.value),
                entity.rule,
                alert.origin,
                alert.emit_time,
            ));
        }
        append_more(&mut detail, spurious.len());
    }

    detail
}

fn append_more(detail: &mut String, total: usize) {
    if total > MAX_EXAMPLES {
        detail.push_str(&format!("  …同码合计 {} 个实体\n", total));
    }
}

/// INJ1 的原因：把该实体在各 bind 上的实际条数 / 阈值摊开。
///
/// 阈值口径来自规则 `on event` 步（`RuleStructure` 只收 event 步）；全部达阈值
/// 却仍没报警时，原因通常在 `on close` 未满足、conv top-N 过滤，或该实体确实
/// 没能成簇（事件被窗口/实例边界切开）——消息里点出这些方向。
fn missing_reason(steps: &[InjectStepCount]) -> String {
    if steps.is_empty() {
        return "该规则没有可注入的 bind".to_string();
    }
    let detail = steps
        .iter()
        .map(|step| format!("{} {}/{}", step.bind_alias, step.count, step.threshold))
        .collect::<Vec<_>>()
        .join("、");

    let unmet: Vec<&str> = steps
        .iter()
        .filter(|step| step.count < step.threshold)
        .map(|step| step.bind_alias.as_str())
        .collect();
    if unmet.is_empty() {
        format!(
            "条数/阈值 {}（on event 步均达阈值：可能被 on close / conv top-N 过滤）",
            detail
        )
    } else {
        format!("条数/阈值 {}（{} 未达阈值）", detail, unmet.join("、"))
    }
}

fn mode_name(mode: InjectCaseMode) -> &'static str {
    match mode {
        InjectCaseMode::Hit => "hit",
        InjectCaseMode::NearMiss => "near_miss",
        InjectCaseMode::Miss => "miss",
    }
}

/// 注入字段值 → 与 oracle `entity_id` 同一口径的字符串。
///
/// oracle 侧 `entity_id` 由 `wf_cep::cep::key::value_to_string` 渲染（Str 透传 /
/// 数字整数不带 `.0` / 容器退化为 `[array]`、`[object]`），这里保持一致。
pub fn entity_id_of_value(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n
            .as_f64()
            .map(crate::oracle::format_f64)
            .unwrap_or_else(|| n.to_string()),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Null => String::new(),
        serde_json::Value::Array(_) => "[array]".to_string(),
        serde_json::Value::Object(_) => "[object]".to_string(),
    }
}
