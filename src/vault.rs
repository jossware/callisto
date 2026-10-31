//! Every note in the vault that names a bead, found by `bead:` and never
//! by filename, so notes the user moved are still found.

use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};

use crate::fields;
use crate::write;

/// The frontmatter line that marks a note as callisto's to rewrite.
pub const MARKER_KEY: &str = "generatedBy";
pub const MARKER_VALUE: &str = "callisto";

#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    /// Relative to the vault.
    pub path: PathBuf,
    pub beads: Vec<String>,
    /// `type: task`.
    pub task: bool,
    pub generated: bool,
    /// `beadPrefix`. Notes older than the field have none.
    pub prefix: Option<String>,
}

impl Note {
    /// Whether the repo with `prefix` owns `id` in this note. The
    /// recorded prefix decides, because the repo that wrote the note
    /// may be missing from `prefixes`. Older notes fall back to the
    /// longest of `prefixes`.
    pub fn owned_by(&self, id: &str, prefix: &str, prefixes: &[&str]) -> bool {
        match &self.prefix {
            Some(recorded) => {
                recorded == prefix
                    && id
                        .strip_prefix(prefix)
                        .is_some_and(|rest| rest.starts_with('-'))
            }
            None => owner(id, prefixes) == Some(prefix),
        }
    }

    /// What a wikilink to this note names.
    pub fn name(&self) -> &str {
        self.path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
    }
}

#[derive(Debug)]
pub enum ScanError {
    Read { path: PathBuf, source: io::Error },
}

impl fmt::Display for ScanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScanError::Read { path, source } => write!(f, "{}: {source}", path.display()),
        }
    }
}

impl Error for ScanError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            ScanError::Read { source, .. } => Some(source),
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum Generated<'i> {
    Missing,
    One(&'i Note),
    /// Sorted by path. None of them may be written.
    Duplicate(Vec<&'i Note>),
}

#[derive(Debug)]
pub struct Index {
    notes: Vec<Note>,
    by_bead: HashMap<String, Vec<usize>>,
    stale: Vec<PathBuf>,
}

impl Index {
    /// Any unreadable note fails the scan: it might be the marked note
    /// for some id, and syncing without it would create a duplicate.
    pub fn scan(vault: &Path) -> Result<Index, ScanError> {
        let mut notes = Vec::new();
        let mut stale = Vec::new();
        walk(vault, Path::new(""), &mut notes, &mut stale)?;
        notes.sort_by(|a, b| a.path.cmp(&b.path));
        let mut by_bead: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, note) in notes.iter().enumerate() {
            for id in &note.beads {
                let at = by_bead.entry(id.clone()).or_default();
                if at.last() != Some(&i) {
                    at.push(i);
                }
            }
        }
        Ok(Index {
            notes,
            by_bead,
            stale,
        })
    }

    /// Sorted by path.
    fn citing(&self, id: &str) -> impl Iterator<Item = &Note> {
        self.by_bead
            .get(id)
            .into_iter()
            .flatten()
            .map(|&i| &self.notes[i])
    }

    pub fn generated(&self, id: &str) -> Generated<'_> {
        let mut found: Vec<&Note> = self.citing(id).filter(|n| n.generated).collect();
        match found.len() {
            0 => Generated::Missing,
            1 => Generated::One(found.remove(0)),
            _ => Generated::Duplicate(found),
        }
    }

    /// Hand-written task notes for the Task note row, sorted by path.
    pub fn task_notes(&self, id: &str) -> Vec<&Note> {
        self.citing(id).filter(|n| n.task && !n.generated).collect()
    }

    /// The marked notes whose beads belong to `prefix`. `prefix` need
    /// not be configured.
    pub fn repo_notes(&self, prefix: &str, configured: &[&str]) -> Vec<&Note> {
        let mut prefixes = configured.to_vec();
        prefixes.push(prefix);
        self.notes
            .iter()
            .filter(|n| n.generated)
            .filter(|n| n.beads.iter().any(|id| n.owned_by(id, prefix, &prefixes)))
            .collect()
    }

    /// Sorted by path.
    pub fn notes(&self) -> &[Note] {
        &self.notes
    }

    /// Temp files left by a killed write, relative to the vault.
    pub fn stale(&self) -> &[PathBuf] {
        &self.stale
    }
}

/// The repo a bead id belongs to. Prefixes and id suffixes may both
/// contain hyphens, so only the configured prefixes can tell.
pub fn owner<'p>(id: &str, prefixes: &[&'p str]) -> Option<&'p str> {
    prefixes
        .iter()
        .filter(|p| {
            id.strip_prefix(**p)
                .is_some_and(|rest| rest.starts_with('-'))
        })
        .max_by_key(|p| p.len())
        .copied()
}

fn read_error(path: &Path) -> impl FnOnce(io::Error) -> ScanError {
    let path = path.to_path_buf();
    move |source| ScanError::Read { path, source }
}

fn walk(
    vault: &Path,
    rel: &Path,
    notes: &mut Vec<Note>,
    stale: &mut Vec<PathBuf>,
) -> Result<(), ScanError> {
    let dir = vault.join(rel);
    for entry in fs::read_dir(&dir).map_err(read_error(&dir))? {
        let entry = entry.map_err(read_error(&dir))?;
        let name = entry.file_name();
        if write::is_temp(&name) && entry.file_type().is_ok_and(|t| t.is_file()) {
            stale.push(rel.join(&name));
        }
        // Covers `.obsidian`, `.git`, `.trash` and Syncthing's temp files.
        if name.as_encoded_bytes().starts_with(b".") {
            continue;
        }
        let path = rel.join(&name);
        let kind = entry.file_type().map_err(read_error(&entry.path()))?;
        if kind.is_dir() {
            walk(vault, &path, notes, stale)?;
        } else if kind.is_file() && path.extension().is_some_and(|e| e == "md") {
            let full = entry.path();
            let found = fields::read(&head(&full).map_err(read_error(&full))?);
            if !found.beads.is_empty() {
                notes.push(Note {
                    path,
                    beads: found.beads,
                    task: found.kind.as_deref() == Some("task"),
                    generated: found.generated_by.as_deref() == Some(MARKER_VALUE),
                    prefix: found.bead_prefix,
                });
            }
        }
    }
    Ok(())
}

/// A note's lines up to its frontmatter's closing line, so a long body
/// is never read.
fn head(path: &Path) -> io::Result<String> {
    let mut reader = BufReader::new(File::open(path)?);
    let mut out = String::new();
    let mut line = Vec::new();
    let mut first = true;
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 {
            break;
        }
        let text = String::from_utf8_lossy(&line);
        out.push_str(&text);
        let bare = text.trim_end_matches(['\n', '\r']);
        let bare = bare.strip_prefix('\u{feff}').unwrap_or(bare);
        if (first && bare != "---") || (!first && bare == "---") {
            break;
        }
        first = false;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::{PermissionsExt, symlink};

    fn put(root: &Path, rel: &str, text: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn paths(index: &Index) -> Vec<&Path> {
        index.notes().iter().map(|n| n.path.as_path()).collect()
    }

    #[test]
    fn indexes_notes_with_beads() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        put(
            root,
            "Projects/a/one.md",
            "---\nbead: a-1\ntype: task\n---\n",
        );
        put(
            root,
            "Projects/a/two - a-2.md",
            "---\nbead: a-2\ngeneratedBy: callisto\n---\n",
        );
        put(root, "Projects/a/plain.md", "---\ntype: task\n---\n");
        let index = Index::scan(root).unwrap();
        assert_eq!(
            paths(&index),
            [
                Path::new("Projects/a/one.md"),
                Path::new("Projects/a/two - a-2.md")
            ]
        );
        let [one, two] = index.notes() else {
            panic!("want two notes");
        };
        assert!(one.task && !one.generated);
        assert_eq!(one.beads, ["a-1"]);
        assert!(two.generated && !two.task);
        assert_eq!(two.name(), "two - a-2");
    }

    #[test]
    fn skips_dot_entries() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for rel in [
            ".obsidian/x.md",
            ".trash/y.md",
            "Projects/.syncthing.z.md.tmp",
            "Projects/.hidden.md",
        ] {
            put(root, rel, "---\nbead: a-1\n---\n");
        }
        assert!(Index::scan(root).unwrap().notes().is_empty());
    }

    #[test]
    fn skips_non_markdown() {
        let dir = tempfile::tempdir().unwrap();
        put(dir.path(), "Projects/a/one.txt", "---\nbead: a-1\n---\n");
        assert!(Index::scan(dir.path()).unwrap().notes().is_empty());
    }

    #[test]
    fn missing_vault_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let vault = dir.path().join("nope");
        let err = Index::scan(&vault).err().unwrap();
        assert!(matches!(err, ScanError::Read { path, .. } if path == vault));
    }

    #[test]
    fn unreadable_note_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        put(dir.path(), "Projects/secret.md", "---\nbead: a-1\n---\n");
        let note = dir.path().join("Projects/secret.md");
        fs::set_permissions(&note, fs::Permissions::from_mode(0o000)).unwrap();
        if fs::read(&note).is_ok() {
            return; // root reads anything
        }
        let err = Index::scan(dir.path()).err().unwrap();
        assert!(matches!(err, ScanError::Read { path, .. } if path.ends_with("secret.md")));
    }

    #[test]
    fn does_not_follow_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        put(root, "Projects/a/one.md", "---\nbead: a-1\n---\n");
        fs::create_dir(root.join("Archives")).unwrap();
        symlink(root.join("Projects"), root.join("Archives/link")).unwrap();
        assert_eq!(
            paths(&Index::scan(root).unwrap()),
            [Path::new("Projects/a/one.md")]
        );
    }

    const MARKED: &str = "generatedBy: callisto\n";

    fn note(root: &Path, rel: &str, bead: &str, extra: &str) {
        put(root, rel, &format!("---\nbead: {bead}\n{extra}---\n"));
    }

    fn rels<'i>(notes: &[&'i Note]) -> Vec<&'i Path> {
        notes.iter().map(|n| n.path.as_path()).collect()
    }

    #[test]
    fn finds_generated_by_scalar_or_list() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        note(root, "x.md", "a-1", MARKED);
        note(root, "y.md", "[a-2, a-3]", MARKED);
        let index = Index::scan(root).unwrap();
        let [x, y] = index.notes() else {
            panic!("want two notes");
        };
        assert_eq!(index.generated("a-1"), Generated::One(x));
        assert_eq!(index.generated("a-3"), Generated::One(y));
        assert_eq!(index.generated("a-9"), Generated::Missing);
    }

    #[test]
    fn unmarked_notes_are_never_generated() {
        let dir = tempfile::tempdir().unwrap();
        note(dir.path(), "h.md", "a-1", "");
        let index = Index::scan(dir.path()).unwrap();
        assert_eq!(index.generated("a-1"), Generated::Missing);
    }

    #[test]
    fn two_marked_notes_are_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        note(root, "Projects/a/x.md", "a-1", MARKED);
        note(root, "Archives/x.md", "a-1", MARKED);
        let index = Index::scan(root).unwrap();
        let Generated::Duplicate(found) = index.generated("a-1") else {
            panic!("want duplicates");
        };
        assert_eq!(
            rels(&found),
            [Path::new("Archives/x.md"), Path::new("Projects/a/x.md")]
        );
    }

    #[test]
    fn task_notes_are_hand_written_tasks() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let task = "type: task\n";
        note(root, "b.md", "a-1", task);
        note(root, "a.md", "a-1", task);
        note(root, "plan.md", "a-1", "");
        note(root, "gen.md", "a-1", &format!("{task}{MARKED}"));
        note(root, "other.md", "[a-2, a-1]", task);
        let index = Index::scan(root).unwrap();
        assert_eq!(
            rels(&index.task_notes("a-1")),
            [Path::new("a.md"), Path::new("b.md"), Path::new("other.md")]
        );
    }

    #[test]
    fn owner_takes_the_longest_prefix() {
        let prefixes = ["forge", "forge-web", "ganymede"];
        assert_eq!(owner("forge-web-1x", &prefixes), Some("forge-web"));
        assert_eq!(owner("forge-2y", &prefixes), Some("forge"));
        assert_eq!(owner("ganymede-mol-5ed", &prefixes), Some("ganymede"));
        assert_eq!(owner("ganymedex-1", &prefixes), None);
        assert_eq!(owner("ganymede", &prefixes), None);
    }

    #[test]
    fn repo_notes_scope_by_longest_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        note(root, "f1.md", "forge-1", MARKED);
        note(root, "w2.md", "forge-web-2", MARKED);
        note(root, "g.md", "ganymede-mol-5ed", MARKED);
        note(root, "h3.md", "forge-3", "");
        let index = Index::scan(root).unwrap();
        let configured = ["forge", "forge-web", "ganymede"];
        assert_eq!(
            rels(&index.repo_notes("forge", &configured)),
            [Path::new("f1.md")]
        );
        assert_eq!(
            rels(&index.repo_notes("ganymede", &["forge"])),
            [Path::new("g.md")]
        );
    }

    #[test]
    fn repo_notes_prefer_bead_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let with = |p: &str| format!("beadPrefix: {p}\n{MARKED}");
        note(root, "mine.md", "a-1", &with("a"));
        note(root, "theirs.md", "a-b-1", &with("a-b"));
        note(root, "legacy.md", "a-2", MARKED);
        note(root, "liar.md", "x-1", &with("a"));
        note(root, "odd.md", "a-b-1", &with("a"));
        let index = Index::scan(root).unwrap();
        assert_eq!(
            rels(&index.repo_notes("a", &["a"])),
            [
                Path::new("legacy.md"),
                Path::new("mine.md"),
                Path::new("odd.md")
            ]
        );
        assert_eq!(
            rels(&index.repo_notes("a-b", &["a"])),
            [Path::new("theirs.md")]
        );
        let [legacy, liar, mine, odd, theirs] = index.notes() else {
            panic!("want five notes");
        };
        assert_eq!(mine.prefix.as_deref(), Some("a"));
        assert_eq!(legacy.prefix, None);
        assert!(mine.owned_by("a-1", "a", &["a"]));
        assert!(!theirs.owned_by("a-b-1", "a", &["a"]));
        assert!(!liar.owned_by("x-1", "a", &["a", "x"]));
        assert!(odd.owned_by("a-b-1", "a", &["a", "a-b"]));
        assert!(!odd.owned_by("a-b-1", "a-b", &["a", "a-b"]));
        assert!(legacy.owned_by("a-2", "a", &["a"]));
        assert!(!legacy.owned_by("a-2", "a-b", &["a", "a-b"]));
    }
}
