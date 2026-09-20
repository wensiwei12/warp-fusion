use winnow::combinator::{cut_err, opt};
use winnow::error::{StrContext, StrContextValue};
use winnow::prelude::*;
use winnow::token::literal;

use wf_lang::parse_utils::{ident, number_literal};

use crate::wfg_ast::*;
use crate::wfg_parser::primitives::ws_skip;
pub(crate) fn scenario_attrs(input: &mut &str) -> ModalResult<Vec<ScenarioAttr>> {
    ws_skip(input)?;
    cut_err(literal("#["))
        .context(StrContext::Expected(StrContextValue::Description(
            "scenario annotation '#['",
        )))
        .parse_next(input)?;
    let attrs = parse_attr_list(input, "]")?;
    cut_err(literal("]"))
        .context(StrContext::Expected(StrContextValue::Description(
            "closing ']' for scenario annotation",
        )))
        .parse_next(input)?;
    Ok(attrs)
}

pub(crate) fn inline_annos(input: &mut &str) -> ModalResult<Vec<ScenarioAttr>> {
    ws_skip(input)?;
    cut_err(literal("<"))
        .context(StrContext::Expected(StrContextValue::Description(
            "opening '<' for inline annotations",
        )))
        .parse_next(input)?;
    let attrs = parse_attr_list(input, ">")?;
    cut_err(literal(">"))
        .context(StrContext::Expected(StrContextValue::Description(
            "closing '>' for inline annotations",
        )))
        .parse_next(input)?;
    Ok(attrs)
}

fn parse_attr_list(input: &mut &str, end_delim: &str) -> ModalResult<Vec<ScenarioAttr>> {
    let mut attrs = Vec::new();
    ws_skip(input)?;
    if input.starts_with(end_delim) {
        return Ok(attrs);
    }

    attrs.push(parse_attr(input)?);
    loop {
        ws_skip(input)?;
        if opt(literal(",")).parse_next(input)?.is_some() {
            ws_skip(input)?;
            attrs.push(parse_attr(input)?);
        } else {
            break;
        }
    }
    Ok(attrs)
}

fn parse_attr(input: &mut &str) -> ModalResult<ScenarioAttr> {
    let key = ident(input)?.to_string();
    ws_skip(input)?;
    cut_err(literal("="))
        .context(StrContext::Expected(StrContextValue::Description(
            "'=' in annotation",
        )))
        .parse_next(input)?;
    ws_skip(input)?;
    let value = parse_attr_value(input)?;
    Ok(ScenarioAttr { key, value })
}

pub(crate) fn parse_attr_value(input: &mut &str) -> ModalResult<AttrValue> {
    // 结构化值：`{...}` / `[...]`（内部是纯 JSON，不是 WFG 语法）。放在最前面，
    // 因为 `{` / `[` 不可能是其他分支的首字符。
    if matches!(input.chars().next(), Some('{') | Some('[')) {
        let json = crate::wfg_parser::primitives::json_container(input)?;
        return Ok(AttrValue::Json(json));
    }

    if let Some(s) = opt(wf_lang::parse_utils::quoted_string).parse_next(input)? {
        return Ok(AttrValue::String(s));
    }

    // Duration is parsed before bare number to avoid consuming `10m` as `10`.
    //
    // 但**必须带单位**才算时长：`wf_lang::parse_utils::duration_value` 为了 `.wfs` 的
    // `over = 0`（静态窗）刻意接受裸 `0`。这里若照收，就会出两个静默错误：
    //   `use(f=0)`   → `Duration::ZERO`，落盘成字符串 `"0ns"`（写的不是 0）；
    //   `<seed=0>`   → 被 VN29 当成“时长”拒掉（而 0 正是 seed 默认值）。
    // 所以要求消费到的文本含单位字母，否则回退按数字解析（`0` → `Number(0)`）。
    let duration_saved = *input;
    if let Ok(d) = wf_lang::parse_utils::duration_value.parse_next(input) {
        let consumed = &duration_saved[..duration_saved.len() - input.len()];
        if consumed.bytes().any(|b| b.is_ascii_alphabetic()) {
            return Ok(AttrValue::Duration(d));
        }
    }
    *input = duration_saved;

    let number_saved = *input;
    if let Ok(n) = number_literal.parse_next(input) {
        return Ok(AttrValue::Number(n));
    }
    *input = number_saved;

    let word = ident(input)?.to_string();
    match word.as_str() {
        "true" => Ok(AttrValue::Bool(true)),
        "false" => Ok(AttrValue::Bool(false)),
        // JSON null（字段写入 null，而不是"字段缺席"）。
        "null" => Ok(AttrValue::Json(serde_json::Value::Null)),
        _ => Ok(AttrValue::String(word)),
    }
}
