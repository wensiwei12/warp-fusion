//! wfadm engine — manage the running wfusion engine via admin API

use std::path::{Path, PathBuf};

use clap::{ArgAction, Subcommand};
use serde::{Deserialize, Serialize};

// ── CLI subcommands ────────────────────────────────────────────────────

#[derive(Subcommand, Clone)]
pub enum EngineCommands {
    /// Query engine runtime status
    Status {
        #[arg(short, long, default_value = "conf/wfusion.toml")]
        config: PathBuf,
        #[arg(long)]
        admin_url: Option<String>,
        #[arg(long)]
        token_file: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Trigger model reload via the daemon admin API
    #[command(disable_version_flag = true)]
    Reload {
        #[arg(short, long, default_value = "conf/wfusion.toml")]
        config: PathBuf,
        #[arg(long)]
        admin_url: Option<String>,
        #[arg(long)]
        token_file: Option<PathBuf>,
        /// Wait for the reload result before returning
        #[arg(long, action = ArgAction::Set, default_value_t = true)]
        wait: bool,
        /// Request timeout while waiting for reload result
        #[arg(long, default_value_t = 15_000)]
        timeout_ms: u64,
        /// Sync managed dirs from `[project_remote]` before reloading
        #[arg(long)]
        update: bool,
        /// Target version for the remote sync (auto-resolved if omitted)
        #[arg(long, requires = "update")]
        version: Option<String>,
        /// Target group for update in dual-repo mode: models or infra
        #[arg(long, requires = "update")]
        group: Option<String>,
        /// Reason string included in daemon logs
        #[arg(long)]
        reason: Option<String>,
        /// Request id to send as X-Request-Id
        #[arg(long)]
        request_id: Option<String>,
        #[arg(long)]
        json: bool,
    },
}

// ── Runner ────────────────────────────────────────────────────────────

pub fn run(command: EngineCommands) -> Result<(), String> {
    match command {
        EngineCommands::Status {
            config,
            admin_url,
            token_file,
            json,
        } => cmd_status(&config, admin_url.as_deref(), token_file.as_deref(), json),
        EngineCommands::Reload {
            config,
            admin_url,
            token_file,
            wait,
            timeout_ms,
            update,
            version,
            group,
            reason,
            request_id,
            json,
        } => cmd_reload(
            &config,
            admin_url.as_deref(),
            token_file.as_deref(),
            wait,
            timeout_ms,
            update,
            version.as_deref(),
            group.as_deref(),
            reason.as_deref(),
            request_id.as_deref(),
            json,
        ),
    }
}

#[derive(Debug, Serialize)]
struct EngineReloadRequest<'a> {
    wait: bool,
    timeout_ms: u64,
    update: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    group: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<&'a str>,
}

#[derive(Debug, Deserialize)]
struct EngineReloadResponse {
    request_id: String,
    accepted: bool,
    result: String,
    update: Option<bool>,
    requested_version: Option<String>,
    current_version: Option<String>,
    resolved_tag: Option<String>,
    group: Option<String>,
    force_replaced: Option<bool>,
    warning: Option<String>,
    error: Option<String>,
}

// ── Helpers ───────────────────────────────────────────────────────────

struct AdminApiTarget {
    base_url: String,
    token: String,
}

fn resolve_target(
    config_path: &Path,
    admin_url: Option<&str>,
    token_file: Option<&Path>,
) -> Result<AdminApiTarget, String> {
    // If admin_url and token_file are explicitly provided, use them
    if let (Some(url), Some(tf)) = (admin_url, token_file) {
        let token = std::fs::read_to_string(tf)
            .map_err(|e| format!("read token file '{}': {e}", tf.display()))?
            .trim()
            .to_string();
        if token.is_empty() {
            return Err(format!("token file '{}' is empty", tf.display()));
        }
        return Ok(AdminApiTarget {
            base_url: url.trim_end_matches('/').to_string(),
            token,
        });
    }

    // Otherwise, read from config file
    let content = std::fs::read_to_string(config_path)
        .map_err(|e| format!("read config '{}': {e}", config_path.display()))?;
    let val: toml::Value = content
        .parse()
        .map_err(|e| format!("parse config TOML: {e}"))?;

    let admin_api = val
        .get("admin_api")
        .ok_or_else(|| "admin_api section not found in config (is it enabled?)".to_string())?;

    let enabled = admin_api
        .get("enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if !enabled {
        return Err("admin_api is not enabled in config".to_string());
    }

    let bind = admin_api
        .get("bind")
        .and_then(|v| v.as_str())
        .unwrap_or("127.0.0.1:19090");
    let base_url = if admin_url.is_none_or(|u| u.is_empty()) {
        format!("http://{bind}")
    } else {
        admin_url.unwrap().trim_end_matches('/').to_string()
    };

    let token_path = admin_api
        .get("auth")
        .and_then(|a| a.get("token_file"))
        .and_then(|v| v.as_str())
        .unwrap_or("${HOME}/.wfusion/admin_api.token");

    // Expand ${HOME} in token path
    let token_path = token_path.replace("${HOME}", &std::env::var("HOME").unwrap_or_default());
    let token = std::fs::read_to_string(&token_path)
        .map_err(|e| format!("read token file '{}': {e}", token_path))?
        .trim()
        .to_string();
    if token.is_empty() {
        return Err(format!("token file '{}' is empty", token_path));
    }

    Ok(AdminApiTarget { base_url, token })
}

// ── Status ────────────────────────────────────────────────────────────

fn cmd_status(
    config_path: &Path,
    admin_url: Option<&str>,
    token_file: Option<&Path>,
    json: bool,
) -> Result<(), String> {
    let target = resolve_target(config_path, admin_url, token_file)?;
    let url = format!("{}/admin/v1/runtime/status", target.base_url);

    let resp = ureq::get(&url)
        .header("Authorization", &format!("Bearer {}", target.token))
        .header("Accept", "application/json")
        .call()
        .map_err(|e| format!("request failed: {e}"))?;

    let status = resp.status();
    let body = resp
        .into_body()
        .read_to_string()
        .map_err(|e| format!("read response: {e}"))?;

    if status != 200 {
        return Err(format!("HTTP {status}: {body}"));
    }

    if json {
        println!("{body}");
        return Ok(());
    }

    // Parse and display nicely
    let val: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| format!("parse response JSON: {e}"))?;

    println!("Engine status");
    println!("  Endpoint  : {}", target.base_url);
    if let Some(id) = val.get("instance_id").and_then(|v| v.as_str()) {
        println!("  Instance  : {id}");
    }
    if let Some(ver) = val.get("version").and_then(|v| v.as_str()) {
        println!("  Version   : {ver}");
    }
    if let Some(acc) = val.get("accepting_commands").and_then(|v| v.as_bool()) {
        println!("  Accepting : {acc}");
    }
    if let Some(reloading) = val.get("reloading").and_then(|v| v.as_bool()) {
        println!("  Reloading : {reloading}");
    }
    if let Some(project_version) = val.get("project_version") {
        println!("  Project V : {project_version}");
    }
    Ok(())
}

// ── Reload ─────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn cmd_reload(
    config_path: &Path,
    admin_url: Option<&str>,
    token_file: Option<&Path>,
    wait: bool,
    timeout_ms: u64,
    update: bool,
    version: Option<&str>,
    group: Option<&str>,
    reason: Option<&str>,
    request_id: Option<&str>,
    json: bool,
) -> Result<(), String> {
    if !update && version.is_some() {
        return Err("--version requires --update".to_string());
    }
    if !update && group.is_some() {
        return Err("--group requires --update".to_string());
    }

    let target = resolve_target(config_path, admin_url, token_file)?;
    let url = format!("{}/admin/v1/reloads/model", target.base_url);

    let body = EngineReloadRequest {
        wait,
        timeout_ms,
        update,
        version,
        group,
        reason,
    };
    let mut req = ureq::post(&url)
        .header("Authorization", &format!("Bearer {}", target.token))
        .header("Accept", "application/json");
    if let Some(request_id) = request_id {
        req = req.header("X-Request-Id", request_id);
    }
    let resp = req
        .send(serde_json::to_string(&body).map_err(|e| format!("encode request: {e}"))?)
        .map_err(|e| format!("request failed: {e}"))?;

    let status = resp.status();
    let resp_body = resp
        .into_body()
        .read_to_string()
        .map_err(|e| format!("read response: {e}"))?;

    if json {
        // Forward the daemon's JSON response verbatim.
        println!("{resp_body}");
    } else {
        let body: EngineReloadResponse =
            serde_json::from_str(&resp_body).map_err(|e| format!("parse response JSON: {e}"))?;
        println!("Engine reload");
        println!("  Endpoint : {}", target.base_url);
        println!("  Request  : {}", body.request_id);
        println!("  Accepted : {}", body.accepted);
        println!("  Result   : {}", body.result);
        if let Some(update) = body.update {
            println!("  Updated  : {update}");
        }
        if let Some(version) = body.requested_version.as_deref() {
            println!("  Request V: {version}");
        }
        if let Some(version) = body.current_version.as_deref() {
            println!("  Current V: {version}");
        }
        if let Some(tag) = body.resolved_tag.as_deref() {
            println!("  Tag      : {tag}");
        }
        if let Some(group) = body.group.as_deref() {
            println!("  Group    : {group}");
        }
        if let Some(force_replaced) = body.force_replaced {
            println!("  Forced   : {force_replaced}");
        }
        if let Some(warning) = body.warning.as_deref() {
            println!("  Warning  : {warning}");
        }
        if let Some(error) = body.error.as_deref() {
            println!("  Error    : {error}");
        }
    }

    if !(status.is_success() || status.as_u16() == 202) {
        return Err(format!("HTTP {status}"));
    }
    Ok(())
}
