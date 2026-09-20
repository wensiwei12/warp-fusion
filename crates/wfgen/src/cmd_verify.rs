use std::path::PathBuf;

use orion_error::conversion::SourceErr;

use crate::error::{WfgenReason, WfgenResult};
use crate::oracle::OracleTolerances;
use crate::output::jsonl::{read_alerts_jsonl, read_oracle_jsonl};
use crate::verify::{EmptyPolicy, verify};

/// `wfgen verify` 参数：比对实际 alerts 与 oracle 期望。
#[derive(clap::Args)]
pub struct Args {
    /// Path to the oracle (expected) JSONL file
    #[arg(long)]
    pub expected: PathBuf,

    /// Path to the actual alerts JSONL file
    #[arg(long)]
    pub actual: PathBuf,

    /// Score tolerance for matching (overrides meta file if set)
    #[arg(long)]
    pub score_tolerance: Option<f64>,

    /// Time tolerance for matching in seconds (overrides meta file if set)
    #[arg(long)]
    pub time_tolerance: Option<f64>,

    /// Path to oracle meta JSON with tolerances (written by gen)
    #[arg(long)]
    pub meta: Option<PathBuf>,

    /// Output format: "json" or "markdown" (default: json)
    #[arg(long, default_value = "json")]
    pub format: String,

    /// Accept a comparison with no evidence (both sides empty) as a pass.
    ///
    /// Without this flag an empty-on-both-sides comparison exits non-zero: an
    /// empty comparison has no diffs, so it used to look like a green light
    /// even when expectation generation or the assertions never ran.
    #[arg(long)]
    pub allow_empty: bool,
}

pub fn run(args: Args) -> WfgenResult<()> {
    let Args {
        expected,
        actual,
        score_tolerance,
        time_tolerance,
        meta,
        format,
        allow_empty,
    } = args;
    // Load tolerances: CLI flags > meta file > defaults
    let base_tolerances = if let Some(meta_path) = &meta {
        let content = std::fs::read_to_string(meta_path).source_err(
            WfgenReason::Io,
            format!("reading meta: {}", meta_path.display()),
        )?;
        serde_json::from_str::<OracleTolerances>(&content).source_err(
            WfgenReason::Serialization,
            format!("parsing meta: {}", meta_path.display()),
        )?
    } else {
        OracleTolerances::default()
    };

    let effective_score_tol = score_tolerance.unwrap_or(base_tolerances.score_tolerance);
    let effective_time_tol = time_tolerance.unwrap_or(base_tolerances.time_tolerance_secs);

    let oracle_alerts = read_oracle_jsonl(&expected)?;
    let actual_alerts = read_alerts_jsonl(&actual)?;

    let report = verify(
        &oracle_alerts,
        &actual_alerts,
        effective_score_tol,
        effective_time_tol,
        if allow_empty {
            EmptyPolicy::Allow
        } else {
            EmptyPolicy::Deny
        },
    );

    match format.as_str() {
        "markdown" | "md" => {
            println!("{}", report.to_markdown());
        }
        _ => {
            let json = serde_json::to_string_pretty(&report)
                .source_err(WfgenReason::Serialization, "serializing verify report")?;
            println!("{}", json);
        }
    }

    if report.status == "pass" {
        std::process::exit(0);
    } else {
        std::process::exit(1);
    }
}
