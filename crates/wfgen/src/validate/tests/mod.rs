mod field;
mod stream;
mod syntax;

use super::*;
use crate::wfg_ast::*;
use std::time::Duration;
use wf_lang::{BaseType, FieldDef, FieldType, WindowSchema};

/// Helper: build a minimal WfgFile.
fn minimal_wfg(streams: Vec<StreamBlock>) -> WfgFile {
    WfgFile {
        uses: vec![],
        scenario: ScenarioDecl {
            name: "test".into(),
            seed: 1,
            time_clause: TimeClause {
                start: "2024-01-01T00:00:00Z".into(),
                duration: Duration::from_secs(3600),
            },
            total: 100,
            streams,
            faults: None,
        },
        syntax: None,
    }
}

/// Helper: build a WindowSchema.
fn make_schema(name: &str, fields: Vec<(&str, BaseType)>) -> WindowSchema {
    make_schema_with_field_types(
        name,
        fields
            .into_iter()
            .map(|(name, base)| (name, FieldType::Base(base)))
            .collect(),
    )
}

fn make_schema_with_field_types(name: &str, fields: Vec<(&str, FieldType)>) -> WindowSchema {
    WindowSchema {
        name: name.into(),
        streams: vec![],
        time_field: None,
        over: Duration::from_secs(300),
        fields: fields
            .into_iter()
            .map(|(n, field_type)| FieldDef {
                name: n.into(),
                field_type,
            })
            .collect(),
    }
}

/// Helper: build a minimal WflFile with one rule that references given event windows.
///
/// Uses the parser to avoid `#[non_exhaustive]` construction issues.
fn make_wfl(rule_name: &str, event_windows: Vec<(&str, &str)>) -> wf_lang::ast::WflFile {
    make_wfl_match(rule_name, event_windows, "sip", None)
}

/// 同 [`make_wfl`]，但可指定 `match` 的 key 列表（多 key / 消歧用例）与 `entity(...)`
/// 的字段（`None` = 第一个 alias 的 `sip`）。
fn make_wfl_match(
    rule_name: &str,
    event_windows: Vec<(&str, &str)>,
    keys: &str,
    entity_field: Option<&str>,
) -> wf_lang::ast::WflFile {
    let events_str: String = event_windows
        .iter()
        .map(|(alias, window)| format!("        {alias} : {window}"))
        .collect::<Vec<_>>()
        .join("\n");
    let first_alias = event_windows.first().map(|(a, _)| *a).unwrap_or("e");
    let entity_field = entity_field.unwrap_or("sip");
    let wfl_src = format!(
        r#"rule {rule_name} {{
    events {{
{events_str}
    }}
    match<{keys} : 1m> {{
        on event {{
            {first_alias} | count >= 1;
        }}
    }}
    -> score(1)
    entity(ip, {first_alias}.{entity_field})
    yield AlertWindow()
}}"#
    );
    wf_lang::parse_wfl(&wfl_src)
        .unwrap_or_else(|e| panic!("make_wfl parse failed: {e}\nsource:\n{wfl_src}"))
}

/// `on each` 形态（无 match key）：实体字段由 `entity(...)` 的单字段推断（设计 §3.7）。
fn make_wfl_each(rule_name: &str, window: &str, entity_field: &str) -> wf_lang::ast::WflFile {
    let wfl_src = format!(
        r#"rule {rule_name} {{
    events {{
        e : {window}
    }}
    on each e -> score(1)
    entity(ip, e.{entity_field})
    yield AlertWindow()
}}"#
    );
    wf_lang::parse_wfl(&wfl_src)
        .unwrap_or_else(|e| panic!("make_wfl_each parse failed: {e}\nsource:\n{wfl_src}"))
}

fn stream(alias: &str, window: &str) -> StreamBlock {
    StreamBlock {
        alias: alias.into(),
        window: window.into(),
        rate: Rate {
            count: 10,
            unit: RateUnit::PerSecond,
        },
        overrides: vec![],
    }
}

fn stream_with_override(alias: &str, window: &str, field: &str, expr: GenExpr) -> StreamBlock {
    StreamBlock {
        alias: alias.into(),
        window: window.into(),
        rate: Rate {
            count: 10,
            unit: RateUnit::PerSecond,
        },
        overrides: vec![FieldOverride {
            field_name: field.into(),
            gen_expr: expr,
        }],
    }
}
