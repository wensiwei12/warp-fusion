use chrono::{DateTime, Duration as ChronoDuration, Utc};
use rand::Rng;
use rand::rngs::StdRng;
use serde_json::Value;
use wf_lang::{BaseType, FieldType, WindowSchema};

use crate::wfg_ast::StreamBlock;

use crate::datagen::inject_gen::entity_value_for_index;

use super::field_gen::generate_field_value;

/// 24 位实体索引空间（与注入侧 [`crate::datagen::inject_gen::ENTITY_ID_SPACE`] 同一空间）。
const ENTITY_INDEX_TOP: u64 = 1 << 24;

/// 一个字段的实体值池（设计 §10）：Zipf 加权的热点池 + 池外新值带。
///
/// 池内值由**索引**直接导出（`[TOP − pool, TOP)`），不消耗 RNG，因此跨运行确定；采样时
/// 按 Zipf 权重抽 rank——热点实体反复出现、长尾偶发。`fresh` 比例的事件取池外新值带
/// `[TOP − 2·pool, TOP − pool)`，与池、与注入实体值都不重叠。
pub struct EntityPool {
    field: String,
    values: Vec<serde_json::Value>,
    cumulative: Vec<f64>,
    fresh_start: u64,
    fresh_len: u64,
    fresh: f64,
}

impl EntityPool {
    pub(crate) fn new(
        field: &str,
        field_type: Option<&FieldType>,
        pool: u64,
        exponent: f64,
        fresh: f64,
    ) -> Self {
        let pool = pool.max(1);
        let start = ENTITY_INDEX_TOP - pool;
        let values: Vec<serde_json::Value> = (0..pool)
            .map(|i| entity_value_for_index(field_type, start + i, "bg", field))
            .collect();
        // Zipf：rank k（从 0 起）权重 1/(k+1)^exponent。
        let mut cumulative = Vec::with_capacity(values.len());
        let mut acc = 0.0;
        for k in 0..values.len() {
            acc += 1.0 / ((k + 1) as f64).powf(exponent);
            cumulative.push(acc);
        }
        Self {
            field: field.to_string(),
            values,
            cumulative,
            fresh_start: ENTITY_INDEX_TOP - 2 * pool,
            fresh_len: pool,
            fresh,
        }
    }

    /// 抽一个值：`fresh` 比例取池外新值，其余按 Zipf 权重取池内值。
    fn next_value(&self, field_type: Option<&FieldType>, rng: &mut StdRng) -> serde_json::Value {
        if self.fresh > 0.0 && rng.random_bool(self.fresh.min(1.0)) {
            let index = self.fresh_start + rng.random_range(0..self.fresh_len.max(1));
            return entity_value_for_index(field_type, index, "bg", &self.field);
        }
        let total = *self.cumulative.last().expect("pool 非空");
        let draw = rng.random_range(0.0..total);
        let rank = match self
            .cumulative
            .binary_search_by(|c| c.partial_cmp(&draw).unwrap())
        {
            Ok(i) => i,
            Err(i) => i.min(self.values.len() - 1),
        };
        self.values[rank].clone()
    }
}

/// A single generated event.
#[derive(Debug, Clone)]
pub struct GenEvent {
    /// The actual stream name from schema (e.g., "syslog"), used for `_stream` in output.
    pub stream_name: String,
    /// The window name (e.g., "auth_events").
    pub window_name: String,
    pub timestamp: DateTime<Utc>,
    pub fields: serde_json::Map<String, Value>,
}

/// Generate events for a single stream.
pub fn generate_stream_events(
    stream: &StreamBlock,
    schema: &WindowSchema,
    pools: &[EntityPool],
    event_count: u64,
    start: &DateTime<Utc>,
    duration: &std::time::Duration,
    rng: &mut StdRng,
) -> Vec<GenEvent> {
    let mut events = Vec::with_capacity(event_count as usize);

    // Get the actual stream name from schema (e.g., "syslog")
    let stream_name = schema
        .streams
        .first()
        .cloned()
        .unwrap_or_else(|| schema.name.clone());

    let duration_nanos = duration.as_nanos() as i64;
    let interval = if event_count > 1 {
        duration_nanos / (event_count as i64)
    } else {
        0
    };

    // 按字段对齐实体值池（设计 §10）：命中池的字段从池里抽值，其余照旧随机。
    let pools: Vec<Option<&EntityPool>> = schema
        .fields
        .iter()
        .map(|field_def| pools.iter().find(|p| p.field == field_def.name))
        .collect();

    for i in 0..event_count {
        let ts = *start + ChronoDuration::nanoseconds(interval * i as i64);

        let mut fields = serde_json::Map::new();

        for (field_idx, field_def) in schema.fields.iter().enumerate() {
            // For Time fields, set the timestamp
            if matches!(&field_def.field_type, FieldType::Base(BaseType::Time)) {
                fields.insert(
                    field_def.name.clone(),
                    serde_json::json!(ts.timestamp_nanos_opt().unwrap_or(0)),
                );
                continue;
            }

            // 实体分布字段：从池里抽（热点重复出现；`fresh` 比例取池外新值）。
            if let Some(Some(pool)) = pools.get(field_idx) {
                let value = pool.next_value(Some(&field_def.field_type), rng);
                fields.insert(field_def.name.clone(), value);
                continue;
            }

            let value = generate_field_value(&field_def.field_type, rng);
            fields.insert(field_def.name.clone(), value);
        }

        events.push(GenEvent {
            stream_name: stream_name.clone(),
            window_name: stream.window.clone(),
            timestamp: ts,
            fields,
        });
    }

    events
}
