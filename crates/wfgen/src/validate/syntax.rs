use std::collections::{HashMap, HashSet};

use wf_lang::WindowSchema;
use wf_lang::ast::{Expr, FieldRef, RuleDecl};

use super::ValidationError;
use crate::datagen::inject_gen::ENTITY_ID_SPACE;
use crate::datagen::inject_gen::field_ref_field_name;
use crate::wfg_ast::{
    AttrValue, FieldPredicate, RateExpr, SyntaxScenario, ValueSource, WfgFile,
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
    let duration = wfg.scenario.time_clause.duration;

    validate_scenario_annos(syntax, &mut errors);

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
        // VN28：`wave` / `burst` / `timeline` 三个速率形态**语法已定、语义未实现**——降级时
        // 只取 `base=`（`timeline` 取第一段）当常量速率，生成结果与写法不符且不报错
        // （`burst(peak=300/s, …)` 拿到的是 `base` 的平坦流量，压测强度静默失效）。
        // 在语义落地（设计 §7.3 P2）之前先明确拦下，给出改写方向。
        if let Some(kind) = unimplemented_rate_kind(&s.rate) {
            errors.push(ValidationError {
                code: "VN28",
                message: format!(
                    "stream '{}': `gen {kind}` 的随时间变化尚未实现（当前会按 `base=` 的常量速率生成，与写法不符）；请先改用常量速率 `gen 100/s`",
                    s.stream
                ),
            });
        }
    }

    // `replay`：与 WFL 无关（`--no-wfl` 也要发数据），因此不受 `skip_wfl` 门控（设计 §8）。
    validate_replays(syntax, schemas, duration, &mut errors);

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
        let mut total_entity_ids: u128 = 0;

        for case in &inj.cases {
            total_entity_ids += entity_ids_consumed(case);
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

        // VN27：实体 id 总数不得超过 24 位地址空间（设计 §7.2）。用例之间靠**分段**保证
        // 实体值不重叠，而实体值按 24 位地址映射（Ip 写 `10.a.b.c`）；超出后不同用例会
        // 拿到同一个值，`hit` 与 `near_miss` 指向同一实体、两个口径互相污染，且无任何报错。
        // 这里用 u128 累加，避免大数字先把 u64 溢出成小数字而绕过检查。
        if total_entity_ids >= u128::from(ENTITY_ID_SPACE) {
            errors.push(ValidationError {
                code: "VN27",
                message: format!(
                    "injection 实体 id 总数 {} 达到上限 {}（实体值按 24 位地址映射，超出后用例之间的实体会重叠：hit 与 near_miss 会指向同一实体）；请减少各用例的实体个数或 `x N`",
                    total_entity_ids, ENTITY_ID_SPACE
                ),
            });
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

/// 注解值的人话类型名（VN29 消息用）。
fn attr_value_kind(value: &AttrValue) -> &'static str {
    match value {
        AttrValue::Number(_) => "数字",
        AttrValue::Duration(_) => "时长",
        AttrValue::String(_) => "字符串",
        AttrValue::Bool(_) => "布尔",
        AttrValue::Json(_) => "JSON 值",
    }
}

/// VN29：场景注解的**键/值白名单**。
///
/// `#[...]` 只认 `duration`（时长字面量），`<...>` 只认 `seed`（非负数字）。注解列表本身是
/// 泛化解析的，于是其余键此前被**静默忽略**——文档里声明「未实现」的 `tick` / `rows` / `emit`
/// 就是这一类；写错键名（`#[duratoin=10m]`）或写错值类型（`#[duration=10]`）同样会悄悄退回
/// 默认值（60s / seed 0），语义与写法不符且无任何提示。
fn validate_scenario_annos(syntax: &SyntaxScenario, errors: &mut Vec<ValidationError>) {
    for attr in &syntax.attrs {
        if attr.key != "duration" {
            errors.push(ValidationError {
                code: "VN29",
                message: format!(
                    "注解键 '{}' 不支持：`#[...]` 只认 'duration'（tick / rows / emit 从未实现）",
                    attr.key
                ),
            });
        } else if !matches!(attr.value, AttrValue::Duration(_)) {
            errors.push(ValidationError {
                code: "VN29",
                message: format!(
                    "注解 'duration' 的值必须是时长字面量（如 `10m`），实际是{}",
                    attr_value_kind(&attr.value)
                ),
            });
        }
    }
    for attr in &syntax.inline_annos {
        if attr.key != "seed" {
            errors.push(ValidationError {
                code: "VN29",
                message: format!("注解键 '{}' 不支持：`<...>` 只认 'seed'", attr.key),
            });
        } else if !matches!(attr.value, AttrValue::Number(n) if n >= 0.0) {
            errors.push(ValidationError {
                code: "VN29",
                message: format!(
                    "注解 'seed' 的值必须是非负数字，实际是{}",
                    attr_value_kind(&attr.value)
                ),
            });
        }
    }
}

/// 尚未实现的速率形态名（VN28 的点名用）；`Constant` 返回 `None`。
fn unimplemented_rate_kind(rate: &RateExpr) -> Option<&'static str> {
    match rate {
        RateExpr::Constant(_) => None,
        RateExpr::Wave { .. } => Some("wave(...)"),
        RateExpr::Burst { .. } => Some("burst(...)"),
        RateExpr::Timeline(_) => Some("timeline { ... }"),
    }
}

/// 一个注入用例消耗的实体 id 数（设计 §7.2 的 VN27 口径）。
///
/// 与生成侧逐实体循环同口径：
///
/// - `hit` / `near_miss`：每个实体一个 id → `entity_count`；
/// - `miss`：**每个事件一个独立键**（不然就成簇报警了），一轮用例内每个事件各占一个 id
///   → `entity_count × Σ(N)`。
///
/// 用 `u128` 累加：这两个数都来自用户书写的 `u64`，先做 u64 乘法会把大数字翻成小数字、
/// 反而绕过了上限检查。
fn entity_ids_consumed(case: &crate::wfg_ast::InjectCase) -> u128 {
    let count = u128::from(case.entity_count);
    match case.mode {
        crate::wfg_ast::InjectCaseMode::Miss => {
            let per_entity: u128 = case
                .groups
                .iter()
                .map(|group| u128::from(group.count))
                .sum();
            count * per_entity
        }
        _ => count,
    }
}

/// `replay` 语句的校验（设计 §8.3）。
///
/// - **VN3**：目标 stream 不在已加载的 schema 里（与 `background` 的 stream 同一类错，
///   但 `replay` **不要求**写在 `background` 里——回放流可以完全不要背景噪声）；
/// - **VN26**：文件记录为空、或记录里的时间字段口径不齐（部分记录有、部分没有）——
///   后者会静默把一部分数据摆在错误的时间上，所以在校验期就拦；
/// - **VN25**：文件的时间跨度超过 `#[duration]`（平移后必然溢出，不截断）。
fn validate_replays(
    syntax: &crate::wfg_ast::SyntaxScenario,
    schemas: &[WindowSchema],
    duration: std::time::Duration,
    errors: &mut Vec<ValidationError>,
) {
    let schemas_by_name: HashMap<&str, &WindowSchema> = schemas
        .iter()
        .map(|schema| (schema.name.as_str(), schema))
        .collect();
    let duration_nanos = duration.as_nanos().min(i64::MAX as u128) as i64;

    for stmt in &syntax.replays {
        let schema = schemas_by_name.get(stmt.window.as_str()).copied();
        if schema.is_none() {
            errors.push(ValidationError {
                code: "VN3",
                message: format!(
                    "replay 目标 stream '{}' not found in loaded schemas (.wfs windows)",
                    stmt.window
                ),
            });
        }

        if stmt.records.is_none() {
            errors.push(ValidationError {
                code: "VN26",
                message: format!(
                    "replay 文件 `{}` 尚未解析（未经 loader）；请通过 CLI（wfgen lint / gen）加载场景",
                    stmt.file
                ),
            });
            continue;
        }

        let records = match super::super::datagen::replay_gen::replay_records(stmt) {
            Ok(records) => records,
            Err(err) => {
                errors.push(ValidationError {
                    code: "VN26",
                    message: err
                        .detail()
                        .clone()
                        .unwrap_or_else(|| format!("replay 文件 `{}` 的记录形态非法", stmt.file)),
                });
                continue;
            }
        };
        if records.is_empty() {
            errors.push(ValidationError {
                code: "VN26",
                message: format!("replay 文件 `{}` 为空", stmt.file),
            });
            continue;
        }

        match super::super::datagen::replay_gen::plan_replay_timeline(&records, schema, duration) {
            Ok(timeline) => {
                let span = timeline.offsets_nanos.iter().max().copied().unwrap_or(0);
                if span > duration_nanos {
                    errors.push(ValidationError {
                        code: "VN25",
                        message: format!(
                            "replay 文件 `{}` 的时间跨度 {} 超过场景 duration {:?}（平移后必然溢出）",
                            stmt.file,
                            std::time::Duration::from_nanos(span as u64).as_secs_f64(),
                            duration
                        ),
                    });
                }
            }
            Err(err) => errors.push(ValidationError {
                code: "VN26",
                message: err
                    .detail()
                    .clone()
                    .unwrap_or_else(|| format!("replay 文件 `{}` 的时间字段口径非法", stmt.file)),
            }),
        }
    }
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
