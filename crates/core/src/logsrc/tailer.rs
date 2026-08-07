//! Following a log file as it grows.
//!
//! Roblox appends to a file the scanner does not own, so three things can happen
//! between reads and all three must be survivable:
//!
//! - **Append** — the normal case; read from the stored offset.
//! - **Truncation** — the file shrank, so our offset points past the end. This
//!   means the client restarted and reused the name; start over from zero.
//! - **Rotation** — a new file appeared and the old one stopped growing. Handled
//!   above this type, by re-running the finder.
//!
//! The file is opened per read rather than held open, which keeps a Windows file
//! handle from interfering with the writer and makes rotation detection trivial.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use super::resume::{find_resume_point, ResumePoint};

/// Upper bound on lines returned from a single read, so a large backlog is
/// delivered in chunks rather than blocking the caller for a whole file.
const MAX_LINES_PER_READ: usize = 500;

pub struct Tailer {
    path: PathBuf,
    offset: u64,
}

/// What a read produced.
#[derive(Debug, Default, PartialEq)]
pub struct Batch {
    pub lines: Vec<String>,
    /// Set when the file shrank and we restarted from the beginning. The caller
    /// must reset per-session state, because line numbering is no longer
    /// continuous with what it has already seen.
    pub restarted: bool,
    /// True when more lines remain past `MAX_LINES_PER_READ`.
    pub more_pending: bool,
}

impl Tailer {
    /// Begin at a byte offset, typically from [`find_resume_point`].
    pub fn at_offset(path: impl Into<PathBuf>, offset: u64) -> Self {
        Self {
            path: path.into(),
            offset,
        }
    }

    /// Begin at the most recent run start, skipping the lobby.
    ///
    /// Also returns the state established before that point — the server address
    /// in particular, which is logged before the run begins and would otherwise
    /// be lost.
    pub fn at_run_start(path: impl Into<PathBuf>) -> io::Result<(Self, ResumePoint)> {
        let path = path.into();
        let mut reader = BufReader::new(File::open(&path)?);
        let point = find_resume_point(&mut reader)?;
        Ok((
            Self {
                path,
                offset: point.offset,
            },
            point,
        ))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn offset(&self) -> u64 {
        self.offset
    }

    /// Read whatever has been appended since the last call.
    pub fn read_new(&mut self) -> io::Result<Batch> {
        let file = File::open(&self.path)?;
        let len = file.metadata()?.len();

        let mut batch = Batch::default();

        if len < self.offset {
            // Truncated: the client restarted into the same filename.
            self.offset = 0;
            batch.restarted = true;
        }
        if len == self.offset {
            return Ok(batch);
        }

        let mut reader = BufReader::new(file);
        reader.seek(SeekFrom::Start(self.offset))?;

        let mut consumed = 0u64;
        let mut line = String::new();
        while batch.lines.len() < MAX_LINES_PER_READ {
            line.clear();
            let read = reader.read_line(&mut line)?;
            if read == 0 {
                break;
            }
            // A final line with no newline is still being written. Leave the
            // offset before it so the next read picks it up whole; otherwise a
            // room name could be split across two reads and parse as garbage.
            if !line.ends_with('\n') {
                break;
            }
            consumed += read as u64;
            batch.lines.push(line.trim_end().to_string());
        }

        self.offset += consumed;
        batch.more_pending = self.offset < len && batch.lines.len() >= MAX_LINES_PER_READ;
        Ok(batch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn append(path: &Path, text: &str) {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        f.write_all(text.as_bytes()).unwrap();
    }

    fn temp() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("player.log");
        (dir, path)
    }

    #[test]
    fn reads_appended_lines_once() {
        let (_d, path) = temp();
        append(&path, "one\ntwo\n");

        let mut t = Tailer::at_offset(&path, 0);
        assert_eq!(t.read_new().unwrap().lines, vec!["one", "two"]);

        // Nothing new: must not re-deliver. This is the property whose absence
        // made v1 re-report every room on restart.
        assert_eq!(t.read_new().unwrap().lines, Vec::<String>::new());

        append(&path, "three\n");
        assert_eq!(t.read_new().unwrap().lines, vec!["three"]);
    }

    #[test]
    fn withholds_a_partially_written_line() {
        let (_d, path) = temp();
        append(&path, "complete\npartial-no-newline");

        let mut t = Tailer::at_offset(&path, 0);
        let batch = t.read_new().unwrap();
        assert_eq!(batch.lines, vec!["complete"], "partial line held back");

        // Once the writer finishes the line, we get it whole.
        append(&path, "-now-done\n");
        assert_eq!(
            t.read_new().unwrap().lines,
            vec!["partial-no-newline-now-done"]
        );
    }

    #[test]
    fn restarts_when_the_file_is_truncated() {
        let (_d, path) = temp();
        append(&path, "old content that is long\n");

        let mut t = Tailer::at_offset(&path, 0);
        t.read_new().unwrap();

        std::fs::write(&path, "fresh\n").unwrap();
        let batch = t.read_new().unwrap();

        assert!(batch.restarted, "shrinking file signals a restart");
        assert_eq!(batch.lines, vec!["fresh"]);
    }

    #[test]
    fn caps_a_large_backlog_and_flags_more() {
        let (_d, path) = temp();
        let big: String = (0..MAX_LINES_PER_READ + 50)
            .map(|i| format!("line{i}\n"))
            .collect();
        append(&path, &big);

        let mut t = Tailer::at_offset(&path, 0);
        let first = t.read_new().unwrap();
        assert_eq!(first.lines.len(), MAX_LINES_PER_READ);
        assert!(first.more_pending);

        let second = t.read_new().unwrap();
        assert_eq!(second.lines.len(), 50);
        assert!(!second.more_pending);
    }

    #[test]
    fn strips_line_endings() {
        let (_d, path) = temp();
        append(&path, "windows\r\nunix\n");

        let mut t = Tailer::at_offset(&path, 0);
        assert_eq!(t.read_new().unwrap().lines, vec!["windows", "unix"]);
    }

    #[test]
    fn starting_at_the_run_start_skips_earlier_lines() {
        let (_d, path) = temp();
        append(
            &path,
            "2026-01-01T00:00:00Z,1.0,a,7 [FLog::Network] UDMUX Address = 1.2.3.4, Port = 1\n\
             2026-01-01T00:00:01Z,2.0,a,6,Info [FLog::CreatorOutput] Room Name: Lobby\n\
             2026-01-01T00:00:02Z,3.0,a,6,Info [FLog::CreatorOutput] Room Name: Start\n\
             2026-01-01T00:00:03Z,4.0,a,6,Info [FLog::CreatorOutput] Room Name: Straight2\n",
        );

        let (mut t, point) = Tailer::at_run_start(&path).unwrap();
        assert_eq!(point.server_ip.as_deref(), Some("1.2.3.4"));

        let lines = t.read_new().unwrap().lines;
        assert_eq!(lines.len(), 2, "only the run, not the lobby");
        assert!(lines[0].contains("Room Name: Start"));
    }

    #[test]
    fn a_missing_file_is_an_error_not_a_panic() {
        let mut t = Tailer::at_offset("/definitely/not/here.log", 0);
        assert!(t.read_new().is_err());
    }
}
