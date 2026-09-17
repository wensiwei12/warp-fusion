use std::time::Duration;

use winnow::combinator::{cut_err, opt};
use winnow::error::{AddContext, StrContext, StrContextValue};
use winnow::prelude::*;
use winnow::token::literal;

use crate::wfg_ast::*;
use crate::wfg_parser::primitives::ws_skip;
pub(super) fn parse_syntax_body(
    input: &mut &str,
    name: String,
    attrs: Vec<ScenarioAttr>,
    inline_annos: Vec<ScenarioAttr>,
) -> ModalResult<(ScenarioDecl, SyntaxScenario)> {
    ws_skip(input)?;
    cut_err(literal("{"))
        .context(StrContext::Expected(StrContextValue::Description(
            "opening brace for scenario body",
        )))
        .parse_next(input)?;

    let mut background: Option<BackgroundBlock> = None;
    let mut injection: Option<SyntaxInjectionBlock> = None;

    loop {
        ws_skip(input)?;
        if opt(literal("}")).parse_next(input)?.is_some() {
            break;
        }

        if opt(wf_lang::parse_utils::kw("background"))
            .parse_next(input)?
            .is_some()
        {
            background = Some(parse_background_block(input)?);
            continue;
        }
        if opt(wf_lang::parse_utils::kw("injection"))
            .parse_next(input)?
            .is_some()
        {
            injection = Some(parse_injection_block(input)?);
            continue;
        }

        return Err(winnow::error::ErrMode::Cut(
            winnow::error::ContextError::new().add_context(
                input,
                &input.checkpoint(),
                StrContext::Expected(StrContextValue::Description(
                    "background, injection, or closing brace",
                )),
            ),
        ));
    }

    let Some(background) = background else {
        return Err(winnow::error::ErrMode::Cut(
            winnow::error::ContextError::new().add_context(
                input,
                &input.checkpoint(),
                StrContext::Expected(StrContextValue::Description("background block")),
            ),
        ));
    };

    let seed = extract_seed(&inline_annos).unwrap_or(0);
    let duration = extract_duration(&attrs).unwrap_or_else(|| Duration::from_secs(60));
    let total = derive_total(&background, duration);
    let streams = derive_legacy_streams(&background);

    let scenario = ScenarioDecl {
        name,
        seed,
        time_clause: TimeClause {
            start: "2026-01-01T00:00:00Z".to_string(),
            duration,
        },
        total,
        streams,
        faults: None,
        oracle: None,
    };

    let syntax = SyntaxScenario {
        attrs,
        inline_annos,
        background,
        injection,
    };

    Ok((scenario, syntax))
}

fn extract_seed(inline_annos: &[ScenarioAttr]) -> Option<u64> {
    inline_annos
        .iter()
        .find(|a| a.key == "seed")
        .and_then(|a| match a.value {
            AttrValue::Number(n) if n >= 0.0 => Some(n as u64),
            _ => None,
        })
}

fn extract_duration(attrs: &[ScenarioAttr]) -> Option<Duration> {
    attrs
        .iter()
        .find(|a| a.key == "duration")
        .and_then(|a| match a.value {
            AttrValue::Duration(d) => Some(d),
            _ => None,
        })
}

fn derive_legacy_streams(background: &BackgroundBlock) -> Vec<StreamBlock> {
    background
        .streams
        .iter()
        .map(|s| StreamBlock {
            alias: s.stream.clone(),
            window: s.stream.clone(),
            rate: rate_from_expr(&s.rate),
            overrides: Vec::new(),
        })
        .collect()
}

fn rate_from_expr(rate_expr: &RateExpr) -> Rate {
    match rate_expr {
        RateExpr::Constant(r) => r.clone(),
        RateExpr::Wave { base, .. } => base.clone(),
        RateExpr::Burst { base, .. } => base.clone(),
        RateExpr::Timeline(segments) => segments.first().map(|s| s.rate.clone()).unwrap_or(Rate {
            count: 1,
            unit: RateUnit::PerSecond,
        }),
    }
}

fn derive_total(background: &BackgroundBlock, duration: Duration) -> u64 {
    let eps_sum: f64 = background.streams.iter().map(|s| s.rate.approx_eps()).sum();
    if eps_sum <= 0.0 {
        return 1;
    }
    let total = (eps_sum * duration.as_secs_f64()).round() as u64;
    total.max(1)
}

mod attrs;
mod background;
mod inject;

pub(super) use attrs::{inline_annos, scenario_attrs};
pub(super) use background::parse_background_block;
pub(super) use inject::parse_injection_block;
