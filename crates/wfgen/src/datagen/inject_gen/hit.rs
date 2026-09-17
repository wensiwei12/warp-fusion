use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, Utc};
use rand::Rng;
use rand::rngs::StdRng;
use wf_lang::WindowSchema;

use super::helpers::{
    compute_hit_counts, compute_window_bounds, generate_cluster_events, generate_key_values,
    resolve_cluster_count,
};
use super::structures::{InjectOverrides, RuleStructure};
use crate::datagen::stream_gen::GenEvent;
use crate::error::WfgenResult;
use crate::wfg_ast::StreamBlock;

#[allow(clippy::too_many_arguments)]
pub(super) fn generate_hit_clusters(
    rule_struct: &RuleStructure,
    schemas: &[WindowSchema],
    scenario_streams: &[StreamBlock],
    start: &DateTime<Utc>,
    duration: &Duration,
    rng: &mut StdRng,
    inject_counts: &mut HashMap<String, u64>,
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

    // Update inject counts
    for (step, event_count) in effective_steps
        .iter()
        .zip(step_event_counts.iter().copied())
    {
        *inject_counts
            .entry(step.scenario_alias.clone())
            .or_insert(0) += event_count * num_clusters;
    }

    let dur_secs = duration.as_secs_f64();
    let window_dur = overrides.within.unwrap_or(rule_struct.window_dur);
    let (window_secs, max_start_offset) = compute_window_bounds(dur_secs, window_dur);

    let mut events = Vec::new();

    for (entity_counter, _cluster_idx) in (0_u64..).zip(0..num_clusters) {
        let key_overrides = generate_key_values(
            &rule_struct.keys,
            entity_counter,
            "hit",
            schemas,
            effective_steps,
            overrides.entity_field.as_deref(),
        );

        let cluster_start_secs = if max_start_offset > 0.0 {
            rng.random_range(0.0..max_start_offset)
        } else {
            0.0
        };
        generate_cluster_events(
            effective_steps,
            &step_event_counts,
            &key_overrides,
            &overrides.use_steps,
            cluster_start_secs,
            window_secs,
            schemas,
            scenario_streams,
            start,
            rng,
            &mut events,
        )?;
    }

    Ok(events)
}
