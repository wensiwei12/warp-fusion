use std::collections::{HashMap, HashSet};

use wf_lang::WindowSchema;
use wf_lang::ast::RuleDecl;

use super::ValidationError;
use crate::wfg_ast::{
    AttrValue, ExpectValue, FieldPredicate, InjectCase, SeqStep, ValueSource, WfgFile,
    json_top_level_entries,
};

pub(super) fn validate_syntax(
    wfg: &WfgFile,
    schemas: &[WindowSchema],
    all_rules: &[&RuleDecl],
    skip_wfl: bool,
) -> Vec<ValidationError> {
    let mut errors = Vec::new();
    let Some(syntax) = &wfg.syntax else {
        return errors;
    };

    if syntax.traffic.streams.is_empty() {
        errors.push(ValidationError {
            code: "VN1",
            message: "traffic block must contain at least one stream".to_string(),
        });
    }

    for s in &syntax.traffic.streams {
        if s.rate.approx_eps() <= 0.0 {
            errors.push(ValidationError {
                code: "VN2",
                message: format!("stream '{}': rate must be greater than 0", s.stream),
            });
        }
        if !schemas.iter().any(|ws| ws.name == s.stream) {
            errors.push(ValidationError {
                code: "VN3",
                message: format!(
                    "stream '{}' not found in loaded schemas (.wfs windows)",
                    s.stream
                ),
            });
        }
    }

    if let Some(inj) = &syntax.injection {
        let traffic_streams: HashSet<&str> = syntax
            .traffic
            .streams
            .iter()
            .map(|stream| stream.stream.as_str())
            .collect();
        let schemas_by_name: HashMap<&str, &WindowSchema> = schemas
            .iter()
            .map(|schema| (schema.name.as_str(), schema))
            .collect();
        // 两种形态并存期：旧形态校验比例与 seq 步骤，新形态校验显式数量与事件组。
        let mut legacy_percent_sum = 0.0;
        let duration = wfg.scenario.time_clause.duration;

        for case in &inj.cases {
            let stream = case.stream();
            let case_schema = schemas_by_name.get(stream).copied();
            if !traffic_streams.contains(stream) {
                errors.push(ValidationError {
                    code: "VN10",
                    message: format!(
                        "injection case stream '{}' is not declared in traffic",
                        stream
                    ),
                });
            }

            match case {
                InjectCase::Legacy(legacy) => {
                    if legacy.percent <= 0.0 || legacy.percent > 100.0 {
                        errors.push(ValidationError {
                            code: "VN4",
                            message: format!(
                                "injection case '{}' percent {} must be in (0, 100]",
                                stream, legacy.percent
                            ),
                        });
                    }
                    legacy_percent_sum += legacy.percent;

                    if legacy.seq.steps.is_empty() {
                        errors.push(ValidationError {
                            code: "VN5",
                            message: format!(
                                "injection case '{}' must contain at least one seq step",
                                stream
                            ),
                        });
                    }

                    for (step_idx, step) in legacy.seq.steps.iter().enumerate() {
                        // `use({...})` 的顶层键就是字段覆盖：物化成 predicates 后走同一套
                        // 检查（重名 / 与 seq 实体键重复 / 字段不在 schema）。
                        let json_predicates: Vec<FieldPredicate>;
                        let (step_kind, predicates, count) = match step {
                            SeqStep::Use {
                                predicates, count, ..
                            } => ("use(...)", predicates.as_slice(), Some(*count)),
                            SeqStep::UseJson { json, count } => {
                                json_predicates = source_predicates(
                                    &ValueSource::Json(json.clone()),
                                    &mut errors,
                                    stream,
                                    step_idx,
                                );
                                ("use({...})", json_predicates.as_slice(), Some(*count))
                            }
                            SeqStep::Not { predicates, .. } => {
                                errors.push(ValidationError {
                                    code: "VN16",
                                    message: format!(
                                        "injection case '{}' step {} not(...) is not supported by datagen yet",
                                        stream, step_idx
                                    ),
                                });
                                ("not(...)", predicates.as_slice(), None)
                            }
                        };
                        if count == Some(0) {
                            errors.push(ValidationError {
                                code: "VN15",
                                message: format!(
                                    "injection case '{}' step {} use(...) count must be greater than 0",
                                    stream, step_idx
                                ),
                            });
                        }
                        check_predicate_fields(
                            &mut errors,
                            stream,
                            step_idx,
                            step_kind,
                            predicates,
                            Some(legacy.seq.entity.as_str()),
                            case_schema,
                        );
                    }
                }
                InjectCase::Explicit(explicit) => {
                    if explicit.entity_count == 0 {
                        errors.push(ValidationError {
                            code: "VN21",
                            message: format!(
                                "injection case '{}' 实体个数必须大于 0（hit<0>）",
                                stream
                            ),
                        });
                    }
                    if explicit.groups.is_empty() {
                        errors.push(ValidationError {
                            code: "VN21",
                            message: format!(
                                "injection case '{}' 至少需要一个 `use ... x N` 事件组",
                                stream
                            ),
                        });
                    }
                    if let Some(spread) = explicit.spread
                        && spread > duration
                    {
                        errors.push(ValidationError {
                            code: "VN25",
                            message: format!(
                                "injection case '{}' spread {:?} 超过场景 duration {:?}",
                                stream, spread, duration
                            ),
                        });
                    }
                    for (idx, group) in explicit.groups.iter().enumerate() {
                        if group.count == 0 {
                            errors.push(ValidationError {
                                code: "VN21",
                                message: format!(
                                    "injection case '{}' 第 {} 个事件组 x 0：每个实体的条数必须大于 0",
                                    stream,
                                    idx + 1
                                ),
                            });
                        }
                        let predicates = source_predicates(&group.source, &mut errors, stream, idx);
                        check_predicate_fields(
                            &mut errors,
                            stream,
                            idx,
                            "use",
                            &predicates,
                            explicit.entity_field.as_deref(),
                            case_schema,
                        );
                    }
                }
            }
        }
        if legacy_percent_sum > 100.0 {
            errors.push(ValidationError {
                code: "VN6",
                message: format!(
                    "injection percentages sum to {}, which exceeds 100%",
                    legacy_percent_sum
                ),
            });
        }
    }

    let expected_rules: HashSet<&str> = syntax
        .expect
        .as_ref()
        .map(|expect| {
            expect
                .checks
                .iter()
                .map(|check| check.rule.as_str())
                .collect()
        })
        .unwrap_or_default();

    // Rule-presence checks (VN13/VN14) are skipped when the WFL pipeline is
    // opted out (--no-wfl / --no-oracle): there are no rules to reference.
    if !skip_wfl && let Some(inj) = &syntax.injection {
        for case in &inj.cases {
            if let Some(target_rule) = case.target_rule() {
                if !all_rules.iter().any(|rule| rule.name == target_rule) {
                    errors.push(ValidationError {
                        code: "VN14",
                        message: format!(
                            "injection case '{}' targets rule '{}' not found in WFL files",
                            case.stream(),
                            target_rule
                        ),
                    });
                }
            } else if expected_rules.len() != 1 {
                errors.push(ValidationError {
                    code: "VN13",
                    message: format!(
                        "injection case '{}' must use 'for RULE' because expect identifies {} target rules",
                        case.stream(),
                        expected_rules.len()
                    ),
                });
            }
        }
    }

    if let Some(expect) = &syntax.expect {
        for check in &expect.checks {
            if !skip_wfl && !all_rules.iter().any(|r| r.name == check.rule) {
                errors.push(ValidationError {
                    code: "VN7",
                    message: format!("expect: rule '{}' not found in WFL files", check.rule),
                });
            }
            if let ExpectValue::Percent(p) = check.value
                && !(0.0..=100.0).contains(&p)
            {
                errors.push(ValidationError {
                    code: "VN8",
                    message: format!(
                        "expect percentage for rule '{}' must be in [0, 100], got {}",
                        check.rule, p
                    ),
                });
            }
        }
    }

    errors
}

/// 事件组的字段覆盖：`use({...})` 物化顶层键；`use from` 的内容由 loader 校验。
fn source_predicates(
    source: &ValueSource,
    errors: &mut Vec<ValidationError>,
    stream: &str,
    idx: usize,
) -> Vec<FieldPredicate> {
    match source {
        ValueSource::Predicates(predicates) => predicates.clone(),
        ValueSource::Json(json) => {
            let entries = json_top_level_entries(json).unwrap_or_else(|| {
                errors.push(ValidationError {
                    code: "VN17",
                    message: format!(
                        "injection case '{}' 第 {} 个事件组 use({{...}}) 的顶层必须是 JSON object",
                        stream,
                        idx + 1
                    ),
                });
                Vec::new()
            });
            entries
                .into_iter()
                .map(|(field, value)| FieldPredicate {
                    field,
                    value: AttrValue::Json(value),
                })
                .collect()
        }
        ValueSource::File(_) => Vec::new(),
    }
}

/// 注入用例里字段覆盖的公共检查（重名 / 与实体键重复 / 字段不在 schema）。
fn check_predicate_fields(
    errors: &mut Vec<ValidationError>,
    stream: &str,
    step_idx: usize,
    step_kind: &str,
    predicates: &[FieldPredicate],
    entity_field: Option<&str>,
    case_schema: Option<&WindowSchema>,
) {
    let mut seen = HashSet::new();
    for pred in predicates {
        if !seen.insert(pred.field.as_str()) {
            errors.push(ValidationError {
                code: "VN9",
                message: format!(
                    "injection case '{}' step {} has duplicate field '{}' in {}",
                    stream, step_idx, pred.field, step_kind
                ),
            });
        }
        if let Some(entity) = entity_field
            && pred.field == entity
        {
            errors.push(ValidationError {
                code: "VN12",
                message: format!(
                    "injection case '{}' step {} repeats entity field '{}' in {}",
                    stream, step_idx, pred.field, step_kind
                ),
            });
        }
        if let Some(schema) = case_schema
            && !schema.fields.iter().any(|field| field.name == pred.field)
        {
            errors.push(ValidationError {
                code: "VN11",
                message: format!(
                    "injection case '{}' step {} field '{}' not found in schema '{}'",
                    stream, step_idx, pred.field, schema.name
                ),
            });
        }
    }
}
