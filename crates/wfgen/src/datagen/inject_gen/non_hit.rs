use crate::wfg_ast::InjectCase;
use std::time::Duration;

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use rand::rngs::StdRng;
use wf_lang::WindowSchema;

use super::helpers::{
    build_event_fields_with_predicates, generate_key_values,
    plan_use_steps_allowing_filter_conflicts, push_join_events, resolve_cluster_count,
};
use super::structures::{InjectEntities, InjectOverrides, RuleStructure};
use crate::datagen::stream_gen::GenEvent;
use crate::error::{self, WfgenReason, WfgenResult};

/// `miss` 用例：条数就是 `use ... x N`，每个实体一条独立键 → 永远不成簇、不触发规则。
#[allow(clippy::too_many_arguments)]
pub(super) fn generate_non_hit_events(
    case: &InjectCase,
    rule_struct: &RuleStructure,
    entity_base: u64,
    schemas: &[WindowSchema],
    start: &DateTime<Utc>,
    duration: &Duration,
    rng: &mut StdRng,
    entities: &mut InjectEntities,
    overrides: &InjectOverrides,
) -> WfgenResult<Vec<GenEvent>> {
    if overrides.use_steps.is_empty() {
        // 数量是写下来的：没写 `use ... x N` 就没有要发的定向事件。
        // 旧的「stream 配额 × 比例」兜底路径已随旧语法删除（设计 §4.1）。
        return Ok(Vec::new());
    }

    generate_non_hit_use_step_events(
        case,
        rule_struct,
        entity_base,
        schemas,
        start,
        duration,
        rng,
        entities,
        overrides,
    )
}

#[allow(clippy::too_many_arguments)]
fn generate_non_hit_use_step_events(
    case: &InjectCase,
    rule_struct: &RuleStructure,
    entity_base: u64,
    schemas: &[WindowSchema],
    start: &DateTime<Utc>,
    duration: &Duration,
    rng: &mut StdRng,
    entities: &mut InjectEntities,
    overrides: &InjectOverrides,
) -> WfgenResult<Vec<GenEvent>> {
    let steps = &rule_struct.steps;
    if steps.is_empty() {
        return Ok(Vec::new());
    }

    let planned_use_steps = plan_use_steps_allowing_filter_conflicts(steps, &overrides.use_steps)?;
    let mut step_event_counts = vec![0_u64; steps.len()];
    let mut step_records = vec![None; steps.len()];
    for planned in planned_use_steps {
        step_event_counts[planned.rule_step_idx] += planned.count;
        step_records[planned.rule_step_idx] = Some(planned.records);
    }
    if step_event_counts.iter().all(|count| *count == 0) {
        return Ok(Vec::new());
    }

    let shape_repeats = resolve_cluster_count(overrides);
    if shape_repeats == 0 {
        return Ok(Vec::new());
    }

    let dur_nanos = duration.as_nanos() as i64;

    let mut events = Vec::new();
    let mut entity_index = 0_u64;
    let mut event_index = 0_i64;
    let total_events = (shape_repeats * step_event_counts.iter().sum::<u64>()).max(1) as i64;

    for _repeat_idx in 0..shape_repeats {
        for (step_idx, step) in steps.iter().enumerate() {
            let event_count = step_event_counts.get(step_idx).copied().unwrap_or(0);
            if event_count == 0 {
                continue;
            }

            let schema = schemas
                .iter()
                .find(|s| s.name == step.window_name)
                .ok_or_else(|| {
                    error::error(
                        WfgenReason::Validation,
                        format!("schema not found for '{}'", step.window_name),
                    )
                })?;

            let records = step_records[step_idx].as_ref().ok_or_else(|| {
                error::error(
                    WfgenReason::Validation,
                    format!(
                        "injection use step {} is missing predicates for planned miss events",
                        step_idx + 1
                    ),
                )
            })?;

            for event_in_step in 0..event_count {
                // 每个 miss 实体只有一条事件、且只落在本步骤上。
                let entity_id = entity_base + entity_index;
                entity_index += 1;
                let key_overrides = generate_key_values(
                    &rule_struct.keys,
                    entity_id,
                    "miss",
                    schemas,
                    steps,
                    rule_struct.effective_entity_field(overrides.entity_field.as_deref()),
                );
                let mut entity_step_counts = vec![0_u64; steps.len()];
                entity_step_counts[step_idx] = 1;
                entities.record_entity(
                    case,
                    rule_struct,
                    entity_id,
                    entity_index,
                    &key_overrides,
                    &entity_step_counts,
                );

                let offset_nanos = if total_events > 1 {
                    dur_nanos * event_index / total_events
                } else {
                    dur_nanos / 2
                };
                event_index += 1;
                let ts = *start + ChronoDuration::nanoseconds(offset_nanos);

                // 记录按步骤内事件序号轮转（单记录形态恒为第 0 条）。
                let predicates = &records[event_in_step as usize % records.len()];
                let fields = build_event_fields_with_predicates(
                    schema,
                    &key_overrides,
                    &step.filter_overrides,
                    predicates,
                    &ts,
                    rng,
                );

                let stream_name = schema
                    .streams
                    .first()
                    .cloned()
                    .unwrap_or_else(|| schema.name.clone());

                events.push(GenEvent {
                    stream_name,
                    window_name: step.window_name.clone(),
                    timestamp: ts,
                    fields,
                });
                // 设计 §9：为一个左事件补发 `join` 块声明的右事件。
                push_join_events(
                    &overrides.joins,
                    &rule_struct.joins,
                    &key_overrides,
                    &ts,
                    schemas,
                    rng,
                    &mut events,
                )?;
            }
        }
    }

    Ok(events)
}
