use std::time::Duration;

use winnow::combinator::{cut_err, opt};
use winnow::error::{AddContext, StrContext, StrContextValue};
use winnow::prelude::*;
use winnow::token::literal;

use crate::wfg_ast::*;
use crate::wfg_parser::primitives::ws_skip;
/// VN20：旧的块关键字 `traffic` 已改名为 `background`（设计 §5.1）。
const VN20_LEGACY_TRAFFIC_KEYWORD: &str = "VN20 旧注入语法已移除：块关键字 `traffic` 已改名为 `background`（只描述背景流量）。请把 `traffic { … }` 改写为 `background { … }`。";

/// VN20：旧的关键字 `injection` 已改名为 `inject`（设计 §1.1 P7 / §5.1）。
const VN20_LEGACY_INJECTION_KEYWORD: &str = "VN20 旧注入语法已移除：块关键字 `injection` 已改名为 `inject`（`background` / `inject` 两个块名成对）。请把 `injection { … }` 改写为 `inject { … }`。";

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
    let mut replays: Vec<ReplayStmt> = Vec::new();

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
        if opt(wf_lang::parse_utils::kw("inject"))
            .parse_next(input)?
            .is_some()
        {
            injection = Some(parse_injection_block(input)?);
            continue;
        }
        // `replay <window> { use from "…" }`：照单发货（设计 §8）；可写多条。
        if opt(wf_lang::parse_utils::kw("replay"))
            .parse_next(input)?
            .is_some()
        {
            replays.push(parse_replay_stmt(input)?);
            continue;
        }
        // VN20：`traffic` 是旧关键字（设计 §5.1 旧语法清单），已改名为 `background`。
        if opt(wf_lang::parse_utils::kw("traffic"))
            .parse_next(input)?
            .is_some()
        {
            return Err(winnow::error::ErrMode::Cut(
                winnow::error::ContextError::new().add_context(
                    input,
                    &input.checkpoint(),
                    StrContext::Expected(StrContextValue::Description(VN20_LEGACY_TRAFFIC_KEYWORD)),
                ),
            ));
        }
        // VN20：`injection` 是旧关键字（设计 §5.1 旧语法清单），已改名为 `inject`。
        // 这里单独接住并给出改写方向——否则用户只会看到"期望 background/inject"。
        if opt(wf_lang::parse_utils::kw("injection"))
            .parse_next(input)?
            .is_some()
        {
            return Err(winnow::error::ErrMode::Cut(
                winnow::error::ContextError::new().add_context(
                    input,
                    &input.checkpoint(),
                    StrContext::Expected(StrContextValue::Description(
                        VN20_LEGACY_INJECTION_KEYWORD,
                    )),
                ),
            ));
        }

        return Err(winnow::error::ErrMode::Cut(
            winnow::error::ContextError::new().add_context(
                input,
                &input.checkpoint(),
                StrContext::Expected(StrContextValue::Description(
                    "background, inject, replay, or closing brace",
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
    };

    let syntax = SyntaxScenario {
        attrs,
        inline_annos,
        background,
        injection,
        replays,
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
mod replay;

pub(super) use attrs::{inline_annos, scenario_attrs};
pub(super) use background::parse_background_block;
pub(super) use inject::parse_injection_block;
pub(super) use replay::parse_replay_stmt;
