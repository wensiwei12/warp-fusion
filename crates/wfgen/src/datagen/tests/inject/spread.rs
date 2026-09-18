//! 注入时间铺开：簇起点在场景 `duration` 内**等距**（设计 §4.7）。
//!
//! 旧策略是"每簇随机起点 + 窗口内铺开"，实体之间会互相重叠、也会留出空档；现在首簇贴 0、
//! 末簇贴 `duration − 窗口`，整段时长被均匀覆盖。

use std::collections::{BTreeMap, HashSet};

use super::*;

/// 跑一遍场景，返回「注入实体值 → 该实体首条事件相对场景起点的偏移（纳秒）」。
fn first_event_offsets(wfg_src: &str) -> BTreeMap<String, i64> {
    let wfg = parse_wfg(wfg_src).unwrap();
    let schemas = vec![make_login_schema()];
    let plans = vec![make_brute_force_plan()];
    let result = generate(&wfg, &schemas, &plans).unwrap();

    let injected: HashSet<&str> = result
        .inject_entities
        .iter()
        .filter_map(|entity| entity.value.as_str())
        .collect();
    assert_eq!(injected.len(), 5, "场景应注入 5 个实体");

    let start_nanos = wfg
        .scenario
        .time_clause
        .start
        .parse::<chrono::DateTime<chrono::Utc>>()
        .unwrap()
        .timestamp_nanos_opt()
        .unwrap();

    // 只统计注入实体的事件：背景噪声（gen 1/s）也会落在同一个 stream 上。
    let mut first: BTreeMap<String, i64> = BTreeMap::new();
    for event in &result.events {
        let Some(entity) = event.fields.get("src_ip").and_then(|value| value.as_str()) else {
            continue;
        };
        if !injected.contains(entity) {
            continue;
        }
        let offset = event.timestamp.timestamp_nanos_opt().unwrap() - start_nanos;
        first
            .entry(entity.to_string())
            .and_modify(|current| *current = (*current).min(offset))
            .or_insert(offset);
    }
    first
}

/// 5 个实体、`#[duration=100s]`、`spread 10s` → span 90s → 起点 0 / 22.5 / 45 / 67.5 / 90s
/// （首簇贴 0、末簇贴 span，覆盖整段）。
fn assert_evenly_spread(offsets: &BTreeMap<String, i64>) {
    let mut values: Vec<i64> = offsets.values().copied().collect();
    values.sort_unstable();

    let seconds: Vec<f64> = values.iter().map(|ns| *ns as f64 / 1e9).collect();
    let expected = [0.0, 22.5, 45.0, 67.5, 90.0];
    assert_eq!(seconds.len(), expected.len(), "偏移: {seconds:?}");
    for (actual, want) in seconds.iter().zip(expected.iter()) {
        assert!(
            (actual - want).abs() < 1e-6,
            "簇起点应等距铺满 [0, span]：期望 {expected:?}，实际 {seconds:?}"
        );
    }
}

#[test]
fn hit_clusters_are_evenly_spread_across_duration() {
    let input = r#"
#[duration=100s]
scenario spread_uniform_hit<seed=1> {
    background { stream LoginWindow gen 1/s }
    inject {
        hit<src_ip: 5> for brute_force LoginWindow {
            use(success=false) x 3
            spread 10s
        }
    }
}
"#;
    assert_evenly_spread(&first_event_offsets(input));
}

#[test]
fn near_miss_clusters_are_evenly_spread_across_duration() {
    let input = r#"
#[duration=100s]
scenario spread_uniform_near_miss<seed=1> {
    background { stream LoginWindow gen 1/s }
    inject {
        near_miss<src_ip: 5> for brute_force LoginWindow {
            use(success=false) x 1
            spread 10s
        }
    }
}
"#;
    assert_evenly_spread(&first_event_offsets(input));
}

/// 窗口不短于 duration（规则窗口 300s > `#[duration=100s]`）时无法错开：所有簇都在 0。
#[test]
fn clusters_pile_up_when_window_exceeds_duration() {
    let input = r#"
#[duration=100s]
scenario spread_no_room<seed=1> {
    background { stream LoginWindow gen 1/s }
    inject {
        hit<src_ip: 5> for brute_force LoginWindow {
            use(success=false) x 3
        }
    }
}
"#;
    let offsets = first_event_offsets(input);
    assert!(
        offsets.values().all(|offset| *offset == 0),
        "窗口比场景还长时退回起点 0：{offsets:?}"
    );
}
