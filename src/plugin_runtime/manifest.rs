use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Matches the robinebers/openusage plugin.json schema (schemaVersion 1).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginManifest {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    pub version: String,
    pub entry: String,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub brand_color: Option<String>,
    #[serde(default)]
    pub lines: Vec<ManifestLine>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestLine {
    #[serde(rename = "type")]
    pub kind: String, // "progress" | "text" | "badge"
    pub label: String,
    #[serde(default)]
    pub scope: Option<String>, // "overview" | "detail"
    #[serde(default)]
    pub primary_order: Option<u32>,
}

/// A plugin loaded from disk, ready to run.
#[derive(Debug, Clone)]
pub struct LoadedPlugin {
    pub manifest: PluginManifest,
    /// Absolute path to the plugin directory.
    pub dir: PathBuf,
    /// Contents of the entry JS file.
    pub script: String,
}

const SCHEMA_VERSION_MAX: u32 = 1;

/// Load all valid plugins from a directory.
/// Each subdirectory is expected to contain a plugin.json + entry JS file.
pub fn load_plugins(plugins_dir: &PathBuf) -> Vec<LoadedPlugin> {
    let Ok(entries) = fs::read_dir(plugins_dir) else {
        tracing::warn!(
            "could not read plugins directory: {}",
            plugins_dir.display()
        );
        return vec![];
    };

    let mut plugins = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        match load_plugin(&path) {
            Ok(p) => plugins.push(p),
            Err(e) => {
                tracing::warn!("skipping plugin at {}: {e}", path.display());
            }
        }
    }

    // Sort by plugin id for deterministic output
    plugins.sort_by(|a, b| a.manifest.id.cmp(&b.manifest.id));
    plugins
}

/// Load a single plugin from a directory.
pub fn load_plugin(dir: &Path) -> Result<LoadedPlugin> {
    let manifest_path = dir.join("plugin.json");
    let manifest_str = fs::read_to_string(&manifest_path)
        .with_context(|| format!("missing plugin.json in {}", dir.display()))?;

    let manifest: PluginManifest = serde_json::from_str(&manifest_str)
        .with_context(|| format!("invalid plugin.json in {}", dir.display()))?;

    if manifest.schema_version > SCHEMA_VERSION_MAX {
        tracing::warn!(
            "plugin '{}' uses schemaVersion {} (max supported: {}); loading anyway",
            manifest.id,
            manifest.schema_version,
            SCHEMA_VERSION_MAX
        );
    }

    let entry_path = dir.join(&manifest.entry);
    let script = fs::read_to_string(&entry_path).with_context(|| {
        format!(
            "could not read entry '{}' in {}",
            manifest.entry,
            dir.display()
        )
    })?;

    Ok(LoadedPlugin {
        manifest,
        dir: dir.to_path_buf(),
        script,
    })
}
