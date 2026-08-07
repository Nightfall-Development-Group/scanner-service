//! Persistent user settings.
//!
//! v1 stored these as a flat JSON file rewritten in place on every change, with
//! no locking and no atomic rename. An interrupted write left truncated JSON,
//! the reader swallowed the parse error and returned defaults, and the user's
//! settings were gone with nothing reported. Two things here prevent that:
//!
//! 1. Writes go to a temp file in the same directory and are then renamed over
//!    the target, which is atomic on every platform we ship to. A crash leaves
//!    either the old file or the new one, never a half-written one.
//! 2. [`Config::load`] returns an error rather than defaults. A caller that
//!    cannot read the file must decide what to do; it must not silently save
//!    over a file it failed to parse.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("could not locate a config directory for this platform")]
    NoConfigDir,
    #[error("reading {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("writing {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{path} is not valid config JSON: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

/// Every field carries a default, and `#[serde(default)]` on the container means
/// a file missing any of them still loads. v1 declared only one key centrally and
/// duplicated the other seven defaults at each read site, so they drifted.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// The user's personal db-api key. Empty until they enter one; the app is
    /// unusable without it, since the API has no anonymous access.
    pub api_key: String,
    /// Explicit Roblox log directory or file. `None` means auto-detect.
    pub log_path: Option<PathBuf>,
    pub window: WindowConfig,
    pub images: ImageConfig,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowConfig {
    pub opacity: f32,
    pub scale: f32,
    pub always_on_top: bool,
    /// Logical size, persisted across runs.
    pub size: [f32; 2],
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ImageConfig {
    pub auto_rotate: bool,
    pub rotate_interval_secs: f32,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            opacity: 1.0,
            scale: 1.0,
            always_on_top: true,
            size: [900.0, 640.0],
        }
    }
}

impl Default for ImageConfig {
    fn default() -> Self {
        Self {
            auto_rotate: false,
            rotate_interval_secs: 5.0,
        }
    }
}

impl Config {
    /// Default on-disk location, e.g. `%APPDATA%\NightfallDivision\scanner\config.json`.
    pub fn default_path() -> Result<PathBuf, ConfigError> {
        let dirs = directories::ProjectDirs::from("com", "NightfallDivision", "scanner")
            .ok_or(ConfigError::NoConfigDir)?;
        Ok(dirs.config_dir().join("config.json"))
    }

    /// Load from `path`. A missing file yields defaults — that is a first run,
    /// not a failure. A file that exists but does not parse is an error, so the
    /// caller can warn instead of overwriting it.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = match fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(source) => {
                return Err(ConfigError::Read {
                    path: path.to_owned(),
                    source,
                })
            }
        };
        serde_json::from_str(&text).map_err(|source| ConfigError::Parse {
            path: path.to_owned(),
            source,
        })
    }

    /// Write atomically: temp file in the same directory, flushed and synced,
    /// then renamed over the target. Same-directory matters — a rename across
    /// filesystems is not atomic.
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        let dir = path.parent().ok_or(ConfigError::NoConfigDir)?;
        fs::create_dir_all(dir).map_err(|source| ConfigError::Write {
            path: dir.to_owned(),
            source,
        })?;

        let json = serde_json::to_vec_pretty(self).expect("Config always serializes");

        let write = || -> std::io::Result<()> {
            let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
            tmp.write_all(&json)?;
            tmp.as_file().sync_all()?;
            tmp.persist(path)?;
            Ok(())
        };
        write().map_err(|source| ConfigError::Write {
            path: path.to_owned(),
            source,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir() -> tempfile::TempDir {
        tempfile::tempdir().expect("temp dir")
    }

    #[test]
    fn missing_file_is_a_first_run_not_an_error() {
        let dir = tmpdir();
        let cfg = Config::load(&dir.path().join("nope.json")).unwrap();
        assert_eq!(cfg, Config::default());
    }

    #[test]
    fn round_trips() {
        let dir = tmpdir();
        let path = dir.path().join("config.json");
        let cfg = Config {
            api_key: "test-key".into(),
            window: WindowConfig {
                opacity: 0.7,
                ..Default::default()
            },
            images: ImageConfig {
                auto_rotate: true,
                ..Default::default()
            },
            ..Default::default()
        };

        cfg.save(&path).unwrap();
        assert_eq!(Config::load(&path).unwrap(), cfg);
    }

    #[test]
    fn a_partial_file_fills_in_defaults() {
        // The forward-compatibility property: a config written by an older build
        // must still load once we add fields.
        let dir = tmpdir();
        let path = dir.path().join("config.json");
        fs::write(&path, r#"{"api_key":"abc"}"#).unwrap();

        let cfg = Config::load(&path).unwrap();
        assert_eq!(cfg.api_key, "abc");
        assert_eq!(cfg.window.opacity, WindowConfig::default().opacity);
        assert_eq!(cfg.images.rotate_interval_secs, 5.0);
    }

    #[test]
    fn corrupt_file_errors_instead_of_silently_resetting() {
        // This is the v1 bug, asserted as a regression test: a truncated file
        // must not read back as defaults, because the caller would then save
        // over it and destroy the user's settings.
        let dir = tmpdir();
        let path = dir.path().join("config.json");
        fs::write(&path, r#"{"api_key":"abc""#).unwrap();

        assert!(matches!(
            Config::load(&path),
            Err(ConfigError::Parse { .. })
        ));
    }

    #[test]
    fn save_creates_missing_directories() {
        let dir = tmpdir();
        let path = dir.path().join("a").join("b").join("config.json");
        Config::default().save(&path).unwrap();
        assert!(path.exists());
    }

    #[test]
    fn save_replaces_an_existing_file_wholesale() {
        let dir = tmpdir();
        let path = dir.path().join("config.json");
        fs::write(&path, "{\"api_key\":\"old\"}").unwrap();

        let cfg = Config {
            api_key: "new".into(),
            ..Default::default()
        };
        cfg.save(&path).unwrap();

        assert_eq!(Config::load(&path).unwrap().api_key, "new");
    }
}
