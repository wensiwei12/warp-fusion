use winnow::combinator::{alt, cut_err, opt};
use winnow::error::{AddContext, StrContext, StrContextValue};
use winnow::prelude::*;
use winnow::token::literal;

use wf_lang::parse_utils::ident;

use crate::wfg_ast::*;
use crate::wfg_parser::primitives::ws_skip;

use super::attrs::parse_attr_value;

/// VN20：旧的按比例形式（`mode<N%>` + `with(N)`）已移除。
///
/// 文案是静态的：winnow 的 `StrContextValue` 只接受 `&'static str`，而百分比
/// 数值不会被错误渲染带出来；等价值本身也需要规则上下文才能计算。
const VN20_LEGACY_PERCENT: &str = "VN20 旧注入语法已移除：`hit<N%>` 里的 N 是 stream 配额的百分比，实际数量由「配额 × 比例 ÷ 每实体条数」推出，与「数量写在用例里」不能共存。请改写为显式的实体个数与每实体条数，例如 `hit<sip: 500> for RULE STREAM { use(...) x 12 }`；实体个数 ≈ round(配额 × N%) ÷ 每实体条数。";
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

    // 可选实体键：`hit<sip: 500>`；`hit<500>` 首字符是数字，`ident` 失败后整体回退。
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
            "entity count (`hit<500>`, or `hit<sip: 500>` with an explicit entity field)",
        )))
        .parse_next(input)? as u64;
    ws_skip(input)?;

    // VN20：旧的按比例形式在语法层就拒绝。检查点放在 `%` 上是因为这里还能
    // 确定「用户写的是百分比」；等价值（`hit<sip: 500>`）需要规则阈值与
    // 每实体条数，属生成期信息，纯语法层给不出，所以文案只给公式与改写形态。
    if opt(literal("%")).parse_next(input)?.is_some() {
        return Err(winnow::error::ErrMode::Cut(
            winnow::error::ContextError::new().add_context(
                input,
                &input.checkpoint(),
                StrContext::Expected(StrContextValue::Description(VN20_LEGACY_PERCENT)),
            ),
        ));
    }

    ws_skip(input)?;
    cut_err(literal(">")).parse_next(input)?;
    parse_explicit_injection_case(input, mode, n, entity_field)
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

    Ok(InjectCase {
        mode,
        entity_count,
        entity_field,
        target_rule,
        stream,
        groups,
        spread,
    })
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
