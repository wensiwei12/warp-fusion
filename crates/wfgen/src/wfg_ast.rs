use std::time::Duration;

// ---------------------------------------------------------------------------
// Top-level
// ---------------------------------------------------------------------------

/// A complete `.wfg` scenario file.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct WfgFile {
    pub uses: Vec<UseDecl>,
    pub scenario: ScenarioDecl,
    /// Parsed new syntax section when the file uses new stream-first syntax.
    pub syntax: Option<SyntaxScenario>,
}

/// `use "path.wfs"` or `use "path.wfl"`
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct UseDecl {
    pub path: String,
}

// ---------------------------------------------------------------------------
// Scenario
// ---------------------------------------------------------------------------

/// `scenario NAME seed NUMBER { ... }`
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct ScenarioDecl {
    pub name: String,
    pub seed: u64,
    pub time_clause: TimeClause,
    pub total: u64,
    pub streams: Vec<StreamBlock>,
    pub faults: Option<FaultsBlock>,
    pub oracle: Option<OracleBlock>,
}

/// `time "ISO8601" duration DURATION`
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct TimeClause {
    pub start: String,
    pub duration: Duration,
}

// ---------------------------------------------------------------------------
// new syntax (stream-first) extension
// ---------------------------------------------------------------------------

/// new syntax scenario data parsed from the new syntax.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct SyntaxScenario {
    /// `#[key=value, ...]` attributes attached to this scenario.
    pub attrs: Vec<ScenarioAttr>,
    /// `scenario name<k=v, ...>` inline annotations.
    pub inline_annos: Vec<ScenarioAttr>,
    pub background: BackgroundBlock,
    pub injection: Option<SyntaxInjectionBlock>,
}

#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct ScenarioAttr {
    pub key: String,
    pub value: AttrValue,
}

#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum AttrValue {
    Number(f64),
    Duration(Duration),
    String(String),
    Bool(bool),
    /// 结构化值（object / array / null）——`use(field={"a": {"b": 1}})`、
    /// `use(tags=["x"])`、`use(v=null)`。直接持 `serde_json::Value`，避免再
    /// 造一套嵌套枚举，也天然复用 serde_json 的整数/转义语义。
    Json(serde_json::Value),
}

#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct BackgroundBlock {
    pub streams: Vec<SyntaxStreamDecl>,
}

#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct SyntaxStreamDecl {
    pub stream: String,
    pub rate: RateExpr,
}

#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum RateExpr {
    Constant(Rate),
    Wave {
        base: Rate,
        amp: Rate,
        period: Duration,
        shape: WaveShape,
    },
    Burst {
        base: Rate,
        peak: Rate,
        every: Duration,
        hold: Duration,
    },
    Timeline(Vec<TimelineSegment>),
}

impl RateExpr {
    /// EPS approximation used by datagen for total event budgeting.
    pub fn approx_eps(&self) -> f64 {
        match self {
            RateExpr::Constant(r) => r.events_per_second(),
            RateExpr::Wave { base, .. } => base.events_per_second(),
            RateExpr::Burst { base, .. } => base.events_per_second(),
            RateExpr::Timeline(segments) => segments
                .first()
                .map(|s| s.rate.events_per_second())
                .unwrap_or(0.0),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum WaveShape {
    Sine,
    Triangle,
    Square,
}

#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct TimelineSegment {
    pub start: Duration,
    pub end: Duration,
    pub rate: Rate,
}

#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct SyntaxInjectionBlock {
    pub cases: Vec<InjectCase>,
}

/// 注入用例：数量是**写下来的**（设计 §4.1）。
///
/// 旧的按比例形态（`hit<20%> ... with(N)`）已删除：它的数量由「stream 配额 ×
/// 比例 ÷ 每实体条数」推出，与「数量可见」直接冲突。解析期遇到 `mode<N%>`
/// 会报 VN20。
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct InjectCase {
    pub mode: InjectCaseMode,
    /// 实体个数。
    pub entity_count: u64,
    /// 实体标识字段；`None` = 从规则推断（match key / entity 表达式字段）。
    pub entity_field: Option<String>,
    /// 目标规则（必填：`for RULE`）。
    pub target_rule: String,
    pub stream: String,
    /// 按步骤顺序的事件组；每组给出「每实体几条」与「值从哪来」。
    pub groups: Vec<UseGroup>,
    /// `without(...)` 构造约束（设计 §3.8）：该实体在其事件跨度内不得出现匹配的事件。
    ///
    /// 与 `groups` **解耦**（位置无语义）：它不是"一个步骤"、不参与 VN24 的组数口径、
    /// 也不注入事件，故单独存放而不混进有序步骤列表。
    pub withouts: Vec<WithoutStep>,
    /// 时间铺开窗口；`None` = 均匀铺满场景 duration。
    pub spread: Option<Duration>,
}

/// `without(preds) [within D]`：该实体的窗口内**不得出现**匹配 `preds` 的事件。
///
/// 用途：给带否定步骤（`on event seq { … not has x … }`）的规则造"该触发"的数据——
/// 只注入正向事件不够，窗口里一旦落进一条匹配的背景噪声，规则就不会触发。
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct WithoutStep {
    /// 禁止出现的字段等值约束（与 `use(...)` 同形式）。
    pub predicates: Vec<FieldPredicate>,
    /// 判定窗口（自该实体首条注入事件起算）；`None` = 目标规则的 `match` 窗口长度。
    pub within: Option<Duration>,
}

/// 一个事件组（对应规则的一个步骤）。
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct UseGroup {
    /// 每个实体在该步骤上的条数。
    pub count: u64,
    pub source: ValueSource,
}

/// 事件字段值的来源。
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ValueSource {
    /// `use(field=value, ...)`
    Predicates(Vec<FieldPredicate>),
    /// `use({...})` 整份 JSON 内联
    Json(serde_json::Value),
    /// `use from "path"` 整份 JSON 来自文件（相对 `.wfg` 所在目录）
    File(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum InjectCaseMode {
    Hit,
    NearMiss,
    Miss,
}

/// 把整份 JSON 对象的顶层键展开为 `(字段, 值)` 列表。
///
/// - 顶层必须是 object，否则返回 `None`（由调用方报错）；
/// - `_` 前缀的键视为 WFGen 内部字段（`_stream` / `_window` / `_timestamp` 等），
///   直接忽略——用户可以把 WParse 原始输出整份粘进来。
pub fn json_top_level_entries(
    json: &serde_json::Value,
) -> Option<Vec<(String, serde_json::Value)>> {
    json.as_object().map(|map| {
        map.iter()
            .filter(|(k, _)| !k.starts_with('_'))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    })
}

#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct FieldPredicate {
    pub field: String,
    pub value: AttrValue,
}

// ---------------------------------------------------------------------------
// Rate
// ---------------------------------------------------------------------------

/// Event rate, e.g. `100/s`, `50/m`, `10/h`
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Rate {
    pub count: u64,
    pub unit: RateUnit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RateUnit {
    PerSecond,
    PerMinute,
    PerHour,
}

impl Rate {
    /// Convert rate to events per second.
    pub fn events_per_second(&self) -> f64 {
        match self.unit {
            RateUnit::PerSecond => self.count as f64,
            RateUnit::PerMinute => self.count as f64 / 60.0,
            RateUnit::PerHour => self.count as f64 / 3600.0,
        }
    }
}

// ---------------------------------------------------------------------------
// Stream
// ---------------------------------------------------------------------------

/// Stream declaration.
///
/// Supported forms:
/// - `stream ALIAS : WINDOW RATE { field_override* }` (legacy)
/// - `stream ALIAS from WINDOW rate RATE { field_override* }` (readable)
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct StreamBlock {
    pub alias: String,
    pub window: String,
    pub rate: Rate,
    pub overrides: Vec<FieldOverride>,
}

/// `FIELD_NAME = gen_expr`
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct FieldOverride {
    pub field_name: String,
    pub gen_expr: GenExpr,
}

/// Generator expression for a field override.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum GenExpr {
    StringLit(String),
    NumberLit(f64),
    BoolLit(bool),
    GenFunc { name: String, args: Vec<GenArg> },
}

/// A gen function argument, optionally named.
///
/// Supports both positional `ipv4(500)` and named `ipv4(pool: 500)` syntax.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct GenArg {
    pub name: Option<String>,
    pub value: GenExpr,
}

impl GenArg {
    pub fn positional(value: GenExpr) -> Self {
        Self { name: None, value }
    }

    pub fn named(name: impl Into<String>, value: GenExpr) -> Self {
        Self {
            name: Some(name.into()),
            value,
        }
    }
}

// ---------------------------------------------------------------------------
// Faults
// ---------------------------------------------------------------------------

/// `faults { fault_line* }`
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct FaultsBlock {
    pub faults: Vec<FaultLine>,
}

/// Supported fault types for temporal perturbation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum FaultType {
    /// Swap adjacent events' arrival order.
    OutOfOrder,
    /// Delay event arrival position (across watermark boundary).
    Late,
    /// Clone event and insert a duplicate.
    Duplicate,
    /// Remove event from the output stream.
    Drop,
}

impl std::fmt::Display for FaultType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FaultType::OutOfOrder => write!(f, "out_of_order"),
            FaultType::Late => write!(f, "late"),
            FaultType::Duplicate => write!(f, "duplicate"),
            FaultType::Drop => write!(f, "drop"),
        }
    }
}

/// `FAULT_TYPE PERCENT%`
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct FaultLine {
    pub fault_type: FaultType,
    pub percent: f64,
}

// ---------------------------------------------------------------------------
// Oracle
// ---------------------------------------------------------------------------

/// `oracle { param_assigns }`
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct OracleBlock {
    pub params: Vec<ParamAssign>,
}

// ---------------------------------------------------------------------------
// Shared
// ---------------------------------------------------------------------------

/// `NAME = VALUE`
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct ParamAssign {
    pub name: String,
    pub value: ParamValue,
}

/// Value in a parameter assignment.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ParamValue {
    Number(f64),
    Duration(Duration),
    String(String),
}
