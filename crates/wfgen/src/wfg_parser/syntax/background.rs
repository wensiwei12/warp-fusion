use winnow::combinator::{alt, cut_err, opt};
use winnow::error::{AddContext, StrContext, StrContextValue};
use winnow::prelude::*;
use winnow::token::literal;

use wf_lang::parse_utils::ident;

use crate::wfg_ast::*;
use crate::wfg_parser::primitives::{rate, ws_skip};
/// `zipf(...)` 接受的参数集合（未知参数报错文案，设计 §10）。
const ZIPF_UNKNOWN_PARAM: &str = "zipf(...) 只认 `pool` / `exponent` / `fresh` 三个参数";

pub(crate) fn parse_background_block(input: &mut &str) -> ModalResult<BackgroundBlock> {
    ws_skip(input)?;
    cut_err(literal("{"))
        .context(StrContext::Expected(StrContextValue::Description(
            "opening brace for background block",
        )))
        .parse_next(input)?;

    let mut streams = Vec::new();
    let mut entities = Vec::new();
    loop {
        ws_skip(input)?;
        // `entity <window>.<field> zipf(pool=N, exponent=S, fresh=R)`（设计 §10）
        if opt(wf_lang::parse_utils::kw("entity"))
            .parse_next(input)?
            .is_some()
        {
            ws_skip(input)?;
            let window = cut_err(ident)
                .context(StrContext::Expected(StrContextValue::Description(
                    "window name after `entity`",
                )))
                .parse_next(input)?
                .to_string();
            ws_skip(input)?;
            cut_err(literal(".")).parse_next(input)?;
            ws_skip(input)?;
            let field = cut_err(ident)
                .context(StrContext::Expected(StrContextValue::Description(
                    "field name after `entity <window>.`",
                )))
                .parse_next(input)?
                .to_string();
            ws_skip(input)?;
            cut_err(wf_lang::parse_utils::kw("zipf"))
                .context(StrContext::Expected(StrContextValue::Description(
                    "`zipf(...)` after `entity <window>.<field>`",
                )))
                .parse_next(input)?;
            ws_skip(input)?;
            cut_err(literal("(")).parse_next(input)?;
            let mut pool: Option<u64> = None;
            let mut exponent = 1.0f64;
            let mut fresh = 0.0f64;
            loop {
                ws_skip(input)?;
                if opt(literal(")")).parse_next(input)?.is_some() {
                    break;
                }
                let key = cut_err(ident).parse_next(input)?.to_string();
                // 未知参数在下面的 `other` 分支报错（消息里点名允许的三个键）。
                ws_skip(input)?;
                cut_err(literal("=")).parse_next(input)?;
                ws_skip(input)?;
                match key.as_str() {
                    "pool" => {
                        pool = Some(
                            cut_err(wf_lang::parse_utils::nonneg_integer).parse_next(input)? as u64,
                        );
                    }
                    "exponent" => {
                        exponent =
                            cut_err(wf_lang::parse_utils::number_literal).parse_next(input)?;
                    }
                    "fresh" => {
                        fresh = cut_err(wf_lang::parse_utils::number_literal).parse_next(input)?;
                    }
                    _other => {
                        return Err(winnow::error::ErrMode::Cut(
                            winnow::error::ContextError::new().add_context(
                                input,
                                &input.checkpoint(),
                                StrContext::Expected(StrContextValue::Description(
                                    ZIPF_UNKNOWN_PARAM,
                                )),
                            ),
                        ));
                    }
                }
                ws_skip(input)?;
                let _ = opt(literal(",")).parse_next(input)?;
            }
            let Some(pool) = pool else {
                return Err(winnow::error::ErrMode::Cut(
                    winnow::error::ContextError::new().add_context(
                        input,
                        &input.checkpoint(),
                        StrContext::Expected(StrContextValue::Description(
                            "zipf(...) 缺少必填参数 `pool=N`",
                        )),
                    ),
                ));
            };
            ws_skip(input)?;
            let _ = opt(literal(";")).parse_next(input)?;
            entities.push(EntityDistStmt {
                window,
                field,
                pool,
                exponent,
                fresh,
            });
            continue;
        }
        if opt(literal("}")).parse_next(input)?.is_some() {
            break;
        }
        cut_err(wf_lang::parse_utils::kw("stream"))
            .context(StrContext::Expected(StrContextValue::Description(
                "'stream' in background block",
            )))
            .parse_next(input)?;
        ws_skip(input)?;
        let stream = cut_err(ident)
            .context(StrContext::Expected(StrContextValue::Description(
                "stream name",
            )))
            .parse_next(input)?
            .to_string();
        ws_skip(input)?;
        cut_err(wf_lang::parse_utils::kw("gen"))
            .context(StrContext::Expected(StrContextValue::Description(
                "'gen' keyword",
            )))
            .parse_next(input)?;
        ws_skip(input)?;
        let rate_expr = cut_err(parse_rate_expr)
            .context(StrContext::Expected(StrContextValue::Description(
                "rate expression",
            )))
            .parse_next(input)?;
        ws_skip(input)?;
        let _ = opt(literal(";")).parse_next(input)?;

        streams.push(SyntaxStreamDecl {
            stream,
            rate: rate_expr,
        });
    }

    Ok(BackgroundBlock { streams, entities })
}

fn parse_rate_expr(input: &mut &str) -> ModalResult<RateExpr> {
    if opt(wf_lang::parse_utils::kw("wave"))
        .parse_next(input)?
        .is_some()
    {
        return parse_wave(input);
    }
    if opt(wf_lang::parse_utils::kw("burst"))
        .parse_next(input)?
        .is_some()
    {
        return parse_burst(input);
    }
    if opt(wf_lang::parse_utils::kw("timeline"))
        .parse_next(input)?
        .is_some()
    {
        return parse_timeline(input);
    }
    Ok(RateExpr::Constant(rate(input)?))
}

fn parse_wave(input: &mut &str) -> ModalResult<RateExpr> {
    ws_skip(input)?;
    cut_err(literal("(")).parse_next(input)?;
    ws_skip(input)?;
    cut_err(wf_lang::parse_utils::kw("base")).parse_next(input)?;
    cut_err(literal("=")).parse_next(input)?;
    let base = cut_err(rate).parse_next(input)?;
    ws_skip(input)?;
    cut_err(literal(",")).parse_next(input)?;
    ws_skip(input)?;
    cut_err(wf_lang::parse_utils::kw("amp")).parse_next(input)?;
    cut_err(literal("=")).parse_next(input)?;
    let amp = cut_err(rate).parse_next(input)?;
    ws_skip(input)?;
    cut_err(literal(",")).parse_next(input)?;
    ws_skip(input)?;
    cut_err(wf_lang::parse_utils::kw("period")).parse_next(input)?;
    cut_err(literal("=")).parse_next(input)?;
    let period = cut_err(wf_lang::parse_utils::duration_value).parse_next(input)?;

    let mut shape = WaveShape::Sine;
    ws_skip(input)?;
    if opt(literal(",")).parse_next(input)?.is_some() {
        ws_skip(input)?;
        cut_err(wf_lang::parse_utils::kw("shape")).parse_next(input)?;
        cut_err(literal("=")).parse_next(input)?;
        shape = cut_err(alt((
            wf_lang::parse_utils::kw("sine").value(WaveShape::Sine),
            wf_lang::parse_utils::kw("triangle").value(WaveShape::Triangle),
            wf_lang::parse_utils::kw("square").value(WaveShape::Square),
        )))
        .parse_next(input)?;
    }
    ws_skip(input)?;
    cut_err(literal(")")).parse_next(input)?;

    Ok(RateExpr::Wave {
        base,
        amp,
        period,
        shape,
    })
}

fn parse_burst(input: &mut &str) -> ModalResult<RateExpr> {
    ws_skip(input)?;
    cut_err(literal("(")).parse_next(input)?;
    ws_skip(input)?;
    cut_err(wf_lang::parse_utils::kw("base")).parse_next(input)?;
    cut_err(literal("=")).parse_next(input)?;
    let base = cut_err(rate).parse_next(input)?;
    ws_skip(input)?;
    cut_err(literal(",")).parse_next(input)?;
    ws_skip(input)?;
    cut_err(wf_lang::parse_utils::kw("peak")).parse_next(input)?;
    cut_err(literal("=")).parse_next(input)?;
    let peak = cut_err(rate).parse_next(input)?;
    ws_skip(input)?;
    cut_err(literal(",")).parse_next(input)?;
    ws_skip(input)?;
    cut_err(wf_lang::parse_utils::kw("every")).parse_next(input)?;
    cut_err(literal("=")).parse_next(input)?;
    let every = cut_err(wf_lang::parse_utils::duration_value).parse_next(input)?;
    ws_skip(input)?;
    cut_err(literal(",")).parse_next(input)?;
    ws_skip(input)?;
    cut_err(wf_lang::parse_utils::kw("hold")).parse_next(input)?;
    cut_err(literal("=")).parse_next(input)?;
    let hold = cut_err(wf_lang::parse_utils::duration_value).parse_next(input)?;
    ws_skip(input)?;
    cut_err(literal(")")).parse_next(input)?;

    Ok(RateExpr::Burst {
        base,
        peak,
        every,
        hold,
    })
}

fn parse_timeline(input: &mut &str) -> ModalResult<RateExpr> {
    ws_skip(input)?;
    cut_err(literal("{")).parse_next(input)?;
    let mut segments = Vec::new();
    loop {
        ws_skip(input)?;
        if opt(literal("}")).parse_next(input)?.is_some() {
            break;
        }
        let start = cut_err(wf_lang::parse_utils::duration_value).parse_next(input)?;
        ws_skip(input)?;
        cut_err(literal("..")).parse_next(input)?;
        ws_skip(input)?;
        let end = cut_err(wf_lang::parse_utils::duration_value).parse_next(input)?;
        ws_skip(input)?;
        cut_err(literal("=")).parse_next(input)?;
        ws_skip(input)?;
        let seg_rate = cut_err(rate).parse_next(input)?;
        ws_skip(input)?;
        let _ = opt(literal(";")).parse_next(input)?;
        segments.push(TimelineSegment {
            start,
            end,
            rate: seg_rate,
        });
    }
    Ok(RateExpr::Timeline(segments))
}
