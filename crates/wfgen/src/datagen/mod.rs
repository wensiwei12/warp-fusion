pub mod fault_gen;
pub mod field_gen;
pub mod inject_gen;
pub mod stream_gen;
#[cfg(test)]
mod tests;

use std::cmp::{Ordering, Reverse};
use std::collections::BinaryHeap;

use chrono::{DateTime, Utc};
use rand::SeedableRng;
use rand::rngs::StdRng;
use wf_lang::WindowSchema;
use wf_lang::plan::RulePlan;

use crate::error::{self, WfgenReason, WfgenResult};
use crate::wfg_ast::WfgFile;
use inject_gen::InjectEntityKey;
use inject_gen::generate_inject_events;
use inject_gen::{InjectGenResult, WithoutGuard};
use stream_gen::{GenEvent, generate_stream_events};

/// Result of data generation.
pub struct GenResult {
    pub events: Vec<GenEvent>,
    /// 注入实体清单（生成期断言 INJ1/INJ2 的输入，设计 §4.2）；无注入用例时为空。
    pub inject_entities: Vec<InjectEntityKey>,
    /// 实体标识无法与 oracle `entity_id` 对齐、未纳入断言的实体个数。
    pub unasserted_inject_entities: u64,
}

/// Generate events from a parsed and validated `.wfg` scenario.
///
/// When `rule_plans` is non-empty and the scenario contains inject blocks,
/// rule-aware inject events are generated (hit / near-miss / non-hit clusters)
/// and merged with background events. When `rule_plans` is empty or no inject
/// blocks exist, the behaviour is identical to the M31 baseline.
pub fn generate(
    wfg: &WfgFile,
    schemas: &[WindowSchema],
    rule_plans: &[RulePlan],
) -> WfgenResult<GenResult> {
    let scenario = &wfg.scenario;

    // Parse start time
    let start: DateTime<Utc> = scenario.time_clause.start.parse().map_err(|e| {
        error::error(
            WfgenReason::Generation,
            format!("invalid start time '{}': {}", scenario.time_clause.start, e),
        )
    })?;

    let duration = scenario.time_clause.duration;
    let total = scenario.total;

    // Create deterministic RNG
    let mut rng = StdRng::seed_from_u64(scenario.seed);

    // --- Inject generation (if applicable) ---
    let mut sorted_chunks: Vec<Vec<GenEvent>> = Vec::new();
    let mut inject_entities: Vec<InjectEntityKey> = Vec::new();
    let mut unasserted_inject_entities = 0_u64;
    let mut without_guards: Vec<WithoutGuard> = Vec::new();

    let has_syntax_inject = wfg
        .syntax
        .as_ref()
        .and_then(|syntax| syntax.injection.as_ref())
        .is_some_and(|injection| !injection.cases.is_empty());
    let has_inject = has_syntax_inject && !rule_plans.is_empty();
    if has_inject {
        let InjectGenResult {
            events: mut inject_events,
            entity_keys,
            unasserted_entities,
            without_guards: guards,
        } = generate_inject_events(wfg, rule_plans, schemas, &start, &duration, &mut rng)?;
        inject_entities = entity_keys;
        unasserted_inject_entities = unasserted_entities;
        without_guards = guards;
        inject_events.sort_by_key(|a| a.timestamp);
        if !inject_events.is_empty() {
            sorted_chunks.push(inject_events);
        }
    }

    // --- Background event generation ---
    let total_rate: f64 = scenario
        .streams
        .iter()
        .map(|s| s.rate.events_per_second())
        .sum();

    if total_rate == 0.0 {
        return error::fail(
            WfgenReason::Validation,
            "total rate across all streams is 0",
        );
    }

    let mut remaining = total;

    for (i, stream) in scenario.streams.iter().enumerate() {
        let proportion = stream.rate.events_per_second() / total_rate;
        let stream_total = if i == scenario.streams.len() - 1 {
            remaining
        } else {
            let count = (total as f64 * proportion).round() as u64;
            let count = count.min(remaining);
            remaining -= count;
            count
        };

        // 背景与注入**完全分离**（设计 §3.1/§3.4）：背景就是它自己的配额
        // （`rate × duration`），注入事件是额外的——总条数 = 背景 + 注入，
        // 改背景速率不会改变注入条数，注入也不会挤压背景。
        let bg_count = stream_total;

        if bg_count == 0 {
            continue;
        }

        let schema = schemas
            .iter()
            .find(|s| s.name == stream.window)
            .ok_or_else(|| {
                error::error(
                    WfgenReason::Validation,
                    format!("schema not found for window '{}'", stream.window),
                )
            })?;

        let events = generate_stream_events(stream, schema, bg_count, &start, &duration, &mut rng);
        let events = suppress_without_guards(events, &without_guards);
        if !events.is_empty() {
            sorted_chunks.push(events);
        }
    }

    let all_events = merge_sorted_chunks(sorted_chunks);

    Ok(GenResult {
        events: all_events,
        inject_entities,
        unasserted_inject_entities,
    })
}

#[derive(Debug)]
struct HeapItem {
    ts_nanos: i64,
    chunk_idx: usize,
    event: GenEvent,
}

impl HeapItem {
    fn new(chunk_idx: usize, event: GenEvent) -> Self {
        let ts_nanos = event.timestamp.timestamp_nanos_opt().unwrap_or(i64::MAX);
        Self {
            ts_nanos,
            chunk_idx,
            event,
        }
    }
}

impl PartialEq for HeapItem {
    fn eq(&self, other: &Self) -> bool {
        self.ts_nanos == other.ts_nanos && self.chunk_idx == other.chunk_idx
    }
}

impl Eq for HeapItem {}

impl PartialOrd for HeapItem {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for HeapItem {
    fn cmp(&self, other: &Self) -> Ordering {
        self.ts_nanos
            .cmp(&other.ts_nanos)
            .then_with(|| self.chunk_idx.cmp(&other.chunk_idx))
    }
}

/// 剔除「属于受约束实体、落在约束窗内、且命中 `without(...)` 谓词」的背景事件
/// （设计 §3.8）。
///
/// 背景是随机流量，不认领任何实体；但随机 IP / 随机数字很容易恰好撞上注入实体的键值
/// ——一旦撞上，这条噪声就会被规则当成**该实体**的事件看到（`wf-cep` 的否定扫描按
/// 窗口实例进行）。带否定步骤的规则对“窗口内不得出现匹配事件”敏感，撞上就静默不触发。
///
/// 只剔命中谓词的那些：不命中谓词的背景噪声不会违反否定步骤，剔除它只会无故偏离
/// 背景配额。判定口径全部收在 [`WithoutGuard::is_violation`]，与注入侧冲突检查同源。
fn suppress_without_guards(events: Vec<GenEvent>, guards: &[WithoutGuard]) -> Vec<GenEvent> {
    if guards.is_empty() {
        return events;
    }

    events
        .into_iter()
        .filter(|event| !guards.iter().any(|guard| guard.is_violation(event)))
        .collect()
}

fn merge_sorted_chunks(chunks: Vec<Vec<GenEvent>>) -> Vec<GenEvent> {
    let total_events: usize = chunks.iter().map(Vec::len).sum();
    if total_events == 0 {
        return Vec::new();
    }

    let mut iters: Vec<std::vec::IntoIter<GenEvent>> =
        chunks.into_iter().map(Vec::into_iter).collect();
    let mut heap: BinaryHeap<Reverse<HeapItem>> = BinaryHeap::new();

    for (idx, iter) in iters.iter_mut().enumerate() {
        if let Some(event) = iter.next() {
            heap.push(Reverse(HeapItem::new(idx, event)));
        }
    }

    let mut merged = Vec::with_capacity(total_events);
    while let Some(Reverse(item)) = heap.pop() {
        let idx = item.chunk_idx;
        merged.push(item.event);
        if let Some(next_event) = iters[idx].next() {
            heap.push(Reverse(HeapItem::new(idx, next_event)));
        }
    }

    merged
}
