//! wfadm config — inspect and diff wfusion configuration
//!
//! Mirrors `wfusion config` subcommands, relocated to the admin CLI
//! where they belong as project-management tools.

use std::path::{Path, PathBuf};

use clap::Subcommand;
use wf_config::{ConfigResult, ConfigVarContext, FusionConfigLoader, parse_vars};

// ── CLI subcommands ───────────────────────────────────────────────────

#[derive(Subcommand, Clone)]
pub enum ConfigCommands {
    /// Render merged configuration (raw or expanded)
    Render {
        #[arg(short, long, default_value = "conf/wfusion.toml")]
        config: PathBuf,
        #[arg(long)]
        overlay: Vec<PathBuf>,
        #[arg(long)]
        var: Vec<String>,
        #[arg(long)]
        work_dir: Option<PathBuf>,
        #[arg(long)]
        raw: bool,
    },
    /// Show the file origin of each config key
    Origins {
        #[arg(short, long, default_value = "conf/wfusion.toml")]
        config: PathBuf,
        #[arg(long)]
        overlay: Vec<PathBuf>,
        #[arg(long)]
        var: Vec<String>,
        #[arg(long)]
        work_dir: Option<PathBuf>,
        #[arg(long)]
        path_prefix: Vec<String>,
    },
    /// List all resolved configuration variables
    Vars {
        #[arg(short, long, default_value = "conf/wfusion.toml")]
        config: PathBuf,
        #[arg(long)]
        overlay: Vec<PathBuf>,
        #[arg(long)]
        var: Vec<String>,
        #[arg(long)]
        work_dir: Option<PathBuf>,
        #[arg(long)]
        var_prefix: Vec<String>,
    },
    /// Diff two configuration sets
    Diff {
        #[arg(short, long, default_value = "conf/wfusion.toml")]
        config: PathBuf,
        #[arg(long)]
        overlay: Vec<PathBuf>,
        #[arg(long)]
        var: Vec<String>,
        #[arg(long)]
        work_dir: Option<PathBuf>,
        #[arg(long = "to-config")]
        to_config: Option<PathBuf>,
        #[arg(long = "to-overlay")]
        to_overlay: Vec<PathBuf>,
        #[arg(long = "to-var")]
        to_var: Vec<String>,
        #[arg(long = "to-work-dir")]
        to_work_dir: Option<PathBuf>,
        #[arg(long = "path-prefix")]
        path_prefix: Vec<String>,
        #[arg(long)]
        expanded: bool,
    },
}

// ── Runner ────────────────────────────────────────────────────────────

pub fn run(command: ConfigCommands) -> Result<(), String> {
    match command {
        ConfigCommands::Render {
            config,
            overlay,
            var,
            work_dir,
            raw,
        } => cmd_render(&config, &overlay, &var, work_dir.as_deref(), raw),
        ConfigCommands::Origins {
            config,
            overlay,
            var,
            work_dir,
            path_prefix,
        } => cmd_origins(&config, &overlay, &var, work_dir.as_deref(), &path_prefix),
        ConfigCommands::Vars {
            config,
            overlay,
            var,
            work_dir,
            var_prefix,
        } => cmd_vars(&config, &overlay, &var, work_dir.as_deref(), &var_prefix),
        ConfigCommands::Diff {
            config,
            overlay,
            var,
            work_dir,
            to_config,
            to_overlay,
            to_var,
            to_work_dir,
            path_prefix,
            expanded,
        } => cmd_diff(
            &config,
            &overlay,
            &var,
            work_dir.as_deref(),
            to_config.as_deref(),
            &to_overlay,
            &to_var,
            to_work_dir.as_deref(),
            &path_prefix,
            expanded,
        ),
    }
}

// ── Helpers ───────────────────────────────────────────────────────────

struct LoadCtx {
    config_path: PathBuf,
    overlay_paths: Vec<PathBuf>,
    config_ctx: ConfigVarContext,
    base_dir: PathBuf,
}

fn resolve_load(
    config: &Path,
    overlays: &[PathBuf],
    vars: &[String],
    work_dir: Option<&Path>,
) -> Result<LoadCtx, String> {
    let config_path = config
        .canonicalize()
        .map_err(|e| format!("config path '{}': {e}", config.display()))?;
    let overlay_paths: Vec<PathBuf> = overlays
        .iter()
        .map(|p| {
            p.canonicalize()
                .map_err(|e| format!("overlay path '{}': {e}", p.display()))
        })
        .collect::<Result<_, _>>()?;
    let base_dir = if let Some(wd) = work_dir {
        let path = wd
            .canonicalize()
            .map_err(|e| format!("work-dir '{}': {e}", wd.display()))?;
        if !path.is_dir() {
            return Err(format!("work-dir '{}' is not a directory", path.display()));
        }
        path
    } else {
        config_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."))
    };
    let cli_vars = parse_vars(vars).map_err(|e| format!("parse vars: {e}"))?;
    let config_ctx = ConfigVarContext::from_explicit_vars(cli_vars);
    Ok(LoadCtx {
        config_path,
        overlay_paths,
        config_ctx,
        base_dir,
    })
}

fn to_toml_res(e: ConfigResult<String>) -> Result<String, String> {
    e.map_err(|e| format!("{e}"))
}

// ── Render ────────────────────────────────────────────────────────────

fn cmd_render(
    config: &Path,
    overlays: &[PathBuf],
    vars: &[String],
    work_dir: Option<&Path>,
    raw: bool,
) -> Result<(), String> {
    let ctx = resolve_load(config, overlays, vars, work_dir)?;
    let loader = FusionConfigLoader::new(
        &ctx.config_path,
        &ctx.overlay_paths,
        &ctx.config_ctx,
        Some(&ctx.base_dir),
    );
    let rendered = if raw {
        to_toml_res(loader.load_merged_toml())?
    } else {
        to_toml_res(loader.load_expanded_toml())?
    };
    println!("{rendered}");
    Ok(())
}

// ── Origins ───────────────────────────────────────────────────────────

fn cmd_origins(
    config: &Path,
    overlays: &[PathBuf],
    vars: &[String],
    work_dir: Option<&Path>,
    path_prefix: &[String],
) -> Result<(), String> {
    let ctx = resolve_load(config, overlays, vars, work_dir)?;
    let raw = FusionConfigLoader::new(
        &ctx.config_path,
        &ctx.overlay_paths,
        &ctx.config_ctx,
        Some(&ctx.base_dir),
    )
    .load_raw()
    .map_err(|e| format!("{e}"))?;
    let mut matched = 0usize;
    for (path, origin) in raw.origin_entries() {
        if !matches_any_prefix(&path, path_prefix) {
            continue;
        }
        matched += 1;
        println!("{path}\t{}", origin.display());
    }
    if matched == 0 {
        println!("(no matching paths)");
    }
    Ok(())
}

// ── Vars ──────────────────────────────────────────────────────────────

fn cmd_vars(
    config: &Path,
    overlays: &[PathBuf],
    vars: &[String],
    work_dir: Option<&Path>,
    var_prefix: &[String],
) -> Result<(), String> {
    let ctx = resolve_load(config, overlays, vars, work_dir)?;
    let vars = FusionConfigLoader::new(
        &ctx.config_path,
        &ctx.overlay_paths,
        &ctx.config_ctx,
        Some(&ctx.base_dir),
    )
    .load_effective_vars()
    .map_err(|e| format!("{e}"))?;
    let mut matched = 0usize;
    for entry in &vars {
        if !var_prefix.is_empty() && !var_prefix.iter().any(|p| entry.key.starts_with(p.as_str())) {
            continue;
        }
        matched += 1;
        println!("{}\t{}\t{}", entry.key, entry.value, entry.source);
    }
    if matched == 0 {
        println!("(no matching vars)");
    }
    Ok(())
}

// ── Diff ──────────────────────────────────────────────────────────────

fn cmd_diff(
    config: &Path,
    overlays: &[PathBuf],
    vars: &[String],
    work_dir: Option<&Path>,
    to_config: Option<&Path>,
    to_overlays: &[PathBuf],
    to_vars: &[String],
    to_work_dir: Option<&Path>,
    path_prefix: &[String],
    expanded: bool,
) -> Result<(), String> {
    let ctx = resolve_load(config, overlays, vars, work_dir)?;
    let cmp_config = to_config.unwrap_or(&ctx.config_path);
    let cmp_ctx = resolve_load(cmp_config, to_overlays, to_vars, to_work_dir)?;

    let l = FusionConfigLoader::new(
        &ctx.config_path,
        &ctx.overlay_paths,
        &ctx.config_ctx,
        Some(&ctx.base_dir),
    );
    let r = FusionConfigLoader::new(
        &cmp_ctx.config_path,
        &cmp_ctx.overlay_paths,
        &cmp_ctx.config_ctx,
        Some(&cmp_ctx.base_dir),
    );
    let left = if expanded {
        l.load_expanded_raw().map_err(|e| format!("{e}"))?
    } else {
        l.load_raw().map_err(|e| format!("{e}"))?
    };
    let right = if expanded {
        r.load_expanded_raw().map_err(|e| format!("{e}"))?
    } else {
        r.load_raw().map_err(|e| format!("{e}"))?
    };
    let changes: Vec<_> = left
        .diff(&right)
        .into_iter()
        .filter(|c| matches_any_prefix(&c.path, path_prefix))
        .collect();
    if changes.is_empty() {
        println!("(no changes)");
        return Ok(());
    }
    for c in &changes {
        println!("path: {}", c.path);
        println!(
            "  old: {}",
            c.old_value
                .as_ref()
                .map(|v| v.to_string())
                .unwrap_or_else(|| "<none>".to_string())
        );
        println!(
            "  new: {}",
            c.new_value
                .as_ref()
                .map(|v| v.to_string())
                .unwrap_or_else(|| "<none>".to_string())
        );
        println!(
            "  old_origin: {}",
            c.old_origin
                .as_deref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "<none>".to_string())
        );
        println!(
            "  new_origin: {}",
            c.new_origin
                .as_deref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "<none>".to_string())
        );
    }
    Ok(())
}

// ── Prefix matching ───────────────────────────────────────────────────

fn matches_any_prefix(path: &str, prefixes: &[String]) -> bool {
    prefixes.is_empty() || prefixes.iter().any(|p| path_matches_prefix(path, p))
}

fn path_matches_prefix(path: &str, prefix: &str) -> bool {
    path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('.') || rest.starts_with('['))
}
