//! Learned compression profile — persisted settings derived from the `tare learn` command.
//!
//! Location (first match wins):
//!   1. `$TARE_PROFILE` — explicit override, useful for tests and multi-project setups
//!   2. `$XDG_CONFIG_HOME/tare/profile.json`
//!   3. `$HOME/.config/tare/profile.json`

use std::path::PathBuf;

/// Compression settings learned from a real codebase or session corpus.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Profile {
    /// Number of most-recent turns to keep fully verbatim (sane default: 4).
    pub recommended_recency_keep: usize,
    /// Apply code skeletonization by default for code blocks.
    pub lossy_code: bool,
    /// Maximum rows to emit for tabular data (0 = off).
    pub lossy_tabular_max_rows: usize,
    /// Maximum field width for tabular data (0 = off).
    pub lossy_tabular_max_field: usize,
    /// Overall lossless compression ratio observed during the learn run.
    pub measured_ratio: f64,
    /// One-line human-readable summary of what the profile represents.
    pub summary: String,
    /// Path or description of the source the profile was learned from.
    pub source: String,
}

/// Returns the path where the profile is stored.
///
/// Resolution order:
/// 1. `$TARE_PROFILE`
/// 2. `$XDG_CONFIG_HOME/tare/profile.json`
/// 3. `$HOME/.config/tare/profile.json`
pub fn path() -> PathBuf {
    if let Ok(p) = std::env::var("TARE_PROFILE") {
        return PathBuf::from(p);
    }
    let config_base = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_default();
            PathBuf::from(home).join(".config")
        });
    config_base.join("tare").join("profile.json")
}

/// Load and parse the profile from [`path()`]. Returns `None` if the file is missing or invalid.
pub fn load() -> Option<Profile> {
    let data = std::fs::read_to_string(path()).ok()?;
    serde_json::from_str(&data).ok()
}

/// Write `p` to [`path()`] as pretty-printed JSON, creating parent directories as needed.
pub fn save(p: &Profile) -> std::io::Result<()> {
    let out = path();
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(p)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(&out, json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_load_round_trips() {
        // Point $TARE_PROFILE at a process-unique temp file so the test is hermetic
        // and never touches the user's real config directory.
        let dir = std::env::temp_dir().join(format!("tare-profile-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let profile_path = dir.join("profile.json");
        std::env::set_var("TARE_PROFILE", &profile_path);

        let original = Profile {
            recommended_recency_keep: 4,
            lossy_code: true,
            lossy_tabular_max_rows: 100,
            lossy_tabular_max_field: 64,
            measured_ratio: 0.42,
            summary: "test corpus".to_string(),
            source: "/tmp/corpus".to_string(),
        };

        save(&original).expect("save must succeed");
        let loaded = load().expect("load must return Some after save");
        assert_eq!(original, loaded);

        // Restore environment and clean up.
        std::env::remove_var("TARE_PROFILE");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
