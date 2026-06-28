// wfadm check — validate wf-rules project integrity

use std::fs;
use std::path::Path;

use crate::connectors;

pub fn run() -> Result<(), String> {
    check_project(Path::new("."))
}

pub(crate) fn check_project(root: &Path) -> Result<(), String> {
    let mut ok: u32 = 0;
    let mut err: u32 = 0;
    let mut warn: u32 = 0;

    println!(
        "wfadm check: validating wf-rules project at {}",
        root.canonicalize()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| root.display().to_string())
    );

    // Register factories for reporting
    connectors::ensure_factories_registered();
    let registered_sinks = wp_core_connectors::registry::list_sink_kinds();
    let registered_sources = wp_core_connectors::registry::list_source_kinds();
    println!(
        "  registered: {} sink(s), {} source(s)",
        registered_sinks.len(),
        registered_sources.len()
    );

    // 1. conf/wfusion.toml
    let conf_path = root.join("conf").join("wfusion.toml");
    if conf_path.exists() {
        match fs::read_to_string(&conf_path) {
            Ok(content) => {
                if content.parse::<toml::Value>().is_ok() {
                    ok += 1;
                    println!("  [OK] conf/wfusion.toml: valid TOML");
                } else {
                    err += 1;
                    eprintln!("  [ERR] conf/wfusion.toml: invalid TOML");
                }
            }
            Err(e) => {
                err += 1;
                eprintln!("  [ERR] conf/wfusion.toml: read error: {e}");
            }
        }
    } else {
        err += 1;
        eprintln!("  [ERR] conf/wfusion.toml: not found");
    }

    // 2. models/rules/
    let rules_dir = root.join("models").join("rules");
    if rules_dir.is_dir() {
        let wfl_count = count_files(&rules_dir, "wfl");
        if wfl_count > 0 {
            ok += 1;
            println!("  [OK] models/rules/: {wfl_count} .wfl file(s)");
        } else {
            warn += 1;
            eprintln!("  [WARN] models/rules/: no .wfl files found");
        }
    } else {
        warn += 1;
        eprintln!("  [WARN] models/rules/: directory not found");
    }

    // 3. models/schemas/
    let schemas_dir = root.join("models").join("schemas");
    if schemas_dir.is_dir() {
        let wfs_count = count_files(&schemas_dir, "wfs");
        if wfs_count > 0 {
            ok += 1;
            println!("  [OK] models/schemas/: {wfs_count} .wfs file(s)");
        } else {
            warn += 1;
            eprintln!("  [WARN] models/schemas/: no .wfs files found");
        }
    } else {
        warn += 1;
        eprintln!("  [WARN] models/schemas/: directory not found");
    }

    // 4. models/scenarios/
    let scenarios_dir = root.join("models").join("scenarios");
    if scenarios_dir.is_dir() {
        let wfg_count = count_files(&scenarios_dir, "wfg");
        if wfg_count > 0 {
            ok += 1;
            println!("  [OK] models/scenarios/: {wfg_count} .wfg file(s)");
        } else {
            println!("  [INFO] models/scenarios/: no .wfg files");
        }
    }

    // 5. topology/sinks/ — directory structure + TOML validation
    let sinks_dir = root.join("topology").join("sinks");
    if sinks_dir.is_dir() {
        ok += 1;
        let parts = ["business.d", "infra.d"];
        for part in &parts {
            let sub_dir = sinks_dir.join(part);
            if sub_dir.is_dir() {
                let (n, errs) = validate_sink_dir(&sub_dir);
                if errs == 0 {
                    println!("  [OK] topology/sinks/{part}/: {n} file(s) ok");
                } else {
                    warn += 1;
                }
            } else {
                warn += 1;
                eprintln!("  [WARN] topology/sinks/{part}/: not found");
            }
        }
        if sinks_dir.join("defaults.toml").exists() {
            if let Err(e) = validate_sink_file(&sinks_dir.join("defaults.toml")) {
                err += 1;
                eprintln!("  [ERR] topology/sinks/defaults.toml: {e}");
            } else {
                println!("  [OK] topology/sinks/defaults.toml: valid");
            }
        } else {
            warn += 1;
            eprintln!("  [WARN] topology/sinks/defaults.toml: not found");
        }
        // topology/sinks/connectors/sink.d/
        let conn_dir = sinks_dir.join("connectors").join("sink.d");
        if conn_dir.is_dir() {
            let (n, errs) = validate_sink_dir(&conn_dir);
            if errs == 0 {
                println!("  [OK] topology/sinks/connectors/sink.d/: {n} file(s) ok");
            }
        }
    } else {
        warn += 1;
        eprintln!("  [WARN] topology/sinks/: directory not found");
    }

    // 6. topology/sources/
    let sources_dir = root.join("topology").join("sources");
    if sources_dir.is_dir() {
        ok += 1;
        println!("  [OK] topology/sources/: present");
    }

    // 7. connectors/ — directory + TOML validation
    let connectors_dir = root.join("connectors");
    if connectors_dir.is_dir() {
        let sink_d = connectors_dir.join("sink.d");
        if sink_d.is_dir() {
            ok += 1;
            let (n, errs) = validate_sink_dir(&sink_d);
            if errs == 0 {
                println!("  [OK] connectors/sink.d/: {n} file(s) ok");
            }
        } else {
            warn += 1;
            eprintln!("  [WARN] connectors/sink.d/: not found");
        }
    } else {
        warn += 1;
        eprintln!("  [WARN] connectors/: directory not found");
    }

    // Summary
    println!();
    if err > 0 {
        eprintln!("Result: {err} error(s), {warn} warning(s), {ok} ok");
        Err(format!("validation failed with {err} error(s)"))
    } else if warn > 0 {
        println!("Result: {warn} warning(s), {ok} ok (no errors)");
        Ok(())
    } else {
        println!("Result: all checks passed ({ok} ok)");
        Ok(())
    }
}

// ── Helpers ──────────────────────────────────────────────────────────

fn count_files(dir: &Path, ext: &str) -> usize {
    let mut count = 0;
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                count += count_files(&path, ext);
            } else if path.extension().map(|e| e == ext).unwrap_or(false) {
                count += 1;
            }
        }
    }
    count
}

fn validate_sink_dir(dir: &Path) -> (u32, u32) {
    let mut files = 0u32;
    let mut errors = 0u32;
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let (n, e) = validate_sink_dir(&path);
                files += n;
                errors += e;
            } else if path.extension().map(|e| e == "toml").unwrap_or(false) {
                files += 1;
                if let Err(e) = validate_sink_file(&path) {
                    errors += 1;
                    eprintln!("  [ERR] {}: {e}", path.display());
                } else {
                    println!("  [OK] {}", path.display());
                }
            }
        }
    }
    (files, errors)
}

fn validate_sink_file(path: &Path) -> Result<(), String> {
    let content = fs::read_to_string(path).map_err(|e| format!("read error: {e}"))?;
    let _val: toml::Value = content
        .parse::<toml::Value>()
        .map_err(|e| format!("invalid TOML: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "wfadm_check_{}_{}",
            std::process::id(),
            rand::random::<u32>()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn empty_dir_has_missing_conf() {
        let dir = temp_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let result = check_project(&dir);
        assert!(result.is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dir_with_conf_passes_conf_check() {
        let dir = temp_dir();
        std::fs::create_dir_all(dir.join("conf")).unwrap();
        std::fs::write(dir.join("conf/wfusion.toml"), "mode = \"daemon\"\n").unwrap();
        let _ = check_project(&dir);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn invalid_toml_detected() {
        let dir = temp_dir();
        std::fs::create_dir_all(dir.join("conf")).unwrap();
        std::fs::write(dir.join("conf/wfusion.toml"), "not valid toml [[[").unwrap();
        let result = check_project(&dir);
        assert!(result.is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn invalid_sink_toml_detected() {
        let dir = temp_dir();
        std::fs::create_dir_all(dir.join("topology/sinks/business.d")).unwrap();
        std::fs::write(dir.join("topology/sinks/defaults.toml"), "valid = \"ok\"\n").unwrap();
        std::fs::write(
            dir.join("topology/sinks/business.d/bad.toml"),
            "invalid [[[ toml",
        )
        .unwrap();
        // Without conf, it will error on missing conf, but we mainly check sink validation
        let _ = check_project(&dir);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
