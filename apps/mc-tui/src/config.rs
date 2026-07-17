//! Versioned atomic JSON configuration for the MC TUI.
//!
//! Persists connection defaults, filter state, known plugins, and last target
//! across sessions.  CLI arguments take precedence over config values.
//!
//! # Atomic writes
//!
//! [`save`] writes to a temporary sibling file then renames atomically,
//! preventing partial/corrupt writes.  Config is versioned (`ConfigVersion`)
//! for forward compatibility.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use mc_protocol::events::PluginDetails;

use crate::app::{LogKindFilter, LogLevelFilter};

// ── Config version ────────────────────────────────────────────────────

/// Schema version for forward compatibility.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigVersion {
    #[default]
    V1,
}

// ── Filter serialization helpers ──────────────────────────────────────

/// Serializable mirror of [`LogKindFilter`].
/// Each field defaults independently so that partial configs are accepted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SerializableKindFilter {
    #[serde(default = "default_true")]
    pub system: bool,
    #[serde(default = "default_true")]
    pub protocol: bool,
    #[serde(default = "default_true")]
    pub stopped: bool,
    #[serde(default = "default_true")]
    pub thread: bool,
    #[serde(default = "default_true")]
    pub print: bool,
    #[serde(default = "default_true")]
    pub notification: bool,
    #[serde(default = "default_true")]
    pub stat: bool,
    #[serde(default = "default_true")]
    pub profiler_capture: bool,
    #[serde(default = "default_true")]
    pub schema: bool,
    #[serde(default = "default_true")]
    pub terminated: bool,
    #[serde(default = "default_true")]
    pub unknown: bool,
}

fn default_true() -> bool {
    true
}

impl Default for SerializableKindFilter {
    fn default() -> Self {
        Self::all_enabled()
    }
}

impl SerializableKindFilter {
    pub fn all_enabled() -> Self {
        Self {
            system: true,
            protocol: true,
            stopped: true,
            thread: true,
            print: true,
            notification: true,
            stat: true,
            profiler_capture: true,
            schema: true,
            terminated: true,
            unknown: true,
        }
    }
}

impl From<&LogKindFilter> for SerializableKindFilter {
    fn from(f: &LogKindFilter) -> Self {
        Self {
            system: f.system,
            protocol: f.protocol,
            stopped: f.stopped,
            thread: f.thread,
            print: f.print,
            notification: f.notification,
            stat: f.stat,
            profiler_capture: f.profiler_capture,
            schema: f.schema,
            terminated: f.terminated,
            unknown: f.unknown,
        }
    }
}

impl From<SerializableKindFilter> for LogKindFilter {
    fn from(f: SerializableKindFilter) -> Self {
        Self {
            system: f.system,
            protocol: f.protocol,
            stopped: f.stopped,
            thread: f.thread,
            print: f.print,
            notification: f.notification,
            stat: f.stat,
            profiler_capture: f.profiler_capture,
            schema: f.schema,
            terminated: f.terminated,
            unknown: f.unknown,
        }
    }
}

/// Serializable mirror of [`LogLevelFilter`].
/// Each field defaults independently so that partial configs are accepted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SerializableLevelFilter {
    #[serde(default = "default_true")]
    pub log: bool,
    #[serde(default = "default_true")]
    pub warn: bool,
    #[serde(default = "default_true")]
    pub error: bool,
}

impl Default for SerializableLevelFilter {
    fn default() -> Self {
        Self::all_enabled()
    }
}

impl SerializableLevelFilter {
    pub fn all_enabled() -> Self {
        Self {
            log: true,
            warn: true,
            error: true,
        }
    }
}

impl From<&LogLevelFilter> for SerializableLevelFilter {
    fn from(f: &LogLevelFilter) -> Self {
        Self {
            log: f.log,
            warn: f.warn,
            error: f.error,
        }
    }
}

impl From<SerializableLevelFilter> for LogLevelFilter {
    fn from(f: SerializableLevelFilter) -> Self {
        Self {
            log: f.log,
            warn: f.warn,
            error: f.error,
        }
    }
}

/// Serializable filter configuration.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SerializableFilterState {
    #[serde(default)]
    pub search: String,
    #[serde(default)]
    pub kinds: SerializableKindFilter,
    #[serde(default)]
    pub levels: SerializableLevelFilter,
}

// ── Plugin cache entry ─────────────────────────────────────────────────

/// A cached plugin descriptor stored in the config.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CachedPlugin {
    pub name: String,
    pub module_uuid: String,
}

impl From<&PluginDetails> for CachedPlugin {
    fn from(p: &PluginDetails) -> Self {
        Self {
            name: p.name.clone(),
            module_uuid: p.module_uuid.clone(),
        }
    }
}

impl From<CachedPlugin> for PluginDetails {
    fn from(c: CachedPlugin) -> Self {
        Self {
            name: c.name,
            module_uuid: c.module_uuid,
        }
    }
}

// ── Top-level config ──────────────────────────────────────────────────

/// The on-disk configuration schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct AppConfigFile {
    /// Schema version for forward compat.
    #[serde(default)]
    pub version: ConfigVersion,
    /// Cached plugins from previous connections.
    #[serde(default)]
    pub known_plugins: Vec<CachedPlugin>,
    /// Last-used target module UUID (for quick reconnect).
    #[serde(default)]
    pub last_target_uuid: Option<String>,
    /// Passcode from last successful connection.
    #[serde(default)]
    pub passcode: Option<String>,
    /// Full filter state (search, kind toggles, level toggles).
    #[serde(default)]
    pub filters: SerializableFilterState,
}

impl Default for AppConfigFile {
    fn default() -> Self {
        Self {
            version: ConfigVersion::V1,
            known_plugins: Vec::new(),
            last_target_uuid: None,
            passcode: None,
            filters: SerializableFilterState::default(),
        }
    }
}

// ── Runtime config ────────────────────────────────────────────────────

/// Resolved configuration that merges file defaults with CLI overrides.
#[derive(Debug, Clone)]
pub struct AppConfig {
    pub known_plugins: Vec<PluginDetails>,
    pub last_target_uuid: Option<String>,
    pub passcode: Option<String>,
    pub search: String,
    pub kinds: LogKindFilter,
    pub levels: LogLevelFilter,
}

impl AppConfig {
    /// Build a runtime config from the on-disk config, applying CLI overrides.
    ///
    /// `cli_target_uuid` and `cli_passcode` override the file values when `Some`.
    pub fn from_file_with_cli(
        file: &AppConfigFile,
        cli_target_uuid: Option<String>,
        cli_passcode: Option<String>,
    ) -> Self {
        Self {
            known_plugins: file
                .known_plugins
                .iter()
                .map(|c| PluginDetails {
                    name: c.name.clone(),
                    module_uuid: c.module_uuid.clone(),
                })
                .collect(),
            last_target_uuid: cli_target_uuid.or_else(|| file.last_target_uuid.clone()),
            passcode: cli_passcode.or_else(|| file.passcode.clone()),
            search: file.filters.search.clone(),
            kinds: LogKindFilter::from(file.filters.kinds.clone()),
            levels: LogLevelFilter::from(file.filters.levels.clone()),
        }
    }

    /// Merge current runtime state back into a serializable file struct.
    pub fn into_file(self, new_plugins: Vec<PluginDetails>) -> AppConfigFile {
        let mut known = self
            .known_plugins
            .into_iter()
            .map(|p| CachedPlugin {
                name: p.name,
                module_uuid: p.module_uuid,
            })
            .collect::<Vec<_>>();

        // Merge new plugins without duplicating (newest first)
        for p in new_plugins.iter().rev() {
            if !known.iter().any(|c| c.module_uuid == p.module_uuid) {
                known.insert(
                    0,
                    CachedPlugin {
                        name: p.name.clone(),
                        module_uuid: p.module_uuid.clone(),
                    },
                );
            }
        }

        AppConfigFile {
            version: ConfigVersion::V1,
            known_plugins: known,
            last_target_uuid: self.last_target_uuid.clone(),
            passcode: self.passcode.clone(),
            filters: SerializableFilterState {
                search: self.search,
                kinds: SerializableKindFilter {
                    system: self.kinds.system,
                    protocol: self.kinds.protocol,
                    stopped: self.kinds.stopped,
                    thread: self.kinds.thread,
                    print: self.kinds.print,
                    notification: self.kinds.notification,
                    stat: self.kinds.stat,
                    profiler_capture: self.kinds.profiler_capture,
                    schema: self.kinds.schema,
                    terminated: self.kinds.terminated,
                    unknown: self.kinds.unknown,
                },
                levels: SerializableLevelFilter {
                    log: self.levels.log,
                    warn: self.levels.warn,
                    error: self.levels.error,
                },
            },
        }
    }
}

// ── Default config path ───────────────────────────────────────────────

/// Return the default config file path.
///
/// Order of precedence:
/// 1. `MC_TUI_CONFIG` environment variable (if set and non-empty)
/// 2. Platform-appropriate default:
///    - Windows: `%APPDATA%/minecraft-debugger/settings.json`
///    - Other:   `$XDG_CONFIG_HOME/minecraft-debugger/settings.json`
///      or `$HOME/.config/minecraft-debugger/settings.json`
pub fn default_config_path() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            return PathBuf::from(appdata)
                .join("minecraft-debugger")
                .join("settings.json");
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            if !xdg.is_empty() {
                return PathBuf::from(xdg)
                    .join("minecraft-debugger")
                    .join("settings.json");
            }
        }
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home)
                .join(".config")
                .join("minecraft-debugger")
                .join("settings.json");
        }
    }
    // Fallback (unlikely to be reached)
    PathBuf::from("mc-tui-config.json")
}

// ── Load / Save ───────────────────────────────────────────────────────

/// Load configuration from `path`.  Returns `Ok(None)` when the file does
/// not exist (first run).  Returns `Ok(Some(defaults))` for corrupt files
/// after logging a warning.  Unknown JSON fields are silently ignored
/// thanks to `#[serde(deny_unknown_fields)]` not being set.
pub fn load(path: &Path) -> Result<Option<AppConfigFile>, String> {
    if !path.exists() {
        return Ok(None);
    }

    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("failed to read config file '{}': {e}", path.display()))?;

    // Attempt to deserialize; every nested filter bool defaults independently
    // via #[serde(default)], so partial configs are accepted.
    // Malformed JSON falls back to defaults with a warning.
    match serde_json::from_str::<AppConfigFile>(&content) {
        Ok(config) => Ok(Some(config)),
        Err(e) => {
            // Log a warning but return defaults instead of failing
            eprintln!(
                "Warning: malformed config file '{}' ({e}); using defaults",
                path.display()
            );
            Ok(Some(AppConfigFile::default()))
        }
    }
}

/// Atomically save configuration to `path`.
///
/// Creates parent directories if they do not exist.
/// Writes to a unique sibling `.tmp` file, then renames (removing target
/// first on Windows where `rename` does not overwrite).
/// This prevents partial/corrupt writes from being read on next launch.
pub fn save(path: &Path, config: &AppConfigFile) -> Result<(), String> {
    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            format!(
                "failed to create config directory '{}': {e}",
                parent.display()
            )
        })?;
    }

    let json = serde_json::to_string_pretty(config)
        .map_err(|e| format!("failed to serialize config: {e}"))?;

    // Use a unique temp sibling path to avoid collision across processes
    let unique_suffix = format!("{}.{}", std::process::id(), {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
    });
    let tmp_path = {
        let stem = path.file_stem().unwrap_or_default();
        let ext = path.extension().unwrap_or_default();
        let mut s = stem.to_os_string();
        s.push(format!(".tmp.{unique_suffix}"));
        s.push(".");
        s.push(ext);
        path.with_file_name(s)
    };

    std::fs::write(&tmp_path, &json)
        .map_err(|e| format!("failed to write config temp file: {e}"))?;

    // Windows: rename does not overwrite; remove target explicitly first.
    // Other platforms: atomic overwrite is fine.
    #[cfg(target_os = "windows")]
    {
        let _ = std::fs::remove_file(path);
    }
    std::fs::rename(&tmp_path, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp_path);
        format!("failed to rename config file: {e}")
    })?;

    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::LogFilterState;

    #[test]
    fn default_config_roundtrip() {
        let config = AppConfigFile::default();
        let json = serde_json::to_string_pretty(&config).unwrap();
        let parsed: AppConfigFile = serde_json::from_str(&json).unwrap();
        assert_eq!(config, parsed);
        assert_eq!(parsed.version, ConfigVersion::V1);
    }

    #[test]
    fn known_plugins_roundtrip() {
        let plugins = vec![
            PluginDetails {
                name: "Alpha".into(),
                module_uuid: "uuid-alpha".into(),
            },
            PluginDetails {
                name: "Beta".into(),
                module_uuid: "uuid-beta".into(),
            },
        ];
        let config = AppConfig {
            known_plugins: plugins.clone(),
            last_target_uuid: Some("uuid-beta".into()),
            passcode: None,
            search: String::new(),
            kinds: LogKindFilter::all_enabled(),
            levels: LogLevelFilter::all_enabled(),
        };

        let file = config.into_file(vec![]);
        let json = serde_json::to_string_pretty(&file).unwrap();
        let parsed: AppConfigFile = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.known_plugins.len(), 2);
        assert_eq!(parsed.last_target_uuid.as_deref(), Some("uuid-beta"));
    }

    #[test]
    fn filter_state_roundtrip() {
        let mut filter = LogFilterState::show_all();
        filter.kinds.print = false;
        filter.levels.warn = false;
        filter.search = "test".into();

        let kinds: SerializableKindFilter = (&filter.kinds).into();
        let levels: SerializableLevelFilter = (&filter.levels).into();

        let file = AppConfigFile {
            version: ConfigVersion::V1,
            known_plugins: vec![],
            last_target_uuid: None,
            passcode: None,
            filters: SerializableFilterState {
                search: filter.search.clone(),
                kinds,
                levels,
            },
        };

        let json = serde_json::to_string_pretty(&file).unwrap();
        let parsed: AppConfigFile = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.filters.search, "test");
        assert!(!LogKindFilter::from(parsed.filters.kinds.clone()).print,);
        assert!(!LogLevelFilter::from(parsed.filters.levels.clone()).warn,);
    }

    #[test]
    fn cli_target_uuid_overrides_config() {
        let file = AppConfigFile {
            last_target_uuid: Some("from-config".into()),
            passcode: Some("pass-from-config".into()),
            ..Default::default()
        };

        let config = AppConfig::from_file_with_cli(
            &file,
            Some("from-cli".into()), // CLI override
            None,                    // no CLI passcode → keep file value
        );

        assert_eq!(config.last_target_uuid.as_deref(), Some("from-cli"));
        assert_eq!(config.passcode.as_deref(), Some("pass-from-config"));
    }

    #[test]
    fn cli_none_uses_config_value() {
        let file = AppConfigFile {
            last_target_uuid: Some("from-config".into()),
            passcode: None,
            ..Default::default()
        };

        let config = AppConfig::from_file_with_cli(&file, None, None);

        assert_eq!(config.last_target_uuid.as_deref(), Some("from-config"));
        assert!(config.passcode.is_none());
    }

    #[test]
    fn save_and_load_roundtrip() {
        let dir = std::env::temp_dir();
        let path = dir.join("mc-tui-test-config.json");

        // Ensure clean state (wildcard for .tmp.* files)
        let _ = std::fs::remove_file(&path);
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.starts_with("mc-tui-test-config.tmp.") {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }

        let file = AppConfigFile {
            version: ConfigVersion::V1,
            known_plugins: vec![CachedPlugin {
                name: "TestPlugin".into(),
                module_uuid: "test-uuid".into(),
            }],
            last_target_uuid: Some("last-uuid".into()),
            passcode: Some("s3cret".into()),
            filters: SerializableFilterState {
                search: "search-term".into(),
                kinds: SerializableKindFilter {
                    print: false,
                    ..SerializableKindFilter::all_enabled()
                },
                levels: SerializableLevelFilter {
                    warn: false,
                    ..SerializableLevelFilter::all_enabled()
                },
            },
        };

        save(&path, &file).unwrap();
        let loaded = load(&path).unwrap().expect("should load saved config");
        assert_eq!(loaded.version, ConfigVersion::V1);
        assert_eq!(loaded.known_plugins.len(), 1);
        assert_eq!(loaded.known_plugins[0].module_uuid, "test-uuid");
        assert_eq!(loaded.last_target_uuid.as_deref(), Some("last-uuid"));
        assert_eq!(loaded.passcode.as_deref(), Some("s3cret"));
        assert_eq!(loaded.filters.search, "search-term");
        assert!(!loaded.filters.kinds.print);
        assert!(!loaded.filters.levels.warn);

        // Cleanup
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn save_is_atomic() {
        let dir = std::env::temp_dir();
        let path = dir.join("mc-tui-atomic-test.json");

        let _ = std::fs::remove_file(&path);
        // Clean up any leftover .tmp files from previous runs
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.starts_with("mc-tui-atomic-test.tmp.") {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }

        let config = AppConfigFile::default();
        save(&path, &config).unwrap();

        // Confirm the real file exists and no .tmp.* files linger
        assert!(path.exists(), "config file must exist after save");
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                assert!(
                    !name.starts_with("mc-tui-atomic-test.tmp."),
                    "temp file '{name}' must be removed after save"
                );
            }
        }

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn load_missing_file_returns_none() {
        let dir = std::env::temp_dir();
        let path = dir.join("mc-tui-nonexistent.json");
        let _ = std::fs::remove_file(&path);

        let result = load(&path).unwrap();
        assert!(result.is_none(), "missing file should return None");
    }

    #[test]
    fn partial_config_uses_defaults_for_missing_fields() {
        // Only supply search and one kind toggle
        let json = r#"{
            "filters": {
                "search": "error",
                "kinds": { "print": false }
            }
        }"#;
        let config: AppConfigFile = serde_json::from_str(json).unwrap();
        assert_eq!(config.filters.search, "error");
        assert!(!config.filters.kinds.print);
        // All other kinds default to true
        assert!(config.filters.kinds.system);
        assert!(config.filters.kinds.protocol);
        // Levels all default to true
        assert!(config.filters.levels.log);
        assert!(config.filters.levels.warn);
        assert!(config.filters.levels.error);
        // Version defaults to V1
        assert_eq!(config.version, ConfigVersion::V1);
        // Known plugins default to empty
        assert!(config.known_plugins.is_empty());
    }

    #[test]
    fn malformed_json_falls_back_to_defaults() {
        let dir = std::env::temp_dir();
        let path = dir.join("mc-tui-malformed-test.json");
        let _ = std::fs::remove_file(&path);

        // Write completely invalid JSON
        std::fs::write(&path, "not valid json [[[").unwrap();
        let result = load(&path).unwrap();
        assert!(result.is_some(), "malformed JSON should return defaults");
        let config = result.unwrap();
        assert_eq!(config.version, ConfigVersion::V1);
        assert!(config.known_plugins.is_empty());

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let json = r#"{
            "version": "v1",
            "unknown_field": "should be ignored",
            "filters": {
                "unknown_nested": true
            }
        }"#;
        let config: AppConfigFile = serde_json::from_str(json).unwrap();
        assert_eq!(config.version, ConfigVersion::V1);
    }

    #[test]
    fn save_creates_parent_directories() {
        let dir = std::env::temp_dir().join("mc-tui-test-deep-nested-dir");
        let path = dir.join("sub").join("settings.json");

        // Clean up
        let _ = std::fs::remove_dir_all(&dir);

        let config = AppConfigFile::default();
        save(&path, &config).unwrap();

        assert!(path.exists(), "config file must exist after save");

        // Clean up
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn merge_new_plugins_dedups() {
        let existing = vec![PluginDetails {
            name: "Alpha".into(),
            module_uuid: "uuid-alpha".into(),
        }];
        let config = AppConfig {
            known_plugins: existing,
            last_target_uuid: None,
            passcode: None,
            search: String::new(),
            kinds: LogKindFilter::all_enabled(),
            levels: LogLevelFilter::all_enabled(),
        };

        let new_plugins = vec![
            PluginDetails {
                name: "Beta".into(),
                module_uuid: "uuid-beta".into(),
            },
            PluginDetails {
                name: "Alpha".into(), // duplicate — same uuid
                module_uuid: "uuid-alpha".into(),
            },
        ];

        let file = config.into_file(new_plugins);
        // Beta should be first (new), Alpha already existed
        assert_eq!(file.known_plugins.len(), 2);
        assert_eq!(file.known_plugins[0].module_uuid, "uuid-beta");
        assert_eq!(file.known_plugins[1].module_uuid, "uuid-alpha");
    }
}
