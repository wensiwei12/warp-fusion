mod dispatch;
mod extract;
mod helpers;
mod hit;
mod near_miss;
mod non_hit;
mod structures;

use chrono::{DateTime, Utc};
use rand::rngs::StdRng;
use std::time::Duration;
use wf_lang::WindowSchema;
use wf_lang::plan::RulePlan;

use crate::error::{self, WfgenReason, WfgenResult};
use crate::injection_targets::injected_rule_names;
use crate::wfg_ast::WfgFile;

use dispatch::build_alias_map_for_syntax_case;
use extract::extract_rule_structure;
use structures::InjectEntities;
pub use structures::{InjectEntityKey, InjectGenResult, InjectStepCount};

/// Generate inject events driven by rule plans.
///
/// For each inject block in the scenario, generates hit / near-miss / non-hit
/// event clusters according to the rule's structure and thresholds.
pub fn generate_inject_events(
    wfg: &WfgFile,
    rule_plans: &[RulePlan],
    schemas: &[WindowSchema],
    start: &DateTime<Utc>,
    duration: &Duration,
    rng: &mut StdRng,
) -> WfgenResult<InjectGenResult> {
    let scenario = &wfg.scenario;

    let mut all_events = Vec::new();
    let mut entities = InjectEntities::default();

    if let Some(injection) = wfg
        .syntax
        .as_ref()
        .and_then(|syntax| syntax.injection.as_ref())
    {
        let _ = injected_rule_names(wfg)?;

        for case in &injection.cases {
            let rule_plan = resolve_rule_plan(&case.target_rule, rule_plans)?;
            let alias_map = build_alias_map_for_syntax_case(case, &scenario.streams, rule_plan)?;
            let rule_struct = extract_rule_structure(rule_plan, &alias_map)?;
            // 每个用例独占一段实体 id：不同的用例（尤其 hit 与 near_miss）不能
            // 指向同一个实体，否则两个模式的口径互相污染。
            let entity_base = entities.next_entity_base();
            let events = dispatch::generate_for_syntax_case(
                case,
                &rule_struct,
                entity_base,
                schemas,
                &scenario.streams,
                start,
                duration,
                rng,
                &mut entities,
            )?;
            all_events.extend(events);
        }
    }

    Ok(InjectGenResult {
        events: all_events,
        entity_keys: entities.keys,
        unasserted_entities: entities.unasserted,
    })
}

fn resolve_rule_plan(
    inject_rule: impl AsRef<str>,
    rule_plans: &[RulePlan],
) -> WfgenResult<&RulePlan> {
    let inject_rule = inject_rule.as_ref();
    if inject_rule.is_empty() {
        if rule_plans.len() == 1 {
            return Ok(&rule_plans[0]);
        }
        return error::fail(
            WfgenReason::Validation,
            format!(
                "injection target rule is ambiguous: expect(...) is missing and {} rules are loaded",
                rule_plans.len()
            ),
        );
    }

    rule_plans
        .iter()
        .find(|p| p.name == inject_rule)
        .ok_or_else(|| {
            error::error(
                WfgenReason::Validation,
                format!(
                    "inject references rule '{}' not found in compiled plans",
                    inject_rule
                ),
            )
        })
}
