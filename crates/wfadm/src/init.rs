use std::fs;
use std::path::Path;

use crate::init_tpl::{Scope, templates_for};

// =====================================================================
// Init
// =====================================================================

pub fn init_project(project_dir: &str, _name: &str, scope: &str) -> Result<(), String> {
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

    let scope: Scope = scope.parse().map_err(|e| format!("invalid scope: {e}"))?;

    // 1. Write static templates (rules, schemas, scenarios, topology, conf)
    for (template_path, data) in templates_for(scope) {
        let full = root.join(template_path);

        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("create parent for {template_path}: {e}"))?;
        }
        fs::write(&full, data).map_err(|e| format!("write {template_path}: {e}"))?;
    }

    // 2. Generate connector templates from registry
    crate::connectors::generate_connector_templates(root)
        .map_err(|e| format!("connector generation: {e}"))?;

    // Make scripts executable (on Unix)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for script in &["test_run.sh", "smoke.sh"] {
            let path = root.join(script);
            if path.exists() {
                let mut perms = fs::metadata(&path)
                    .map_err(|e| format!("metadata {script}: {e}"))?
                    .permissions();
                perms.set_mode(0o755);
                fs::set_permissions(&path, perms).map_err(|e| format!("chmod {script}: {e}"))?;
            }
        }
    }

    println!(
        "wf-rules project created at {} (scope: {scope:?})",
        root.canonicalize().unwrap_or(root.to_path_buf()).display()
    );
    println!("  cd {} && wfusion run", project_dir);
    Ok(())
}

// =====================================================================
// Remote bootstrap (stub)
// =====================================================================

pub fn init_from_remote(
    project_dir: &str,
    repo_url: &str,
    version: Option<&str>,
) -> Result<(), String> {
    let _ = (project_dir, repo_url, version);
    Err("remote bootstrap not yet implemented (TODO)".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "wfadm_test_{}_{}",
            std::process::id(),
            rand::random::<u32>()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn init_rules_creates_expected_dirs() {
        let dir = temp_dir();
        init_project(dir.to_str().unwrap(), "test", "rules").expect("init rules");
        assert!(dir.join("conf/wfusion.toml").exists());
        assert!(dir.join("models/rules").is_dir());
        assert!(dir.join("models/schemas").is_dir());
        assert!(dir.join("models/scenarios").is_dir());
        assert!(dir.join("smoke.sh").exists());
        // Rules scope should NOT have topology
        assert!(!dir.join("topology").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn init_conf_creates_expected_dirs() {
        let dir = temp_dir();
        init_project(dir.to_str().unwrap(), "test", "conf").expect("init conf");
        assert!(dir.join("conf/wfusion.toml").exists());
        assert!(dir.join("topology/sinks").is_dir());
        assert!(dir.join("topology/sources").is_dir());
        // Conf scope should NOT have models
        assert!(!dir.join("models").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn init_rejects_nonempty_dir() {
        let dir = temp_dir();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("existing.txt"), b"hello").unwrap();
        let err = init_project(dir.to_str().unwrap(), "test", "normal").unwrap_err();
        assert!(err.contains("already exists"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn init_invalid_scope() {
        let dir = temp_dir();
        let err = init_project(dir.to_str().unwrap(), "test", "bad").unwrap_err();
        assert!(err.contains("invalid scope"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
