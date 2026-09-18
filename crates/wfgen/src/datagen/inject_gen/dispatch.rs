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
use super::structures::{AliasMap, InjectEntities, InjectOverrides, RuleStructure};
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
    let map_bind = |bind_alias: &str, bind_to_scenario: &mut HashMap<String, (String, String)>| {
        let Some(bind) = rule_plan.binds.iter().find(|bind| bind.alias == bind_alias) else {
            return;
        };
        if bind.window != stream_block.window {
            return;
        }
        bind_to_scenario
            .entry(bind_alias.to_string())
            .or_insert_with(|| (stream_block.alias.clone(), stream_block.window.clone()));
    };

    for step_plan in &rule_plan.match_plan.event_steps {
        for branch in &step_plan.branches {
            map_bind(&branch.source, &mut bind_to_scenario);
        }
    }

    // `on each` 规则没有 match 步骤（`match_plan` 是空的 `Fixed(0)`）：别名来自
    // `each_plan.alias`（设计 §3.7：`on each s` + `entity(…, s.event_id)` 可被注入）。
    if let Some(each) = &rule_plan.each_plan {
        map_bind(&each.alias, &mut bind_to_scenario);
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
    entity_base: u64,
    schemas: &[WindowSchema],
    start: &DateTime<Utc>,
    duration: &Duration,
    rng: &mut StdRng,
    entities: &mut InjectEntities,
) -> WfgenResult<Vec<GenEvent>> {
    let overrides = extract_syntax_case_overrides(case)?;
    generate_for_mode(
        case,
        &overrides,
        rule_struct,
        entity_base,
        schemas,
        start,
        duration,
        rng,
        entities,
    )
}

#[allow(clippy::too_many_arguments)]
fn generate_for_mode(
    case: &InjectCase,
    overrides: &InjectOverrides,
    rule_struct: &RuleStructure,
    entity_base: u64,
    schemas: &[WindowSchema],
    start: &DateTime<Utc>,
    duration: &Duration,
    rng: &mut StdRng,
    entities: &mut InjectEntities,
) -> WfgenResult<Vec<GenEvent>> {
    match case.mode {
        InjectCaseMode::Hit => generate_hit_clusters(
            case,
            rule_struct,
            entity_base,
            schemas,
            start,
            duration,
            rng,
            entities,
            overrides,
        ),
        InjectCaseMode::NearMiss => generate_near_miss_clusters(
            case,
            rule_struct,
            entity_base,
            schemas,
            start,
            duration,
            rng,
            entities,
            overrides,
        ),
        InjectCaseMode::Miss => generate_non_hit_events(
            case,
            rule_struct,
            entity_base,
            schemas,
            start,
            duration,
            rng,
            entities,
            overrides,
        ),
    }
}
