use winnow::combinator::{alt, cut_err, opt};
use winnow::error::{AddContext, StrContext, StrContextValue};
use winnow::prelude::*;
use winnow::token::literal;

use wf_lang::parse_utils::ident;

use crate::wfg_ast::*;
use crate::wfg_parser::primitives::ws_skip;

use super::attrs::parse_attr_value;
pub(crate) fn parse_injection_block(input: &mut &str) -> ModalResult<SyntaxInjectionBlock> {
    ws_skip(input)?;
    cut_err(literal("{"))
        .context(StrContext::Expected(StrContextValue::Description(
            "opening brace for injection block",
        )))
        .parse_next(input)?;
    let mut cases = Vec::new();
    loop {
        ws_skip(input)?;
        if opt(literal("}")).parse_next(input)?.is_some() {
            break;
        }
        cases.push(parse_injection_case(input)?);
    }
    Ok(SyntaxInjectionBlock { cases })
}

fn parse_injection_case(input: &mut &str) -> ModalResult<InjectCase> {
    let mode = alt((
        wf_lang::parse_utils::kw("hit").value(InjectCaseMode::Hit),
        wf_lang::parse_utils::kw("near_miss").value(InjectCaseMode::NearMiss),
        wf_lang::parse_utils::kw("miss").value(InjectCaseMode::Miss),
    ))
    .context(StrContext::Expected(StrContextValue::Description(
        "injection mode (hit, near_miss, miss)",
    )))
    .parse_next(input)?;
    ws_skip(input)?;
    cut_err(literal("<")).parse_next(input)?;
    ws_skip(input)?;

    // 可选实体键：`hit<sip: 500>`。`hit<100%>` 首字符是数字 → 整体回退。
    let saved = *input;
    let mut entity_field: Option<String> = None;
    if let Ok(name) = ident(input) {
        ws_skip(input)?;
        if opt(literal(":")).parse_next(input)?.is_some() {
            ws_skip(input)?;
            entity_field = Some(name.to_string());
        } else {
            *input = saved;
        }
    } else {
        *input = saved;
    }

    let n = cut_err(wf_lang::parse_utils::nonneg_integer)
        .context(StrContext::Expected(StrContextValue::Description(
            "entity count (new syntax) or percentage (legacy syntax)",
        )))
        .parse_next(input)? as u64;
    ws_skip(input)?;
    let is_percent = opt(literal("%")).parse_next(input)?.is_some();
    ws_skip(input)?;
    cut_err(literal(">")).parse_next(input)?;

    if is_percent {
        if entity_field.is_some() {
            return Err(winnow::error::ErrMode::Cut(
                winnow::error::ContextError::new().add_context(
                    input,
                    &input.checkpoint(),
                    StrContext::Expected(StrContextValue::Description(
                        "legacy `mode<percent%>` form takes no entity field",
                    )),
                ),
            ));
        }
        parse_legacy_injection_case(input, mode, n as f64, saved)
    } else {
        parse_explicit_injection_case(input, mode, n, entity_field)
    }
}

/// 旧形态体：`[for RULE] STREAM { FIELD seq { ... } }`
fn parse_legacy_injection_case(
    input: &mut &str,
    mode: InjectCaseMode,
    percent: f64,
    _saved: &str,
) -> ModalResult<InjectCase> {
    ws_skip(input)?;
    let target_rule = if opt(wf_lang::parse_utils::kw("for"))
        .parse_next(input)?
        .is_some()
    {
        ws_skip(input)?;
        Some(
            cut_err(ident)
                .context(StrContext::Expected(StrContextValue::Description(
                    "target rule name in injection case",
                )))
                .parse_next(input)?
                .to_string(),
        )
    } else {
        None
    };
    ws_skip(input)?;
    let stream = cut_err(ident)
        .context(StrContext::Expected(StrContextValue::Description(
            "stream name in injection case",
        )))
        .parse_next(input)?
        .to_string();
    ws_skip(input)?;
    cut_err(literal("{")).parse_next(input)?;
    ws_skip(input)?;
    let seq = cut_err(parse_seq_block).parse_next(input)?;
    ws_skip(input)?;
    cut_err(literal("}")).parse_next(input)?;
    Ok(InjectCase::Legacy(LegacyInjectCase {
        mode,
        percent,
        target_rule,
        stream,
        seq,
    }))
}

/// 新形态体：`for RULE STREAM { use ... x N [spread D] }`
fn parse_explicit_injection_case(
    input: &mut &str,
    mode: InjectCaseMode,
    entity_count: u64,
    entity_field: Option<String>,
) -> ModalResult<InjectCase> {
    ws_skip(input)?;
    cut_err(wf_lang::parse_utils::kw("for"))
        .context(StrContext::Expected(StrContextValue::Description(
            "`for RULE` after `mode<count>` (required in the new syntax)",
        )))
        .parse_next(input)?;
    ws_skip(input)?;
    let target_rule = cut_err(ident)
        .context(StrContext::Expected(StrContextValue::Description(
            "target rule name in injection case",
        )))
        .parse_next(input)?
        .to_string();
    ws_skip(input)?;
    let stream = cut_err(ident)
        .context(StrContext::Expected(StrContextValue::Description(
            "stream name in injection case",
        )))
        .parse_next(input)?
        .to_string();
    ws_skip(input)?;
    cut_err(literal("{")).parse_next(input)?;

    let mut groups = Vec::new();
    let mut spread = None;
    loop {
        ws_skip(input)?;
        if opt(literal("}")).parse_next(input)?.is_some() {
            break;
        }
        if opt(wf_lang::parse_utils::kw("spread"))
            .parse_next(input)?
            .is_some()
        {
            ws_skip(input)?;
            spread = Some(cut_err(wf_lang::parse_utils::duration_value).parse_next(input)?);
            ws_skip(input)?;
            let _ = opt(literal(";")).parse_next(input)?;
            continue;
        }
        let _ = opt(wf_lang::parse_utils::kw("then")).parse_next(input)?;
        ws_skip(input)?;
        cut_err(wf_lang::parse_utils::kw("use"))
            .context(StrContext::Expected(StrContextValue::Description(
                "use(...) / use({...}) / use from <file> event group",
            )))
            .parse_next(input)?;
        let source = parse_value_source(input)?;
        ws_skip(input)?;
        cut_err(wf_lang::parse_utils::kw("x"))
            .context(StrContext::Expected(StrContextValue::Description(
                "`x N` (events per entity for this step) after the value source",
            )))
            .parse_next(input)?;
        ws_skip(input)?;
        let count = cut_err(wf_lang::parse_utils::nonneg_integer).parse_next(input)? as u64;
        ws_skip(input)?;
        let _ = opt(literal(";")).parse_next(input)?;
        groups.push(UseGroup { count, source });
    }

    Ok(InjectCase::Explicit(ExplicitInjectCase {
        mode,
        entity_count,
        entity_field,
        target_rule,
        stream,
        groups,
        spread,
    }))
}

/// 事件字段值的来源：`(preds)` / `({json})` / `from "path"`
fn parse_value_source(input: &mut &str) -> ModalResult<ValueSource> {
    ws_skip(input)?;
    if opt(wf_lang::parse_utils::kw("from"))
        .parse_next(input)?
        .is_some()
    {
        ws_skip(input)?;
        let path = cut_err(wf_lang::parse_utils::quoted_string)
            .context(StrContext::Expected(StrContextValue::Description(
                "JSON/NDJSON file path after `from`",
            )))
            .parse_next(input)?;
        return Ok(ValueSource::File(path));
    }
    cut_err(literal("(")).parse_next(input)?;
    ws_skip(input)?;
    let source = if input.starts_with('{') {
        ValueSource::Json(crate::wfg_parser::primitives::json_container(input)?)
    } else {
        ValueSource::Predicates(parse_predicates(input)?)
    };
    ws_skip(input)?;
    cut_err(literal(")")).parse_next(input)?;
    Ok(source)
}

fn parse_seq_block(input: &mut &str) -> ModalResult<SeqBlock> {
    let entity = cut_err(ident)
        .context(StrContext::Expected(StrContextValue::Description(
            "entity key for seq",
        )))
        .parse_next(input)?
        .to_string();
    ws_skip(input)?;
    cut_err(wf_lang::parse_utils::kw("seq"))
        .context(StrContext::Expected(StrContextValue::Description(
            "'seq' keyword",
        )))
        .parse_next(input)?;
    ws_skip(input)?;
    cut_err(literal("{")).parse_next(input)?;
    let mut steps = Vec::new();
    loop {
        ws_skip(input)?;
        if opt(literal("}")).parse_next(input)?.is_some() {
            break;
        }
        steps.push(parse_seq_step(input)?);
    }
    Ok(SeqBlock { entity, steps })
}

fn parse_seq_step(input: &mut &str) -> ModalResult<SeqStep> {
    if opt(wf_lang::parse_utils::kw("then"))
        .parse_next(input)?
        .is_some()
    {
        ws_skip(input)?;
        cut_err(wf_lang::parse_utils::kw("use"))
            .context(StrContext::Expected(StrContextValue::Description(
                "'use' after 'then'",
            )))
            .parse_next(input)?;
        return parse_use_step_after_keyword(input);
    }

    if opt(wf_lang::parse_utils::kw("use"))
        .parse_next(input)?
        .is_some()
    {
        return parse_use_step_after_keyword(input);
    }

    if opt(wf_lang::parse_utils::kw("not"))
        .parse_next(input)?
        .is_some()
    {
        ws_skip(input)?;
        cut_err(literal("(")).parse_next(input)?;
        let predicates = parse_predicates(input)?;
        cut_err(literal(")")).parse_next(input)?;
        ws_skip(input)?;
        cut_err(wf_lang::parse_utils::kw("within")).parse_next(input)?;
        ws_skip(input)?;
        cut_err(literal("(")).parse_next(input)?;
        ws_skip(input)?;
        let within = cut_err(wf_lang::parse_utils::duration_value).parse_next(input)?;
        ws_skip(input)?;
        cut_err(literal(")")).parse_next(input)?;
        ws_skip(input)?;
        let _ = opt(literal(";")).parse_next(input)?;
        return Ok(SeqStep::Not { predicates, within });
    }

    Err(winnow::error::ErrMode::Cut(
        winnow::error::ContextError::new().add_context(
            input,
            &input.checkpoint(),
            StrContext::Expected(StrContextValue::Description(
                "use(...) or not(...) seq step",
            )),
        ),
    ))
}

fn parse_use_step_after_keyword(input: &mut &str) -> ModalResult<SeqStep> {
    ws_skip(input)?;
    cut_err(literal("(")).parse_next(input)?;
    // `(` 之后允许换行/缩进——整份 JSON 内联时必然是多行排版。
    ws_skip(input)?;
    let payload = parse_use_payload(input)?;
    ws_skip(input)?;
    cut_err(literal(")")).parse_next(input)?;
    ws_skip(input)?;
    cut_err(wf_lang::parse_utils::kw("with")).parse_next(input)?;
    ws_skip(input)?;
    cut_err(literal("(")).parse_next(input)?;
    ws_skip(input)?;
    let count = cut_err(wf_lang::parse_utils::nonneg_integer).parse_next(input)? as u64;
    ws_skip(input)?;
    cut_err(literal(")")).parse_next(input)?;
    ws_skip(input)?;
    let _ = opt(literal(";")).parse_next(input)?;
    Ok(match payload {
        UsePayload::Predicates(predicates) => SeqStep::Use { predicates, count },
        UsePayload::Json(json) => SeqStep::UseJson { json, count },
    })
}

/// `use(...)` 的载荷：按字段覆盖，或整份 JSON 内联（`{` 开头）。
enum UsePayload {
    Predicates(Vec<FieldPredicate>),
    Json(serde_json::Value),
}

fn parse_use_payload(input: &mut &str) -> ModalResult<UsePayload> {
    if input.starts_with('{') {
        let json = crate::wfg_parser::primitives::json_container(input)?;
        return Ok(UsePayload::Json(json));
    }
    Ok(UsePayload::Predicates(parse_predicates(input)?))
}

fn parse_predicates(input: &mut &str) -> ModalResult<Vec<FieldPredicate>> {
    let mut predicates = Vec::new();
    predicates.push(parse_predicate(input)?);
    loop {
        ws_skip(input)?;
        if opt(literal(",")).parse_next(input)?.is_some() {
            ws_skip(input)?;
            predicates.push(parse_predicate(input)?);
        } else {
            break;
        }
    }
    Ok(predicates)
}

fn parse_predicate(input: &mut &str) -> ModalResult<FieldPredicate> {
    let field = cut_err(ident)
        .context(StrContext::Expected(StrContextValue::Description(
            "predicate field",
        )))
        .parse_next(input)?
        .to_string();
    ws_skip(input)?;
    cut_err(literal("=")).parse_next(input)?;
    ws_skip(input)?;
    let value = parse_attr_value(input)?;
    Ok(FieldPredicate { field, value })
}
