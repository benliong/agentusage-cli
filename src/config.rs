use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use uuid::Uuid;

const APP_NAME: &str = "agentusage";

/// Returns the application data directory, creating it if necessary.
///
/// - macOS:   ~/Library/Application Support/agentusage
/// - Linux:   ~/.local/share/agentusage
/// - Windows: %APPDATA%\agentusage
pub fn app_data_dir() -> Result<PathBuf> {
    let base = dirs::data_dir().context("could not determine platform data directory")?;
    let dir = base.join(APP_NAME);
    fs::create_dir_all(&dir)
        .with_context(|| format!("could not create app data directory: {}", dir.display()))?;
    Ok(dir)
}

/// Returns the plugins directory ($appDataDir/plugins), creating it if necessary.
pub fn plugins_dir() -> Result<PathBuf> {
    let dir = app_data_dir()?.join("plugins");
    fs::create_dir_all(&dir)
        .with_context(|| format!("could not create plugins directory: {}", dir.display()))?;
    Ok(dir)
}

/// Returns a stable machine ID (UUID v4), persisted on first run.
pub fn machine_id() -> Result<String> {
    let path = app_data_dir()?.join("machine_id");
    if path.exists() {
        let id = fs::read_to_string(&path).context("could not read machine_id")?;
        return Ok(id.trim().to_string());
    }
    let id = Uuid::new_v4().to_string();
    fs::write(&path, &id).context("could not write machine_id")?;
    Ok(id)
}

/// Resolves the bundled plugins source directory.
///
/// Resolution order:
/// 1. `AU_PLUGINS_DIR` environment variable (dev/testing override)
/// 2. `{dir_of_binary}/../bundled_plugins` (works for Homebrew libexec layout and dev builds)
/// 3. `{dir_of_binary}/bundled_plugins` (same directory as binary)
/// 4. `CARGO_MANIFEST_DIR/bundled_plugins` (dev builds via cargo run)
pub fn bundled_plugins_source_dir() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("AU_PLUGINS_DIR") {
        let path = PathBuf::from(p);
        if path.is_dir() {
            return Some(path);
        }
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            let candidate = exe_dir.join("../bundled_plugins");
            if candidate.is_dir() {
                return Some(candidate.canonicalize().unwrap_or(candidate));
            }
            let candidate = exe_dir.join("bundled_plugins");
            if candidate.is_dir() {
                return Some(candidate);
            }
        }
    }

    // Fallback for `cargo run` during development
    if let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") {
        let candidate = PathBuf::from(manifest_dir).join("bundled_plugins");
        if candidate.is_dir() {
            return Some(candidate);
        }
    }

    None
}

/// Ensures all bundled plugins are copied to the plugins data dir on first run.
/// Skips plugins that already exist in the target dir.
pub fn ensure_bundled_plugins_installed() -> Result<()> {
    let Some(source_dir) = bundled_plugins_source_dir() else {
        tracing::warn!("bundled plugins directory not found; skipping plugin installation");
        return Ok(());
    };

    let target_dir = plugins_dir()?;

    let entries = fs::read_dir(&source_dir).with_context(|| {
        format!(
            "could not read bundled plugins from {}",
            source_dir.display()
        )
    })?;

    for entry in entries.flatten() {
        let src = entry.path();
        if !src.is_dir() {
            continue;
        }
        let plugin_id = entry.file_name();
        let dst = target_dir.join(&plugin_id);
        if dst.exists() {
            continue; // already installed, never overwrite user modifications
        }
        copy_dir_recursive(&src, &dst)
            .with_context(|| format!("could not copy plugin {:?}", plugin_id))?;
        tracing::debug!("installed bundled plugin {:?}", plugin_id);
    }

    Ok(())
}

fn copy_dir_recursive(src: &PathBuf, dst: &PathBuf) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)?.flatten() {
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}
