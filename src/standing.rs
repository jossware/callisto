use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::errlog::{self, Entry, Stage};
use crate::group::Group;
use crate::lock::Lock;
use crate::paths;

const SHOWN_IDS: usize = 3;

#[derive(Debug, Clone, PartialEq)]
pub enum Condition {
    Unclaimed { ids: Vec<String> },
    MissingProject { project: String, ids: Vec<String> },
    UnfiledMissing { folder: PathBuf },
    IndexTaken { path: PathBuf },
    Unresolvable { path: PathBuf, reason: String },
    GroupRule { reason: String },
    UnlistedSkipped,
}

type OrderKey = (u8, String, String, Vec<String>);

impl Condition {
    pub fn fails_sync(&self) -> bool {
        !matches!(
            self,
            Condition::Unclaimed { .. } | Condition::MissingProject { .. }
        )
    }

    fn rank(&self) -> u8 {
        match self {
            Condition::Unclaimed { .. } => 0,
            Condition::MissingProject { .. } => 1,
            Condition::UnfiledMissing { .. } => 2,
            Condition::IndexTaken { .. } => 3,
            Condition::Unresolvable { .. } => 4,
            Condition::GroupRule { .. } => 5,
            Condition::UnlistedSkipped => 6,
        }
    }

    fn order_key(&self) -> OrderKey {
        let rank = self.rank();
        let none = String::new;
        match self {
            Condition::Unclaimed { ids } => (rank, none(), none(), sorted_ids(ids)),
            Condition::MissingProject { project, ids } => {
                (rank, project.clone(), none(), sorted_ids(ids))
            }
            Condition::UnfiledMissing { folder } => {
                (rank, folder.display().to_string(), none(), vec![])
            }
            Condition::IndexTaken { path } => (rank, path.display().to_string(), none(), vec![]),
            Condition::Unresolvable { path, reason } => {
                (rank, path.display().to_string(), reason.clone(), vec![])
            }
            Condition::GroupRule { reason } => (rank, none(), reason.clone(), vec![]),
            Condition::UnlistedSkipped => (rank, none(), none(), vec![]),
        }
    }

    fn text(&self) -> String {
        match self {
            Condition::Unclaimed { ids } => {
                let (count, shown) = summarize_ids(ids);
                let noun = if count == 1 { "bead" } else { "beads" };
                format!("{count} unclaimed {noun} ({shown})")
            }
            Condition::MissingProject { project, ids } => {
                let (count, shown) = summarize_ids(ids);
                let noun = if count == 1 { "bead" } else { "beads" };
                format!("{count} {noun} claimed for missing project \"{project}\" ({shown})")
            }
            Condition::UnfiledMissing { folder } => format!(
                "unfiled folder \"{}\" is missing; index not written",
                folder.display()
            ),
            Condition::IndexTaken { path } => format!(
                "\"{}\" exists and callisto did not write it; index not written",
                path.display()
            ),
            Condition::Unresolvable { path, reason } => {
                format!("table entry {} is unresolvable: {reason}", path.display())
            }
            Condition::GroupRule { reason } => format!("group not synced: {reason}"),
            Condition::UnlistedSkipped => {
                "not in repos and a table entry is unresolvable; nothing synced".to_string()
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Subject {
    Group { database: PathBuf, repo: PathBuf },
    Config,
    Checkout(PathBuf),
}

impl Subject {
    pub fn group(group: &Group) -> Subject {
        Subject::Group {
            database: group.database.clone(),
            repo: group.members[0].root.clone(),
        }
    }

    pub fn key(&self) -> String {
        match self {
            Subject::Group { database, .. } => format!("group:{}", database.display()),
            Subject::Config => "config".to_string(),
            Subject::Checkout(root) => format!("checkout:{}", root.display()),
        }
    }

    pub fn label(&self) -> String {
        match self {
            Subject::Group { repo, .. } => repo.display().to_string(),
            Subject::Config => "config".to_string(),
            Subject::Checkout(root) => root.display().to_string(),
        }
    }

    pub fn repo(&self) -> Option<&Path> {
        match self {
            Subject::Group { repo, .. } => Some(repo),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Summary {
    pub subject: Subject,
    pub conditions: Vec<Condition>,
}

impl Summary {
    pub fn text(&self) -> String {
        if self.conditions.is_empty() {
            return "standing conditions cleared".to_string();
        }
        let mut sorted: Vec<&Condition> = self.conditions.iter().collect();
        sorted.sort_by_key(|c| c.order_key());
        sorted
            .iter()
            .map(|c| c.text())
            .collect::<Vec<_>>()
            .join("; ")
    }

    pub fn fails_sync(&self) -> bool {
        self.conditions.iter().any(Condition::fails_sync)
    }
}

/// The config has no standing conditions yet. Its summary is still
/// recorded so a stale entry clears.
pub fn config_summary() -> Summary {
    Summary {
        subject: Subject::Config,
        conditions: Vec::new(),
    }
}

pub fn any_fails(summaries: &[Summary]) -> bool {
    summaries.iter().any(Summary::fails_sync)
}

fn sorted_ids(ids: &[String]) -> Vec<String> {
    let mut v = ids.to_vec();
    v.sort_by(|a, b| natural_cmp(a, b));
    v.dedup();
    v
}

fn summarize_ids(ids: &[String]) -> (usize, String) {
    let v = sorted_ids(ids);
    let mut shown = v
        .iter()
        .take(SHOWN_IDS)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    if v.len() > SHOWN_IDS {
        shown.push_str(", …");
    }
    (v.len(), shown)
}

fn runs(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut prev: Option<bool> = None;
    for (i, ch) in s.char_indices() {
        let digit = ch.is_ascii_digit();
        if prev.is_some_and(|p| p != digit) {
            out.push(&s[start..i]);
            start = i;
        }
        prev = Some(digit);
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (ra, rb) = (runs(a), runs(b));
    for (x, y) in ra.iter().zip(&rb) {
        let is_digits = |r: &str| r.starts_with(|c: char| c.is_ascii_digit());
        let ord = if is_digits(x) && is_digits(y) {
            // Unparsed, so a run of any length keeps the order total.
            let (x, y) = (x.trim_start_matches('0'), y.trim_start_matches('0'));
            x.len().cmp(&y.len()).then_with(|| x.cmp(y))
        } else {
            x.cmp(y)
        };
        if ord != Ordering::Equal {
            return ord;
        }
    }
    ra.len().cmp(&rb.len()).then_with(|| a.cmp(b))
}

/// Logs each summary that differs from the last one logged for its
/// subject. Holding the sync lock keeps the state file single-writer.
pub fn record(_lock: &Lock, state: &Path, command: &str, summaries: &[Summary]) {
    record_to(state, command, summaries, &mut io::stderr());
}

fn record_to(state: &Path, command: &str, summaries: &[Summary], stderr: &mut dyn Write) {
    let mut last = read_state(state);
    let mut changed = false;
    for summary in summaries {
        let key = summary.subject.key();
        let text = summary.text();
        let clear = summary.conditions.is_empty();
        let due = if clear {
            last.contains_key(&key)
        } else {
            last.get(&key) != Some(&text)
        };
        if !due {
            continue;
        }
        let mut entry = Entry::new(Stage::Conditions, text.clone());
        entry.command = Some(command.to_string());
        entry.repo = summary.subject.repo().map(Path::to_path_buf);
        // Saving a summary the log never got would hide it from every
        // later run.
        if !errlog::record_to(&paths::error_log(state), &entry, stderr) {
            continue;
        }
        if clear {
            last.remove(&key);
        } else {
            last.insert(key, text);
        }
        changed = true;
    }
    if changed && let Err(e) = write_state(state, &last) {
        // `eprintln!` panics on a closed stderr.
        let _ = writeln!(
            stderr,
            "callisto: couldn't write {}: {e}",
            paths::conditions_state(state).display()
        );
    }
}

fn read_state(state: &Path) -> BTreeMap<String, String> {
    fs::read(paths::conditions_state(state))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn write_state(state: &Path, map: &BTreeMap<String, String>) -> io::Result<()> {
    fs::create_dir_all(state)?;
    let mut tmp = tempfile::NamedTempFile::new_in(state)?;
    tmp.write_all(&serde_json::to_vec(map)?)?;
    tmp.persist(paths::conditions_state(state))
        .map_err(|e| e.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn unclaimed(v: &[&str]) -> Condition {
        Condition::Unclaimed { ids: ids(v) }
    }

    fn summary(conditions: Vec<Condition>) -> Summary {
        Summary {
            subject: Subject::Config,
            conditions,
        }
    }

    fn text(c: Condition) -> String {
        summary(vec![c]).text()
    }

    #[test]
    fn fails_sync_per_variant() {
        let failing = [
            Condition::UnfiledMissing {
                folder: "Inbox".into(),
            },
            Condition::IndexTaken {
                path: "i.md".into(),
            },
            Condition::Unresolvable {
                path: "/w".into(),
                reason: "r".into(),
            },
            Condition::GroupRule { reason: "r".into() },
            Condition::UnlistedSkipped,
        ];
        for c in failing {
            assert!(c.fails_sync(), "{c:?}");
        }
        assert!(!unclaimed(&["w-1"]).fails_sync());
        let missing = Condition::MissingProject {
            project: "P".into(),
            ids: ids(&["w-1"]),
        };
        assert!(!missing.fails_sync());
    }

    #[test]
    fn any_fails_checks_every_summary() {
        assert!(!any_fails(&[]));
        let quiet = summary(vec![unclaimed(&["w-1"])]);
        assert!(!quiet.fails_sync());
        assert!(!any_fails(std::slice::from_ref(&quiet)));
        let loud = summary(vec![Condition::GroupRule { reason: "r".into() }]);
        assert!(loud.fails_sync());
        assert!(any_fails(&[quiet, loud]));
    }

    #[test]
    fn unclaimed_text() {
        assert_eq!(
            text(unclaimed(&["w-7", "w-1", "w-2", "w-9"])),
            "4 unclaimed beads (w-1, w-2, w-7, …)"
        );
        assert_eq!(text(unclaimed(&["w-1"])), "1 unclaimed bead (w-1)");
    }

    #[test]
    fn missing_project_text() {
        let c = Condition::MissingProject {
            project: "Foo".into(),
            ids: ids(&["w-4", "w-3"]),
        };
        assert_eq!(
            text(c),
            "2 beads claimed for missing project \"Foo\" (w-3, w-4)"
        );
    }

    #[test]
    fn ids_sort_naturally() {
        assert_eq!(
            text(unclaimed(&["w-10", "w-2", "w-3.10", "w-3.2"])),
            "4 unclaimed beads (w-2, w-3.2, w-3.10, …)"
        );
    }

    #[test]
    fn ids_sort_past_leading_zeros() {
        assert_eq!(
            text(unclaimed(&["w-2", "w-1", "w-01"])),
            "3 unclaimed beads (w-01, w-1, w-2)"
        );
    }

    #[test]
    fn ids_sort_totally_past_u64() {
        let big = format!("x{}", "1000".repeat(5) + "0");
        let mut v = vec![big.clone(), "x10".to_string(), "x2".to_string()];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, ["x2", "x10", big.as_str()]);
        assert_eq!(natural_cmp(&big, "x2"), Ordering::Greater);
    }

    #[test]
    fn ids_dedupe() {
        assert_eq!(text(unclaimed(&["w-1", "w-1"])), "1 unclaimed bead (w-1)");
    }

    #[test]
    fn other_variant_texts() {
        assert_eq!(
            text(Condition::UnfiledMissing {
                folder: "Inbox".into()
            }),
            "unfiled folder \"Inbox\" is missing; index not written"
        );
        assert_eq!(
            text(Condition::IndexTaken {
                path: "Inbox/Unfiled beads - w.md".into()
            }),
            "\"Inbox/Unfiled beads - w.md\" exists and callisto did not write it; index not written"
        );
        assert_eq!(
            text(Condition::Unresolvable {
                path: "/w/x".into(),
                reason: "no origin remote".into()
            }),
            "table entry /w/x is unresolvable: no origin remote"
        );
        assert_eq!(
            text(Condition::GroupRule { reason: "r".into() }),
            "group not synced: r"
        );
        assert_eq!(
            text(Condition::UnlistedSkipped),
            "not in repos and a table entry is unresolvable; nothing synced"
        );
    }

    #[test]
    fn mixed_conditions_render_in_fixed_order() {
        let a = vec![
            Condition::GroupRule { reason: "r".into() },
            Condition::MissingProject {
                project: "B".into(),
                ids: ids(&["w-2"]),
            },
            Condition::MissingProject {
                project: "A".into(),
                ids: ids(&["w-1"]),
            },
            unclaimed(&["w-9"]),
            Condition::Unresolvable {
                path: "/b".into(),
                reason: "x".into(),
            },
            Condition::Unresolvable {
                path: "/a".into(),
                reason: "y".into(),
            },
        ];
        let mut b = a.clone();
        b.reverse();
        let expected = "1 unclaimed bead (w-9); \
            1 bead claimed for missing project \"A\" (w-1); \
            1 bead claimed for missing project \"B\" (w-2); \
            table entry /a is unresolvable: y; \
            table entry /b is unresolvable: x; \
            group not synced: r";
        assert_eq!(summary(a).text(), expected);
        assert_eq!(summary(b).text(), expected);
    }

    #[test]
    fn empty_summary_text_is_cleared() {
        assert_eq!(summary(vec![]).text(), "standing conditions cleared");
    }

    #[test]
    fn subject_keys() {
        let g = Subject::Group {
            database: PathBuf::from("/d/.beads"),
            repo: PathBuf::from("/r"),
        };
        assert_eq!(g.key(), "group:/d/.beads");
        assert_eq!(g.label(), "/r");
        assert_eq!(g.repo(), Some(std::path::Path::new("/r")));
        assert_eq!(Subject::Config.key(), "config");
        assert_eq!(Subject::Config.label(), "config");
        assert_eq!(Subject::Config.repo(), None);
        let c = Subject::Checkout(PathBuf::from("/c"));
        assert_eq!(c.key(), "checkout:/c");
        assert_eq!(c.label(), "/c");
    }

    use std::collections::BTreeMap;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use tempfile::TempDir;

    use crate::{errlog, lock, paths};

    fn group(repo: &str, v: &[&str]) -> Summary {
        Summary {
            subject: Subject::Group {
                database: "/db".into(),
                repo: repo.into(),
            },
            conditions: if v.is_empty() {
                vec![]
            } else {
                vec![unclaimed(v)]
            },
        }
    }

    struct Fixture {
        tmp: TempDir,
    }

    impl Fixture {
        fn new() -> Fixture {
            Fixture {
                tmp: tempfile::tempdir().unwrap(),
            }
        }

        fn state(&self) -> &Path {
            self.tmp.path()
        }

        fn record(&self, summaries: &[Summary]) -> String {
            let _lock = lock::acquire(&paths::sync_lock(self.state())).unwrap();
            let mut err = Vec::new();
            record_to(self.state(), "sync", summaries, &mut err);
            String::from_utf8(err).unwrap()
        }

        fn entries(&self) -> Vec<errlog::Entry> {
            match fs::read_to_string(paths::error_log(self.state())) {
                Ok(t) => t
                    .lines()
                    .map(|l| serde_json::from_str(l).unwrap())
                    .collect(),
                Err(_) => vec![],
            }
        }

        fn saved(&self) -> BTreeMap<String, String> {
            let t = fs::read_to_string(paths::conditions_state(self.state())).unwrap();
            serde_json::from_str(&t).unwrap()
        }
    }

    #[test]
    fn first_summary_logs_and_saves() {
        let f = Fixture::new();
        let s = group("/r", &["a-1"]);
        assert_eq!(f.record(std::slice::from_ref(&s)), "");
        let entries = f.entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].command.as_deref(), Some("sync"));
        assert_eq!(entries[0].repo.as_deref(), Some(Path::new("/r")));
        assert_eq!(entries[0].stage, errlog::Stage::Conditions);
        assert_eq!(entries[0].error, s.text());
        assert_eq!(f.saved(), BTreeMap::from([(s.subject.key(), s.text())]));
        let mode = fs::metadata(paths::conditions_state(f.state()))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn same_summary_logs_once() {
        let f = Fixture::new();
        let s = [group("/r", &["a-1"])];
        f.record(&s);
        let before = fs::read(paths::conditions_state(f.state())).unwrap();
        f.record(&s);
        assert_eq!(f.entries().len(), 1);
        assert_eq!(
            fs::read(paths::conditions_state(f.state())).unwrap(),
            before
        );
    }

    #[test]
    fn changed_summary_logs_again() {
        let f = Fixture::new();
        f.record(&[group("/r", &["a-1"])]);
        f.record(&[group("/r", &["a-2"])]);
        assert_eq!(f.entries().len(), 2);
    }

    #[test]
    fn cleared_subject_logs_once_and_drops_key() {
        let f = Fixture::new();
        f.record(&[group("/r", &["a-1"])]);
        f.record(&[group("/r", &[])]);
        let entries = f.entries();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].error, "standing conditions cleared");
        assert_eq!(entries[1].repo.as_deref(), Some(Path::new("/r")));
        assert!(f.saved().is_empty());
        f.record(&[group("/r", &[])]);
        assert_eq!(f.entries().len(), 2);
    }

    #[test]
    fn empty_without_state_writes_nothing() {
        let f = Fixture::new();
        f.record(&[group("/r", &[])]);
        assert!(!paths::conditions_state(f.state()).exists());
        assert!(!paths::error_log(f.state()).exists());
    }

    #[test]
    fn record_keeps_subjects_not_passed() {
        let f = Fixture::new();
        let a = group("/a", &["a-1"]);
        let seed = format!("{{\"{}\":\"old\",\"other\":\"kept\"}}", a.subject.key());
        fs::write(paths::conditions_state(f.state()), seed).unwrap();
        f.record(std::slice::from_ref(&a));
        let saved = f.saved();
        assert_eq!(saved["other"], "kept");
        assert_eq!(saved[&a.subject.key()], a.text());
    }

    #[test]
    fn record_treats_corrupt_state_as_empty() {
        let f = Fixture::new();
        fs::write(paths::conditions_state(f.state()), "not json").unwrap();
        let s = group("/r", &["a-1"]);
        f.record(std::slice::from_ref(&s));
        assert_eq!(f.entries().len(), 1);
        assert_eq!(f.saved(), BTreeMap::from([(s.subject.key(), s.text())]));
    }

    #[test]
    fn record_treats_wrong_shape_as_empty() {
        for seed in ["[1]", "{\"k\":1}"] {
            let f = Fixture::new();
            fs::write(paths::conditions_state(f.state()), seed).unwrap();
            let s = group("/r", &["a-1"]);
            f.record(std::slice::from_ref(&s));
            assert_eq!(f.entries().len(), 1, "{seed}");
            assert_eq!(f.saved(), BTreeMap::from([(s.subject.key(), s.text())]));
        }
    }

    #[test]
    fn unwritable_state_goes_to_stderr() {
        let f = Fixture::new();
        let target = paths::conditions_state(f.state());
        fs::create_dir(&target).unwrap();
        fs::write(target.join("x"), "x").unwrap();
        let err = f.record(&[group("/r", &["a-1"])]);
        assert!(err.starts_with("callisto: couldn't write "), "{err}");
        assert_eq!(f.entries().len(), 1);
    }

    #[test]
    fn failed_append_leaves_the_subject_as_saved() {
        let f = Fixture::new();
        f.record(&[group("/r", &["a-1"])]);
        let before = f.saved();
        let log = paths::error_log(f.state());
        let kept = fs::read(&log).unwrap();
        fs::remove_file(&log).unwrap();
        fs::create_dir(&log).unwrap();
        f.record(&[group("/r", &["a-2"])]);
        assert_eq!(f.saved(), before);
        f.record(&[group("/r", &[])]);
        assert_eq!(f.saved(), before);
        fs::remove_dir(&log).unwrap();
        fs::write(&log, kept).unwrap();
        f.record(&[group("/r", &["a-2"])]);
        let entries = f.entries();
        assert_eq!(entries.len(), 2);
        assert!(entries[1].error.contains("a-2"), "{}", entries[1].error);
    }
}
