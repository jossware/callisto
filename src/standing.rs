use std::cmp::Ordering;
use std::path::{Path, PathBuf};

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

    // Within a variant: path or project first, then the remaining fields.
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
        let both_digits = x.starts_with(|c: char| c.is_ascii_digit())
            && y.starts_with(|c: char| c.is_ascii_digit());
        let ord = match (both_digits, x.parse::<u64>(), y.parse::<u64>()) {
            (true, Ok(m), Ok(n)) => m.cmp(&n),
            _ => x.cmp(y),
        };
        if ord != Ordering::Equal {
            return ord;
        }
    }
    ra.len().cmp(&rb.len()).then_with(|| a.cmp(b))
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
}
