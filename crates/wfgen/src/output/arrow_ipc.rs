use std::collections::HashMap;
use std::fs::File;
use std::path::Path;
use std::sync::Arc;

use arrow::array::{
    ArrayRef, BooleanArray, Float64Array, Int64Array, RecordBatch, StringArray,
    TimestampNanosecondArray,
};
use arrow::datatypes::{DataType, Field, Schema, TimeUnit};
use arrow::ipc::writer::FileWriter;
use chrono::{DateTime, SecondsFormat, Utc};
use orion_error::conversion::SourceRawErr;

use wf_engine::match_engine::{
    WFL_FIELD_TYPE_ARRAY, WFL_FIELD_TYPE_METADATA_KEY, WFL_FIELD_TYPE_OBJECT,
};
use wf_lang::{BaseType, FieldType, WindowSchema};

use crate::datagen::stream_gen::GenEvent;
use crate::error::{self, WfgenReason, WfgenResult};

/// Write events as Arrow IPC file.
///
/// All fields are stored as UTF-8 strings (JSON-encoded for non-string values)
/// with metadata columns `_stream`, `_window`, `_timestamp`.
///
/// 结构化列（object / array）按**列内实际值**打 `wf.wfl.field_type` metadata：
/// 没有 schema 可用，而引擎只认这个 metadata 才会把 JSON 文本解析成
/// `Value::Object` / `Value::Array`（见 [`schema_structured_kind`] 的说明）。
pub fn write_arrow_ipc(events: &[GenEvent], output_path: &Path) -> WfgenResult<()> {
    if events.is_empty() {
        return error::fail(WfgenReason::Generation, "no events to write");
    }

    // Create parent directories if needed
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent).source_raw_err(
            WfgenReason::Io,
            format!("creating output directory: {}", parent.display()),
        )?;
    }

    // Collect all field names from events (preserving order from first event)
    let mut field_names: Vec<String> = Vec::new();
    // Always include metadata columns first
    field_names.push("_stream".to_string());
    field_names.push("_window".to_string());
    field_names.push("_timestamp".to_string());

    // Collect data field names from all events to avoid dropping sparse fields.
    for event in events {
        for key in event.fields.keys() {
            if !field_names.contains(key) {
                field_names.push(key.clone());
            }
        }
    }

    // Build Arrow schema — all fields as Utf8, structured columns tagged by value.
    let arrow_fields: Vec<Field> = field_names
        .iter()
        .map(|name| {
            let field = Field::new(name, DataType::Utf8, true);
            match inferred_structured_kind(events, name) {
                Some(kind) => field.with_metadata(structured_metadata(kind)),
                None => field,
            }
        })
        .collect();
    let schema = Arc::new(Schema::new(arrow_fields));

    // Build columns
    let mut columns: Vec<ArrayRef> = Vec::new();

    for field_name in &field_names {
        let values: Vec<Option<String>> = events
            .iter()
            .map(|event| match field_name.as_str() {
                "_stream" => Some(event.stream_name.clone()),
                "_window" => Some(event.window_name.clone()),
                "_timestamp" => Some(event.timestamp.to_rfc3339_opts(SecondsFormat::Millis, true)),
                name => event.fields.get(name).map(|v| match v {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                }),
            })
            .collect();

        let array = StringArray::from(values);
        columns.push(Arc::new(array) as ArrayRef);
    }

    let batch = RecordBatch::try_new(schema.clone(), columns)
        .source_raw_err(WfgenReason::Serialization, "building Arrow record batch")?;

    let file = File::create(output_path).source_raw_err(
        WfgenReason::Io,
        format!("creating {}", output_path.display()),
    )?;
    let mut writer = FileWriter::try_new(file, &schema)
        .source_raw_err(WfgenReason::Serialization, "creating Arrow IPC writer")?;
    writer
        .write(&batch)
        .source_raw_err(WfgenReason::Serialization, "writing Arrow IPC batch")?;
    writer
        .finish()
        .source_raw_err(WfgenReason::Serialization, "finishing Arrow IPC writer")?;

    Ok(())
}

/// Default upper bound on the encoded size of a single Arrow frame sent to the
/// runtime.
///
/// A frame is appended to a window as *one* batch, and window memory eviction
/// operates on whole batches — a single oversized frame that exceeds the
/// window's `max_window_bytes` is dropped entirely (wp-labs/wp-reactor#18/#20).
/// Keeping frames at a small fraction of the window cap (default 256MB) avoids
/// that, and keeps the ordered commit worker from ever holding one giant
/// RecordBatch. Overcounting the per-event estimate only splits a frame a
/// little earlier — the safe direction.
pub const DEFAULT_MAX_FRAME_BYTES: usize = 8 * 1024 * 1024; // 8 MiB

/// Default secondary per-frame row cap: protects builder memory even when the
/// byte estimate is tiny (e.g. mostly-null or narrow rows).
pub const DEFAULT_MAX_FRAME_ROWS: usize = 100_000;

/// Group GenEvents by window, build typed Arrow RecordBatches keyed by stream
/// name, splitting each window's events into multiple frames once a frame
/// exceeds `max_frame_bytes` or `max_frame_rows`.
///
/// Column types are derived from the [`WindowSchema`] field definitions,
/// matching the runtime's expected schema exactly. Frame splitting preserves
/// event order and never drops events.
pub fn events_to_typed_batches(
    events: &[GenEvent],
    schemas: &[WindowSchema],
    max_frame_bytes: usize,
    max_frame_rows: usize,
) -> WfgenResult<Vec<(String, RecordBatch)>> {
    let schema_by_window: HashMap<&str, &WindowSchema> =
        schemas.iter().map(|s| (s.name.as_str(), s)).collect();
    let mut groups: HashMap<&str, Vec<&GenEvent>> = HashMap::new();
    for event in events {
        groups
            .entry(event.window_name.as_str())
            .or_default()
            .push(event);
    }

    let mut batches = Vec::new();

    for (window_name, group_events) in groups {
        let schema = schema_by_window.get(window_name).copied().ok_or_else(|| {
            error::error(
                WfgenReason::Validation,
                format!("schema not found for window '{window_name}'"),
            )
        })?;
        let stream_name = schema.streams.first().ok_or_else(|| {
            error::error(
                WfgenReason::Validation,
                format!("no stream defined for window '{window_name}'"),
            )
        })?;

        let mut frame: Vec<&GenEvent> = Vec::new();
        let mut frame_bytes = 0usize;
        for event in group_events {
            let est = event_frame_bytes(event, schema);
            if !frame.is_empty()
                && (frame_bytes + est > max_frame_bytes || frame.len() + 1 > max_frame_rows)
            {
                build_frame(&mut batches, stream_name, schema, &frame)?;
                frame.clear();
                frame_bytes = 0;
            }
            frame.push(event);
            frame_bytes += est;
        }
        if !frame.is_empty() {
            build_frame(&mut batches, stream_name, schema, &frame)?;
        }
    }

    // 跨流时间序：帧内各 stream 的 batch 按**最小事件时间**排序写入——
    // HashMap 分组迭代序随机，会导致高流量流（bid 92%）的 batch 随机排后，
    // receiver 按帧序 commit 时 bid 窗口 append 滞后 → 驱动=低流量流
    // （person/auction）的 join 在右窗行到达前评估 → snapshot miss（Q3 差
    // 45%、Q9 差 24% 的跨流顺序根因，2026-08-22 实测）。
    batches.sort_by_key(|(_, batch)| batch_min_ts(batch));

    Ok(batches)
}

/// 批次的最小事件时间（排序键）：取常见时间列（dateTime/event_time/ts）首行。
fn batch_min_ts(batch: &RecordBatch) -> i64 {
    for name in ["dateTime", "event_time", "timestamp", "ts"] {
        if let Ok(idx) = batch.schema().index_of(name)
            && let Some(arr) = batch
                .column(idx)
                .as_any()
                .downcast_ref::<TimestampNanosecondArray>()
            && !arr.is_empty()
        {
            return arr.value(0);
        }
    }
    i64::MAX
}

/// Build one typed Arrow RecordBatch from a frame of events and push it.
fn build_frame(
    batches: &mut Vec<(String, RecordBatch)>,
    stream_name: &str,
    schema: &WindowSchema,
    frame_events: &[&GenEvent],
) -> WfgenResult<()> {
    let arrow_fields: Vec<Field> = schema
        .fields
        .iter()
        .map(|f| field_with_schema_metadata(&f.name, &f.field_type))
        .collect();
    let arrow_schema = Arc::new(Schema::new(arrow_fields));

    let mut builders: Vec<ColumnBuilder> = schema
        .fields
        .iter()
        .map(|f| ColumnBuilder::new(&f.field_type, frame_events.len()))
        .collect();
    for event in frame_events {
        let fallback_ts = event.timestamp.timestamp_nanos_opt();
        for (field_def, builder) in schema.fields.iter().zip(builders.iter_mut()) {
            builder.push(event.fields.get(field_def.name.as_str()), fallback_ts);
        }
    }
    let columns: Vec<ArrayRef> = builders.into_iter().map(ColumnBuilder::finish).collect();

    let batch = RecordBatch::try_new(arrow_schema, columns).source_raw_err(
        WfgenReason::Serialization,
        "building typed Arrow record batch",
    )?;
    batches.push((stream_name.to_string(), batch));
    Ok(())
}

/// Conservative byte estimate of one event within a frame.
///
/// The runtime's window accounting charges *both* the Arrow content and the
/// parsed-event `HashMap` footprint (`content_bytes + events_bytes`). Object and
/// array fields decode into nested maps/vecs ~2-4× the JSON string, so they are
/// weighted accordingly; every other field is `4B offset + payload`, which
/// overcounts fixed-width primitives. Overestimating only splits frames a little
/// earlier — the safe direction.
fn event_frame_bytes(event: &GenEvent, schema: &WindowSchema) -> usize {
    let mut bytes = 16usize; // per-event row overhead
    for field in &schema.fields {
        let Some(value) = event.fields.get(field.name.as_str()) else {
            continue;
        };
        let blowup = match field.field_type {
            FieldType::Object | FieldType::ArrayAny | FieldType::Array(_) => 3,
            _ => 1,
        };
        bytes += 4 + json_value_len(value) * blowup;
    }
    bytes
}

/// Approximate serialized length of a JSON value (strings as-is, everything
/// else JSON-encoded — matching what the UTF-8 columns actually store).
fn json_value_len(v: &serde_json::Value) -> usize {
    match v {
        serde_json::Value::String(s) => s.len(),
        other => other.to_string().len(),
    }
}

/// Convert a wf-lang [`FieldType`] to the corresponding Arrow [`DataType`].
///
/// 结构化值（`object` / `array` / `array/<base>`）一律写成 **JSON 文本的 Utf8 列**，
/// 并附 `wf.wfl.field_type` metadata（见 [`field_with_schema_metadata`]）。定类型
/// 数组不能退化成元素标量列——那样数组结构直接丢失、值全变 null（`array/digit`
/// 曾经写成 Int64）。
fn field_type_to_arrow(ft: &FieldType) -> DataType {
    match ft {
        FieldType::Object | FieldType::ArrayAny | FieldType::Array(_) => DataType::Utf8,
        FieldType::Base(base) => base_type_to_arrow(base),
    }
}

fn base_type_to_arrow(base: &BaseType) -> DataType {
    match base {
        BaseType::Chars | BaseType::Ip | BaseType::Hex => DataType::Utf8,
        BaseType::Digit => DataType::Int64,
        BaseType::Float => DataType::Float64,
        BaseType::Bool => DataType::Boolean,
        BaseType::Time => DataType::Timestamp(TimeUnit::Nanosecond, None),
    }
}

/// 结构化列的 metadata：`wf.wfl.field_type` = `object` / `array`。
///
/// 常量取自 `wf-engine`（单一事实源）：引擎只在 Utf8 列带该 metadata 时把列值当
/// JSON 解析成 `Value::Object` / `Value::Array`，否则一律 `Value::Str`——规则读
/// 嵌套字段/列表就会静默不产出，且与 oracle（直读结构值）不一致。
fn structured_metadata(kind: &'static str) -> HashMap<String, String> {
    HashMap::from([(WFL_FIELD_TYPE_METADATA_KEY.to_string(), kind.to_string())])
}

/// 按 schema 判定字段的结构化 metadata 值（非结构化字段为 `None`）。
fn schema_structured_kind(field_type: &FieldType) -> Option<&'static str> {
    match field_type {
        FieldType::Object => Some(WFL_FIELD_TYPE_OBJECT),
        FieldType::ArrayAny | FieldType::Array(_) => Some(WFL_FIELD_TYPE_ARRAY),
        FieldType::Base(_) => None,
    }
}

/// 有 schema 时的字段构造：结构化字段附 metadata。
fn field_with_schema_metadata(name: &str, field_type: &FieldType) -> Field {
    let field = Field::new(name, field_type_to_arrow(field_type), true);
    match schema_structured_kind(field_type) {
        Some(kind) => field.with_metadata(structured_metadata(kind)),
        None => field,
    }
}

/// 无 schema 时按列内实际值推断结构化类型：整列（忽略标量值）**同形**才打标。
///
/// 同列混了 object 与 array 时不打标——引擎的 JSON 解析对形状是**严格**的
/// （`json_to_structured_value` 只认匹配形状），打任一种都会让另一种形状的格子解析
/// 失败而丢字段；保持"当字符串"是唯一不丢数据的退路。
fn inferred_structured_kind(events: &[GenEvent], field_name: &str) -> Option<&'static str> {
    let mut kind: Option<&'static str> = None;
    for event in events {
        let Some(value) = event.fields.get(field_name) else {
            continue;
        };
        let this = match value {
            serde_json::Value::Object(_) => WFL_FIELD_TYPE_OBJECT,
            serde_json::Value::Array(_) => WFL_FIELD_TYPE_ARRAY,
            _ => continue,
        };
        match kind {
            None => kind = Some(this),
            Some(previous) if previous == this => {}
            Some(_) => return None,
        }
    }
    kind
}

enum ColumnBuilder {
    Utf8(Vec<Option<String>>),
    Int64(Vec<Option<i64>>),
    Float64(Vec<Option<f64>>),
    Bool(Vec<Option<bool>>),
    TimeNanos(Vec<Option<i64>>),
}

impl ColumnBuilder {
    fn new(field_type: &FieldType, cap: usize) -> Self {
        // 与 `field_type_to_arrow` 同口径：结构化值全部走 Utf8（JSON 文本）。
        match field_type {
            FieldType::Object | FieldType::ArrayAny | FieldType::Array(_) => {
                Self::Utf8(Vec::with_capacity(cap))
            }
            FieldType::Base(base) => match base {
                BaseType::Chars | BaseType::Ip | BaseType::Hex => {
                    Self::Utf8(Vec::with_capacity(cap))
                }
                BaseType::Digit => Self::Int64(Vec::with_capacity(cap)),
                BaseType::Float => Self::Float64(Vec::with_capacity(cap)),
                BaseType::Bool => Self::Bool(Vec::with_capacity(cap)),
                BaseType::Time => Self::TimeNanos(Vec::with_capacity(cap)),
            },
        }
    }

    fn push(&mut self, value: Option<&serde_json::Value>, fallback_time: Option<i64>) {
        match self {
            Self::Utf8(col) => col.push(value.map(|v| match v {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            })),
            Self::Int64(col) => col.push(value.and_then(|v| v.as_i64())),
            Self::Float64(col) => col.push(value.and_then(|v| v.as_f64())),
            Self::Bool(col) => col.push(value.and_then(|v| v.as_bool())),
            Self::TimeNanos(col) => {
                let parsed = value.and_then(|v| {
                    if let Some(n) = v.as_i64() {
                        return Some(n);
                    }
                    if let Some(s) = v.as_str()
                        && let Ok(dt) = s.parse::<DateTime<Utc>>()
                    {
                        return dt.timestamp_nanos_opt();
                    }
                    None
                });
                col.push(parsed.or(fallback_time));
            }
        }
    }

    fn finish(self) -> ArrayRef {
        match self {
            Self::Utf8(col) => Arc::new(StringArray::from(col)),
            Self::Int64(col) => Arc::new(Int64Array::from(col)),
            Self::Float64(col) => Arc::new(Float64Array::from(col)),
            Self::Bool(col) => Arc::new(BooleanArray::from(col)),
            Self::TimeNanos(col) => Arc::new(TimestampNanosecondArray::from(col)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use serde_json::json;
    use std::time::Duration;
    use wf_lang::{BaseType, FieldDef, FieldType, WindowSchema};

    fn schema() -> WindowSchema {
        WindowSchema {
            name: "conn_events".into(),
            streams: vec!["conn_events".into()],
            time_field: Some("event_time".into()),
            over: Duration::from_secs(120),
            fields: vec![
                FieldDef {
                    name: "sip".into(),
                    field_type: FieldType::Base(BaseType::Ip),
                },
                FieldDef {
                    name: "event_time".into(),
                    field_type: FieldType::Base(BaseType::Time),
                },
                FieldDef {
                    name: "conn_info".into(),
                    field_type: FieldType::Object,
                },
            ],
        }
    }

    fn event(_i: usize, conn_info: Option<&serde_json::Value>) -> GenEvent {
        let mut fields = serde_json::Map::new();
        fields.insert("sip".into(), json!("10.0.0.1"));
        fields.insert("event_time".into(), json!("2026-08-13T00:00:00Z"));
        if let Some(v) = conn_info {
            fields.insert("conn_info".into(), v.clone());
        }
        GenEvent {
            stream_name: "conn_events".into(),
            window_name: "conn_events".into(),
            timestamp: Utc::now(),
            fields,
        }
    }

    /// Object-heavy events must split into multiple byte-bounded frames instead
    /// of one giant frame per window (which would exceed a window's
    /// `max_window_bytes` and be dropped whole — wp-labs/wp-reactor#20).
    #[test]
    fn object_heavy_events_split_into_byte_bounded_frames() {
        let big = json!({"data": "x".repeat(2000)}); // ~2KB object field per row
        let events: Vec<GenEvent> = (0..10_000).map(|i| event(i, Some(&big))).collect();
        let per_event = event_frame_bytes(&events[0], &schema());

        let batches = events_to_typed_batches(
            &events,
            &[schema()],
            DEFAULT_MAX_FRAME_BYTES,
            DEFAULT_MAX_FRAME_ROWS,
        )
        .unwrap();

        assert!(
            batches.len() >= 2,
            "10k × ~2KB events must split into multiple frames ({}), not one giant frame",
            batches.len()
        );
        let total_rows: usize = batches.iter().map(|(_, b)| b.num_rows()).sum();
        assert_eq!(
            total_rows,
            events.len(),
            "no event may be dropped or duplicated"
        );
        for (_, b) in &batches {
            assert!(
                b.num_rows() * per_event <= DEFAULT_MAX_FRAME_BYTES,
                "frame of {} rows × {per_event}B must stay under the byte cap",
                b.num_rows()
            );
            assert!(b.num_rows() <= DEFAULT_MAX_FRAME_ROWS);
        }
    }

    /// Narrow (mostly-null) events must still split at the row cap so a single
    /// RecordBatch never pins the commit worker with one huge vector.
    #[test]
    fn narrow_events_split_at_row_cap() {
        // ~52B/event → 100k rows ≈ 5.2MB < DEFAULT_MAX_FRAME_BYTES, so the row
        // cap is the binding constraint.
        let events: Vec<GenEvent> = (0..(DEFAULT_MAX_FRAME_ROWS + DEFAULT_MAX_FRAME_ROWS / 2))
            .map(|i| event(i, None))
            .collect();

        let batches = events_to_typed_batches(
            &events,
            &[schema()],
            DEFAULT_MAX_FRAME_BYTES,
            DEFAULT_MAX_FRAME_ROWS,
        )
        .unwrap();

        assert!(
            batches.len() >= 2,
            "150k narrow events must split at the row cap ({} frames)",
            batches.len()
        );
        let total_rows: usize = batches.iter().map(|(_, b)| b.num_rows()).sum();
        assert_eq!(total_rows, events.len());
        for (_, b) in &batches {
            assert!(
                b.num_rows() <= DEFAULT_MAX_FRAME_ROWS,
                "frame must not exceed the row cap ({} rows)",
                b.num_rows()
            );
        }
    }

    // -----------------------------------------------------------------------
    // 结构化列（object / array）的 `wf.wfl.field_type` metadata
    // -----------------------------------------------------------------------

    use wf_engine::match_engine::{
        Value as EngineValue, batch_to_events, wfl_structured_field_kind,
    };

    /// sip（ip）+ conn_info（object）+ tags（array）+ ports（array/digit）+ action（chars）。
    fn structured_schema() -> WindowSchema {
        WindowSchema {
            name: "conn_events".into(),
            streams: vec!["conn_events".into()],
            time_field: Some("event_time".into()),
            over: Duration::from_secs(120),
            fields: vec![
                FieldDef {
                    name: "sip".into(),
                    field_type: FieldType::Base(BaseType::Ip),
                },
                FieldDef {
                    name: "event_time".into(),
                    field_type: FieldType::Base(BaseType::Time),
                },
                FieldDef {
                    name: "conn_info".into(),
                    field_type: FieldType::Object,
                },
                FieldDef {
                    name: "tags".into(),
                    field_type: FieldType::ArrayAny,
                },
                FieldDef {
                    name: "ports".into(),
                    field_type: FieldType::Array(BaseType::Digit),
                },
                FieldDef {
                    name: "action".into(),
                    field_type: FieldType::Base(BaseType::Chars),
                },
            ],
        }
    }

    fn structured_event() -> GenEvent {
        let mut fields = serde_json::Map::new();
        fields.insert("sip".into(), json!("10.0.0.1"));
        fields.insert("event_time".into(), json!("2026-08-13T00:00:00Z"));
        fields.insert(
            "conn_info".into(),
            json!({"bytes_out": 500, "nested": {"sev": 10}}),
        );
        fields.insert("tags".into(), json!(["ssh", 22]));
        fields.insert("ports".into(), json!([22, 80]));
        fields.insert("action".into(), json!("syn"));
        GenEvent {
            stream_name: "conn_events".into(),
            window_name: "conn_events".into(),
            timestamp: Utc::now(),
            fields,
        }
    }

    /// 定类型数组 `array/digit` 曾写成 Int64 标量列——数组结构丢失、值全 null。
    #[test]
    fn typed_array_column_is_json_text_not_element_scalar() {
        let batch = &events_to_typed_batches(
            &[structured_event()],
            &[structured_schema()],
            DEFAULT_MAX_FRAME_BYTES,
            DEFAULT_MAX_FRAME_ROWS,
        )
        .unwrap()[0]
            .1;

        for name in ["conn_info", "tags", "ports"] {
            let field = batch.schema().field_with_name(name).unwrap().clone();
            assert_eq!(
                field.data_type(),
                &DataType::Utf8,
                "{name} 必须以 JSON 文本存储（实际 {:?}）",
                field.data_type()
            );
            assert!(
                wfl_structured_field_kind(&field).is_some(),
                "{name} 必须带引擎认可的结构化 metadata（实际 metadata {:?}）",
                field.metadata()
            );
        }
        assert_eq!(
            wfl_structured_field_kind(batch.schema().field_with_name("conn_info").unwrap()),
            Some("object")
        );
        assert_eq!(
            wfl_structured_field_kind(batch.schema().field_with_name("tags").unwrap()),
            Some("array")
        );
        // 标量字段不得被打标（否则引擎会去 JSON 解析普通字符串）。
        let schema = batch.schema();
        for name in ["sip", "action", "event_time"] {
            let field = schema.field_with_name(name).unwrap();
            assert!(
                wfl_structured_field_kind(field).is_none(),
                "{name} 是标量字段，不应有结构化 metadata（实际 {:?}）",
                field.metadata()
            );
        }
    }

    /// 契约级：用**引擎自己的**读列器验证结构值真的还原成 Object / Array。
    ///
    /// 缺少 metadata 时引擎一律给 `Value::Str`（规则读嵌套字段静默不产出、
    /// 与直读 GenEvent 的 oracle 不一致）——这条断言就是那个缺口的回归护栏。
    #[test]
    fn engine_reads_structured_columns_back_as_object_and_array() {
        let batch = &events_to_typed_batches(
            &[structured_event()],
            &[structured_schema()],
            DEFAULT_MAX_FRAME_BYTES,
            DEFAULT_MAX_FRAME_ROWS,
        )
        .unwrap()[0]
            .1;

        let events = batch_to_events(batch);
        assert_eq!(events.len(), 1);
        let fields = &events[0].fields;

        let EngineValue::Object(conn_info) = &fields["conn_info"] else {
            panic!("conn_info 应为 Object，实际 {:?}", fields["conn_info"]);
        };
        assert_eq!(
            conn_info.get("bytes_out"),
            Some(&EngineValue::Number(500.0))
        );
        let Some(EngineValue::Object(nested)) = conn_info.get("nested") else {
            panic!("嵌套 object 应保留，实际 {:?}", conn_info.get("nested"));
        };
        assert_eq!(nested.get("sev"), Some(&EngineValue::Number(10.0)));

        assert_eq!(
            fields["ports"],
            EngineValue::Array(vec![EngineValue::Number(22.0), EngineValue::Number(80.0)]),
            "array/digit 必须还原成数组，而不是标量或字符串"
        );
        assert_eq!(fields["sip"], EngineValue::Str("10.0.0.1".into()));
    }

    /// 无 schema 的 `.arrow` 文件输出：按列内实际值打标；普通字符串字段即使内容
    /// 形如 JSON 也不打标（否则引擎会把它当结构化值）。
    #[test]
    fn arrow_file_output_tags_structured_columns_by_value() {
        let mut fields = serde_json::Map::new();
        fields.insert("conn_info".into(), json!({"bytes_out": 500}));
        fields.insert("json_like_text".into(), json!(r#"{"bytes_out":500}"#));
        let event = GenEvent {
            stream_name: "conn_events".into(),
            window_name: "conn_events".into(),
            timestamp: Utc::now(),
            fields,
        };

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.arrow");
        write_arrow_ipc(std::slice::from_ref(&event), &path).unwrap();

        let file = File::open(&path).unwrap();
        let reader = arrow::ipc::reader::FileReader::try_new(file, None).unwrap();
        let schema = reader.schema();
        let conn_info = schema.field_with_name("conn_info").unwrap();
        assert_eq!(wfl_structured_field_kind(conn_info), Some("object"));
        assert!(
            wfl_structured_field_kind(schema.field_with_name("json_like_text").unwrap()).is_none(),
            "内容像 JSON 的普通字符串字段不应被打标"
        );
    }

    /// 规则级：读**嵌套字段**的规则能命中——metadata 缺失时引擎把列值当字符串，
    /// 比较恒为 None，规则静默不产出（负向对照同时断言这点）。
    ///
    /// 走的就是运行时那条链：`events_to_typed_batches` →（帧）→ 引擎
    /// `batch_to_events` → `CepStateMachine` + `RuleExecutor`。
    #[test]
    fn rule_reading_nested_field_alerts_only_with_metadata() {
        use wf_engine::match_engine::{CepStateMachine, RuleExecutor, StepResult};

        let schemas = wf_lang::parse_wfs(
            r#"
window conn_events {
    stream_tag = "netflow"
    time = event_time
    over = 5m
    fields {
        sip: ip
        event_time: time
        conn_info: object
    }
}

window alerts {
    over = 0
    fields { sip: ip }
}
"#,
        )
        .expect("parse .wfs");
        let wfl = wf_lang::parse_wfl(
            r#"
rule nested_read {
  events { c : conn_events && c.conn_info.bytes_out >= 400 }
  match<sip:5m> {
    on event { c | count >= 1; }
  } -> score(80.0)
  entity(ip, c.sip)
  yield alerts (sip = c.sip)
}
"#,
        )
        .expect("parse .wfl");
        let plans = wf_lang::compile_wfl(&wfl, &schemas).expect("compile .wfl");
        let plan = &plans[0];

        let mut fields = serde_json::Map::new();
        fields.insert("sip".into(), json!("10.0.0.1"));
        fields.insert("event_time".into(), json!("2026-08-13T00:00:00Z"));
        fields.insert("conn_info".into(), json!({"bytes_out": 500}));
        let event = GenEvent {
            stream_name: "conn_events".into(),
            window_name: "conn_events".into(),
            timestamp: Utc::now(),
            fields,
        };
        let nanos = event.timestamp.timestamp_nanos_opt().unwrap();

        let batch = &events_to_typed_batches(
            std::slice::from_ref(&event),
            &schemas,
            DEFAULT_MAX_FRAME_BYTES,
            DEFAULT_MAX_FRAME_ROWS,
        )
        .unwrap()[0]
            .1;

        /// 把引擎素材化后的事件喂给规则，返回是否产出告警。
        fn alerts(batch: &RecordBatch, plan: &wf_lang::plan::RulePlan, nanos: i64) -> bool {
            let mut sm = CepStateMachine::new(plan.name.clone(), plan.match_plan.clone(), None);
            let executor = RuleExecutor::new(plan.clone());
            for event in batch_to_events(batch) {
                // 与运行时同序：先过 bind 别名 filter，再推进状态机。
                if !executor.event_matches_alias("c", &event, None) {
                    continue;
                }
                if let StepResult::Matched(ctx) =
                    sm.advance_at_with_masks("c", &event, nanos, None, 0, None)
                    && executor.execute_match(&ctx).is_ok()
                {
                    return true;
                }
            }
            false
        }

        assert!(
            alerts(batch, plan, nanos),
            "带 metadata 的结构化列必须让嵌套字段比较成立并产出告警"
        );

        // 负向对照：抹掉结构化 metadata（复现修复前的列出形态）→ 不产出。
        let stripped_fields: Vec<Field> = batch
            .schema()
            .fields()
            .iter()
            .map(|f| {
                let mut f = f.as_ref().clone();
                f.set_metadata(HashMap::new());
                f
            })
            .collect();
        let stripped = RecordBatch::try_new(
            Arc::new(Schema::new(stripped_fields)),
            batch.columns().to_vec(),
        )
        .unwrap();
        assert!(
            !alerts(&stripped, plan, nanos),
            "缺少 metadata 时引擎只看到字符串，规则应静默不产出（负向对照）"
        );
    }

    /// 同列混了 object 与 array：不打标（打任一种都会丢另一种形状的格子）。
    #[test]
    fn mixed_object_and_array_column_is_not_tagged() {
        let mut first = serde_json::Map::new();
        first.insert("payload".into(), json!({"a": 1}));
        let mut second = serde_json::Map::new();
        second.insert("payload".into(), json!([1, 2]));
        let events: Vec<GenEvent> = [first, second]
            .into_iter()
            .map(|fields| GenEvent {
                stream_name: "s".into(),
                window_name: "w".into(),
                timestamp: Utc::now(),
                fields,
            })
            .collect();

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mixed.arrow");
        write_arrow_ipc(&events, &path).unwrap();

        let file = File::open(&path).unwrap();
        let reader = arrow::ipc::reader::FileReader::try_new(file, None).unwrap();
        assert!(
            wfl_structured_field_kind(reader.schema().field_with_name("payload").unwrap())
                .is_none(),
            "混形列不打标（保持当字符串，不丢数据）"
        );
    }
}
