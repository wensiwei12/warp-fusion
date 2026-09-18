use std::collections::HashMap;

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use rand::rngs::StdRng;
use wf_lang::{BaseType, FieldType, WindowSchema};

use crate::datagen::field_gen::generate_field_value;
use crate::datagen::inject_gen::extract::source_to_records;
use crate::datagen::inject_gen::structures::{InjectUseStepOverrides, RuleJoinInfo, StepInfo};
use crate::datagen::stream_gen::GenEvent;
use crate::error::{self, WfgenReason, WfgenResult};
use crate::wfg_ast::JoinStmt;

use super::plan::*;

/// 实体 id 空间：实体值按 **24 位**地址映射（Ip 字段写 `10.a.b.c`，`a`/`b`/`c` 各 8 位），
/// 因此一个场景的实体 id 总数必须 < 该值（校验期 VN27 拦下，见 `validate/syntax.rs`）；
/// 超出后映射会回绕，不同用例的实体会拿到同一个值——`hit` 与 `near_miss` 指向同一实体，
/// 两个口径互相污染。
pub(crate) const ENTITY_ID_SPACE: u64 = 1 << 24;

/// Generate cluster events across all steps.
#[allow(clippy::too_many_arguments)]
pub(crate) fn generate_cluster_events(
    steps: &[StepInfo],
    step_event_counts: &[u64],
    key_overrides: &HashMap<String, serde_json::Value>,
    use_step_overrides: &[InjectUseStepOverrides],
    joins: &[JoinStmt],
    rule_joins: &[RuleJoinInfo],
    cluster_start_secs: f64,
    window_secs: f64,
    schemas: &[WindowSchema],
    start: &DateTime<Utc>,
    rng: &mut StdRng,
    out: &mut Vec<GenEvent>,
) -> WfgenResult<()> {
    generate_cluster_events_with_filter_validation(
        steps,
        step_event_counts,
        key_overrides,
        use_step_overrides,
        joins,
        rule_joins,
        cluster_start_secs,
        window_secs,
        schemas,
        start,
        rng,
        out,
        true,
    )
}

#[allow(clippy::too_many_arguments)]
fn generate_cluster_events_with_filter_validation(
    steps: &[StepInfo],
    step_event_counts: &[u64],
    key_overrides: &HashMap<String, serde_json::Value>,
    use_step_overrides: &[InjectUseStepOverrides],
    joins: &[JoinStmt],
    rule_joins: &[RuleJoinInfo],
    cluster_start_secs: f64,
    window_secs: f64,
    schemas: &[WindowSchema],
    start: &DateTime<Utc>,
    rng: &mut StdRng,
    out: &mut Vec<GenEvent>,
    validate_filter_conflicts: bool,
) -> WfgenResult<()> {
    let step_predicate_overrides = map_use_predicates_to_rule_steps(
        steps,
        use_step_overrides,
        step_event_counts,
        validate_filter_conflicts,
    )?;

    // Track cumulative time offset across steps for multi-step ordering
    let mut cumulative_offset = 0.0;
    let per_step_window = if steps.len() > 1 {
        window_secs / steps.len() as f64
    } else {
        window_secs
    };

    for (step_idx, step) in steps.iter().enumerate() {
        let event_count = step_event_counts.get(step_idx).copied().unwrap_or(0);
        if event_count == 0 {
            continue;
        }

        let schema = schemas
            .iter()
            .find(|s| s.name == step.window_name)
            .ok_or_else(|| {
                error::error(
                    WfgenReason::Validation,
                    format!("schema not found for '{}'", step.window_name),
                )
            })?;

        let empty_predicates: HashMap<String, serde_json::Value> = HashMap::new();
        let step_event_predicates = step_predicate_overrides.get(step_idx);

        for i in 0..event_count {
            let event_offset_secs = cluster_start_secs
                + cumulative_offset
                + (per_step_window * i as f64 / event_count.max(1) as f64);
            let ts = *start + ChronoDuration::nanoseconds((event_offset_secs * 1e9) as i64);
            let per_event_predicates = step_event_predicates
                .and_then(|v| v.get(i as usize))
                .unwrap_or(&empty_predicates);

            let fields = build_event_fields_with_predicates(
                schema,
                key_overrides,
                &step.filter_overrides,
                per_event_predicates,
                &ts,
                rng,
            );

            // Use the actual stream name from schema (e.g., "syslog")
            let stream_name = schema
                .streams
                .first()
                .cloned()
                .unwrap_or_else(|| schema.name.clone());

            out.push(GenEvent {
                stream_name,
                window_name: step.window_name.clone(),
                timestamp: ts,
                fields,
            });
            // 设计 §9：为一个左事件补发 `join` 块声明的右事件。
            push_join_events(joins, rule_joins, key_overrides, &ts, schemas, rng, out)?;
        }

        cumulative_offset += per_step_window;
    }

    Ok(())
}

/// 为一个左事件补发 `join` 块声明的右事件（设计 §9 跨流注入）。
///
/// 右行的**连接键** = 左实体键值、**时间** = 该左事件时间（校验期 VN30 已保证规则的
/// `within` 区间含左事件时间）；其余字段按目标窗 schema 随机生成、再由 `use(...)` 的谓词
/// 覆盖——复用 `build_event_fields_with_predicates`，时间字段口径因此与左事件天然一致。
pub(crate) fn push_join_events(
    joins: &[JoinStmt],
    rule_joins: &[RuleJoinInfo],
    key_overrides: &HashMap<String, serde_json::Value>,
    left_ts: &DateTime<Utc>,
    schemas: &[WindowSchema],
    rng: &mut StdRng,
    out: &mut Vec<GenEvent>,
) -> WfgenResult<()> {
    if joins.is_empty() {
        return Ok(());
    }

    let no_filters: HashMap<String, serde_json::Value> = HashMap::new();
    for join in joins {
        // 规则侧口径：决定右事件相对左事件的偏移（deferred 同刻 / snapshot 前挪 1ns）。
        let info = rule_joins
            .iter()
            .find(|info| info.window == join.window && info.right_field == join.key_field)
            .ok_or_else(|| {
                error::error(
                    WfgenReason::Validation,
                    format!(
                        "join 块 `join {} as {}` 在规则里找不到匹配的 join 子句（校验期应已由 VN30 拦下）",
                        join.window, join.key_field
                    ),
                )
            })?;
        let right_ts = *left_ts + ChronoDuration::nanoseconds(info.offset_nanos);

        // 连接键：取规则 `on <left> == <right>` 的 **left（驱动侧）字段**值——两侧因此指向
        // 同一个实体（q6 的 `b.auction` ↔ `auction_events.id`）。
        let connective = key_overrides
            .get(&info.left_field)
            .or_else(|| {
                if key_overrides.len() == 1 {
                    key_overrides.values().next()
                } else {
                    None
                }
            })
            .ok_or_else(|| {
                error::error(
                    WfgenReason::Validation,
                    format!(
                        "join 块需要规则 join 驱动侧键 `{}` 的取值（`on {} == {}.{}`），但本用例没有它",
                        info.left_field, info.left_field, join.window, join.key_field
                    ),
                )
            })?;

        let schema = schemas
            .iter()
            .find(|s| s.name == join.window)
            .ok_or_else(|| {
                error::error(
                    WfgenReason::Validation,
                    format!("schema not found for join window '{}'", join.window),
                )
            })?;
        let stream_name = schema
            .streams
            .first()
            .cloned()
            .unwrap_or_else(|| schema.name.clone());

        // 右行带上本案的全部键值（右窗 schema 里有的字段才会被写进去）：驱动键 + join 侧键
        // （join-then-key 的 `seller` 就是这样落到右行上的），再把连接键覆盖成上面那个值。
        let mut right_keys = key_overrides.clone();
        right_keys.insert(join.key_field.clone(), connective.clone());

        for group in &join.groups {
            let records = source_to_records(&group.source)?;
            for i in 0..group.count {
                let predicates = &records[i as usize % records.len()];
                let fields = build_event_fields_with_predicates(
                    schema,
                    &right_keys,
                    &no_filters,
                    predicates,
                    &right_ts,
                    rng,
                );
                out.push(GenEvent {
                    stream_name: stream_name.clone(),
                    window_name: join.window.clone(),
                    timestamp: right_ts,
                    fields,
                });
            }
        }
    }
    Ok(())
}

pub(crate) fn map_use_predicates_to_rule_steps(
    steps: &[StepInfo],
    use_steps: &[InjectUseStepOverrides],
    step_event_counts: &[u64],
    validate_filter_conflicts: bool,
) -> WfgenResult<Vec<Vec<HashMap<String, serde_json::Value>>>> {
    let mut per_rule_step = vec![Vec::new(); step_event_counts.len()];
    if use_steps.is_empty() || step_event_counts.is_empty() {
        return Ok(per_rule_step);
    }

    for planned in plan_use_steps(steps, use_steps, validate_filter_conflicts)? {
        if let Some(step_predicates) = per_rule_step.get_mut(planned.rule_step_idx) {
            let expected = step_event_counts
                .get(planned.rule_step_idx)
                .copied()
                .unwrap_or(0) as usize;
            // 事件序号 `step_predicates.len()` 就是循环取用的下标：单记录形态
            // （长度 1）恒取第 0 条，`use from` 的数组形态按记录顺序轮转。
            let records = &planned.records;
            let slots = expected
                .saturating_sub(step_predicates.len())
                .min(planned.count as usize);
            for _ in 0..slots {
                let record_idx = step_predicates.len() % records.len();
                step_predicates.push(records[record_idx].clone());
            }
        }
    }

    // Fill missing event slots with empty predicates.
    for (idx, expected) in step_event_counts.iter().copied().enumerate() {
        let step_predicates = &mut per_rule_step[idx];
        while step_predicates.len() < expected as usize {
            step_predicates.push(HashMap::new());
        }
    }

    Ok(per_rule_step)
}

/// Build event fields with key, filter, and predicate overrides applied.
pub(crate) fn build_event_fields_with_predicates(
    schema: &WindowSchema,
    key_overrides: &HashMap<String, serde_json::Value>,
    filter_overrides: &HashMap<String, serde_json::Value>,
    predicate_overrides: &HashMap<String, serde_json::Value>,
    ts: &DateTime<Utc>,
    rng: &mut StdRng,
) -> serde_json::Map<String, serde_json::Value> {
    let mut fields = serde_json::Map::new();

    for field_def in &schema.fields {
        // 1. Key field override (highest priority)
        if let Some(value) = key_overrides.get(&field_def.name) {
            fields.insert(field_def.name.clone(), value.clone());
            continue;
        }

        // 2. Predicate override from use(...) (second priority)
        if let Some(value) = predicate_overrides.get(&field_def.name) {
            fields.insert(field_def.name.clone(), value.clone());
            continue;
        }

        // 3. Filter override (bind filter constraints)
        if let Some(value) = filter_overrides.get(&field_def.name) {
            fields.insert(field_def.name.clone(), value.clone());
            continue;
        }

        // 4. Time field
        if matches!(&field_def.field_type, FieldType::Base(BaseType::Time)) {
            fields.insert(
                field_def.name.clone(),
                serde_json::json!(ts.timestamp_nanos_opt().unwrap_or(0)),
            );
            continue;
        }

        // 5. Normal field：按类型生成随机值
        let value = generate_field_value(&field_def.field_type, rng);
        fields.insert(field_def.name.clone(), value);
    }

    fields
}

/// schema 里某个字段的类型（不存在则 `None`）。
fn field_type_of<'a>(schema: &'a WindowSchema, field: &str) -> Option<&'a FieldType> {
    schema
        .fields
        .iter()
        .find(|f| f.name == field)
        .map(|f| &f.field_type)
}

/// join 侧键（join-then-key：`match<seller:…>` 而 `seller` 在 join 目标窗上）的**值带起点**。
///
/// 必须与背景噪声分开——背景 digit 落在 `0..100_000`、ip 是随机 24 位；否则注入实例会和
/// 背景事件并到同一条窗口实例上，阈值（`avg >= 200` 之类）被背景稀释，断言随背景波动。
/// 已知边界：驱动实体值域超过 `1<<22`（> 420 万实体）时可能与实体段重叠（设计 §9.5）。
const JOIN_SIDE_KEY_BASE: u64 = 1 << 22;

/// join 侧键的值：按目标窗字段类型生成，落在与背景噪声分开的带里。
fn join_side_key_value(
    field_type: Option<&FieldType>,
    index: u64,
    key_name: &str,
) -> serde_json::Value {
    let v = JOIN_SIDE_KEY_BASE + index;
    match field_type {
        Some(FieldType::Base(BaseType::Digit)) => serde_json::json!(v as i64),
        Some(FieldType::Base(BaseType::Float)) => serde_json::json!(v as f64),
        Some(FieldType::Base(BaseType::Ip)) => entity_value_for_index(field_type, v, "j", key_name),
        Some(FieldType::Base(BaseType::Hex)) => serde_json::Value::String(format!("{v:032x}")),
        _ => serde_json::Value::String(format!("join_{key_name}_{index:06}")),
    }
}

/// 24 位实体索引 → 字段值。**注入与背景实体池共用**这套映射，值域才能分区
/// （注入占底部 `[0, total_entity_ids)`、背景池占顶部两段，设计 §10）。
pub(crate) fn entity_value_for_index(
    field_type: Option<&FieldType>,
    index: u64,
    prefix: &str,
    key_name: &str,
) -> serde_json::Value {
    match field_type {
        Some(FieldType::Base(BaseType::Ip)) => {
            let a = ((index >> 16) & 0xFF) as u8;
            let b = ((index >> 8) & 0xFF) as u8;
            let c = (index & 0xFF) as u8;
            serde_json::Value::String(format!("10.{a}.{b}.{c}"))
        }
        Some(FieldType::Base(BaseType::Digit)) => serde_json::json!(index as i64),
        Some(FieldType::Base(BaseType::Float)) => serde_json::json!(index as f64),
        Some(FieldType::Base(BaseType::Chars)) => {
            serde_json::Value::String(format!("{prefix}_{key_name}_{index:06}"))
        }
        Some(FieldType::Base(BaseType::Hex)) => serde_json::Value::String(format!("{index:032x}")),
        _ => serde_json::Value::String(format!("{prefix}_{key_name}_{index:06}")),
    }
}

/// Generate unique key values for a cluster entity.
///
/// Uses the entity counter and a prefix to produce deterministic unique values
/// based on the field type from the schema.
pub(crate) fn generate_key_values(
    key_names: &[String],
    entity_counter: u64,
    prefix: &str,
    schemas: &[WindowSchema],
    steps: &[StepInfo],
    entity_field: Option<&str>,
) -> HashMap<String, serde_json::Value> {
    let mut overrides = HashMap::new();

    // Find field types from the first step's schema
    let first_schema = steps
        .first()
        .and_then(|s| schemas.iter().find(|sch| sch.name == s.window_name));

    let mut names = key_names.to_vec();
    if let Some(field) = entity_field
        && !names.iter().any(|name| name == field)
    {
        names.push(field.to_string());
    }

    for (i, key_name) in names.iter().enumerate() {
        let id = entity_counter + i as u64;
        let value = match first_schema.and_then(|sch| field_type_of(sch, key_name)) {
            Some(field_type) => {
                debug_assert!(
                    id < ENTITY_ID_SPACE,
                    "实体 id {id} 超出 24 位地址空间（{ENTITY_ID_SPACE}），Ip 映射会回绕、不同实体会拿到同一个值"
                );
                entity_value_for_index(Some(field_type), id, prefix, key_name)
            }
            // 不在驱动事件上 → **join 侧键**（join-then-key，设计 §9）：它属于某个 join 目标窗，
            // 值取自与背景噪声分开的值带。
            None => {
                let field_type = schemas.iter().find_map(|sch| field_type_of(sch, key_name));
                join_side_key_value(field_type, id, key_name)
            }
        };
        overrides.insert(key_name.clone(), value);
    }

    overrides
}
