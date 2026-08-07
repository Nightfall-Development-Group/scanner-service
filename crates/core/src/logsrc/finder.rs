//! Locating the Roblox client log directory.
//!
//! Windows and macOS have one well-known path each. Linux has none — Roblox does
//! not ship a native client, so the logs live inside whichever compatibility
//! prefix the user runs it under (Wine, Proton, Sober), at a path containing a
//! Windows username nobody can predict. Hence the wildcard expansion.
//!
//! v1 read the user's override once at module import, which is why changing it
//! demanded an application restart. Here it is an argument.

use std::fs;
use std::path::{Path, PathBuf};

/// Where logs were found, and how. Worth surfacing in the debug console: "no
/// logs found" and "found the directory but it is empty" are different problems.
#[derive(Debug, Clone, PartialEq)]
pub enum LogSource {
    /// The user pointed us at a specific file.
    ExplicitFile(PathBuf),
    /// The user pointed us at a directory; we picked the newest log in it.
    ExplicitDir { dir: PathBuf, newest: PathBuf },
    /// Found by platform convention.
    Detected { dir: PathBuf, newest: PathBuf },
}

impl LogSource {
    /// The file to actually read.
    pub fn path(&self) -> &Path {
        match self {
            Self::ExplicitFile(p) => p,
            Self::ExplicitDir { newest, .. } | Self::Detected { newest, .. } => newest,
        }
    }

    /// The directory to watch for newer logs, if there is one.
    pub fn dir(&self) -> Option<&Path> {
        match self {
            Self::ExplicitFile(_) => None,
            Self::ExplicitDir { dir, .. } | Self::Detected { dir, .. } => Some(dir),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum FindError {
    #[error("the configured log path does not exist: {0}")]
    ConfiguredPathMissing(PathBuf),
    #[error("no Roblox log directory found; set one manually in settings")]
    NoDirectory,
    #[error("{0} contains no log files yet — join a game first")]
    DirectoryEmpty(PathBuf),
}

/// Resolve which log file to read.
///
/// `override_path` may be a file or a directory; when set it is used verbatim
/// rather than being treated as a hint, so a user who has pointed us somewhere
/// specific never silently gets a different file.
pub fn find_log(override_path: Option<&Path>) -> Result<LogSource, FindError> {
    if let Some(p) = override_path {
        if p.is_file() {
            return Ok(LogSource::ExplicitFile(p.to_owned()));
        }
        if p.is_dir() {
            let newest = newest_log_in(p).ok_or_else(|| FindError::DirectoryEmpty(p.to_owned()))?;
            return Ok(LogSource::ExplicitDir {
                dir: p.to_owned(),
                newest,
            });
        }
        return Err(FindError::ConfiguredPathMissing(p.to_owned()));
    }

    // Several candidates can exist at once (a Wine prefix and a Sober install,
    // say). Pick the directory holding the single newest log rather than the
    // first directory that happens to exist.
    let mut best: Option<(std::time::SystemTime, PathBuf, PathBuf)> = None;
    for dir in default_log_dirs() {
        let Some(newest) = newest_log_in(&dir) else {
            continue;
        };
        let Ok(mtime) = fs::metadata(&newest).and_then(|m| m.modified()) else {
            continue;
        };
        if best.as_ref().is_none_or(|(best_t, _, _)| mtime > *best_t) {
            best = Some((mtime, dir, newest));
        }
    }

    match best {
        Some((_, dir, newest)) => Ok(LogSource::Detected { dir, newest }),
        None => Err(FindError::NoDirectory),
    }
}

/// Newest `*.log` in `dir`, by modification time.
pub fn newest_log_in(dir: &Path) -> Option<PathBuf> {
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        // Roblox writes `<version>_<timestamp>_Player_<id>_last.log`. Restrict to
        // `.log` so a crash dump or config file in the same directory cannot be
        // mistaken for a log, which v1's "newest file of any kind" would do.
        if path
            .extension()
            .is_none_or(|e| !e.eq_ignore_ascii_case("log"))
        {
            continue;
        }
        let Ok(mtime) = entry.metadata().and_then(|m| m.modified()) else {
            continue;
        };
        if best.as_ref().is_none_or(|(t, _)| mtime > *t) {
            best = Some((mtime, path));
        }
    }
    best.map(|(_, p)| p)
}

/// Candidate log directories for this platform, most conventional first.
pub fn default_log_dirs() -> Vec<PathBuf> {
    let home = home_dir();

    if cfg!(target_os = "windows") {
        return std::env::var_os("LOCALAPPDATA")
            .map(|p| vec![PathBuf::from(p).join("Roblox").join("logs")])
            .unwrap_or_default();
    }

    if cfg!(target_os = "macos") {
        return home
            .map(|h| vec![h.join("Library").join("Logs").join("Roblox")])
            .unwrap_or_default();
    }

    // Linux: Roblox runs under a compatibility layer, so the logs sit inside a
    // Windows-shaped path within some prefix.
    let Some(home) = home else {
        return Vec::new();
    };
    const WINE_TAIL: &str = "drive_c/users/*/AppData/Local/Roblox/logs";
    const WINE_TAIL_LEGACY: &str = "drive_c/users/*/Local Settings/Application Data/Roblox/logs";

    let mut out = Vec::new();
    for prefix in [
        home.join(".wine"),
        home.join(".local/share/sober/prefix"),
        home.join(".var/app/org.vinegarhq.Sober/data/sober/prefix"),
    ] {
        out.extend(expand_wildcards(&prefix.join(WINE_TAIL)));
        out.extend(expand_wildcards(&prefix.join(WINE_TAIL_LEGACY)));
    }
    // Proton keeps one prefix per app id.
    for steam in [
        home.join(".steam/steam/steamapps/compatdata"),
        home.join(".local/share/Steam/steamapps/compatdata"),
    ] {
        out.extend(expand_wildcards(&steam.join("*/pfx").join(WINE_TAIL)));
        out.extend(expand_wildcards(
            &steam.join("*/pfx").join(WINE_TAIL_LEGACY),
        ));
    }
    // Sober's native layout.
    out.push(home.join(".local/share/sober/logs"));
    out.retain(|p| p.is_dir());
    out
}

fn home_dir() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|d| d.home_dir().to_owned())
}

/// Expand `*` path components against the filesystem.
///
/// Deliberately tiny: only whole-or-partial `*` within a single component, which
/// is all these patterns need. Avoids a glob dependency for ~30 lines.
fn expand_wildcards(pattern: &Path) -> Vec<PathBuf> {
    let mut results = vec![PathBuf::new()];

    for component in pattern.components() {
        let segment = component.as_os_str().to_string_lossy().into_owned();

        if !segment.contains('*') {
            for path in &mut results {
                path.push(&segment);
            }
            continue;
        }

        let mut expanded = Vec::new();
        for base in &results {
            let Ok(entries) = fs::read_dir(base) else {
                continue;
            };
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if glob_segment_matches(&segment, &name) {
                    expanded.push(entry.path());
                }
            }
        }
        results = expanded;

        if results.is_empty() {
            break;
        }
    }

    results
}

/// Match one path segment against a pattern containing a single `*`.
fn glob_segment_matches(pattern: &str, name: &str) -> bool {
    match pattern.split_once('*') {
        None => pattern == name,
        Some((prefix, suffix)) => {
            name.len() >= prefix.len() + suffix.len()
                && name.starts_with(prefix)
                && name.ends_with(suffix)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;

    fn touch(path: &Path, contents: &str) {
        if let Some(p) = path.parent() {
            fs::create_dir_all(p).unwrap();
        }
        let mut f = File::create(path).unwrap();
        f.write_all(contents.as_bytes()).unwrap();
    }

    #[test]
    fn glob_matches_whole_segment() {
        assert!(glob_segment_matches("*", "anything"));
        assert!(glob_segment_matches("*", ""));
    }

    #[test]
    fn glob_matches_prefix_and_suffix() {
        assert!(glob_segment_matches("*_last.log", "abc_last.log"));
        assert!(!glob_segment_matches("*_last.log", "abc.log"));
        assert!(glob_segment_matches("pfx*", "pfx"));
    }

    #[test]
    fn glob_does_not_overlap_prefix_and_suffix() {
        // "ab*ba" must not match "aba" by letting the halves share a character.
        assert!(!glob_segment_matches("ab*ba", "aba"));
        assert!(glob_segment_matches("ab*ba", "abba"));
    }

    #[test]
    fn an_explicit_file_is_used_verbatim() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("chosen.log");
        touch(&file, "x");

        let found = find_log(Some(&file)).unwrap();
        assert_eq!(found, LogSource::ExplicitFile(file.clone()));
        assert_eq!(found.path(), file);
        assert_eq!(found.dir(), None, "a pinned file has no directory to watch");
    }

    #[test]
    fn an_explicit_dir_picks_the_newest_log() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("old.log"), "old");
        std::thread::sleep(std::time::Duration::from_millis(20));
        touch(&dir.path().join("new.log"), "new");

        let found = find_log(Some(dir.path())).unwrap();
        assert_eq!(found.path().file_name().unwrap(), "new.log");
        assert_eq!(found.dir(), Some(dir.path()));
    }

    #[test]
    fn non_log_files_are_ignored() {
        // v1 took the newest file of any type, so a crash dump written after the
        // log would be parsed as one.
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("real.log"), "log");
        std::thread::sleep(std::time::Duration::from_millis(20));
        touch(&dir.path().join("crash.dmp"), "not a log");

        let found = find_log(Some(dir.path())).unwrap();
        assert_eq!(found.path().file_name().unwrap(), "real.log");
    }

    #[test]
    fn an_empty_directory_is_a_distinct_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            find_log(Some(dir.path())),
            Err(FindError::DirectoryEmpty(_))
        ));
    }

    #[test]
    fn a_missing_configured_path_is_reported() {
        let missing = PathBuf::from("/definitely/not/here");
        assert!(matches!(
            find_log(Some(&missing)),
            Err(FindError::ConfiguredPathMissing(_))
        ));
    }

    #[test]
    fn expands_a_wildcard_directory() {
        let root = tempfile::tempdir().unwrap();
        let real = root.path().join("users/SomeUser/logs");
        fs::create_dir_all(&real).unwrap();

        let found = expand_wildcards(&root.path().join("users/*/logs"));
        assert_eq!(found, vec![real]);
    }

    #[test]
    fn a_wildcard_matching_nothing_yields_nothing() {
        let root = tempfile::tempdir().unwrap();
        assert!(expand_wildcards(&root.path().join("nope/*/logs")).is_empty());
    }
}
