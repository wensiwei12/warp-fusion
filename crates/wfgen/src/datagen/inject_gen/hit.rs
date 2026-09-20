use crate::wfg_ast::InjectCase;
use std::time::Duration;

use chrono::{DateTime, Utc};
use rand::rngs::StdRng;
use wf_lang::WindowSchema;

use super::helpers::{
    compute_hit_counts, compute_window_bounds, generate_cluster_events, generate_key_values,
    resolve_cluster_count, uniform_cluster_start,
};
use super::structures::{InjectEntities, InjectOverrides, RuleStructure};
use crate::datagen::stream_gen::GenEvent;
use crate::error::WfgenResult;

#[allow(clippy::too_many_arguments)]
pub(super) fn generate_hit_clusters(
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
    // 条数完全由 `use ... x N` 决定，模式不改数字、阈值只作断言口径（设计 §4.2）。
    let effective_steps = &rule_struct.steps;
    if effective_steps.is_empty() {
        return Ok(Vec::new());
    }

    let step_event_counts = compute_hit_counts(effective_steps, overrides)?;
    let num_clusters = resolve_cluster_count(overrides);
    if num_clusters == 0 {
        return Ok(Vec::new());
    }

    let dur_secs = duration.as_secs_f64();
    let window_dur = overrides.within.unwrap_or(rule_struct.window_dur);
    let (window_secs, max_start_offset) = compute_window_bounds(dur_secs, window_dur);

    let mut events = Vec::new();

    for (entity_counter, _cluster_idx) in (0_u64..).zip(0..num_clusters) {
        let entity_id = entity_base + entity_counter;
        let mut key_overrides = generate_key_values(
            &rule_struct.entity_key_fields(overrides.entity_field.as_deref()),
            entity_id,
            "hit",
            schemas,
            effective_steps,
        );
        // join 驱动侧连接键跟实体标识同值（否则连接条件恒不成立）。
        rule_struct.mirror_join_keys(&mut key_overrides);
        entities.record_entity(
            case,
            rule_struct,
            entity_id,
            entity_counter + 1,
            &key_overrides,
            &step_event_counts,
        );

        let cluster_start_secs =
            uniform_cluster_start(entity_counter, num_clusters, max_start_offset);
        generate_cluster_events(
            effective_steps,
            &step_event_counts,
            &key_overrides,
            &overrides.use_steps,
            &overrides.joins,
            &rule_struct.joins,
            cluster_start_secs,
            window_secs,
            schemas,
            start,
            rng,
            &mut events,
        )?;
    }

    Ok(events)
}
