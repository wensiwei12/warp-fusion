use std::collections::{HashMap, HashSet};

use wf_lang::WindowSchema;
use wf_lang::ast::{Expr, FieldRef, RuleDecl};

use super::ValidationError;
use crate::datagen::inject_gen::field_ref_field_name;
use crate::wfg_ast::{AttrValue, FieldPredicate, ValueSource, WfgFile, json_top_level_entries};

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

    if syntax.background.streams.is_empty() {
        errors.push(ValidationError {
            code: "VN1",
            message: "background block must contain at least one stream".to_string(),
        });
    }

    for s in &syntax.background.streams {
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

    // VN20（旧的比例形式）在解析期就报错，走不到这里。
    if let Some(inj) = &syntax.injection {
        let background_streams: HashSet<&str> = syntax
            .background
            .streams
            .iter()
            .map(|stream| stream.stream.as_str())
            .collect();
        let schemas_by_name: HashMap<&str, &WindowSchema> = schemas
            .iter()
            .map(|schema| (schema.name.as_str(), schema))
            .collect();
        let duration = wfg.scenario.time_clause.duration;

        for case in &inj.cases {
            let stream = case.stream.as_str();
            let case_schema = schemas_by_name.get(stream).copied();
            if !background_streams.contains(stream) {
                errors.push(ValidationError {
                    code: "VN10",
                    message: format!(
                        "injection case stream '{}' is not declared in background",
                        stream
                    ),
                });
            }

            if case.entity_count == 0 {
                errors.push(ValidationError {
                    code: "VN21",
                    message: format!("injection case '{}' 实体个数必须大于 0（hit<0>）", stream),
                });
            }
            if case.groups.is_empty() {
                errors.push(ValidationError {
                    code: "VN21",
                    message: format!(
                        "injection case '{}' 至少需要一个 `use ... x N` 事件组",
                        stream
                    ),
                });
            }
            // VN25：`spread` 与 `without ... within` 的时间窗不能超过场景 duration——
            // 超出部分会被截断，窗口内“不得出现匹配事件”的保证会静默失效。
            if let Some(spread) = case.spread
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
            for (idx, without) in case.withouts.iter().enumerate() {
                if let Some(within) = without.within
                    && within > duration
                {
                    errors.push(ValidationError {
                        code: "VN25",
                        message: format!(
                            "injection case '{}' 第 {} 个 without 的 within {:?} 超过场景 duration {:?}",
                            stream,
                            idx + 1,
                            within,
                            duration
                        ),
                    });
                }
            }

            // VN22 / VN23：显式实体字段（`hit<sip: 500>`）的静态一致性。
            //
            // 两处漂移都会让注入**静默指向错实体**（生成器拿不到类型时给字符串兜底 /
            // 逐实体变化的字段与规则聚合的 key 不是同一个），最后表现为断言覆盖不到或
            // 断言错对象，因此在校验期就拦下：
            // - VN22：字段必须在该 stream 的 schema 里；
            // - VN23：字段必须与规则**推断**的实体字段一致（单 key `match` = 该 key；
            //   `on each` = `entity(...)` 的单字段）。多 key 规则的实体是 key 元组，
            //   显式写字段是消歧用法，不算不一致（设计 §3.7）。
            if let Some(explicit) = case.entity_field.as_deref() {
                if let Some(schema) = case_schema
                    && !schema.fields.iter().any(|field| field.name == explicit)
                {
                    errors.push(ValidationError {
                        code: "VN22",
                        message: format!(
                            "injection case '{}' 实体字段 '{}' 不在 stream '{}' 的 schema '{}' 里",
                            stream, explicit, stream, schema.name
                        ),
                    });
                }
                if !skip_wfl
                    && let Some(rule) = all_rules.iter().find(|rule| rule.name == case.target_rule)
                    && let Some(inferred) = inferred_entity_field(rule)
                    && inferred != explicit
                {
                    errors.push(ValidationError {
                        code: "VN23",
                        message: format!(
                            "injection case '{}' 显式实体字段 '{}' 与规则 '{}' 推断的实体字段 '{}' 不一致（去掉显式字段即用推断值；多 key 规则才需要显式消歧）",
                            stream, explicit, case.target_rule, inferred
                        ),
                    });
                }
            }

            // VN24：`use` 事件组数不得超过规则的事件步骤数（每个 `use ... x N` 对应
            // 一个步骤）。生成期也拦（`inject_gen::helpers::plan::plan_use_steps`），
            // 这里提前到校验期——数错组数会静默少注入某个步骤的事件。
            if !skip_wfl
                && let Some(rule) = all_rules.iter().find(|rule| rule.name == case.target_rule)
            {
                let step_count = injectable_step_count(rule);
                if case.groups.len() > step_count {
                    errors.push(ValidationError {
                        code: "VN24",
                        message: format!(
                            "injection case '{}' 的 use 事件组数 {} 超过规则 '{}' 的事件步骤数 {}（每个 `use ... x N` 对应一个步骤）",
                            stream,
                            case.groups.len(),
                            case.target_rule,
                            step_count
                        ),
                    });
                }
            }

            for (idx, group) in case.groups.iter().enumerate() {
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
                // 逐条记录检查（`use from` 的数组形态有多条）：字段级校验必须按记录
                // 分别做——不同记录重复出现同一字段是正常的（每条记录都有实体键）。
                for record in source_records(&group.source, &mut errors, stream, idx) {
                    check_predicate_fields(
                        &mut errors,
                        stream,
                        idx,
                        "use",
                        &record,
                        case.entity_field.as_deref(),
                        case_schema,
                    );
                }
            }

            // `without(...)` 的谓词做同款字段检查：写错的字段名会静默变成“删不掉该事件”，
            // 窗口保证失效又不报错（设计 §3.8）。
            for (idx, without) in case.withouts.iter().enumerate() {
                check_predicate_fields(
                    &mut errors,
                    stream,
                    idx,
                    "without",
                    &without.predicates,
                    case.entity_field.as_deref(),
                    case_schema,
                );
            }
        }
    }

    // Rule-presence check (VN14) is skipped when the WFL pipeline is opted out
    // (--no-wfl / --no-oracle): there are no rules to reference.
    if !skip_wfl && let Some(inj) = &syntax.injection {
        for case in &inj.cases {
            if !all_rules.iter().any(|rule| rule.name == case.target_rule) {
                errors.push(ValidationError {
                    code: "VN14",
                    message: format!(
                        "injection case '{}' targets rule '{}' not found in WFL files",
                        case.stream, case.target_rule
                    ),
                });
            }
        }
    }

    errors
}

/// 规则可注入的事件步骤数（设计 §4.1 VN24）。
///
/// 与编译产物同口径（`wf_lang` 的 `compiler/match_build` 装配 `event_steps`）：
/// - `match` 普通形态 = `on event` 的步骤数；
/// - `match` 链形态（`on event seq`）= 链步骤数，**negation 步骤不计**（编译器把它们
///   交给 L2 的 `SeqPlan` 强制执行，不产出 use-step）；
/// - `on each` = 1（生成器为该绑定合成一个步骤）；
/// - stats 形态 = 0（不走 CEP 路径，没有可注入的事件步骤）。
fn injectable_step_count(rule: &RuleDecl) -> usize {
    if rule.each_clause.is_some() {
        return 1;
    }
    if rule.stats_clause.is_some() {
        return 0;
    }
    match &rule.match_clause.seq {
        Some(chain) => chain.steps.iter().filter(|step| !step.neg).count(),
        None => rule.match_clause.on_event.len(),
    }
}

/// 规则推断出的实体字段（设计 §3.7）：单 key `match` → 该 key；`on each` →
/// `entity(<type>, <field>)` 的单字段表达式；显式 key 映射 → 映射的来源字段
/// （真正逐实体变化的字段）。
///
/// 多 key（实体 = key 元组）、stats 形态、`entity(...)` 是复合表达式时返回 `None`
/// ——此时"推断"不是单一字段，显式字段不被视为不一致。
fn inferred_entity_field(rule: &RuleDecl) -> Option<String> {
    if rule.each_clause.is_some() {
        return match &rule.entity.id_expr {
            Expr::Field(fr) => leaf_name(fr),
            _ => None,
        };
    }
    if let Some(mapping) = &rule.match_clause.key_mapping {
        return match mapping.as_slice() {
            [item] => leaf_name(&item.source_field),
            _ => None,
        };
    }
    match rule.match_clause.keys.as_slice() {
        [key] => leaf_name(key),
        _ => None,
    }
}

/// 字段引用的叶子字段名；无法判定（空串）时 `None`，调用方据此跳过一致性检查。
fn leaf_name(fr: &FieldRef) -> Option<String> {
    let name = field_ref_field_name(fr);
    (!name.is_empty()).then(|| name.to_string())
}

/// 事件组的字段覆盖记录：`use({...})` 与 loader 解析后的 `use from` 同构。
///
/// - 顶层 object → 一条记录（顶层键展开为字段）；
/// - 顶层 object 数组 → 多条记录，生成时按事件序号循环取用（设计 §3.3）。
///
/// 记录条数为 0 表示该来源无法物化（VN17 已报），调用方无需再检查字段。
fn source_records(
    source: &ValueSource,
    errors: &mut Vec<ValidationError>,
    stream: &str,
    idx: usize,
) -> Vec<Vec<FieldPredicate>> {
    match source {
        ValueSource::Predicates(predicates) => vec![predicates.clone()],
        ValueSource::Json(json) => match json {
            serde_json::Value::Object(_) => vec![object_record(json)],
            serde_json::Value::Array(items) => {
                let mut records = Vec::with_capacity(items.len());
                for (record_idx, item) in items.iter().enumerate() {
                    if !item.is_object() {
                        errors.push(ValidationError {
                            code: "VN17",
                            message: format!(
                                "injection case '{}' 第 {} 个事件组 use({{...}}) 的记录数组第 {} 个元素不是 JSON object",
                                stream,
                                idx + 1,
                                record_idx + 1
                            ),
                        });
                        return Vec::new();
                    }
                    records.push(object_record(item));
                }
                if records.is_empty() {
                    errors.push(ValidationError {
                        code: "VN17",
                        message: format!(
                            "injection case '{}' 第 {} 个事件组 use({{...}}) 的记录数组为空",
                            stream,
                            idx + 1
                        ),
                    });
                }
                records
            }
            _ => {
                errors.push(ValidationError {
                    code: "VN17",
                    message: format!(
                        "injection case '{}' 第 {} 个事件组 use({{...}}) 的顶层必须是 JSON object 或 object 数组",
                        stream,
                        idx + 1
                    ),
                });
                Vec::new()
            }
        },
        // loader 应已把 `File` 解析成 `Json`（见 loader::resolve_inject_files）；
        // 未解析时 datagen 会明确报错，这里不重复报。
        ValueSource::File(_) => Vec::new(),
    }
}

/// 一条记录：顶层键展开为字段。
fn object_record(json: &serde_json::Value) -> Vec<FieldPredicate> {
    json_top_level_entries(json)
        .unwrap_or_default()
        .into_iter()
        .map(|(field, value)| FieldPredicate {
            field,
            value: AttrValue::Json(value),
        })
        .collect()
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
