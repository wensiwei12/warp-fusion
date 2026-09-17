use std::collections::HashMap;
use std::time::Duration;

use wf_lang::ast::Measure;

use crate::datagen::stream_gen::GenEvent;

/// Result of inject event generation.
pub struct InjectGenResult {
    pub events: Vec<GenEvent>,
    /// Number of inject events per scenario stream alias.
    pub inject_counts: HashMap<String, u64>,
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
