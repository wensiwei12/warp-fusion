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

use crate::datagen::stream_gen::GenEvent;

use crate::error::{self, WfgenReason, WfgenResult};
use crate::injection_targets::injected_rule_names;
use crate::wfg_ast::WfgFile;

use dispatch::build_alias_map_for_syntax_case;
use extract::extract_rule_structure;
/// 校验期（`validate/syntax.rs` 的 VN22/VN23）与生成期共用同一份「字段引用 → 叶子
/// 字段名」规则，避免两边对「实体字段是什么」的判断漂移。
pub(crate) use extract::field_ref_field_name;
/// 实体 id 空间上限（24 位）：校验期 VN27 与生成侧的 Ip 映射共用同一份口径，避免两处漂移。
pub(crate) use helpers::ENTITY_ID_SPACE;
use structures::InjectEntities;
pub use structures::{InjectEntityKey, InjectGenResult, InjectStepCount, WithoutGuard};

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
    let mut without_guards = Vec::new();

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
            let keys_before = entities.keys.len();
            let unasserted_before = entities.unasserted;
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
            if !case.withouts.is_empty() {
                without_guards.extend(build_without_guards(
                    case,
                    &rule_struct,
                    &events,
                    &entities.keys[keys_before..],
                    entities.unasserted - unasserted_before,
                )?);
            }
            all_events.extend(events);
        }
    }

    Ok(InjectGenResult {
        events: all_events,
        entity_keys: entities.keys,
        unasserted_entities: entities.unasserted,
        without_guards,
    })
}

/// 把一个用例的 `without(...)` 子句展开成逐实体的执行清单（设计 §3.8）。
///
/// 窗口起点取该实体**首条注入事件**的时间；窗长取 `without ... within D` 的 D，
/// 省略则取目标规则 `match` 的窗口长度（与规则 `not` 步骤的判定窗同口径）。
///
/// 清单建立时就地检查该实体窗内的**注入事件**：命中谓词的注入事件是“排不掉”的
/// ——窗口内确实会出现匹配事件，规则不会触发，这里直接报错（并给出实体与谓词）。
///
/// 实体标识无法定位（复合 `entity(...)` 等）时同样报生成期错误：`without` 的保证
/// 以实体为单位，定位不到实体就无法把保证落到具体窗口上。
fn build_without_guards(
    case: &crate::wfg_ast::InjectCase,
    rule_struct: &structures::RuleStructure,
    events: &[GenEvent],
    case_keys: &[InjectEntityKey],
    unasserted: u64,
) -> WfgenResult<Vec<WithoutGuard>> {
    if unasserted > 0 {
        return error::fail(
            WfgenReason::Generation,
            format!(
                "injection case '{}' 使用了 without(...)，但规则 '{}' 的实体标识无法定位（复合 entity(...) 表达式）：\
                 without 的保证以实体为单位，需要 `entity(<type>, <单字段>)` 才能确定窗口",
                case.stream, case.target_rule
            ),
        );
    }

    let mut guards = Vec::new();
    for without in &case.withouts {
        let window = without.within.unwrap_or(rule_struct.window_dur);
        let predicates = extract::predicates_to_entries(&without.predicates);
        for key in case_keys {
            let first_ts = first_inject_ts(events, &key.field, &key.value).ok_or_else(|| {
                error::error(
                    WfgenReason::Generation,
                    format!(
                        "injection case '{}' 的实体（{}={}）没有任何注入事件，无法确定 without 窗口起点",
                        case.stream, key.field, key.value
                    ),
                )
            })?;
            let guard = WithoutGuard {
                window_name: case.stream.clone(),
                field: key.field.clone(),
                value: key.value.clone(),
                start: first_ts,
                window,
                predicates: predicates.clone(),
            };
            if let Some(offender) = events.iter().find(|event| guard.is_violation(event)) {
                return error::fail(
                    WfgenReason::Generation,
                    format!(
                        "injection case '{}' 的实体（{}={}）在 without 窗口内注入了命中谓词的事件（{}，时间 {}）：\
                         该实体窗口内不得出现匹配事件，请调整 `use(...)` 的值，或改写 `without(...)` 的谓词",
                        case.stream,
                        key.field,
                        key.value,
                        guard.describe_predicates(),
                        offender.timestamp,
                    ),
                );
            }
            guards.push(guard);
        }
    }
    Ok(guards)
}

/// 该实体在这一批注入事件里的首条时间。
fn first_inject_ts(
    events: &[GenEvent],
    field: &str,
    value: &serde_json::Value,
) -> Option<DateTime<Utc>> {
    events
        .iter()
        .filter(|event| event.fields.get(field) == Some(value))
        .map(|event| event.timestamp)
        .min()
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
