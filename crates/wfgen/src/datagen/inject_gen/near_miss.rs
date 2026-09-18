use crate::wfg_ast::InjectCase;
use std::time::Duration;

use chrono::{DateTime, Utc};
use rand::rngs::StdRng;
use wf_lang::WindowSchema;

use super::helpers::{
    compute_near_miss_counts, compute_window_bounds, generate_cluster_events, generate_key_values,
    resolve_cluster_count, uniform_cluster_start,
};
use super::structures::{InjectEntities, InjectOverrides, RuleStructure};
use crate::datagen::stream_gen::GenEvent;
use crate::error::WfgenResult;

#[allow(clippy::too_many_arguments)]
pub(super) fn generate_near_miss_clusters(
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

    let near_miss_counts = compute_near_miss_counts(steps, overrides)?;

    // Total events per cluster
    let events_per_cluster: u64 = near_miss_counts.iter().sum();
    if events_per_cluster == 0 {
        return Ok(Vec::new());
    }

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
        let key_overrides = generate_key_values(
            &rule_struct.keys,
            entity_id,
            "nm",
            schemas,
            &rule_struct.steps,
            rule_struct.effective_entity_field(overrides.entity_field.as_deref()),
        );
        entities.record_entity(
            case,
            rule_struct,
            entity_id,
            entity_counter + 1,
            &key_overrides,
            &near_miss_counts,
        );

        let cluster_start_secs =
            uniform_cluster_start(entity_counter, num_clusters, max_start_offset);

        generate_cluster_events(
            steps,
            &near_miss_counts,
            &key_overrides,
            &overrides.use_steps,
            &overrides.joins,
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
