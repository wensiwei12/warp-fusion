use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, Utc};
use wf_lang::ast::Measure;

use crate::datagen::stream_gen::GenEvent;
use crate::wfg_ast::{InjectCase, InjectCaseMode, JoinStmt};

/// Result of inject event generation.
pub struct InjectGenResult {
    pub events: Vec<GenEvent>,
    /// 注入实体清单（生成期断言 INJ1/INJ2 的输入，设计 §4.2）。
    pub entity_keys: Vec<InjectEntityKey>,
    /// 实体标识无法与 oracle 的 `entity_id` 口径对齐、因而**未**纳入断言的
    /// 实体个数（复合 `entity(...)`、stats 桶键、实体字段不在 schema）。
    pub unasserted_entities: u64,
    /// `without(...)` 约束的执行清单（设计 §3.8）；由背景生成阶段消费。
    pub without_guards: Vec<WithoutGuard>,
}

/// `without(...)` 约束的执行清单（生成期）。
///
/// 每条记录一个**受约束实体**：某个 `field = value` 的实体在其窗口
/// `[start, start + window]` 内不得出现匹配 `predicates` 的事件。
///
/// 口径与引擎对齐：`not has <alias>` 由 `wf-cep` 的 `SeqRuntime` 按**窗口实例**
/// （即实体）扫描（`scan_negations` 只看传到该实例的事件），所以只有“实体键撞上了
/// 注入值”的事件才可能被规则看到。生成期因此按 `(stream, 实体键, 窗口, 谓词)` 四个
/// 条件同时成立才认定违反：
///
/// - 注入事件命中谓词 → **排不掉**（改成不命中就得改 `use(...)` 的值），报生成期错误；
/// - 背景事件命中谓词 → 直接剔除（维持注入条数不变，只拿掉噪声）。
///
/// 不同时命中谓词的背景噪声不剔：它不会违反否定步骤，且剔除会无故偏离背景配额。
#[derive(Debug, Clone, PartialEq)]
pub struct WithoutGuard {
    /// 约束生效的 stream（窗口名，与 [`GenEvent::window_name`] 对齐）。
    pub window_name: String,
    /// 实体键字段（事件按此字段认领归属）。
    pub field: String,
    /// 受约束实体的键值。
    pub value: serde_json::Value,
    /// 窗口起点：该实体**首条注入事件**的时间。
    pub start: DateTime<Utc>,
    /// 窗口长度：`without ... within D` 的 D，省略则取目标规则 `match` 的窗口长度。
    pub window: Duration,
    /// `without(...)` 声明的谓词（字段 → 期望值，已按与 `use(...)` 同一套规则
    /// 从 `AttrValue` 归一化）。
    pub predicates: Vec<(String, serde_json::Value)>,
}

impl WithoutGuard {
    /// 事件是否属于本 guard 的实体、且落在窗口内（闭区间）。
    pub fn covers(&self, event: &GenEvent) -> bool {
        if self.window_name != event.window_name {
            return false;
        }
        if event.fields.get(&self.field) != Some(&self.value) {
            return false;
        }

        let Some(start_ns) = self.start.timestamp_nanos_opt() else {
            return false;
        };
        let window_ns = self.window.as_nanos().min(i64::MAX as u128) as i64;
        let end_ns = start_ns.saturating_add(window_ns);
        let Some(ts_ns) = event.timestamp.timestamp_nanos_opt() else {
            return false;
        };

        ts_ns >= start_ns && ts_ns <= end_ns
    }

    /// 事件是否命中全部 `without(...)` 谓词。字段缺失视为不命中（与引擎把缺失
    /// 字段读成 null / false 一致）。
    pub fn matches_predicates(&self, event: &GenEvent) -> bool {
        self.predicates.iter().all(|(field, expected)| {
            event
                .fields
                .get(field)
                .is_some_and(|actual| json_value_matches(actual, expected))
        })
    }

    /// 违反构造约束：既属于该实体、又落在窗内、还命中谓词。
    pub fn is_violation(&self, event: &GenEvent) -> bool {
        self.covers(event) && self.matches_predicates(event)
    }

    /// 这批事件里第一条违反本约束的（`replay` 的“排不掉”检查用，设计 §8.3）。
    pub fn first_violation<'a>(&self, events: &'a [GenEvent]) -> Option<&'a GenEvent> {
        events.iter().find(|event| self.is_violation(event))
    }

    /// 谓词的可读渲染（`a=1, b="x"`），错误信息用。
    pub fn describe_predicates(&self) -> String {
        self.predicates
            .iter()
            .map(|(field, value)| format!("{field}={value}"))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// JSON 等值比较：数值按 `f64` 比。
///
/// `use(dport=22)` 现在的整数字面量保持整数，但 `use from` 的数据文件里
/// 整数常写成 `22.0` / `3e7`（JSON 合法数字）——直接用 `Value::eq` 会把
/// “22.0 vs 22”判成不等，因此按数值比。
fn json_value_matches(actual: &serde_json::Value, expected: &serde_json::Value) -> bool {
    match (actual.as_f64(), expected.as_f64()) {
        (Some(a), Some(b)) => a == b,
        _ => actual == expected,
    }
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
/// 规则侧的一个 join 子句口径（设计 §9）：与用例的 `join` 块按
/// `(目标窗, 右侧连接键)` 配对，决定右事件该放哪里。
pub(super) struct RuleJoinInfo {
    pub(super) window: String,
    pub(super) right_field: String,
    /// 驱动侧的连接键字段（规则 `on <left> == <right>` 的 left，如 `b.auction`）——
    /// 右行的连接键取它的值，两侧因此指向同一个实体。
    pub(super) left_field: String,
    /// 右事件相对左事件的时间偏移（纳秒），取值见 [`extract_rule_structure`] 的两个常量：
    /// - `DEFERRED_OFFSET_NANOS` = `0`：deferred（`emit at` + `within`）——右行与左行**同刻**，
    ///   正好落在 `within` 闭区间的下界上，把该边界持续压在回归护栏下
    ///   （历史上这里因 f64 界取整丢过一半配对，详见设计文档 §9.4）；
    /// - `SNAPSHOT_LEAD_NANOS` = `-1ms`：snapshot（无 `within`）——右行必须在驱动事件
    ///   被处理时**已可见**；取 1ms（而非 1ns）是为了让毫秒精度的下游（JSONL 的
    ///   `_timestamp`）也看得出先后。
    ///
    /// [`extract_rule_structure`]: super::extract::extract_rule_structure
    pub(super) offset_nanos: i64,
}

pub(super) struct RuleStructure {
    pub(super) keys: Vec<String>,
    pub(super) window_dur: Duration,
    pub(super) steps: Vec<StepInfo>,
    pub(super) entity_id_field: Option<String>,
    /// 规则的 join 子句口径（设计 §9 跨流注入）。
    pub(super) joins: Vec<RuleJoinInfo>,
}

impl RuleStructure {
    /// 生成器用的实体键字段：用例显式写的优先；`on each` 形态没有 match keys，
    /// 用 `entity(...)` 的单一字段推断（设计 §3.7 第三行）。match 规则的推断仍由
    /// [`RuleStructure::keys`] 承担（多 key = 实体是 key 元组，不在这里代入）。
    pub(super) fn effective_entity_field<'a>(
        &'a self,
        explicit: Option<&'a str>,
    ) -> Option<&'a str> {
        match explicit {
            Some(field) => Some(field),
            None if self.keys.is_empty() => self.entity_id_field.as_deref(),
            None => None,
        }
    }
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
    /// `join <window> as <key> { … }` 块（设计 §9 跨流注入）：为规则的 join 目标窗
    /// 造配对事件，右行连接键 = 左实体键值、时间 = 所属左事件时间。
    pub(super) joins: Vec<JoinStmt>,
}

/// Overrides extracted from one `use …` event group.
///
/// 每条事件取用的字段值来自 `records`：`use(preds)` / `use({object})` 只有一条
/// 记录（该步骤所有事件共用）；`use from` 的文件顶层是数组时有多条记录，生成时
/// 按事件序号**循环取用**（`N > 记录数` 回绕，设计 §3.3）。
pub(super) struct InjectUseStepOverrides {
    pub(super) count: u64,
    pub(super) records: Vec<HashMap<String, serde_json::Value>>,
}

impl InjectUseStepOverrides {
    /// 单记录形态（`use(preds)` / `use({object})`）：该步骤所有事件共用一份值。
    /// 生产路径统一走 [`Self::cycled`]（长度 1 的列表同义），这里只给测试用。
    #[cfg(test)]
    pub(super) fn single(count: u64, predicates: HashMap<String, serde_json::Value>) -> Self {
        Self {
            count,
            records: vec![predicates],
        }
    }

    /// 多记录形态（`use from` 的数组 / NDJSON）：按事件序号循环取用。
    pub(super) fn cycled(count: u64, records: Vec<HashMap<String, serde_json::Value>>) -> Self {
        Self { count, records }
    }
}
