use std::collections::HashMap;
use std::time::Duration;

use wf_lang::ast::Measure;

use crate::datagen::stream_gen::GenEvent;
use crate::wfg_ast::{InjectCase, InjectCaseMode};

/// Result of inject event generation.
pub struct InjectGenResult {
    pub events: Vec<GenEvent>,
    /// 注入实体清单（生成期断言 INJ1/INJ2 的输入，设计 §4.2）。
    pub entity_keys: Vec<InjectEntityKey>,
    /// 实体标识无法与 oracle 的 `entity_id` 口径对齐、因而**未**纳入断言的
    /// 实体个数（复合 `entity(...)`、stats 桶键、实体字段不在 schema）。
    pub unasserted_entities: u64,
}

/// 一个注入实体：断言以实体为单位，需要把生成期的实体值与 oracle 告警的
/// `entity_id` 对上。
#[derive(Debug, Clone, PartialEq)]
pub struct InjectEntityKey {
    /// 目标规则（与 [`crate::oracle::OracleAlert::rule_name`] 对齐）。
    pub rule: String,
    pub mode: InjectCaseMode,
    /// 用例内序号（1-based，与错误消息的「第 %d 个实体」一致）。
    pub index: u64,
    /// 标识实体的字段名（诊断展示）。
    pub field: String,
    /// 该字段的注入值；与告警 `entity_id` 比对时按字段值渲染。
    pub value: serde_json::Value,
    /// 该实体在各 bind 上注入了几条、对应阈值多少（INJ1 的原因诊断）。
    pub steps: Vec<InjectStepCount>,
}

/// 一个注入实体在某个 bind 上的条数 / 阈值。
#[derive(Debug, Clone, PartialEq)]
pub struct InjectStepCount {
    pub bind_alias: String,
    pub count: u64,
    pub threshold: u64,
}

/// 注入实体台账：实体清单 + 实体 id 分段。
///
/// 三处（hit / near_miss / miss）逐实体循环共用，避免各自维护一份口径。
/// 条数与背景配额无关（注入是背景之外的额外事件，设计 §3.4），故只记实体。
#[derive(Default)]
pub(super) struct InjectEntities {
    pub(super) keys: Vec<InjectEntityKey>,
    pub(super) unasserted: u64,
    /// 下一个空闲实体 id。各用例按顺序**分段**取用（段间不重叠）：同一个实体
    /// 不可能既是 `hit` 又是 `near_miss`——那是两个互相矛盾的口径。
    next_entity_id: u64,
}

impl InjectEntities {
    /// 本用例的实体 id 段首（= 上一个用例用掉的 id 之后；用完由
    /// [`Self::record_entity`] 推进，不预设段长）。
    pub(super) fn next_entity_base(&self) -> u64 {
        self.next_entity_id
    }

    /// 记录一个注入实体。
    ///
    /// `entity_id` 是该实体在场景键空间里的 id（`>= base`），`index` 是用例内
    /// 序号（1-based）。实体标识取规则 `entity(...)` 的单一字段（与 oracle 告警
    /// `entity_id` 同源）；规则实体是复合表达式、或该字段值不在本次注入的键覆盖
    /// 里时无法对齐口径，计入 `unasserted` 而不是猜一个。
    pub(super) fn record_entity(
        &mut self,
        case: &InjectCase,
        rule_struct: &RuleStructure,
        entity_id: u64,
        index: u64,
        key_overrides: &HashMap<String, serde_json::Value>,
        step_counts: &[u64],
    ) {
        // 先推进段游标：下面任何提前返回都不能让下一个用例复用同一段 id。
        self.next_entity_id = self.next_entity_id.max(entity_id + 1);

        let Some(field) = entity_identity_field(rule_struct) else {
            self.unasserted += 1;
            return;
        };
        let Some(value) = key_overrides.get(&field).cloned() else {
            self.unasserted += 1;
            return;
        };

        let steps = rule_struct
            .steps
            .iter()
            .enumerate()
            .map(|(idx, step)| InjectStepCount {
                bind_alias: step.bind_alias.clone(),
                count: step_counts.get(idx).copied().unwrap_or(0),
                threshold: step.threshold,
            })
            .collect();

        self.keys.push(InjectEntityKey {
            rule: case.target_rule.clone(),
            mode: case.mode,
            index,
            field,
            value,
            steps,
        });
    }
}

/// 实体标识字段 = 规则 `entity(...)` 的单一字段。
///
/// 只有 `entity(<type>, <field>)` 这种单字段口径才能与 oracle 的 `entity_id`
/// 比对；复合表达式（多 key 组合）返回 `None` → 该用例不参与断言。
fn entity_identity_field(rule_struct: &RuleStructure) -> Option<String> {
    rule_struct.entity_id_field.clone()
}

/// Extracted rule structure for inject generation.
#[allow(dead_code)]
pub(super) struct RuleStructure {
    pub(super) keys: Vec<String>,
    pub(super) window_dur: Duration,
    pub(super) steps: Vec<StepInfo>,
    pub(super) entity_id_field: Option<String>,
}

#[derive(Clone)]
#[allow(dead_code)]
pub(super) struct StepInfo {
    pub(super) bind_alias: String,
    pub(super) scenario_alias: String,
    pub(super) window_name: String,
    #[allow(dead_code)]
    pub(super) measure: Measure,
    pub(super) threshold: u64,
    /// Field equality constraints extracted from bind filter.
    /// These override randomly generated values for hit/near_miss events.
    pub(super) filter_overrides: HashMap<String, serde_json::Value>,
}

/// Alias mapping between scenario streams and rule binds.
pub(super) struct AliasMap {
    /// bind_alias -> (scenario_alias, window_name)
    pub(super) bind_to_scenario: HashMap<String, (String, String)>,
}

/// Override parameters extracted from inject line params.
pub(super) struct InjectOverrides {
    /// Entity field named by the injection case (new syntax: `hit<sip: 500>`
    /// or inferred from the rule when omitted).
    pub(super) entity_field: Option<String>,
    /// 显式实体个数（新语法）。`None` = 旧语法，由「配额 × 比例 ÷ 每实体条数」推出。
    pub(super) entity_count: Option<u64>,
    /// Override the window duration for cluster time distribution.
    pub(super) within: Option<Duration>,
    /// Ordered `use(...)` declarations; each declaration maps to one rule step.
    pub(super) use_steps: Vec<InjectUseStepOverrides>,
}

/// Overrides extracted from one `use(...)` clause.
pub(super) struct InjectUseStepOverrides {
    pub(super) count: u64,
    pub(super) predicates: HashMap<String, serde_json::Value>,
}
