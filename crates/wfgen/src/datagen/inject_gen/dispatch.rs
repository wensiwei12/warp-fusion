use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, Utc};
use rand::rngs::StdRng;
use wf_lang::WindowSchema;
use wf_lang::plan::RulePlan;

use super::extract::extract_syntax_case_overrides;
use super::hit::generate_hit_clusters;
use super::near_miss::generate_near_miss_clusters;
use super::non_hit::generate_non_hit_events;
use super::structures::{AliasMap, InjectOverrides, RuleStructure};
use crate::datagen::stream_gen::GenEvent;
use crate::error::{self, WfgenReason, WfgenResult};
use crate::wfg_ast::{InjectCase, InjectCaseMode, StreamBlock};

pub(super) fn build_alias_map_for_syntax_case(
    case: &InjectCase,
    scenario_streams: &[StreamBlock],
    rule_plan: &RulePlan,
) -> WfgenResult<AliasMap> {
    let stream_block = scenario_streams
        .iter()
        .find(|s| s.alias == case.stream)
        .ok_or_else(|| {
            error::error(
                WfgenReason::Validation,
                format!("inject stream '{}' not found in scenario", case.stream),
            )
        })?;

    let mut bind_to_scenario = HashMap::new();

    for step_plan in &rule_plan.match_plan.event_steps {
        for branch in &step_plan.branches {
            let bind_alias = &branch.source;
            let Some(bind) = rule_plan
                .binds
                .iter()
                .find(|bind| bind.alias == *bind_alias)
            else {
                continue;
            };
            if bind.window != stream_block.window {
                continue;
            }
            bind_to_scenario
                .entry(bind_alias.clone())
                .or_insert_with(|| (stream_block.alias.clone(), stream_block.window.clone()));
        }
    }

    if bind_to_scenario.is_empty() {
        return error::fail(
            WfgenReason::Validation,
            format!(
                "inject stream '{}' cannot be mapped to any event step bind in rule '{}'",
                case.stream, rule_plan.name
            ),
        );
    }

    Ok(AliasMap { bind_to_scenario })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn generate_for_syntax_case(
    case: &InjectCase,
    rule_struct: &RuleStructure,
    schemas: &[WindowSchema],
    scenario_streams: &[StreamBlock],
    start: &DateTime<Utc>,
    duration: &Duration,
    rng: &mut StdRng,
    inject_counts: &mut HashMap<String, u64>,
) -> WfgenResult<Vec<GenEvent>> {
    let overrides = extract_syntax_case_overrides(case)?;
    generate_for_mode(
        case.mode,
        &overrides,
        rule_struct,
        schemas,
        scenario_streams,
        start,
        duration,
        rng,
        inject_counts,
    )
}

#[allow(clippy::too_many_arguments)]
fn generate_for_mode(
    mode: InjectCaseMode,
    overrides: &InjectOverrides,
    rule_struct: &RuleStructure,
    schemas: &[WindowSchema],
    scenario_streams: &[StreamBlock],
    start: &DateTime<Utc>,
    duration: &Duration,
    rng: &mut StdRng,
    inject_counts: &mut HashMap<String, u64>,
) -> WfgenResult<Vec<GenEvent>> {
    match mode {
        InjectCaseMode::Hit => generate_hit_clusters(
            rule_struct,
            schemas,
            scenario_streams,
            start,
            duration,
            rng,
            inject_counts,
            overrides,
        ),
        InjectCaseMode::NearMiss => generate_near_miss_clusters(
            rule_struct,
            schemas,
            scenario_streams,
            start,
            duration,
            rng,
            inject_counts,
            overrides,
        ),
        InjectCaseMode::Miss => generate_non_hit_events(
            rule_struct,
            schemas,
            scenario_streams,
            start,
            duration,
            rng,
            inject_counts,
            overrides,
        ),
    }
}
