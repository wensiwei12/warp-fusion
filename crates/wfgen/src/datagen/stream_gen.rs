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

/// 背景生成时必须避开的注入键值域（背景事件不能拿到注入实体的键值）。
///
/// 注入实体的键值取自 24 位实体空间的**底部**（`entity_base + i`，`i` 走实体键字段），
/// 而背景 `digit` / `float` 字段是小数值均匀分布——两者重叠：背景事件偶尔会拿到某个
/// 注入实体的键值，规则便把那条背景事件算到该实体头上（阈值被噪声顶过、否定步骤被噪声
/// 满足），INJ2「near_miss / miss 必不报警」于是变成**概率性**的（q5 实测踩过；
/// q6 语料靠显式声明 `entity … zipf(...)` 池把背景值放到空间顶部来规避）。
///
/// 这里把**下界**抬到注入占用区间之上：没有声明池的字段也不再与注入实体撞值。
/// 只对数值型字段生效（`digit` / `float`）——`chars` / `hex` / `ip` 的随机值恰好落在
/// 注入值形如 `hit_auction_000123` / `10.0.0.123` 上的概率可忽略。
#[derive(Debug, Default)]
pub struct ReservedKeyBands {
    /// 注入实体占用的 id 上界（不含）：背景数值必须 ≥ 它。
    floor: u64,
    /// 受影响的字段：(窗口名, 字段名)。
    fields: Vec<(String, String)>,
}

impl ReservedKeyBands {
    pub fn new(floor: u64, fields: Vec<(String, String)>) -> Self {
        Self { floor, fields }
    }

    /// 该窗口/字段是否需要避让；是则给出下界。
    fn floor_for(&self, window: &str, field: &str) -> Option<u64> {
        if self.floor == 0 {
            return None;
        }
        self.fields
            .iter()
            .any(|(w, f)| w == window && f == field)
            .then_some(self.floor)
    }
}

/// 把值抬出注入占用区间：**确定性平移**而不是重抽——重抽在值域宽度接近注入实体数时
/// 可能反复抽不到干净值，平移没有这个死角，也不改变分布形状（只是把值域整体右移）。
/// 非数值字段原样返回（调用方本就不对它们避让，见 [`ReservedKeyBands`]）。
fn shift_out_of_inject_band(value: Value, field_type: &FieldType, floor: u64) -> Value {
    let Some(number) = value.as_f64() else {
        return value;
    };
    if number >= floor as f64 {
        return value;
    }
    let shifted = number + floor as f64;
    match field_type {
        // `digit` 列必须是整数：平移后保持整数形态（否则 Arrow 编码与 oracle 的
        // 列式口径都会把它当 float 处理）。
        FieldType::Base(BaseType::Digit) => serde_json::json!(shifted as i64),
        FieldType::Base(BaseType::Float) => serde_json::json!(shifted),
        _ => value,
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
#[allow(clippy::too_many_arguments)]
pub fn generate_stream_events(
    stream: &StreamBlock,
    schema: &WindowSchema,
    pools: &[EntityPool],
    event_count: u64,
    start: &DateTime<Utc>,
    duration: &std::time::Duration,
    rng: &mut StdRng,
    reserved: &ReservedKeyBands,
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
            // 池的值带在 24 位空间**顶部**，与注入实体（底部）天然不重叠，无需避让。
            if let Some(Some(pool)) = pools.get(field_idx) {
                let value = pool.next_value(Some(&field_def.field_type), rng);
                fields.insert(field_def.name.clone(), value);
                continue;
            }

            let floor = reserved.floor_for(&stream.window, &field_def.name);
            let value = generate_field_value(&field_def.field_type, rng);
            let value = match floor {
                Some(floor) => shift_out_of_inject_band(value, &field_def.field_type, floor),
                None => value,
            };
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
