//! `replay` 语句的生成（设计 §8）：把文件里的记录**照单发货**，并把它对齐到场景时间轴。
//!
//! 与 `inject` 不同，这里没有实体数学、没有条数、也不参与实体断言；它只做三件事：
//!
//! 1. 记录 → 事件（记录自身的字段照抄，`_` 前缀的内部键忽略）；
//! 2. 时间对齐：文件里的时间戳以**最早一条为锚平移到场景起点**，使 `#[duration]` 成为
//!    background / inject / replay 三类事件共同的时间窗（§8.3 第 1 条）；文件没有时间
//!    字段时按序号在 `duration` 内均匀落下（与 `miss` 同策略）；有则必须**单调不减**
//!    （见 `ensure_time_ordered`：下游全部按时间有序假定处理）；
//! 3. 时间字段写回（schema 的 `time_field`），与 background / inject 的输出形态一致。

use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde_json::Value;
use wf_lang::WindowSchema;

use crate::error::{self, WfgenReason, WfgenResult};
use crate::wfg_ast::{ReplayStmt, WfgFile, json_top_level_entries};

use super::stream_gen::GenEvent;

/// 记录里的时间字段名（内部约定）。优先级高于 schema 的 `time_field`。
const INTERNAL_TIME_FIELD: &str = "_timestamp";

/// 一条 `replay` 的时间安排。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReplayTimeline {
    /// 识别到的时间字段（`_timestamp` 或 schema 的 `time_field`）；`None` = 按序号均匀落下。
    pub time_field: Option<String>,
    /// 每条记录相对**场景起点**的偏移（纳秒，非负且单调不减）。
    pub offsets_nanos: Vec<i64>,
}

/// 记录里的时间字段识别 + 偏移计算（校验期与生成期共用，避免两处口径漂移）。
///
/// 口径（设计 §8.3 第 2 条）：
///
/// - `_timestamp` 优先，其次 schema 的 `time_field`；
/// - 字段在**部分**记录里出现 ⇒ 报错（同一文件的落时间口径必须唯一）；
/// - 全都没有 ⇒ 按序号在 `duration` 内均匀落下，`offset_i = duration × i / N`（与 `miss` 同策略）。
///
/// 有空位的记录（时间字段不是数字 / 越界）同样按"缺字段"处理：整份都不合法时报错，
/// 部分不合法时落进"部分出现"的分支报错——不静默丢掉某几条记录的时间。
pub(crate) fn plan_replay_timeline(
    records: &[ReplayRecord],
    schema: Option<&WindowSchema>,
    duration: Duration,
) -> WfgenResult<ReplayTimeline> {
    let mut candidates: Vec<&str> = vec![INTERNAL_TIME_FIELD];
    if let Some(field) = schema.and_then(|schema| schema.time_field.as_deref())
        && field != INTERNAL_TIME_FIELD
    {
        candidates.push(field);
    }

    for candidate in candidates {
        let parsed: Vec<Option<i64>> = records
            .iter()
            .map(|record| time_value(record, candidate).and_then(record_time_nanos))
            .collect();
        let present = parsed.iter().filter(|value| value.is_some()).count();
        if present == 0 {
            continue;
        }
        if present != records.len() {
            return error::fail(
                WfgenReason::Validation,
                format!(
                    "replay 记录里的时间字段 '{candidate}' 只在 {} / {} 条记录上出现（或不是合法时间戳）；\
                     同一份文件的落时间口径必须唯一：要么每条都有，要么都没有",
                    present,
                    records.len()
                ),
            );
        }
        let raw: Vec<i64> = parsed.into_iter().flatten().collect();
        ensure_time_ordered(&raw, candidate)?;
        return Ok(ReplayTimeline {
            time_field: Some(candidate.to_string()),
            offsets_nanos: rebase_offsets(&raw),
        });
    }

    let total = duration.as_nanos().min(i64::MAX as u128);
    let count = records.len();
    // 用 `u128` 算 `total × index`：`total` 已钳到 `i64::MAX`，乘上“记录数”在 `i64` 里会溢出
    // （实测 `#[duration=2d]` + 10 万条无时间字段的记录 → `lint` panic；release 下静默回绕成
    // 负偏移，事件会落到场景起点之前）。
    let offsets_nanos = (0..count)
        .map(|index| {
            if count > 0 {
                (total * index as u128 / count as u128) as i64
            } else {
                0
            }
        })
        .collect();
    Ok(ReplayTimeline {
        time_field: None,
        offsets_nanos,
    })
}

/// 以最早一条为锚平移到 0：同文件内的相对间隔保持。
fn rebase_offsets(raw: &[i64]) -> Vec<i64> {
    let anchor = raw.iter().copied().min().unwrap_or(0);
    raw.iter()
        .map(|value| value.saturating_sub(anchor))
        .collect()
}

/// 记录的时间必须**单调不减**（同刻允许）。
///
/// `replay` 按文件顺序发货，而下游全是“按时间有序”的假定：`merge_sorted_chunks` 的
/// k 路归并要求每块有序、oracle 的窗口推进/水位、引擎的收口语义都建在这上面。
/// 文件里一旦出现回退，“后到的更早事件”会被当成未来事件：oracle 与引擎的窗口收口
/// 就此分叉——两侧都“有数据”，只是一个对不上另一个，而且找不到原因。
///
/// 不在这里重排：`replay` 的语义是照单发货，静默改顺序等于改用户的数据。
/// 报错让用户自己选（先把文件按时间排好，或去掉时间字段走“按序号均匀落下”）。
fn ensure_time_ordered(raw: &[i64], field: &str) -> WfgenResult<()> {
    for (idx, pair) in raw.windows(2).enumerate() {
        if pair[1] < pair[0] {
            return error::fail(
                WfgenReason::Validation,
                format!(
                    "replay 记录的时间字段 '{field}' 不是单调不减：第 {} 条（{}）早于第 {} 条（{}）。\
                     记录按文件顺序发货，下游按时间有序假定处理——请先把文件按时间排序，\
                     或去掉时间字段（改为按序号在场景时长内均匀落下）",
                    idx + 2,
                    pair[1],
                    idx + 1,
                    pair[0]
                ),
            );
        }
    }
    Ok(())
}

/// 取候选时间字段的值：`_timestamp` 看记录里单独拎出来的那份，其余看事件字段。
fn time_value<'a>(record: &'a ReplayRecord, field: &str) -> Option<&'a Value> {
    if field == INTERNAL_TIME_FIELD {
        record.internal_timestamp.as_ref()
    } else {
        record.fields.get(field)
    }
}

/// 记录里的时间值 → 纳秒（按位宽归一化，与 NDJSON 输入一致：秒 / 毫秒 / 微秒 / 纳秒）。
fn record_time_nanos(value: &Value) -> Option<i64> {
    let raw = match value {
        Value::Number(number) => number.as_f64()?,
        // 数字字符串同样接受（`"1712345678"`）。
        Value::String(text) => text.trim().parse::<f64>().ok()?,
        _ => return None,
    };
    wf_engine::normalize_epoch_timestamp_float_nanos(raw)
}

/// 一条 `replay` 记录。
///
/// `_timestamp` 单独拎出来：它是**时间来源**，不是事件字段（`json_top_level_entries`
/// 会把其余 `_` 前缀的内部键剔除，时间这份得自己取回）。
#[derive(Debug, Clone)]
pub(crate) struct ReplayRecord {
    /// 事件字段（记录顶层键，`_` 前缀内部键已剔除）。
    pub fields: HashMap<String, Value>,
    /// 记录里的 `_timestamp`（若存在）；不写进事件字段。
    pub internal_timestamp: Option<Value>,
}

/// 一条 `replay` 的记录列表（loader 已把 `use from` 解析进 `stmt.records`）。
pub(crate) fn replay_records(stmt: &ReplayStmt) -> WfgenResult<Vec<ReplayRecord>> {
    let Some(json) = stmt.records.as_ref() else {
        return error::fail(
            WfgenReason::Validation,
            format!(
                "replay `{}` 的值文件尚未解析；请通过 CLI（wfgen gen / lint）加载场景",
                stmt.window
            ),
        );
    };

    let items: Vec<&Value> = match json {
        Value::Object(_) => vec![json],
        Value::Array(items) => items.iter().collect(),
        other => {
            return error::fail(
                WfgenReason::Validation,
                format!(
                    "replay `{}` 的记录必须是 JSON object 或 object 数组，实际有 {}",
                    stmt.window, other
                ),
            );
        }
    };

    Ok(items.into_iter().map(replay_record).collect())
}

fn replay_record(item: &Value) -> ReplayRecord {
    let internal_timestamp = item
        .as_object()
        .and_then(|object| object.get(INTERNAL_TIME_FIELD))
        .cloned();
    ReplayRecord {
        fields: json_top_level_entries(item)
            .unwrap_or_default()
            .into_iter()
            .collect(),
        internal_timestamp,
    }
}

/// 生成全部 `replay` 语句的事件（与 `background` / `inject` 并列的一条来源）。
///
/// 不吃 RNG：replay 的数据全部来自文件，生成它不应扰动既有流的随机序列。
pub(crate) fn generate_replay_events(
    wfg: &WfgFile,
    schemas: &[WindowSchema],
    start: &DateTime<Utc>,
) -> WfgenResult<Vec<GenEvent>> {
    let Some(syntax) = wfg.syntax.as_ref() else {
        return Ok(Vec::new());
    };
    if syntax.replays.is_empty() {
        return Ok(Vec::new());
    }

    let duration = wfg.scenario.time_clause.duration;
    let start_nanos = start.timestamp_nanos_opt().unwrap_or(0);
    let mut events = Vec::new();

    for stmt in &syntax.replays {
        let schema = schemas.iter().find(|schema| schema.name == stmt.window);
        let records = replay_records(stmt)?;
        let timeline = plan_replay_timeline(&records, schema, duration)?;

        for (record, offset) in records.iter().zip(timeline.offsets_nanos.iter()) {
            let ts_nanos = start_nanos.saturating_add(*offset);
            let timestamp = DateTime::from_timestamp_nanos(ts_nanos);

            let mut fields: serde_json::Map<String, Value> = record
                .fields
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            // 时间列与 background / inject 同形态：写 schema 的 `time_field`，单位纳秒。
            if let Some(field) = schema.and_then(|schema| schema.time_field.as_deref()) {
                fields.insert(field.to_string(), Value::from(ts_nanos));
            }

            let stream_name = schema
                .and_then(|schema| schema.streams.first().cloned())
                .unwrap_or_else(|| stmt.window.clone());

            events.push(GenEvent {
                stream_name,
                window_name: stmt.window.clone(),
                timestamp,
                fields,
            });
        }
    }

    Ok(events)
}
