//! The error log.

use std::borrow::Cow;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::lock;

const MAX_LEN: usize = 1024 * 1024;
const MAX_ERROR: usize = 8 * 1024;
const TRUNCATED: &str = " …(truncated)";
/// Entries carry repo paths and `bd` output.
const LOG_MODE: u32 = 0o600;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Stage {
    Config,
    Payload,
    Lock,
    Pull,
    Export,
    Vault,
    Render,
    Write,
    Panic,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub ts: Timestamp,
    pub version: String,
    pub command: Option<String>,
    pub repo: Option<PathBuf>,
    pub stage: Stage,
    pub error: String,
}

impl Entry {
    pub fn new(stage: Stage, error: String) -> Entry {
        Entry {
            ts: Timestamp::now(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            command: None,
            repo: None,
            stage,
            error,
        }
    }
}

#[derive(Debug)]
enum Failure {
    Locked,
    Io { path: PathBuf, source: io::Error },
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Failure::Locked => f.write_str("errors.lock is held by another run"),
            Failure::Io { path, source } => write!(f, "{}: {source}", path.display()),
        }
    }
}

impl From<lock::Error> for Failure {
    fn from(e: lock::Error) -> Failure {
        match e {
            lock::Error::Locked => Failure::Locked,
            lock::Error::Io { path, source } => Failure::Io { path, source },
        }
    }
}

fn io_at(path: &Path) -> impl FnOnce(io::Error) -> Failure + use<> {
    let path = path.to_path_buf();
    move |source| Failure::Io { path, source }
}

/// Appends `entry` to the log. Never panics or fails: a hook must not
/// disturb the harness, so a failure goes to stderr and no further.
pub fn record(log: &Path, entry: &Entry) {
    record_within(log, entry, MAX_LEN, lock::TIMEOUT, &mut io::stderr());
}

fn record_within(
    log: &Path,
    entry: &Entry,
    max_len: usize,
    timeout: Duration,
    stderr: &mut dyn Write,
) {
    if let Err(failure) = append(log, entry, max_len, timeout) {
        // `eprintln!` panics on a closed stderr.
        let _ = writeln!(
            stderr,
            "callisto: couldn't write {}: {failure}",
            log.display()
        );
        if let Ok(json) = serde_json::to_string(entry) {
            let _ = writeln!(stderr, "{json}");
        }
    }
}

fn append(log: &Path, entry: &Entry, max_len: usize, timeout: Duration) -> Result<(), Failure> {
    let _lock = lock::acquire_within(&log.with_file_name("errors.lock"), timeout)?;
    let mut text = read(log).map_err(io_at(log))?;
    let mut line = to_line(entry).map_err(|e| io_at(log)(e.into()))?;
    let torn = text.last().is_some_and(|&b| b != b'\n');
    let needed = line.len() + usize::from(torn);
    if text.len() + needed > max_len {
        text = rotate(log, &text, max_len.saturating_sub(needed))?;
    }
    if text.last().is_some_and(|&b| b != b'\n') {
        line.insert(0, b'\n');
    }
    OpenOptions::new()
        .create(true)
        .append(true)
        .mode(LOG_MODE)
        .open(log)
        .and_then(|mut f| f.write_all(&line))
        .map_err(io_at(log))
}

/// Serializes `entry` with a trailing newline, clipping its error so one
/// entry cannot crowd the rest out of the log.
fn to_line(entry: &Entry) -> serde_json::Result<Vec<u8>> {
    let mut line = match clip(&entry.error, MAX_ERROR) {
        Cow::Borrowed(_) => serde_json::to_vec(entry)?,
        Cow::Owned(error) => serde_json::to_vec(&Entry {
            error,
            ..entry.clone()
        })?,
    };
    line.push(b'\n');
    Ok(line)
}

/// Keeps the newest whole lines that fit in `room`, at most the newer
/// half, and returns them. The replacement is renamed into place, so a
/// reader never sees a half-written log; `NamedTempFile` creates it
/// `0600`, keeping the log private.
fn rotate(log: &Path, text: &[u8], room: usize) -> Result<Vec<u8>, Failure> {
    let cut = (text.len() / 2).max(text.len().saturating_sub(room));
    let from = cut.saturating_sub(1);
    let kept = match text[from..].iter().position(|&b| b == b'\n') {
        Some(i) => text[from + i + 1..].to_vec(),
        None => Vec::new(),
    };
    let dir = log.parent().unwrap_or(Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(dir).map_err(io_at(dir))?;
    let tmp_path = tmp.path().to_path_buf();
    tmp.write_all(&kept).map_err(io_at(&tmp_path))?;
    tmp.persist(log).map_err(|e| io_at(log)(e.error))?;
    Ok(kept)
}

fn read(log: &Path) -> io::Result<Vec<u8>> {
    match fs::read(log) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        other => other,
    }
}

/// At most `max` bytes of `s`, cut on a char boundary and marked.
fn clip(s: &str, max: usize) -> Cow<'_, str> {
    if s.len() <= max {
        return Cow::Borrowed(s);
    }
    Cow::Owned(format!("{}{TRUNCATED}", &s[..s.floor_char_boundary(max)]))
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use tempfile::TempDir;

    use super::*;

    struct Env {
        tmp: TempDir,
    }

    impl Env {
        fn new() -> Env {
            Env {
                tmp: tempfile::tempdir().unwrap(),
            }
        }

        fn log(&self) -> PathBuf {
            self.tmp.path().join("state/callisto/errors.jsonl")
        }

        fn record(&self, entry: &Entry) {
            self.record_small(entry, MAX_LEN);
        }

        fn record_small(&self, entry: &Entry, max_len: usize) {
            record_within(&self.log(), entry, max_len, lock::FREE, &mut io::sink());
        }

        fn lines(&self) -> Vec<String> {
            fs::read_to_string(self.log())
                .unwrap()
                .lines()
                .map(String::from)
                .collect()
        }

        fn entries(&self) -> Vec<Entry> {
            self.lines()
                .iter()
                .filter_map(|l| serde_json::from_str(l).ok())
                .collect()
        }

        fn len(&self) -> usize {
            fs::metadata(self.log()).unwrap().len() as usize
        }

        fn mode(&self) -> u32 {
            fs::metadata(self.log()).unwrap().permissions().mode() & 0o777
        }

        fn seed(&self, text: &str) {
            fs::create_dir_all(self.log().parent().unwrap()).unwrap();
            fs::write(self.log(), text).unwrap();
        }
    }

    fn entry(repo: &str) -> Entry {
        Entry {
            repo: Some(PathBuf::from(repo)),
            ..Entry::new(Stage::Export, "boom".into())
        }
    }

    fn line(e: &Entry) -> String {
        serde_json::to_string(e).unwrap() + "\n"
    }

    fn lines_of(repos: &[&str]) -> String {
        repos.iter().map(|r| line(&entry(r))).collect()
    }

    fn repos(env: &Env) -> Vec<String> {
        env.entries()
            .into_iter()
            .map(|e| e.repo.unwrap().display().to_string())
            .collect()
    }

    // --- entry ---

    #[test]
    fn serializes_keys_in_spec_order() {
        let json = serde_json::to_string(&Entry::new(Stage::Export, "x".into())).unwrap();
        let at: Vec<usize> = ["ts", "version", "command", "repo", "stage", "error"]
            .map(|k| json.find(&format!("\"{k}\":")).expect(k))
            .into();
        assert!(at.is_sorted(), "{json}");
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value.as_object().unwrap().len(), 6);
        assert_eq!(value["stage"], "export");
        assert_eq!(value["repo"], serde_json::Value::Null);
        assert!(value["ts"].as_str().unwrap().ends_with('Z'));
    }

    // --- appending ---

    #[test]
    fn appends_one_line_per_entry() {
        let env = Env::new();
        env.record(&entry("a"));
        env.record(&entry("b"));
        assert_eq!(repos(&env), ["a", "b"]);
        assert_eq!(env.lines().len(), 2);
    }

    #[test]
    fn appends_after_a_torn_line() {
        let env = Env::new();
        env.seed("{\"ts\":\"20");
        env.record(&entry("a"));
        let lines = env.lines();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "{\"ts\":\"20");
        assert_eq!(repos(&env), ["a"]);
    }

    #[test]
    fn log_is_private_from_the_first_write() {
        let env = Env::new();
        env.record(&entry("a"));
        assert_eq!(env.mode(), 0o600, "{:o}", env.mode());
    }

    #[test]
    fn rotated_log_stays_private() {
        let env = Env::new();
        for r in ["a", "b", "c", "d"] {
            env.record(&entry(r));
        }
        env.record_small(&entry("e"), env.len() - 1);
        assert!(!repos(&env).contains(&"a".to_string()), "did not rotate");
        assert_eq!(env.mode(), 0o600, "{:o}", env.mode());
    }

    // --- rotation ---

    #[test]
    fn rotation_keeps_the_newer_half_on_a_line_boundary() {
        let env = Env::new();
        for r in ["a", "b", "c", "d"] {
            env.record(&entry(r));
        }
        let before = env.lines();
        env.record_small(&entry("e"), env.len() - 1);
        let after = env.lines();
        assert_eq!(
            after[..after.len() - 1],
            before[before.len() - after.len() + 1..]
        );
        assert!(after.len() < before.len());
        assert!(after.last().unwrap().contains("\"e\""));
    }

    #[test]
    fn rotation_keeping_a_torn_line_starts_a_new_line() {
        let env = Env::new();
        env.seed(&format!("{}{{\"ts\":\"20", lines_of(&["a", "b", "c", "d"])));
        env.record_small(&entry("e"), env.len() - 1);
        let lines = env.lines();
        assert!(!lines[0].contains("\"a\""), "did not rotate");
        assert_eq!(lines[lines.len() - 2], "{\"ts\":\"20");
        assert!(lines[lines.len() - 1].contains("\"e\""));
    }

    #[test]
    fn one_huge_line_empties_the_log() {
        let env = Env::new();
        env.seed(&"x".repeat(100));
        env.record_small(&entry("s"), 50);
        assert_eq!(repos(&env), ["s"]);
        assert_eq!(env.lines().len(), 1);
    }

    #[test]
    fn append_that_would_cross_the_cap_rotates_first() {
        let env = Env::new();
        for r in ["a", "b", "c", "d"] {
            env.record(&entry(r));
        }
        let e = entry("e");
        let max_len = env.len() + line(&e).len() - 1;
        env.record_small(&e, max_len);
        assert!(!env.lines()[0].contains("\"a\""), "did not rotate");
        assert!(env.len() <= max_len);
    }

    #[test]
    fn rotation_cut_on_a_line_start_keeps_that_line() {
        let env = Env::new();
        let ts: Timestamp = "2026-10-01T00:00:00Z".parse().unwrap();
        let text: String = ["a", "b", "c", "d"]
            .map(|r| line(&Entry { ts, ..entry(r) }))
            .concat();
        env.seed(&text);
        let e = entry("e");
        env.record_small(&e, text.len() / 2 + line(&e).len());
        assert_eq!(repos(&env), ["c", "d", "e"]);
    }

    #[test]
    fn large_entry_rotates_away_more_than_half() {
        let env = Env::new();
        let text: String = (0..10).map(|i| line(&entry(&i.to_string()))).collect();
        env.seed(&text);
        let e = Entry {
            error: "x".repeat(text.len() * 3 / 4),
            ..entry("big")
        };
        env.record_small(&e, text.len());
        assert!(env.len() <= text.len(), "{}", env.len());
        assert!(env.lines().last().unwrap().contains("\"big\""));
    }

    #[test]
    fn torn_line_counts_toward_the_cap() {
        let env = Env::new();
        let text = format!("{}{{\"ts\":\"20", lines_of(&["a", "b", "c", "d"]));
        env.seed(&text);
        let e = entry("e");
        let max_len = text.len() + line(&e).len();
        env.record_small(&e, max_len);
        assert!(env.len() <= max_len, "{}", env.len());
    }

    #[test]
    fn entry_longer_than_the_cap_is_written_alone() {
        let env = Env::new();
        env.seed("{}\n");
        env.record_small(&entry("b"), 50);
        assert_eq!(repos(&env), ["b"]);
        assert_eq!(env.lines().len(), 1);
    }

    #[test]
    fn log_stays_within_the_cap() {
        let env = Env::new();
        let max_len = 4 * MAX_ERROR;
        for i in 0..40 {
            let e = Entry {
                error: "x".repeat(i * MAX_ERROR / 10),
                ..entry(&i.to_string())
            };
            env.record_small(&e, max_len);
            assert!(env.len() <= max_len, "entry {i}: {}", env.len());
        }
    }

    #[test]
    fn long_error_is_truncated_on_a_char_boundary() {
        let env = Env::new();
        let error = "é".repeat(MAX_ERROR);
        env.record(&Entry {
            error: error.clone(),
            ..entry("a")
        });
        let got = &env.entries()[0].error;
        let kept = got.strip_suffix(TRUNCATED).expect(got);
        assert!(kept.len() <= MAX_ERROR && kept.len() > MAX_ERROR - 2);
        assert!(error.starts_with(kept));
    }

    #[test]
    fn short_error_is_kept_whole() {
        let env = Env::new();
        let e = Entry {
            error: "x".repeat(MAX_ERROR),
            ..entry("a")
        };
        env.record(&e);
        assert_eq!(env.entries(), [e]);
    }

    // --- fallback ---

    fn record_to_stderr(env: &Env, e: &Entry, timeout: Duration) -> String {
        let mut stderr = Vec::new();
        record_within(&env.log(), e, MAX_LEN, timeout, &mut stderr);
        String::from_utf8(stderr).unwrap()
    }

    #[test]
    fn unwritable_log_falls_back_to_stderr() {
        let env = Env::new();
        let state = env.log().parent().unwrap().to_path_buf();
        fs::create_dir_all(&state).unwrap();
        fs::set_permissions(&state, fs::Permissions::from_mode(0o555)).unwrap();
        let e = Entry::new(Stage::Write, "boom".into());
        let text = record_to_stderr(&env, &e, lock::FREE);
        fs::set_permissions(&state, fs::Permissions::from_mode(0o755)).unwrap();
        let log = env.log().display().to_string();
        assert!(
            text.starts_with(&format!("callisto: couldn't write {log}: ")),
            "{text}"
        );
        assert!(text.contains("\"stage\":\"write\""), "{text}");
    }

    #[test]
    fn held_log_lock_falls_back_to_stderr() {
        let env = Env::new();
        let _held = lock::acquire(&env.log().with_file_name("errors.lock")).unwrap();
        let e = entry("a");
        let text = record_to_stderr(&env, &e, Duration::ZERO);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "{text}");
        assert_eq!(
            lines[0],
            format!(
                "callisto: couldn't write {}: errors.lock is held by another run",
                env.log().display()
            )
        );
        assert_eq!(serde_json::from_str::<Entry>(lines[1]).unwrap(), e);
        assert!(!env.log().exists());
    }
}
