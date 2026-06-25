// ---------------------------------------------------------------------------
// warp-fusion unified CLI error type
// ---------------------------------------------------------------------------

use orion_error::conversion::ConvStructError;
use orion_error::{OrionError, StructError, UnifiedReason};
use wf_runtime::cli::error::{EngineError, EngineReason};
use wfl::error::{WflError, WflReason};

/// Unified CLI error reason wrapping all sub-command error domains.
#[derive(Debug, Clone, PartialEq, OrionError)]
#[allow(dead_code)]
pub enum CliReason {
    #[orion_error(transparent)]
    Engine(EngineReason),
    #[orion_error(transparent)]
    Wfl(WflReason),
    #[orion_error(transparent)]
    General(UnifiedReason),
}

impl From<EngineReason> for CliReason {
    fn from(r: EngineReason) -> Self {
        CliReason::Engine(r)
    }
}

impl From<WflReason> for CliReason {
    fn from(r: WflReason) -> Self {
        CliReason::Wfl(r)
    }
}

pub type CliError = StructError<CliReason>;
pub type CliResult<T> = Result<T, CliError>;

// Conversion helpers (can't use From due to orphan rule)
pub fn into_cli_error(e: EngineError) -> CliError {
    e.conv()
}
pub fn into_cli_error_from_wfl(e: WflError) -> CliError {
    e.conv()
}
