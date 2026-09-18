mod primitives;
mod scenario;
mod syntax;
#[cfg(test)]
mod tests;

use winnow::combinator::{cut_err, opt};
use winnow::error::{StrContext, StrContextValue};
use winnow::prelude::*;

use wf_lang::parse_utils::quoted_string;

use crate::error::{self, WfgenReason, WfgenResult};
use crate::wfg_ast::*;

use self::primitives::ws_skip;
use self::scenario::scenario_decl;

// ---------------------------------------------------------------------------
// Top-level
// ---------------------------------------------------------------------------

/// Parse a `.wfg` scenario file from a string.
pub fn parse_wfg(input: &str) -> WfgenResult<WfgFile> {
    let mut rest = input;
    let result = wfg_file(&mut rest)
        .map_err(|e| error::error(WfgenReason::Parse, format!("parse error: {e}")))?;

    ws_skip(&mut rest)
        .map_err(|e| error::error(WfgenReason::Parse, format!("parse error: {e}")))?;
    if !rest.is_empty() {
        return error::fail(
            WfgenReason::Parse,
            format!(
                "unexpected trailing content: {:?}",
                &rest[..rest.len().min(60)]
            ),
        );
    }
    Ok(result)
}

/// 覆盖场景时长（`wfgen gen --duration`）。
///
/// 必须在**校验之前**调用：`spread` / `without ... within` / `replay` 跨度的 VN25 都按场景
/// 时长判定，覆盖后应按**生效时长**判定；同时 `scenario.total`（背景条数的推导基数，
/// `datagen` 用它决定每流条数）也要跟着重算——只改 `duration` 会让背景仍按文件里的旧时长生成。
pub fn override_duration(wfg: &mut WfgFile, duration: std::time::Duration) {
    wfg.scenario.time_clause.duration = duration;
    if let Some(syntax) = &wfg.syntax {
        wfg.scenario.total = syntax::derive_total(&syntax.background, duration);
    }
}

fn wfg_file(input: &mut &str) -> ModalResult<WfgFile> {
    let mut uses = Vec::new();
    loop {
        ws_skip(input)?;
        if opt(wf_lang::parse_utils::kw("use"))
            .parse_next(input)?
            .is_some()
        {
            ws_skip(input)?;
            let path = cut_err(quoted_string)
                .context(StrContext::Expected(StrContextValue::Description(
                    "quoted path after 'use'",
                )))
                .parse_next(input)?;
            uses.push(UseDecl { path });
        } else {
            break;
        }
    }

    ws_skip(input)?;
    let parsed = scenario_decl(input)?;

    Ok(WfgFile {
        uses,
        scenario: parsed.scenario,
        syntax: parsed.syntax,
    })
}
