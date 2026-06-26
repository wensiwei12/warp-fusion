use std::fs;
use std::path::Path;

pub fn init_project(project_dir: &str, _name: &str) -> Result<(), String> {
    let root = Path::new(project_dir);

    if root.exists()
        && root
            .read_dir()
            .ok()
            .map(|mut d| d.next().is_some())
            .unwrap_or(false)
    {
        return Err(format!(
            "directory '{}' already exists and is not empty",
            root.display()
        ));
    }

    fs::create_dir_all(root).map_err(|e| format!("create project dir: {e}"))?;

    // ---- Directories ----
    for dir in &[
        "schemas",
        "rules",
        "scenarios",
        "test/sources",
        "test/sinks/connectors/sink.d",
        "test/sinks/business.d",
        "test/sinks/infra.d",
    ] {
        fs::create_dir_all(root.join(dir)).map_err(|e| format!("create {dir}: {e}"))?;
    }

    // ---- Files ----
    let files: Vec<(&str, &str)> = vec![
        (".gitignore", GITIGNORE),
        ("README.md", README),
        ("wfusion.toml", WFUSION_TOML),
        ("schemas/example.wfs", EXAMPLE_WFS),
        ("rules/example.wfl", EXAMPLE_WFL),
        ("scenarios/example.wfg", EXAMPLE_WFG),
        ("test/sources/ingress.toml", TEST_SOURCE),
        ("test/sinks/defaults.toml", SINKS_DEFAULTS),
        ("test/sinks/connectors/sink.d/file.toml", SINKS_CONNECTOR),
        ("test/sinks/business.d/example.toml", SINKS_BUSINESS),
        ("test/sinks/infra.d/default.toml", SINKS_INFRA_DEFAULT),
        ("test/sinks/infra.d/error.toml", SINKS_INFRA_ERROR),
    ];

    for (path, content) in &files {
        let full = root.join(path);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("create parent for {path}: {e}"))?;
        }
        fs::write(&full, content).map_err(|e| format!("write {path}: {e}"))?;
    }

    println!(
        "✓ wf-rules project created at {}",
        root.canonicalize().unwrap_or(root.to_path_buf()).display()
    );
    println!("  cd {} && wfusion run", project_dir);
    Ok(())
}

// ==========================================================================
// Templates (embedded as static strings)
// ==========================================================================

const GITIGNORE: &str = "data/\n";

const README: &str = r##"# wf-rules

Detection rules and test pipeline for WarpFusion CEP engine.

## Quick Start

```bash
# Start engine
wfusion run

# Generate test data
wfgen gen --scenario scenarios/example.wfg --ws schemas/example.wfs --wfl rules/example.wfl --out /tmp/out

# Send to engine
wfgen send --scenario scenarios/example.wfg --input /tmp/out/example.jsonl --ws schemas/example.wfs

# Stream continuously
wfgen stream --scenario-dir scenarios/ --ws schemas/*.wfs --wfl rules/*.wfl --addr 127.0.0.1:9800
```

## Layout

```
schemas/     WFS schema files — define event windows and field types
rules/       WFL rule files — detection logic (match conditions, scoring, yield)
scenarios/   wfgen scenario files — test data generation profiles
test/sinks/  Sink configuration for the test pipeline
test/sources/ Source configuration (TCP ingress)
```
"##;

const WFUSION_TOML: &str = r##"mode = "daemon"
sinks = "test/sinks"

[[sources]]
type = "tcp"
name = "ingress"
addr = "127.0.0.1"
port = "9800"
framing = "len"
data_format = "arrow_framed"

[runtime]
executor_parallelism = 2
rule_exec_timeout = "30s"
schemas = "schemas/*.wfs"
rules = "rules/*.wfl"

[window_defaults]
evict_interval = "30s"
max_window_bytes = "256MB"
max_total_bytes = "2GB"
evict_policy = "time_first"
watermark = "5s"
allowed_lateness = "30m"
late_policy = "drop"

[logging]
level = "debug"
format = "plain"
file = "data/wfusion.log"
"##;

const EXAMPLE_WFS: &str = r##"// Example: authentication events
window auth_events {
    stream = "auth_events"
    time = event_time
    over = 10m
    fields {
        sip: ip
        dip: ip
        user: chars
        service: chars
        result: chars
        event_time: time
    }
}

window security_alerts {
    over = 0
    fields {
        sip: ip
        dip: ip
        user: chars
        alert_type: chars
        detail: chars
    }
}
"##;

const EXAMPLE_WFL: &str = r##"use "example.wfs"

// Detect SSH brute force: >=10 failed login attempts per source IP in 5 min
rule ssh_brute_force {
    events { e : auth_events && e.service == "ssh" && e.result == "failed" }
    match<sip:5m> {
        on event { e | count >= 10; }
    } -> score(70.0)
    entity(ip, e.sip)
    yield security_alerts (
        sip = e.sip,
        dip = e.dip,
        user = e.user,
        alert_type = "ssh_brute_force",
        detail = "failed login >= 10 in 5min"
    )
    limits {
        max_memory = "64MB";
        max_instances = 10000;
        on_exceed = throttle;
    }
}
"##;

const EXAMPLE_WFG: &str = r##"use "../schemas/example.wfs"

#[duration=1m]
scenario ssh_brute {
  traffic { stream auth_events gen 500/s }
  injection {
    hit<20%> auth_events {
      sip fixed(10.0.0.1)
      use(result="failed") with(15,2m)
      use(service="ssh") with(15,0s)
    }
    miss<80%> auth_events {
      use(result="success") with(1,30s)
    }
  }
  expect { hit(ssh_brute_force) >= 80% }
}
"##;

const TEST_SOURCE: &str = r##"# TCP ingress source — used by the test pipeline
"##;

const SINKS_DEFAULTS: &str = "tags = [\"case:wf-rules\"]\n";

const SINKS_CONNECTOR: &str = r##"[[connectors]]
id = "file_json_sink"
type = "file"
allow_override = ["base", "file"]

[connectors.params]
fmt = "json"
base = "data/alerts"
file = "default.ndjson"
"##;

const SINKS_BUSINESS: &str = r##"[sink_group]
name = "security_out"
windows = ["security_alerts"]

[[sink_group.sinks]]
connect = "file_json_sink"
name = "security_file"

[sink_group.sinks.params]
base = "data/alerts"
file = "security.ndjson"
"##;

const SINKS_INFRA_DEFAULT: &str = r##"[sink_group]
name = "__default"

[[sink_group.sinks]]
connect = "file_json_sink"

[sink_group.sinks.params]
base = "data/alerts"
file = "default.ndjson"
"##;

const SINKS_INFRA_ERROR: &str = r##"[sink_group]
name = "__error"

[[sink_group.sinks]]
connect = "file_json_sink"

[sink_group.sinks.params]
base = "data/alerts"
file = "error.ndjson"
"##;
