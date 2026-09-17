use std::collections::HashMap;

use wf_lang::ast::{BinOp, Expr, FieldRef};
use wf_lang::plan::RulePlan;
use wf_lang::plan::WindowSpec;

use super::structures::{
    AliasMap, InjectOverrides, InjectUseStepOverrides, RuleStructure, StepInfo,
};
use crate::error::{self, WfgenReason, WfgenResult};
use crate::wfg_ast::{InjectCase, InjectLine, LegacyInjectCase, ParamValue, SeqStep, ValueSource};

pub(super) fn extract_rule_structure(
    rule_plan: &RulePlan,
    alias_map: &AliasMap,
) -> WfgenResult<RuleStructure> {
    let window_dur = match rule_plan.match_plan.window_spec {
        WindowSpec::Sliding(d) | WindowSpec::Fixed(d) | WindowSpec::Session(d) => d,
        // Hop 的注入窗长 = 窗口大小（slide 步长不改变窗长口径）。
        WindowSpec::Hop { size, .. } => size,
    };

    let keys: Vec<String> = rule_plan
        .match_plan
        .keys
        .iter()
        .map(|fr| field_ref_field_name(fr).to_string())
        .collect();

    let mut steps = Vec::new();
    for step_plan in &rule_plan.match_plan.event_steps {
        // P1: take first branch
        let branch = step_plan
            .branches
            .first()
            .ok_or_else(|| error::error(WfgenReason::Validation, "step has no branches"))?;

        let bind_alias = &branch.source;

        // SC6: inject streams are a *subset* of rule aliases.
        // Skip steps whose bind alias is not covered by inject.
        let (scenario_alias, window_name) = match alias_map.bind_to_scenario.get(bind_alias) {
            Some(pair) => pair,
            None => continue,
        };

        let threshold = eval_const_threshold(&branch.agg.threshold).ok_or_else(|| {
            error::error(
                WfgenReason::Validation,
                "cannot evaluate threshold as constant",
            )
        })? as u64;

        // Extract filter constraints from the corresponding bind
        let mut filter_overrides = rule_plan
            .binds
            .iter()
            .find(|b| b.alias == *bind_alias)
            .and_then(|b| b.filter.as_ref())
            .map(extract_filter_constraints)
            .unwrap_or_default();
        if let Some(guard) = &branch.guard {
            filter_overrides.extend(extract_filter_constraints(guard));
        }

        steps.push(StepInfo {
            bind_alias: bind_alias.clone(),
            scenario_alias: scenario_alias.clone(),
            window_name: window_name.clone(),
            measure: branch.agg.measure,
            threshold,
            filter_overrides,
        });
    }

    if steps.is_empty() {
        return error::fail(
            WfgenReason::Validation,
            format!(
                "no inject streams map to any step in rule '{}'; \
                 at least one inject alias must match a rule bind alias",
                rule_plan.name
            ),
        );
    }

    let entity_id_field = extract_entity_id_field(&rule_plan.entity_plan.entity_id_expr);

    Ok(RuleStructure {
        keys,
        window_dur,
        steps,
        entity_id_field,
    })
}

/// Extract a constant numeric value from an expression (L1 thresholds).
pub(crate) fn eval_const_threshold(expr: &Expr) -> Option<f64> {
    match expr {
        Expr::Number(n) => Some(*n),
        Expr::Neg(inner) => eval_const_threshold(inner).map(|v| -v),
        _ => None,
    }
}

pub(crate) fn field_ref_field_name(fr: &FieldRef) -> &str {
    match fr {
        FieldRef::Simple(name) => name,
        FieldRef::Qualified(_, name) | FieldRef::Bracketed(_, name) => name,
        _ => "",
    }
}

pub(super) fn extract_inject_overrides(inject_line: &InjectLine) -> InjectOverrides {
    let mut overrides = InjectOverrides {
        entity_field: None,
        entity_count: None,
        count_per_entity: None,
        steps_completed: None,
        within: None,
        use_steps: Vec::new(),
    };

    for param in &inject_line.params {
        match param.name.as_str() {
            "count_per_entity" => {
                if let ParamValue::Number(n) = &param.value {
                    overrides.count_per_entity = Some(*n as u64);
                }
            }
            "steps_completed" => {
                if let ParamValue::Number(n) = &param.value {
                    overrides.steps_completed = Some(*n as usize);
                }
            }
            "within" => {
                if let ParamValue::Duration(d) = &param.value {
                    overrides.within = Some(*d);
                }
            }
            _ => {}
        }
    }

    for use_step in &inject_line.use_steps {
        let mut predicates = HashMap::new();
        for pred in &use_step.predicates {
            if let Some(value) = attr_value_to_json(&pred.value) {
                predicates.insert(pred.field.clone(), value);
            }
        }
        overrides.use_steps.push(InjectUseStepOverrides {
            count: use_step.count,
            predicates,
        });
    }

    overrides
}

pub(super) fn extract_syntax_case_overrides(case: &InjectCase) -> WfgenResult<InjectOverrides> {
    match case {
        InjectCase::Legacy(legacy) => Ok(extract_legacy_overrides(legacy)),
        InjectCase::Explicit(explicit) => {
            let mut use_steps = Vec::with_capacity(explicit.groups.len());
            for group in &explicit.groups {
                use_steps.push(InjectUseStepOverrides {
                    count: group.count,
                    predicates: source_to_predicates(&group.source)?,
                });
            }
            Ok(InjectOverrides {
                entity_field: explicit.entity_field.clone(),
                entity_count: Some(explicit.entity_count),
                count_per_entity: None,
                steps_completed: None,
                within: explicit.spread,
                use_steps,
            })
        }
    }
}

/// 旧语法用例的提取（数量仍由配额推导）。
fn extract_legacy_overrides(case: &LegacyInjectCase) -> InjectOverrides {
    let mut overrides = InjectOverrides {
        entity_field: Some(case.seq.entity.clone()),
        entity_count: None,
        count_per_entity: None,
        steps_completed: None,
        within: None,
        use_steps: Vec::new(),
    };

    for step in &case.seq.steps {
        let (predicates, count) = match step {
            SeqStep::Use { predicates, count } => (predicates.as_slice(), *count),
            SeqStep::UseJson { json, count } => {
                overrides.use_steps.push(InjectUseStepOverrides {
                    count: *count,
                    predicates: crate::wfg_ast::json_top_level_entries(json)
                        .unwrap_or_default()
                        .into_iter()
                        .collect(),
                });
                continue;
            }
            SeqStep::Not { .. } => continue,
        };

        let mut pred_map = HashMap::new();
        for pred in predicates {
            if let Some(value) = attr_value_to_json(&pred.value) {
                pred_map.insert(pred.field.clone(), value);
            }
        }
        overrides.use_steps.push(InjectUseStepOverrides {
            count,
            predicates: pred_map,
        });
    }

    overrides
}

/// 事件组的值来源 → 字段覆盖表。
///
/// `use from "file"` 必须已由 loader 解析成 [`ValueSource::Json`]；残留的
/// `File` 会**报错**而不是静默生成空字段。
fn source_to_predicates(source: &ValueSource) -> WfgenResult<HashMap<String, serde_json::Value>> {
    match source {
        ValueSource::Predicates(predicates) => Ok(predicates
            .iter()
            .filter_map(|p| attr_value_to_json(&p.value).map(|v| (p.field.clone(), v)))
            .collect()),
        ValueSource::Json(json) => Ok(crate::wfg_ast::json_top_level_entries(json)
            .unwrap_or_default()
            .into_iter()
            .collect()),
        ValueSource::File(path) => error::fail(
            WfgenReason::Validation,
            format!(
                "use from `{path}` 尚未解析为内联 JSON；请通过 CLI（wfgen gen / lint）加载场景，\
                 或先调用 loader::resolve_inject_files 做路径解析"
            ),
        ),
    }
}

fn attr_value_to_json(value: &crate::wfg_ast::AttrValue) -> Option<serde_json::Value> {
    match value {
        crate::wfg_ast::AttrValue::Json(v) => Some(v.clone()),
        crate::wfg_ast::AttrValue::String(s) => Some(serde_json::Value::String(s.clone())),
        crate::wfg_ast::AttrValue::Number(n) => Some(serde_json::json!(*n)),
        crate::wfg_ast::AttrValue::Bool(b) => Some(serde_json::Value::Bool(*b)),
        crate::wfg_ast::AttrValue::Duration(d) => {
            Some(serde_json::Value::String(format!("{:?}", d)))
        }
    }
}

fn extract_entity_id_field(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Field(fr) => Some(field_ref_field_name(fr).to_string()),
        _ => None,
    }
}

/// Extract field equality constraints from a filter expression.
///
/// Supports:
/// - `field == "value"`, `field == number`, `field == bool`
/// - `cond1 && cond2` (recursively extracts from both sides)
pub(crate) fn extract_filter_constraints(filter: &Expr) -> HashMap<String, serde_json::Value> {
    let mut constraints = HashMap::new();
    extract_filter_constraints_recursive(filter, &mut constraints);
    constraints
}

fn extract_filter_constraints_recursive(
    expr: &Expr,
    constraints: &mut HashMap<String, serde_json::Value>,
) {
    if let Expr::BinOp { op, left, right } = expr {
        match op {
            BinOp::And => {
                // Recursively handle AND-connected conditions
                extract_filter_constraints_recursive(left, constraints);
                extract_filter_constraints_recursive(right, constraints);
            }
            BinOp::Eq => {
                // Extract field == value
                if let Expr::Field(fr) = left.as_ref() {
                    let field_name = field_ref_field_name(fr);
                    if let Some(value) = expr_to_json_value(right.as_ref()) {
                        constraints.insert(field_name.to_string(), value);
                    }
                }
                // Also handle value == field
                if let Expr::Field(fr) = right.as_ref() {
                    let field_name = field_ref_field_name(fr);
                    if let Some(value) = expr_to_json_value(left.as_ref()) {
                        constraints.insert(field_name.to_string(), value);
                    }
                }
            }
            _ => {}
        }
    }
}

fn expr_to_json_value(expr: &Expr) -> Option<serde_json::Value> {
    match expr {
        Expr::StringLit(s) => Some(serde_json::Value::String(s.clone())),
        Expr::Number(n) => Some(serde_json::json!(*n)),
        Expr::Bool(b) => Some(serde_json::Value::Bool(*b)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn case_of(input: &str) -> InjectCase {
        let wfg = crate::wfg_parser::parse_wfg(input).expect("parse");
        wfg.syntax
            .as_ref()
            .and_then(|s| s.injection.as_ref())
            .map(|inj| inj.cases[0].clone())
            .expect("injection case")
    }

    /// `use({...})` 的顶层键必须原样进入 predicates，且值保持嵌套结构；
    /// `_` 前缀的内部字段被忽略。
    #[test]
    fn use_whole_json_expands_to_predicates() {
        let case = case_of(
            r#"
#[duration=1s]
scenario s<seed=1> {
  traffic { stream sdm_event gen 100/s }
  injection {
    hit<100%> sdm_event {
      sip seq {
        use({
          "tenant_id": "tenant02",
          "source_finding_obj": { "title": "t", "rule": { "label": "账号攻击" } },
          "tags": ["a", "b"],
          "_stream": "ignored"
        }) with(2)
      }
    }
  }
}
"#,
        );

        let ov = extract_syntax_case_overrides(&case).expect("extract");
        assert_eq!(ov.entity_field.as_deref(), Some("sip"));
        assert_eq!(ov.use_steps.len(), 1);
        let step = &ov.use_steps[0];
        assert_eq!(step.count, 2);
        assert_eq!(
            step.predicates.get("tenant_id"),
            Some(&serde_json::json!("tenant02"))
        );
        assert_eq!(
            step.predicates
                .get("source_finding_obj")
                .and_then(|v| v.pointer("/rule/label")),
            Some(&serde_json::json!("账号攻击"))
        );
        assert_eq!(
            step.predicates.get("tags"),
            Some(&serde_json::json!(["a", "b"]))
        );
        assert!(
            !step.predicates.contains_key("_stream"),
            "`_` 前缀的内部字段必须被忽略"
        );
    }

    /// 旧的按字段覆盖形态保持不变。
    #[test]
    fn use_predicates_unchanged() {
        let case = case_of(
            r#"
#[duration=1s]
scenario s<seed=1> {
  traffic { stream sdm_event gen 100/s }
  injection {
    hit<100%> sdm_event {
      sip seq { use(tenant_id="t", n=3) with(1) }
    }
  }
}
"#,
        );
        let ov = extract_syntax_case_overrides(&case).expect("extract");
        let step = &ov.use_steps[0];
        assert_eq!(step.count, 1);
        assert_eq!(
            step.predicates.get("tenant_id"),
            Some(&serde_json::json!("t"))
        );
        assert_eq!(step.predicates.get("n"), Some(&serde_json::json!(3.0)));
    }
}
