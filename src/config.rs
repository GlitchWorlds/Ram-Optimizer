//! Zero-dependency settings persistence for Ram-Optimizer.
//!
//! Stores user-facing automation preferences as JSON at
//! `%APPDATA%\RamOptimizer\config.json`. Uses only `std` (no serde / crates),
//! matching the repo's zero-dependency Win32/NT FFI style.
//!
//! The parser is intentionally small and defensive: any malformed / missing /
//! out-of-range file falls back to defaults rather than panicking.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Settings persisted across restarts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    /// Auto-clean toggle state (interval checkbox in the GUI).
    pub auto_clean: bool,
    /// Auto-clean interval in minutes (GUI interval edit).
    pub interval_minutes: u32,
    /// Auto-clean memory-load trigger percentage (GUI threshold edit).
    pub threshold_percent: u32,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            auto_clean: false,
            interval_minutes: 15,
            threshold_percent: 80,
        }
    }
}

/// Returns the config file path: `%APPDATA%\RamOptimizer\config.json`.
/// Falls back to a local `config.json` next to the exe if `%APPDATA%` is unset.
pub fn config_path() -> PathBuf {
    let mut path = match std::env::var_os("APPDATA") {
        Some(appdata) => PathBuf::from(appdata),
        None => PathBuf::from("."),
    };
    path.push("RamOptimizer");
    path.push("config.json");
    path
}

/// Loads settings from disk. Never panics: missing or corrupt files (or
/// out-of-range values) fall back to defaults.
pub fn load() -> Config {
    let path = config_path();
    load_from_path(&path).unwrap_or_default()
}

/// Loads settings from an explicit path (used by tests to avoid touching the
/// real user profile). Returns `Ok(Config)` on success, `Err` on any failure.
pub fn load_from_path(path: &Path) -> io::Result<Config> {
    let text = fs::read_to_string(path)?;
    Ok(parse(&text))
}

/// Saves settings to disk, creating `%APPDATA%\RamOptimizer` if needed.
/// Returns `Ok(())` on success; callers may ignore failures (best-effort).
pub fn save(config: &Config) -> io::Result<()> {
    let path = config_path();
    save_to_path(config, &path)
}

/// Saves settings to an explicit path (used by tests).
pub fn save_to_path(config: &Config, path: &Path) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serialize(config))
}

/// Serializes a `Config` to a compact JSON object string.
fn serialize(config: &Config) -> String {
    format!(
        "{{\"auto_clean\":{},\"interval_minutes\":{},\"threshold_percent\":{}}}",
        config.auto_clean, config.interval_minutes, config.threshold_percent
    )
}

/// Parses JSON text into a `Config`. Any parse failure or out-of-range field
/// falls back to defaults for that field (whole struct on hard failure).
fn parse(text: &str) -> Config {
    let mut cfg = Config::default();
    let trimmed = text.trim();

    if !(trimmed.starts_with('{') && trimmed.ends_with('}')) {
        return cfg;
    }

    let inner = &trimmed[1..trimmed.len() - 1];
    for field in split_fields(inner) {
        let field = field.trim();
        let Some(eq) = find_key_sep(field) else {
            continue;
        };
        let key = field[..eq].trim();
        let value = field[eq + 1..].trim();

        // JSON keys are quoted in the file; strip the quotes before matching.
        let key = if key.len() >= 2 && key.starts_with('"') && key.ends_with('"') {
            &key[1..key.len() - 1]
        } else {
            key
        };

        match key {
            "auto_clean" => {
                cfg.auto_clean = value == "true";
            }
            "interval_minutes" => {
                if let Ok(n) = value.parse::<u32>() {
                    if n > 0 {
                        cfg.interval_minutes = n;
                    }
                }
            }
            "threshold_percent" => {
                if let Ok(n) = value.parse::<u32>() {
                    if n > 0 && n <= 100 {
                        cfg.threshold_percent = n;
                    }
                }
            }
            _ => {}
        }
    }

    cfg
}

/// Finds the `:` separating a JSON key from its value (skipping any inside
/// quoted strings — not needed here, but keeps the naive scan safe).
fn find_key_sep(field: &str) -> Option<usize> {
    let mut in_string = false;
    let mut escaped = false;
    for (i, ch) in field.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' if in_string => escaped = true,
            '"' => in_string = !in_string,
            ':' if !in_string => return Some(i),
            _ => {}
        }
    }
    None
}

/// Splits a JSON object body into top-level `key:value` fields (naive, but
/// sufficient for this flat object; commas inside quoted strings are skipped).
fn split_fields(inner: &str) -> Vec<&str> {
    let mut fields = Vec::new();
    let mut start = 0;
    let mut in_string = false;
    let mut escaped = false;

    for (i, ch) in inner.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' if in_string => escaped = true,
            '"' => in_string = !in_string,
            ',' if !in_string => {
                fields.push(&inner[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    fields.push(&inner[start..]);
    fields
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_round_trip() {
        let dir = std::env::temp_dir().join("ram_optimizer_config_test");
        let path = dir.join("config.json");
        let _ = fs::remove_dir_all(&dir);

        let cfg = Config {
            auto_clean: true,
            interval_minutes: 15,
            threshold_percent: 80,
        };
        save_to_path(&cfg, &path).expect("save should succeed");

        let loaded = load_from_path(&path).expect("load should succeed");
        assert_eq!(loaded, cfg);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_missing_file_returns_defaults() {
        let dir = std::env::temp_dir().join("ram_optimizer_config_missing");
        let path = dir.join("config.json");
        let _ = fs::remove_dir_all(&dir);

        let cfg = load_from_path(&path).unwrap_or_default();
        assert_eq!(cfg, Config::default());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_corrupt_file_returns_defaults() {
        let dir = std::env::temp_dir().join("ram_optimizer_config_corrupt");
        let path = dir.join("config.json");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(&path, "not json at all {{{").unwrap();

        let cfg = load_from_path(&path).unwrap_or_default();
        assert_eq!(cfg, Config::default());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_partial_and_out_of_range_values() {
        // A file missing some fields must fall back to defaults for those.
        let dir = std::env::temp_dir().join("ram_optimizer_config_partial");
        let path = dir.join("config.json");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(&path, r#"{"auto_clean":true}"#).unwrap();

        let cfg = load_from_path(&path).unwrap_or_default();
        assert_eq!(cfg.auto_clean, true);
        assert_eq!(cfg.interval_minutes, 15); // default preserved
        assert_eq!(cfg.threshold_percent, 80); // default preserved

        // Out-of-range values must be ignored, not crash.
        fs::write(&path, r#"{"auto_clean":true,"interval_minutes":0,"threshold_percent":500}"#).unwrap();
        let cfg = load_from_path(&path).unwrap_or_default();
        assert_eq!(cfg.auto_clean, true);
        assert_eq!(cfg.interval_minutes, 15); // 0 rejected -> default
        assert_eq!(cfg.threshold_percent, 80); // 500 rejected -> default

        let _ = fs::remove_dir_all(&dir);
    }
}