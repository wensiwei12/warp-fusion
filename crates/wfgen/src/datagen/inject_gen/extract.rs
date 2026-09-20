use std::collections::HashMap;

use wf_lang::ast::{BinOp, Expr, FieldRef, Measure};
use wf_lang::plan::RulePlan;
use wf_lang::plan::WindowSpec;

use super::structures::{
    AliasMap, InjectOverrides, InjectUseStepOverrides, RuleJoinInfo, RuleStructure, StepInfo,
};
use crate::error::{self, WfgenReason, WfgenResult};
use crate::wfg_ast::{InjectCase, ValueSource};

/// snapshot 形态右事件的提前量：**1ms**。
///
/// 取 1ms 而不是 1ns 是为了同时满足两类下游：时钟按纳秒走的（引擎按 schema 的
/// `time_field` 读事件时间）1ns 就够，而 JSONL 里的 `_timestamp` 便利字段是**毫秒精度**——
/// 1ns 的前挪在同一毫秒内会被抹平，先后关系看不出来。1ms 对 snapshot（无 `within` 时间界）
/// 没有任何副作用。
const SNAPSHOT_LEAD_NANOS: i64 = -1_000_000;

/// deferred 形态右事件相对左事件的偏移量：**0（同刻）**。
///
/// 历史：此处曾是 **+1µs**，用来规避「`within` 下界经 f64 取整把同刻右行挤出区间」
/// （epoch-ns ≈1.77e18 超出 f64 精确整数范围 2^53，往返粒度 ~256ns；同刻时右行正压在下界
/// 上，约一半的 `row_ts >= lo` 翻转 → join miss，实测 200 个实体丢 98 个）。
///
/// 该缺陷已于 2026-09-19 两侧先后修好：
/// - 引擎侧：`within` 的界走 `Value::Int` 精确通道
///   （`wf-engine …/executor/context.rs::eval_interval_bound`）；
/// - wfgen/oracle 侧：`time` / `digit` 列字段也按**列式口径**落 `Value::Int`
///   （`oracle::typed_columns` / `json_to_time_column_value` / `json_to_digit_column_value`）；
///
/// 因此现在故意产**同刻**数据：右行正好落在 `within` 闭区间的下界上，
/// 把这个边界（而不是靠 1µs 余量绕过它）持续压在回归护栏下。
/// 实测同刻 50/50 全命中（见 `deferred_same_instant_right_event_fires_all_entities`）。
pub(super) const DEFERRED_OFFSET_NANOS: i64 = 0;

pub(super) fn extract_rule_structure(
    rule_plan: &RulePlan,
    alias_map: &AliasMap,
) -> WfgenResult<RuleStructure> {
    let window_dur = match rule_plan.match_plan.window_spec {
        WindowSpec::Sliding(d) | WindowSpec::Fixed(d) | WindowSpec::Session(d) => d,
        // Hop 的注入窗长 = 窗口大小（slide 步长不改变窗长口径）。
        WindowSpec::Hop { size, .. } => size,
    };

    // 生成器要写进事件的键字段。
    //
    // `key { login = e.sip }` 这类显式 key 映射下，引擎按 **source 字段**取值
    // （`wf-cep::extract_key`：先按 `(逻辑名, 本别名)` 查 key_map 的 `source_field`，
    // 找不到才回退到“逻辑名字段”）；生成器因此也必须写 `source_field`——
    // 写逻辑名会落到一个 **schema 里不存在的列**上，`build_event_fields_with_predicates`
    // 按 schema 逐字段套覆盖，那条覆盖**静默丢弃**，数据里根本不是注入值。
    // 校验期的 `rule_entity_key_fields` 取同一口径（source 字段）。
    let keys: Vec<String> = if let Some(key_map) = &rule_plan.match_plan.key_map {
        dedup_preserving_order(key_map.iter().map(|item| item.source_field.clone()))
    } else {
        dedup_preserving_order(
            rule_plan
                .match_plan
                .keys
                .iter()
                .map(|fr| field_ref_field_name(fr).to_string()),
        )
    };

    let mut steps = Vec::new();
    for step_plan in &rule_plan.match_plan.event_steps {
        // P1: take first branch
        let branch = step_plan
            .branches
            .first()
            .ok_or_else(|| error::error(WfgenReason::Validation, "step has no branches"))?;

        let bind_alias = &branch.source;

        // SC6: inject streams are a *subset* of rule aliases.
        // Skip steps whose bind alias is not covered by inject.
        let (scenario_alias, window_name) = match alias_map.bind_to_scenario.get(bind_alias) {
            Some(pair) => pair,
            None => continue,
        };

        let threshold = eval_const_threshold(&branch.agg.threshold).ok_or_else(|| {
            error::error(
                WfgenReason::Validation,
                "cannot evaluate threshold as constant",
            )
        })? as u64;

        // Extract filter constraints from the corresponding bind
        let mut filter_overrides = rule_plan
            .binds
            .iter()
            .find(|b| b.alias == *bind_alias)
            .and_then(|b| b.filter.as_ref())
            .map(extract_filter_constraints)
            .unwrap_or_default();
        if let Some(guard) = &branch.guard {
            filter_overrides.extend(extract_filter_constraints(guard));
        }

        steps.push(StepInfo {
            bind_alias: bind_alias.clone(),
            scenario_alias: scenario_alias.clone(),
            window_name: window_name.clone(),
            measure: branch.agg.measure,
            threshold,
            filter_overrides,
        });
    }

    // `on each` 规则：没有 match 步骤，注入的"步骤"就是该规则的 each 绑定本身。
    // 无阈值可言——命中的**一条**事件即产出告警（设计 §3.2：断言以实体为单位，
    // 任一路径产出告警即算报警）。
    if steps.is_empty()
        && let Some(each) = &rule_plan.each_plan
        && let Some((scenario_alias, window_name)) = alias_map.bind_to_scenario.get(&each.alias)
    {
        let mut filter_overrides = rule_plan
            .binds
            .iter()
            .find(|b| b.alias == each.alias)
            .and_then(|b| b.filter.as_ref())
            .map(extract_filter_constraints)
            .unwrap_or_default();
        if let Some(filter) = &each.filter {
            filter_overrides.extend(extract_filter_constraints(filter));
        }
        steps.push(StepInfo {
            bind_alias: each.alias.clone(),
            scenario_alias: scenario_alias.clone(),
            window_name: window_name.clone(),
            measure: Measure::Count,
            threshold: 1,
            filter_overrides,
        });
    }

    // stats 规则：没有 match/each 事件步骤，注入的"步骤"就是该规则的**绑定源窗**本身
    // （合成 1 步，与 `validate::syntax::injectable_step_count` 的 stats 口径一致）。
    // 没有阈值可言——注 1 条事件即成桶、窗口收口时必然产出告警，故 threshold = 1
    // （INJ1 的"必报"由桶存在性保证；stats 没有"不命中"的负样本，语料只写 hit）。
    if steps.is_empty()
        && let Some(stats) = &rule_plan.stats_plan
        && let Some(source_alias) = stats
            .measures
            .first()
            .map(|m| m.source_alias.as_str())
            .or_else(|| rule_plan.binds.first().map(|b| b.alias.as_str()))
        && let Some((scenario_alias, window_name)) = alias_map.bind_to_scenario.get(source_alias)
    {
        let filter_overrides = rule_plan
            .binds
            .iter()
            .find(|b| b.alias == source_alias)
            .and_then(|b| b.filter.as_ref())
            .map(extract_filter_constraints)
            .unwrap_or_default();
        steps.push(StepInfo {
            bind_alias: source_alias.to_string(),
            scenario_alias: scenario_alias.clone(),
            window_name: window_name.clone(),
            measure: Measure::Count,
            threshold: 1,
            filter_overrides,
        });
    }

    if steps.is_empty() {
        return error::fail(
            WfgenReason::Validation,
            format!(
                "no inject streams map to any step in rule '{}'; \
                 at least one inject alias must match a rule bind alias",
                rule_plan.name
            ),
        );
    }

    let entity_id_field = extract_entity_id_field(&rule_plan.entity_plan.entity_id_expr);

    // 规则侧 join 口径（设计 §9）：决定用例 `join` 块的右事件放哪。
    let mut joins = Vec::new();
    let mut join_left_fields: Vec<String> = Vec::new();
    for join in &rule_plan.joins {
        let Some(first_cond) = join.conds.first() else {
            continue;
        };
        let Some(right_field) = first_cond.right_field_name().map(str::to_string) else {
            continue;
        };
        let left_field = field_ref_field_name(&first_cond.left).to_string();
        // 驱动侧的连接键：右行的连接键要跟它一致，所以生成器必须把它一起写进驱动事件
        // （`mirror_join_keys`）。空名（非简单字段引用）不入列——引擎/生成器都只
        // 支持字段引用形态的连接条件（其余由 VN30 在校验期拦下）。
        if !left_field.is_empty() && !join_left_fields.iter().any(|f| f == &left_field) {
            join_left_fields.push(left_field.clone());
        }
        // 只登记生成器支持的两种形态（其余由 VN30 在校验期拦下）：
        //  - deferred：`emit at` + `within` → 右事件与左事件**同刻**（见 DEFERRED_OFFSET_NANOS：
        //    正好压在区间下界上，该边界已被两侧的精确整数口径支撑）；
        //  - snapshot：无 `within` 的点查 → 右事件提前（驱动事件处理时必须已可见）。
        let offset_nanos = if join.emit_at.is_some() && join.within.is_some() {
            DEFERRED_OFFSET_NANOS
        } else if matches!(join.mode, wf_lang::ast::JoinMode::Snapshot) && join.within.is_none() {
            SNAPSHOT_LEAD_NANOS
        } else {
            continue;
        };
        joins.push(RuleJoinInfo {
            window: join.right_window.clone(),
            right_field,
            left_field,
            offset_nanos,
        });
    }

    Ok(RuleStructure {
        keys,
        window_dur,
        steps,
        entity_id_field,
        join_left_fields,
        joins,
    })
}

/// Extract a constant numeric value from an expression (L1 thresholds).
pub(crate) fn eval_const_threshold(expr: &Expr) -> Option<f64> {
    match expr {
        Expr::Number(n) => Some(*n),
        Expr::Neg(inner) => eval_const_threshold(inner).map(|v| -v),
        _ => None,
    }
}

pub(crate) fn field_ref_field_name(fr: &FieldRef) -> &str {
    match fr {
        FieldRef::Simple(name) => name,
        FieldRef::Qualified(_, name) | FieldRef::Bracketed(_, name) => name,
        _ => "",
    }
}

/// 保序去重。键字段列表用：`key { login = e.sip; login = b.user }` 这类映射会在
/// 同一字段上重复出现（`source_field` 也可能相同），重复进 `key_overrides` 无意义，
/// 还会把实体 id 的消耗多算一份。
fn dedup_preserving_order<I: Iterator<Item = String>>(items: I) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for item in items {
        if !out.iter().any(|existing| existing == &item) {
            out.push(item);
        }
    }
    out
}

/// 用例 → 生成期覆盖。
///
/// 数量是写下来的：实体个数取用例头，每实体条数逐步取 `x N`（设计 §4.1）。
pub(super) fn extract_syntax_case_overrides(case: &InjectCase) -> WfgenResult<InjectOverrides> {
    let mut use_steps = Vec::with_capacity(case.groups.len());
    for group in &case.groups {
        use_steps.push(InjectUseStepOverrides::cycled(
            group.count,
            source_to_records(&group.source)?,
        ));
    }
    Ok(InjectOverrides {
        entity_field: case.entity_field.clone(),
        entity_count: Some(case.entity_count),
        within: case.spread,
        use_steps,
        joins: case.joins.clone(),
    })
}

/// 事件组的值来源 → 记录列表（一条记录 = 一组字段值）。
///
/// `use from "file"` 必须已由 `loader::resolve_inject_files` 解析成
/// [`ValueSource::Json`]（`gen` / `lint` / `bench` / `send` 都会走
/// `loader::load_from_uses`）；残留的 `File` 会**报错**而不是静默生成空字段。
pub(crate) fn source_to_records(
    source: &ValueSource,
) -> WfgenResult<Vec<HashMap<String, serde_json::Value>>> {
    let records = match source {
        ValueSource::Predicates(predicates) => {
            vec![predicates_to_entries(predicates).into_iter().collect()]
        }
        ValueSource::Json(json) => json_records(json)?,
        ValueSource::File(path) => {
            return error::fail(
                WfgenReason::Validation,
                format!(
                    "use from `{path}` 尚未解析为内联 JSON；请通过 CLI（wfgen gen / lint）加载场景，或先调用 loader::resolve_inject_files 做路径解析"
                ),
            );
        }
    };

    if records.is_empty() {
        return error::fail(
            WfgenReason::Validation,
            "use 的值来源没有任何记录（数组 / NDJSON 文件为空？）",
        );
    }
    Ok(records)
}

/// 已解析的 JSON 值 → 记录列表：object 一条，object 数组多条。
fn json_records(json: &serde_json::Value) -> WfgenResult<Vec<HashMap<String, serde_json::Value>>> {
    match json {
        serde_json::Value::Object(_) => Ok(vec![json_object_to_map(json)]),
        serde_json::Value::Array(items) => items
            .iter()
            .map(|item| {
                if item.is_object() {
                    Ok(json_object_to_map(item))
                } else {
                    error::fail(
                        WfgenReason::Validation,
                        "use 的记录数组元素必须是 JSON object".to_string(),
                    )
                }
            })
            .collect(),
        _ => error::fail(
            WfgenReason::Validation,
            "use 的记录必须是 JSON object 或 object 数组".to_string(),
        ),
    }
}

/// 一条记录：顶层键展开为字段（`_` 前缀的内部键忽略）。
fn json_object_to_map(json: &serde_json::Value) -> HashMap<String, serde_json::Value> {
    crate::wfg_ast::json_top_level_entries(json)
        .unwrap_or_default()
        .into_iter()
        .collect()
}

fn attr_value_to_json(value: &crate::wfg_ast::AttrValue) -> Option<serde_json::Value> {
    match value {
        crate::wfg_ast::AttrValue::Json(v) => Some(v.clone()),
        crate::wfg_ast::AttrValue::String(s) => Some(serde_json::Value::String(s.clone())),
        crate::wfg_ast::AttrValue::Number(n) => Some(number_to_json(*n)),
        crate::wfg_ast::AttrValue::Bool(b) => Some(serde_json::Value::Bool(*b)),
        crate::wfg_ast::AttrValue::Duration(d) => {
            Some(serde_json::Value::String(format!("{:?}", d)))
        }
    }
}

/// 数值面值 → JSON：**整值保持整数**。
///
/// `use(bytes=30000000)` 若落成 `30000000.0`，Arrow 侧 `digit` 列取 `as_i64()`
/// （对浮点返回 `None`）会把它写成 **null**：引擎侧 `sum(bytes)` 恒为 0、阈值永不满足，
/// 而 oracle 直接读 JSON 能强转、断言（INJ1）说“必报”——两者静默分叉。
/// 整数字面量因此必须保持整数形态；非整值（`exponent=1.1` 这类）仍是浮点。
fn number_to_json(n: f64) -> serde_json::Value {
    if n.is_finite() && n.fract() == 0.0 && n.abs() <= i64::MAX as f64 {
        serde_json::Value::from(n as i64)
    } else {
        serde_json::Value::from(n)
    }
}

/// 谓词列表 → `(字段, 期望值)` 列表，与 `use(...)` 共用同一套 `AttrValue` 归一化
/// （避免 `use` 与 `without` 对同一个值产生两种 JSON 形态）。
pub(super) fn predicates_to_entries(
    predicates: &[crate::wfg_ast::FieldPredicate],
) -> Vec<(String, serde_json::Value)> {
    predicates
        .iter()
        .filter_map(|p| attr_value_to_json(&p.value).map(|v| (p.field.clone(), v)))
        .collect()
}

fn extract_entity_id_field(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Field(fr) => Some(field_ref_field_name(fr).to_string()),
        _ => None,
    }
}

/// Extract field equality constraints from a filter expression.
///
/// Supports:
/// - `field == "value"`, `field == number`, `field == bool`
/// - `cond1 && cond2` (recursively extracts from both sides)
pub(crate) fn extract_filter_constraints(filter: &Expr) -> HashMap<String, serde_json::Value> {
    let mut constraints = HashMap::new();
    extract_filter_constraints_recursive(filter, &mut constraints);
    constraints
}

fn extract_filter_constraints_recursive(
    expr: &Expr,
    constraints: &mut HashMap<String, serde_json::Value>,
) {
    if let Expr::BinOp { op, left, right } = expr {
        match op {
            BinOp::And => {
                // Recursively handle AND-connected conditions
                extract_filter_constraints_recursive(left, constraints);
                extract_filter_constraints_recursive(right, constraints);
            }
            BinOp::Eq => {
                // Extract field == value
                if let Expr::Field(fr) = left.as_ref() {
                    let field_name = field_ref_field_name(fr);
                    if let Some(value) = expr_to_json_value(right.as_ref()) {
                        constraints.insert(field_name.to_string(), value);
                    }
                }
                // Also handle value == field
                if let Expr::Field(fr) = right.as_ref() {
                    let field_name = field_ref_field_name(fr);
                    if let Some(value) = expr_to_json_value(left.as_ref()) {
                        constraints.insert(field_name.to_string(), value);
                    }
                }
            }
            _ => {}
        }
    }
}

fn expr_to_json_value(expr: &Expr) -> Option<serde_json::Value> {
    match expr {
        Expr::StringLit(s) => Some(serde_json::Value::String(s.clone())),
        Expr::Number(n) => Some(serde_json::json!(*n)),
        Expr::Bool(b) => Some(serde_json::Value::Bool(*b)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wfg_ast::AttrValue;

    fn case_of(input: &str) -> InjectCase {
        let wfg = crate::wfg_parser::parse_wfg(input).expect("parse");
        wfg.syntax
            .as_ref()
            .and_then(|s| s.injection.as_ref())
            .map(|inj| inj.cases[0].clone())
            .expect("injection case")
    }

    /// `use({...})` 的顶层键必须原样进入 predicates，且值保持嵌套结构；
    /// `_` 前缀的内部字段被忽略。
    #[test]
    fn use_whole_json_expands_to_predicates() {
        let case = case_of(
            r#"
#[duration=1s]
scenario s<seed=1> {
  background { stream sdm_event gen 100/s }
  inject {
    hit<sip: 5> for sdm_rule sdm_event {
      use({
        "tenant_id": "tenant02",
        "source_finding_obj": { "title": "t", "rule": { "label": "账号攻击" } },
        "tags": ["a", "b"],
        "_stream": "ignored"
      }) x 2
    }
  }
}
"#,
        );

        let ov = extract_syntax_case_overrides(&case).expect("extract");
        assert_eq!(ov.entity_field.as_deref(), Some("sip"));
        assert_eq!(ov.entity_count, Some(5));
        assert_eq!(ov.use_steps.len(), 1);
        let step = &ov.use_steps[0];
        assert_eq!(step.count, 2);
        assert_eq!(
            step.records[0].get("tenant_id"),
            Some(&serde_json::json!("tenant02"))
        );
        assert_eq!(
            step.records[0]
                .get("source_finding_obj")
                .and_then(|v| v.pointer("/rule/label")),
            Some(&serde_json::json!("账号攻击"))
        );
        assert_eq!(
            step.records[0].get("tags"),
            Some(&serde_json::json!(["a", "b"]))
        );
        assert!(
            !step.records[0].contains_key("_stream"),
            "`_` 前缀的内部字段必须被忽略"
        );
    }

    /// 旧的按字段覆盖形态保持不变。
    #[test]
    fn use_predicates_unchanged() {
        let case = case_of(
            r#"
#[duration=1s]
scenario s<seed=1> {
  background { stream sdm_event gen 100/s }
  inject {
    hit<sip: 2> for sdm_rule sdm_event { use(tenant_id="t", n=3) x 1 }
  }
}
"#,
        );
        let ov = extract_syntax_case_overrides(&case).expect("extract");
        let step = &ov.use_steps[0];
        assert_eq!(ov.entity_count, Some(2));
        assert_eq!(step.count, 1);
        assert_eq!(
            step.records[0].get("tenant_id"),
            Some(&serde_json::json!("t"))
        );
        // 整数字面量保持整数形态（早期会归一成 `3.0`，经 Arrow 落 `digit` 列时被写成 null）。
        assert_eq!(step.records[0].get("n"), Some(&serde_json::json!(3)));
    }

    /// 整值数字必须保持**整数**形态。
    ///
    /// 落成浮点（`30000000.0`）时，Arrow 的 `digit` 列取 `as_i64()`（浮点 → `None`）
    /// 会把它写成 null：引擎侧 `sum(bytes)` 恒为 0、阈值永不满足，而 oracle 直接读 JSON
    /// 能强转、INJ1 断言说“必报”——oracle 与引擎静默分叉（L3 语料对拍抓到的就是这个）。
    #[test]
    fn integral_numbers_stay_integers() {
        for literal in [30_000_000.0_f64, 22.0, 0.0, -5.0] {
            let json = attr_value_to_json(&AttrValue::Number(literal)).unwrap();
            assert!(json.is_i64(), "整值 {literal} 必须落成整数，实际 {json}");
        }
        assert_eq!(
            attr_value_to_json(&AttrValue::Number(30_000_000.0)),
            Some(serde_json::json!(30_000_000))
        );
    }

    /// 非整值（`exponent=1.1`、`fresh=0.2` 这类）仍是浮点，不能被截断成整数。
    #[test]
    fn fractional_numbers_stay_floats() {
        let json = attr_value_to_json(&AttrValue::Number(1.1)).unwrap();
        assert!(json.is_f64(), "非整值应保持浮点，实际 {json}");
        assert_eq!(json.as_f64(), Some(1.1));
    }
}
