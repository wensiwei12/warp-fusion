use std::collections::HashMap;
use std::time::Duration;

use crate::datagen::inject_gen::structures::{InjectOverrides, InjectUseStepOverrides, StepInfo};
use crate::error::{self, WfgenReason, WfgenResult};

pub(crate) struct UseStepPlan {
    pub(crate) rule_step_idx: usize,
    pub(crate) count: u64,
    /// 记录列表：单记录形态长度 1；`use from` 的数组多条 → 按事件序号循环取用。
    pub(crate) records: Vec<HashMap<String, serde_json::Value>>,
}

/// Compute the time window bounds for cluster generation.
///
/// Returns `(window_secs, max_start_offset)` where `max_start_offset` is the
/// latest second at which a cluster can start without exceeding the duration.
pub(crate) fn compute_window_bounds(dur_secs: f64, window_dur: Duration) -> (f64, f64) {
    let window_secs = window_dur.as_secs_f64();
    let max_start_offset = (dur_secs - window_secs).max(0.0);
    (window_secs, max_start_offset)
}

/// 每个步骤每实体生成多少条事件。
///
/// 就是 `use ... x N` 写的数：不做任何隐式推导、补全或夹取——旧语法的
/// "未写步骤补到阈值"（hit）与 `min(N, 阈值-1)`（near_miss）夹取已随旧形态删除。
pub(crate) fn compute_hit_counts(
    steps: &[StepInfo],
    overrides: &InjectOverrides,
) -> WfgenResult<Vec<u64>> {
    compute_use_step_counts(steps, &overrides.use_steps)
}

/// near_miss 与 hit 共用同一套条数口径：**模式不改数字**。
pub(crate) fn compute_near_miss_counts(
    steps: &[StepInfo],
    overrides: &InjectOverrides,
) -> WfgenResult<Vec<u64>> {
    compute_use_step_counts(steps, &overrides.use_steps)
}

/// 簇（实体）个数 = 用户写的实体数。
///
/// 旧的「stream 配额 × 比例 ÷ 每实体条数」推导已随旧形态删除
/// （见 docs/design/wfg-design.md §3.1）。
pub(crate) fn resolve_cluster_count(overrides: &InjectOverrides) -> u64 {
    overrides.entity_count.unwrap_or(0)
}

pub(crate) fn compute_use_step_counts(
    steps: &[StepInfo],
    use_steps: &[InjectUseStepOverrides],
) -> WfgenResult<Vec<u64>> {
    compute_use_step_counts_with_filter_validation(steps, use_steps, true)
}

pub(crate) fn plan_use_steps_allowing_filter_conflicts(
    steps: &[StepInfo],
    use_steps: &[InjectUseStepOverrides],
) -> WfgenResult<Vec<UseStepPlan>> {
    plan_use_steps(steps, use_steps, false)
}

fn compute_use_step_counts_with_filter_validation(
    steps: &[StepInfo],
    use_steps: &[InjectUseStepOverrides],
    validate_filter_conflicts: bool,
) -> WfgenResult<Vec<u64>> {
    if steps.is_empty() {
        return Ok(Vec::new());
    }

    let mut counts = vec![0_u64; steps.len()];
    for planned in plan_use_steps(steps, use_steps, validate_filter_conflicts)? {
        counts[planned.rule_step_idx] += planned.count;
    }

    Ok(counts)
}

pub(crate) fn plan_use_steps(
    steps: &[StepInfo],
    use_steps: &[InjectUseStepOverrides],
    validate_filter_conflicts: bool,
) -> WfgenResult<Vec<UseStepPlan>> {
    if steps.is_empty() {
        return Ok(Vec::new());
    }

    let mut planned = Vec::new();
    for (step_idx, use_step) in use_steps.iter().enumerate() {
        if step_idx >= steps.len() {
            return error::fail(
                WfgenReason::Validation,
                format!(
                    "injection use step {} exceeds rule step count {}; each use(...) maps to one rule step",
                    step_idx + 1,
                    steps.len()
                ),
            );
        }
        if use_step.count == 0 {
            return error::fail(
                WfgenReason::Validation,
                format!(
                    "injection use step {} count must be greater than 0",
                    step_idx + 1
                ),
            );
        }
        if validate_filter_conflicts {
            validate_use_step_predicates(step_idx, use_step, &steps[step_idx])?;
        }
        planned.push(UseStepPlan {
            rule_step_idx: step_idx,
            count: use_step.count,
            records: use_step.records.clone(),
        });
    }

    Ok(planned)
}

fn validate_use_step_predicates(
    step_idx: usize,
    use_step: &InjectUseStepOverrides,
    step: &StepInfo,
) -> WfgenResult<()> {
    // 多记录形态（`use from` 数组）逐条检查：任何一条与规则 filter 冲突都要报，
    // 否则那条记录生成的事件根本不会进入规则窗口（静默少报警）。
    let multi = use_step.records.len() > 1;
    for (record_idx, record) in use_step.records.iter().enumerate() {
        for (field, expected) in &step.filter_overrides {
            let Some(actual) = record.get(field) else {
                continue;
            };
            if actual != expected {
                let record_note = if multi {
                    format!(" (记录 #{}/{})", record_idx + 1, use_step.records.len())
                } else {
                    String::new()
                };
                return error::fail(
                    WfgenReason::Validation,
                    format!(
                        "injection use step {} field '{}' conflicts with rule step filter: use has {}, rule requires {}{}",
                        step_idx + 1,
                        field,
                        actual,
                        expected,
                        record_note
                    ),
                );
            }
        }
    }
    Ok(())
}
