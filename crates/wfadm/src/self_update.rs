// wfadm self-update — download latest binary from GitHub Releases
//
// Queries GitHub Releases API for the latest warp-fusion release,
// downloads the binary for the current platform, and replaces the
// running binary.

use std::path::PathBuf;

use std::io;

pub fn run() -> Result<(), String> {
    let current_exe =
        std::env::current_exe().map_err(|e| format!("cannot get current exe path: {e}"))?;

    println!("wfadm self-update");
    println!("  current binary: {}", current_exe.display());

    // Detect platform triple
    let target = detect_target();
    println!("  target: {target}");

    // Query latest release from GitHub
    let release = fetch_latest_release()?;
    println!("  latest version: {}", release.tag_name);

    // Check if update is needed
    let current_version = env!("CARGO_PKG_VERSION");
    if release.tag_name.trim_start_matches('v') == current_version {
        println!("  already up-to-date (v{current_version})");
        return Ok(());
    }

    // Find the right asset
    let asset_name = format!("wfusion-{target}.tar.gz");
    let asset = release
        .assets
        .iter()
        .find(|a| a.name == asset_name)
        .ok_or_else(|| format!("no asset found for target '{target}' (expected: {asset_name})"))?;

    println!(
        "  downloading: {} ({:.1} MB)",
        asset.name,
        asset.size as f64 / 1_048_576.0
    );

    // Download to temp file
    let tmp = download_asset(&asset.browser_download_url)?;

    // Extract wfusion binary from tar.gz
    let wfusion_bin = extract_wfusion(&tmp)?;

    // Find current binary parent dir and backup
    let parent = current_exe
        .parent()
        .ok_or("cannot determine binary directory")?;
    let backup = parent.join("wfadm.bak");

    // On Unix, rename current → backup, copy new → current
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::rename(&current_exe, &backup)
            .map_err(|e| format!("cannot backup current binary: {e}"))?;
        std::fs::copy(&wfusion_bin, &current_exe)
            .map_err(|e| format!("cannot install new binary: {e}"))?;
        let mut perms = std::fs::metadata(&current_exe)
            .map_err(|e| format!("metadata: {e}"))?
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&current_exe, perms).map_err(|e| format!("chmod: {e}"))?;
    }

    #[cfg(not(unix))]
    {
        std::fs::rename(&current_exe, &backup)
            .map_err(|e| format!("cannot backup current binary: {e}"))?;
        std::fs::copy(&wfusion_bin, &current_exe)
            .map_err(|e| format!("cannot install new binary: {e}"))?;
    }

    println!("  updated to {}", release.tag_name);
    println!("  backup kept at {}", backup.display());
    Ok(())
}

fn detect_target() -> &'static str {
    let arch = if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "unknown"
    };

    let os = if cfg!(target_os = "linux") {
        "unknown-linux-gnu"
    } else if cfg!(target_os = "macos") {
        "apple-darwin"
    } else {
        "unknown"
    };

    // We return just the distinguishing part for asset matching
    // Full triple would be {arch}-{os}, but assets may use shorter names
    if arch == "x86_64" && os == "unknown-linux-gnu" {
        "x86_64-linux"
    } else if arch == "aarch64" && os == "apple-darwin" {
        "aarch64-macos"
    } else {
        "x86_64-linux" // default fallback
    }
}

// ----- GitHub API helpers -----

struct Release {
    tag_name: String,
    assets: Vec<Asset>,
}

struct Asset {
    name: String,
    size: u64,
    browser_download_url: String,
}

fn fetch_latest_release() -> Result<Release, String> {
    let url = "https://api.github.com/repos/wp-labs/warp-fusion/releases/latest";
    let body = http_get_json(url)?;

    let tag_name = body["tag_name"]
        .as_str()
        .ok_or("missing tag_name in release JSON")?
        .to_string();

    let assets_json = body["assets"]
        .as_array()
        .ok_or("missing assets array in release JSON")?;

    let mut assets = Vec::new();
    for a in assets_json {
        assets.push(Asset {
            name: a["name"].as_str().unwrap_or("").to_string(),
            size: a["size"].as_u64().unwrap_or(0),
            browser_download_url: a["browser_download_url"].as_str().unwrap_or("").to_string(),
        });
    }

    Ok(Release { tag_name, assets })
}

fn download_asset(url: &str) -> Result<PathBuf, String> {
    let tmp = std::env::temp_dir().join(format!("wfadm_update_{}.tar.gz", std::process::id()));
    let resp = ureq::get(url)
        .call()
        .map_err(|e| format!("download failed: {e}"))?;

    let mut reader = resp.into_body().into_reader();
    let mut file =
        std::fs::File::create(&tmp).map_err(|e| format!("cannot create temp file: {e}"))?;
    io::copy(&mut reader, &mut file).map_err(|e| format!("download write error: {e}"))?;

    Ok(tmp)
}

fn extract_wfusion(tarball: &std::path::Path) -> Result<PathBuf, String> {
    let out_dir = std::env::temp_dir().join(format!("wfadm_extract_{}", std::process::id()));
    std::fs::create_dir_all(&out_dir).map_err(|e| format!("cannot create extract dir: {e}"))?;

    let f = std::fs::File::open(tarball).map_err(|e| format!("cannot open tarball: {e}"))?;
    let decoder = flate2::read::GzDecoder::new(f);
    let mut archive = tar::Archive::new(decoder);
    archive
        .unpack(&out_dir)
        .map_err(|e| format!("extract error: {e}"))?;

    // Find wfusion binary in extracted tree
    for entry in walkdir::WalkDir::new(&out_dir).into_iter().flatten() {
        if entry.file_name() == "wfusion" && entry.path().is_file() {
            return Ok(entry.path().to_path_buf());
        }
    }

    Err("wfusion binary not found in release archive".to_string())
}

// ----- Minimal JSON HTTP helper (no serde_json needed for simple parsing) -----

fn http_get_json(url: &str) -> Result<serde_json::Value, String> {
    let resp = ureq::get(url)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "wfadm/self-update")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .call()
        .map_err(|e| format!("HTTP request failed: {e}"))?;

    let body_str = resp
        .into_body()
        .read_to_string()
        .map_err(|e| format!("read response body: {e}"))?;

    serde_json::from_str(&body_str).map_err(|e| format!("parse JSON: {e}"))
}
