//! `entity <window>.<field> zipf(...)` 实体分布（设计 §10）。
//!
//! 背景事件默认每个字段**每条现随机**（同一 stream 里没有任何值会重复），于是"热点实体"
//! 表达不出来。这里锁定三件事：池把该字段**限定在值域内**、Zipf 权重确实造出热点、
//! `fresh` 比例取池外**不重叠**的新值带。

use std::collections::{BTreeMap, BTreeSet};

use super::*;

/// `LoginWindow.src_ip` 做实体分布；`sip` 是 Ip，值域映射到 24 位空间顶部。
fn schemas() -> Vec<WindowSchema> {
    vec![make_login_schema()]
}

/// pool=8 时池内 IP：索引 `2^24 − 8 .. 2^24` → `10.255.255.248 .. 10.255.255.255`。
fn pool_band() -> BTreeSet<String> {
    (248..=255u8).map(|d| format!("10.255.255.{d}")).collect()
}

/// `fresh` 值带：再往下的 8 个索引 → `10.255.255.240 .. 10.255.255.247`（与池不重叠）。
fn fresh_band() -> BTreeSet<String> {
    (240..=247u8).map(|d| format!("10.255.255.{d}")).collect()
}

fn generate_dist(input: &str) -> Vec<crate::datagen::stream_gen::GenEvent> {
    let wfg = parse_wfg(input).unwrap();
    let schemas = schemas();
    let result = generate(&wfg, &schemas, &[]).unwrap();
    result.events
}

fn ip_counts(events: &[crate::datagen::stream_gen::GenEvent]) -> BTreeMap<String, u64> {
    let mut counts: BTreeMap<String, u64> = BTreeMap::new();
    for event in events {
        let value = event
            .fields
            .get("src_ip")
            .and_then(|v| v.as_str())
            .expect("src_ip 应为字符串")
            .to_string();
        *counts.entry(value).or_default() += 1;
    }
    counts
}

/// 池把该字段限定在值域内：值只可能来自池内 8 个 IP，且**全部** 8 个都出现过。
#[test]
fn zipf_pool_confines_the_field_to_the_pool_band() {
    let events = generate_dist(
        r#"
#[duration=10s]
scenario dist<seed=7> {
    background {
        stream LoginWindow gen 400/s
        entity LoginWindow.src_ip zipf(pool=8, exponent=1.5)
    }
}
"#,
    );
    assert_eq!(events.len(), 4000, "10s × 400/s");

    let counts = ip_counts(&events);
    let values: BTreeSet<String> = counts.keys().cloned().collect();
    assert_eq!(values, pool_band(), "只应取池内 8 个值");
    assert_eq!(counts.len(), 8, "8 个实体都要出现");
}

/// `exponent=1.5` → 热点集中：最热实体的条数远高于最冷实体。
#[test]
fn zipf_exponent_makes_hot_entities() {
    let events = generate_dist(
        r#"
#[duration=10s]
scenario dist<seed=7> {
    background {
        stream LoginWindow gen 400/s
        entity LoginWindow.src_ip zipf(pool=8, exponent=1.5)
    }
}
"#,
    );
    let counts = ip_counts(&events);
    let hot = *counts.get("10.255.255.248").expect("rank 1 最热");
    let cold = *counts.get("10.255.255.255").expect("rank 8 最冷");
    assert!(hot > cold * 3, "热点应显著高于长尾：hot={hot} cold={cold}");
    assert!(hot < 4000, "不应退化成「全是同一个值」：hot={hot}");
    assert!(cold > 0, "长尾也要出现");
}

/// `exponent=0` → 均匀：各实体条数接近（不引入偏置）。
#[test]
fn exponent_zero_is_uniform() {
    let events = generate_dist(
        r#"
#[duration=10s]
scenario dist<seed=7> {
    background {
        stream LoginWindow gen 400/s
        entity LoginWindow.src_ip zipf(pool=8, exponent=0)
    }
}
"#,
    );
    let counts = ip_counts(&events);
    assert_eq!(counts.len(), 8);
    let max = counts.values().max().copied().unwrap();
    let min = counts.values().min().copied().unwrap();
    assert!(
        min * 3 > max * 2,
        "均匀分布下 min/max 应接近 1：max={max} min={min}"
    );
}

/// `fresh=1.0` → 全部取池外新值带，且与池**不重叠**（这是 INJ1/INJ2 口径不被污染的前提）。
#[test]
fn fresh_ratio_draws_from_a_disjoint_band() {
    let events = generate_dist(
        r#"
#[duration=10s]
scenario dist<seed=7> {
    background {
        stream LoginWindow gen 400/s
        entity LoginWindow.src_ip zipf(pool=8, exponent=1.5, fresh=1.0)
    }
}
"#,
    );
    let counts = ip_counts(&events);
    let values: BTreeSet<String> = counts.keys().cloned().collect();
    assert!(
        values.is_subset(&fresh_band()),
        "fresh=1.0 时只应取新值带：{values:?}"
    );
    assert!(
        values.is_disjoint(&pool_band()),
        "新值带与池必须不重叠（否则口径会互相污染）"
    );
}

/// 未声明分布的字段照旧随机（分布只作用于指定字段）。
#[test]
fn other_fields_stay_random() {
    let events = generate_dist(
        r#"
#[duration=10s]
scenario dist<seed=7> {
    background {
        stream LoginWindow gen 400/s
        entity LoginWindow.src_ip zipf(pool=4, exponent=1.0)
    }
}
"#,
    );
    let usernames: BTreeSet<String> = events
        .iter()
        .filter_map(|e| e.fields.get("username").and_then(|v| v.as_str()))
        .map(|s| s.to_string())
        .collect();
    assert!(
        usernames.len() > 100,
        "未声明分布的字段应保持逐条随机：{} 个不同值",
        usernames.len()
    );
}
