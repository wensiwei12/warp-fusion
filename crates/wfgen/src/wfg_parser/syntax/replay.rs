//! `replay <window> { use from "<file>" }` 的解析（设计 §8）。
//!
//! 与 `inject` 的用例不同，`replay` 只接受**文件**来源（`use from`）：它就是一条
//! "把这份文件灌进去"的通道。条数由文件决定，所以不写 `x N`（写了报 VN20）。

use winnow::combinator::{cut_err, opt};
use winnow::error::{AddContext, StrContext, StrContextValue};
use winnow::prelude::*;
use winnow::token::literal;

use wf_lang::parse_utils::ident;

use crate::wfg_ast::*;
use crate::wfg_parser::primitives::ws_skip;

use super::inject::parse_value_source;

/// VN20：`replay` 不写条数（文件有多少条发多少条）。
const VN20_REPLAY_COUNT: &str = "VN20 语法错误：`replay` 不写条数——文件有多少条就发多少条。需要按实体定向构造（含 `x N`）请改用 `inject`。";

/// VN20：`replay` 的值来源只能是文件。
const VN20_REPLAY_SOURCE: &str = "VN20 语法错误：`replay` 的值来源只能是文件（`use from \"path\"`）。内联值（`use(...)` / `use({...})`）请改用 `inject`。";

/// 解析 `replay <window> { use from "<file>" }`（调用方已消费 `replay` 关键字）。
pub(crate) fn parse_replay_stmt(input: &mut &str) -> ModalResult<ReplayStmt> {
    ws_skip(input)?;
    let window = cut_err(ident)
        .context(StrContext::Expected(StrContextValue::Description(
            "target stream (window name) after `replay`",
        )))
        .parse_next(input)?
        .to_string();

    ws_skip(input)?;
    cut_err(literal("{"))
        .context(StrContext::Expected(StrContextValue::Description(
            "opening brace of the replay body",
        )))
        .parse_next(input)?;

    ws_skip(input)?;
    cut_err(wf_lang::parse_utils::kw("use"))
        .context(StrContext::Expected(StrContextValue::Description(
            "use from \"<file>\"",
        )))
        .parse_next(input)?;
    ws_skip(input)?;
    let source = cut_err(parse_value_source).parse_next(input)?;
    let ValueSource::File(file) = source else {
        return Err(cut_with(input, VN20_REPLAY_SOURCE));
    };

    ws_skip(input)?;
    // `x N` 是 inject 的条数写法；replay 的条数由文件决定。
    if opt(wf_lang::parse_utils::kw("x"))
        .parse_next(input)?
        .is_some()
    {
        return Err(cut_with(input, VN20_REPLAY_COUNT));
    }
    let _ = opt(literal(";")).parse_next(input)?;

    ws_skip(input)?;
    cut_err(literal("}"))
        .context(StrContext::Expected(StrContextValue::Description(
            "closing brace of the replay body",
        )))
        .parse_next(input)?;

    Ok(ReplayStmt {
        window,
        file,
        records: None,
    })
}

fn cut_with(
    input: &mut &str,
    message: &'static str,
) -> winnow::error::ErrMode<winnow::error::ContextError> {
    winnow::error::ErrMode::Cut(winnow::error::ContextError::new().add_context(
        input,
        &input.checkpoint(),
        StrContext::Expected(StrContextValue::Description(message)),
    ))
}
