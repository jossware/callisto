//! The sync engine against temporary vaults and a stand-in `bd`.

use std::collections::BTreeMap;
use std::fs::{self, Permissions};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use jiff::tz::{Offset, TimeZone};
use tempfile::TempDir;

use callisto::bd::Bd;
use callisto::claim::{Reason, Unfiled};
use callisto::config::{Filing, Split};
use callisto::database::Database;
use callisto::errlog::Stage;
use callisto::lock;
use callisto::standing::Condition;
use callisto::sync::{self, Report, Request, Scope, Warning};
use callisto::vault::Index;

mod common;

const FAKE_BD: &str = r#"#!/bin/sh
d="$BEADS_DIR"
case "$1" in
export) exec cat "$d/export.jsonl" ;;
config) case "$4" in
  issue_prefix) printf '{"key":"issue_prefix","value":"%s"}\n' "$(cat "$d/prefix")" ;;
  custom.obsidian.project) printf '{"key":"%s","value":"%s"}\n' "$4" "$(cat "$d/project" 2>/dev/null)" ;;
  *) printf '{"key":"%s","value":""}\n' "$4" ;;
  esac ;;
*) exit 9 ;;
esac
"#;

struct Fixture {
    tmp: TempDir,
    vault: PathBuf,
    database: Database,
    bd: Bd,
    prefix: String,
}

/// A vault holding project `name`, and a checkout named `name` whose
/// beads use `prefix`.
fn fixture(prefix: &str, name: &str) -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let vault = tmp.path().join("vault");
    put(&vault, &format!("Projects/{name}/{name}.md"), "# home\n");
    let database = database(tmp.path(), prefix, name);
    let bd_path = tmp.path().join("bd");
    common::write_script(&bd_path, FAKE_BD);
    Fixture {
        database,
        bd: Bd::new(bd_path),
        vault,
        tmp,
        prefix: prefix.into(),
    }
}

/// The database of a checkout named `name`, filed under one project.
fn database(root: &Path, prefix: &str, name: &str) -> Database {
    let dir = root.join("repos").join(name);
    fs::create_dir_all(dir.join(".git")).unwrap();
    fs::create_dir_all(dir.join(".beads")).unwrap();
    fs::write(dir.join(".beads/prefix"), prefix).unwrap();
    Database {
        dir: dir.join(".beads").canonicalize().unwrap(),
        name: name.into(),
        filing: Filing::Single { project: None },
    }
}

fn put(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn line(id: &str, title: &str, status: &str) -> String {
    let closed = if status == "closed" {
        r#","closed_at":"2026-10-12T00:00:00Z""#
    } else {
        ""
    };
    format!(
        r#"{{"id":"{id}","title":"{title}","status":"{status}","priority":2,"created_at":"2026-10-11T02:21:06Z","updated_at":"2026-10-11T02:21:40Z"{closed}}}"#
    )
}

fn set_export(database: &Database, lines: &[String]) {
    let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    fs::write(database.dir.join("export.jsonl"), text).unwrap();
}

fn paths(rels: &[&str]) -> Vec<PathBuf> {
    rels.iter().map(PathBuf::from).collect()
}

type Snapshot = BTreeMap<PathBuf, (Vec<u8>, SystemTime, u32)>;

impl Fixture {
    fn set_export(&self, lines: &[String]) {
        set_export(&self.database, lines);
    }

    fn scope(&self) -> Scope {
        Scope {
            prefixes: vec![self.prefix.clone()],
            unreadable: vec![],
            shared: vec![],
        }
    }

    fn run(&self) -> Result<Report, sync::Error> {
        self.run_with(&self.scope())
    }

    fn run_with(&self, scope: &Scope) -> Result<Report, sync::Error> {
        self.run_database(&self.database, scope)
    }

    /// Another checkout's database, beside this one, whose beads use
    /// `prefix` and sync into this vault's project `project`.
    fn sibling(&self, prefix: &str, name: &str, project: &str) -> Database {
        let database = database(self.tmp.path(), prefix, name);
        fs::write(database.dir.join("project"), project).unwrap();
        database
    }

    /// This database, filed as `filing`.
    fn filed(&self, filing: Filing) -> Database {
        Database {
            filing,
            ..self.database.clone()
        }
    }

    fn run_split(&self, projects: &[(&str, &str)]) -> Result<Report, sync::Error> {
        let filing = Filing::Split(Split {
            by: "metadata.repo".parse().unwrap(),
            projects: projects
                .iter()
                .map(|(value, project)| (value.to_string(), project.to_string()))
                .collect(),
        });
        self.run_database(&self.filed(filing), &self.scope())
    }

    fn run_database(&self, database: &Database, scope: &Scope) -> Result<Report, sync::Error> {
        let lock = lock::acquire(&self.tmp.path().join("state/sync.lock")).unwrap();
        let tz = TimeZone::fixed(Offset::constant(-4));
        sync::sync_database(
            &lock,
            &Request {
                bd: &self.bd,
                vault: &self.vault,
                database,
                configured: true,
                scope,
                tz: &tz,
                unfiled: Path::new("Inbox"),
            },
        )
    }

    fn read(&self, rel: &str) -> String {
        fs::read_to_string(self.vault.join(rel)).unwrap()
    }

    fn snapshot(&self) -> Snapshot {
        let mut out = BTreeMap::new();
        walk(&self.vault, Path::new(""), &mut out);
        out
    }

    /// `snapshot` without the unfiled folder, which a split run rewrites.
    fn snapshot_notes(&self) -> Snapshot {
        let mut out = self.snapshot();
        out.retain(|path, _| !path.starts_with("Inbox"));
        out
    }
}

fn walk(root: &Path, rel: &Path, out: &mut Snapshot) {
    for entry in fs::read_dir(root.join(rel)).unwrap() {
        let entry = entry.unwrap();
        let path = rel.join(entry.file_name());
        let meta = entry.metadata().unwrap();
        if meta.is_dir() {
            walk(root, &path, out);
        } else {
            let bytes = fs::read(entry.path()).unwrap();
            out.insert(
                path,
                (bytes, meta.modified().unwrap(), meta.permissions().mode()),
            );
        }
    }
}

const FIRST: &str = "Projects/a/first - a-1.md";

#[test]
fn first_sync_creates_notes() {
    let f = fixture("a", "a");
    f.set_export(&[
        line("a-1", "first", "open"),
        line("a-2", "second/half", "open"),
    ]);
    let report = f.run().unwrap();
    assert_eq!(
        report.created,
        paths(&[FIRST, "Projects/a/second-half - a-2.md"])
    );
    for rel in &report.created {
        let text = fs::read_to_string(f.vault.join(rel)).unwrap();
        assert!(text.starts_with("---\ntype: task\n"), "{text}");
        assert!(text.contains("\ngeneratedBy: callisto\n"), "{text}");
    }
}

#[test]
fn unchanged_resync_writes_nothing() {
    let f = fixture("a", "a");
    f.set_export(&[line("a-1", "first", "open"), line("a-2", "second", "open")]);
    f.run().unwrap();
    let before = f.snapshot();
    let report = f.run().unwrap();
    assert!(report.created.is_empty() && report.updated.is_empty());
    assert_eq!(report.unchanged, 2);
    assert_eq!(f.snapshot(), before);
}

#[test]
fn retitle_keeps_filename() {
    let f = fixture("a", "a");
    f.set_export(&[line("a-1", "first", "open")]);
    f.run().unwrap();
    f.set_export(&[line("a-1", "renamed", "open")]);
    let report = f.run().unwrap();
    assert_eq!(report.updated, paths(&[FIRST]));
    assert!(f.read(FIRST).contains("\ntitle: \"renamed\"\n"));
    assert!(!f.vault.join("Projects/a/renamed - a-1.md").exists());
}

#[test]
fn moved_note_updates_in_place() {
    let f = fixture("a", "a");
    f.set_export(&[line("a-1", "first", "open")]);
    f.run().unwrap();
    fs::create_dir_all(f.vault.join("Archives")).unwrap();
    fs::rename(f.vault.join(FIRST), f.vault.join("Archives/first - a-1.md")).unwrap();
    f.set_export(&[line("a-1", "renamed", "open")]);
    let report = f.run().unwrap();
    assert_eq!(report.updated, paths(&["Archives/first - a-1.md"]));
    assert!(report.created.is_empty());
    let names: Vec<_> = fs::read_dir(f.vault.join("Projects/a"))
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, ["a.md"]);
}

#[test]
fn duplicate_is_skipped_and_warned() {
    let f = fixture("a", "a");
    f.set_export(&[line("a-1", "first", "open")]);
    f.run().unwrap();
    fs::copy(
        f.vault.join(FIRST),
        f.vault
            .join("Projects/a/first - a-1.sync-conflict-20261014-120000-ABC.md"),
    )
    .unwrap();
    let before = f.snapshot();
    f.set_export(&[line("a-1", "renamed", "open")]);
    let report = f.run().unwrap();
    assert_eq!(f.snapshot(), before);
    let dups: Vec<&Warning> = report
        .warnings
        .iter()
        .filter(|w| matches!(w, Warning::Duplicate { id, .. } if id == "a-1"))
        .collect();
    assert_eq!(dups.len(), 1, "{:?}", report.warnings);
    assert!(dups[0].to_string().contains("conflict"), "{}", dups[0]);
}

#[test]
fn hand_written_notes_are_untouched() {
    let f = fixture("a", "a");
    put(
        &f.vault,
        "Projects/a/hand.md",
        "---\nbead: a-1\ntype: task\n---\nmine\n",
    );
    put(
        &f.vault,
        "Projects/a/plan.md",
        "---\nbead: [a-1, a-9]\n---\nplan\n",
    );
    let before = f.snapshot();
    f.set_export(&[line("a-1", "first", "open")]);
    f.run().unwrap();
    let after = f.snapshot();
    for rel in ["Projects/a/hand.md", "Projects/a/plan.md"] {
        assert_eq!(after[Path::new(rel)], before[Path::new(rel)], "{rel}");
    }
    assert!(f.read(FIRST).contains("[[hand]]"));
}

#[test]
fn taken_filename_is_left_alone() {
    let f = fixture("a", "a");
    put(&f.vault, FIRST, "not callisto's\n");
    f.set_export(&[line("a-1", "first", "open"), line("a-2", "b", "open")]);
    let report = f.run().unwrap();
    assert_eq!(f.read(FIRST), "not callisto's\n");
    assert!(
        report
            .warnings
            .iter()
            .any(|w| matches!(w, Warning::Taken { id, .. } if id == "a-1")),
        "{:?}",
        report.warnings
    );
    assert_eq!(report.created, paths(&["Projects/a/b - a-2.md"]));
}

/// a-1 "first", and a-2 "b" as its child and blocked by it.
fn child_of_first(f: &Fixture) {
    let deps = r#","dependencies":[{"issue_id":"a-2","depends_on_id":"a-1","type":"parent-child","created_at":"2026-10-11T02:21:06Z"},{"issue_id":"a-2","depends_on_id":"a-1","type":"blocks","created_at":"2026-10-11T02:21:06Z"}]}"#;
    let child = line("a-2", "b", "open");
    let child = format!("{}{deps}", child.strip_suffix('}').unwrap());
    f.set_export(&[line("a-1", "first", "open"), child]);
}

const CHILD: &str = "Projects/a/b - a-2.md";

fn assert_links_show_id(text: &str) {
    assert_links_show(text, "a-1");
}

/// `text` names its parent and blocker, bead `id` titled "first", by
/// id alone.
fn assert_links_show(text: &str, id: &str) {
    assert!(!text.contains(&format!("first - {id}")), "{text}");
    assert!(
        !text.contains("\nparent:") && !text.contains("\nblockedBy:"),
        "{text}"
    );
    assert!(text.contains(&format!("> | Parent | ○ {id} |")), "{text}");
    assert!(
        text.contains(&format!("> | Depends on | ○ {id} |")),
        "{text}"
    );
}

#[test]
fn links_to_a_taken_bead_show_its_id() {
    let f = fixture("a", "a");
    put(&f.vault, FIRST, "not callisto's\n");
    child_of_first(&f);
    f.run().unwrap();
    assert_links_show_id(&f.read(CHILD));
}

#[test]
fn dangling_symlink_takes_the_filename() {
    let f = fixture("a", "a");
    std::os::unix::fs::symlink("nowhere.md", f.vault.join(FIRST)).unwrap();
    child_of_first(&f);
    let report = f.run().unwrap();
    assert!(
        has_warning(
            &report,
            |w| matches!(w, Warning::Taken { id, .. } if id == "a-1")
        ),
        "{:?}",
        report.warnings
    );
    assert_links_show_id(&f.read(CHILD));
}

#[test]
fn freed_filename_links_on_the_next_run() {
    let f = fixture("a", "a");
    put(&f.vault, FIRST, "not callisto's\n");
    child_of_first(&f);
    f.run().unwrap();
    fs::remove_file(f.vault.join(FIRST)).unwrap();
    let report = f.run().unwrap();
    assert_eq!(report.created, paths(&[FIRST]));
    let text = f.read(CHILD);
    assert!(text.contains("\nparent: \"[[first - a-1]]\"\n"), "{text}");
    assert!(text.contains("> | Parent | [[first - a-1]] |"), "{text}");
}

/// Whether `dir` looks names up without regard to case, as APFS and
/// HFS+ do by default.
fn folds_case(dir: &Path) -> bool {
    let probe = dir.join("callisto-case-probe");
    fs::write(&probe, "").unwrap();
    let folds = dir.join("CALLISTO-CASE-PROBE").exists();
    fs::remove_file(probe).unwrap();
    folds
}

#[test]
fn case_folded_filename_is_taken() {
    let f = fixture("a", "a");
    if !folds_case(&f.vault) {
        return;
    }
    put(&f.vault, "Projects/a/FIRST - a-1.md", "not callisto's\n");
    child_of_first(&f);
    f.run().unwrap();
    assert_links_show_id(&f.read(CHILD));
}

#[test]
fn project_named_past_a_link_has_no_projects() {
    let f = fixture("a", "x#y");
    f.set_export(&[line("a-1", "first", "open")]);
    let report = f.run().unwrap();
    assert_eq!(report.created, paths(&["Projects/x#y/first - a-1.md"]));
    let text = f.read("Projects/x#y/first - a-1.md");
    assert!(!text.contains("projects:"), "{text}");
    let again = f.run().unwrap();
    assert!(again.created.is_empty() && again.updated.is_empty());
    assert_eq!(again.unchanged, 1);
}

#[test]
fn update_keeps_mode() {
    let f = fixture("a", "a");
    f.set_export(&[line("a-1", "first", "open")]);
    f.run().unwrap();
    let note = f.vault.join(FIRST);
    fs::set_permissions(&note, Permissions::from_mode(0o600)).unwrap();
    f.set_export(&[line("a-1", "renamed", "open")]);
    f.run().unwrap();
    assert_eq!(
        fs::metadata(&note).unwrap().permissions().mode() & 0o7777,
        0o600
    );
}

#[test]
fn foreign_id_refuses_export() {
    let f = fixture("a", "a");
    f.set_export(&[line("a-1", "first", "open"), line("other-1", "x", "open")]);
    let before = f.snapshot();
    let err = f.run().unwrap_err();
    assert_eq!(err.stage(), Stage::Export);
    assert!(err.to_string().contains("other-1"), "{err}");
    assert_eq!(f.snapshot(), before);
}

#[test]
fn export_failure_writes_nothing() {
    let f = fixture("a", "a");
    let before = f.snapshot();
    let err = f.run().unwrap_err();
    assert_eq!(err.stage(), Stage::Export);
    assert_eq!(f.snapshot(), before);
}

#[test]
fn unmapped_repo_is_skipped() {
    let f = fixture("a", "a");
    fs::remove_file(f.vault.join("Projects/a/a.md")).unwrap();
    f.set_export(&[line("a-1", "first", "open")]);
    let report = f.run().unwrap();
    assert!(
        matches!(report.warnings.as_slice(), [Warning::Unmapped(_)]),
        "{:?}",
        report.warnings
    );
    assert!(report.created.is_empty());
    assert!(!f.vault.join(FIRST).exists());
}

#[test]
fn missing_vault_is_an_error() {
    let f = fixture("a", "a");
    fs::remove_dir_all(&f.vault).unwrap();
    f.set_export(&[line("a-1", "first", "open")]);
    assert_eq!(f.run().unwrap_err().stage(), Stage::Vault);
}

#[test]
fn unchecked_prefix_warns() {
    let f = fixture("a", "a");
    f.set_export(&[line("a-1", "first", "open")]);
    let report = f.run_with(&Scope::default()).unwrap();
    assert!(
        report
            .warnings
            .iter()
            .any(|w| matches!(w, Warning::UncheckedPrefix { prefix } if prefix == "a")),
        "{:?}",
        report.warnings
    );
}

fn has_warning(report: &Report, want: impl Fn(&Warning) -> bool) -> bool {
    report.warnings.iter().any(want)
}

#[test]
fn removed_bead_is_marked_dropped() {
    let f = fixture("a", "a");
    f.set_export(&[line("a-1", "first", "open"), line("a-2", "second", "open")]);
    f.run().unwrap();
    let old = f.read(FIRST);
    f.set_export(&[line("a-2", "second", "open")]);
    let report = f.run().unwrap();
    assert_eq!(report.removed, paths(&[FIRST]));
    let now = f.read(FIRST);
    assert_eq!(Some(now.clone()), callisto::removed::mark(&old));
    assert!(now.contains("\nstatus: dropped\n"), "{now}");
    assert!(now.contains("\nbeadStatus: open\n"), "{now}");
    assert!(now.contains("\nbeadRemoved: true\n"), "{now}");
    assert!(now.contains("\nbead: a-1\nbeadPrefix: a\n"), "{now}");
}

#[test]
fn removed_closed_bead_stays_done() {
    let f = fixture("a", "a");
    f.set_export(&[line("a-1", "first", "closed")]);
    f.run().unwrap();
    f.set_export(&[]);
    f.run().unwrap();
    let now = f.read(FIRST);
    assert!(now.contains("\nstatus: done\n"), "{now}");
    assert!(now.contains("\nbeadRemoved: true\n"), "{now}");
}

#[test]
fn removal_is_idempotent() {
    let f = fixture("a", "a");
    f.set_export(&[line("a-1", "first", "open")]);
    f.run().unwrap();
    f.set_export(&[]);
    f.run().unwrap();
    let before = f.snapshot();
    let report = f.run().unwrap();
    assert!(report.removed.is_empty());
    assert_eq!(f.snapshot(), before);
}

#[test]
fn reappearing_bead_is_unmarked() {
    let f = fixture("a", "a");
    f.set_export(&[line("a-1", "first", "open")]);
    f.run().unwrap();
    let rendered = f.read(FIRST);
    f.set_export(&[]);
    f.run().unwrap();
    f.set_export(&[line("a-1", "first", "open")]);
    let report = f.run().unwrap();
    assert_eq!(report.updated, paths(&[FIRST]));
    assert_eq!(f.read(FIRST), rendered);
}

#[test]
fn empty_export_is_authoritative() {
    let f = fixture("a", "a");
    put(
        &f.vault,
        "Projects/a/hand.md",
        "---\nbead: a-1\ntype: task\nstatus: open\n---\n",
    );
    f.set_export(&[line("a-1", "first", "open"), line("a-2", "second", "open")]);
    f.run().unwrap();
    let hand = f.snapshot()[Path::new("Projects/a/hand.md")].clone();
    f.set_export(&[]);
    let report = f.run().unwrap();
    assert_eq!(
        report.removed,
        paths(&[FIRST, "Projects/a/second - a-2.md"])
    );
    assert_eq!(f.snapshot()[Path::new("Projects/a/hand.md")], hand);
}

#[test]
fn removal_spares_other_prefixes() {
    let f = fixture("a", "a");
    put(
        &f.vault,
        "Projects/x/n - a-b-1.md",
        "---\nstatus: open\nbead: a-b-1\ngeneratedBy: callisto\n---\n",
    );
    f.set_export(&[]);
    let before = f.snapshot();
    let scope = Scope {
        prefixes: vec!["a".into(), "a-b".into()],
        unreadable: vec![],
        shared: vec![],
    };
    let report = f.run_with(&scope).unwrap();
    assert!(report.removed.is_empty());
    assert_eq!(f.snapshot(), before);
}

#[test]
fn unreadable_scope_skips_removal() {
    let f = fixture("a", "a");
    f.set_export(&[line("a-1", "first", "open")]);
    f.run().unwrap();
    f.set_export(&[]);
    let before = f.snapshot();
    let scope = Scope {
        prefixes: vec!["a".into()],
        unreadable: vec![PathBuf::from("/nowhere")],
        shared: vec![],
    };
    let report = f.run_with(&scope).unwrap();
    assert!(report.removed.is_empty());
    assert_eq!(f.snapshot(), before);
    assert!(has_warning(&report, |w| matches!(
        w,
        Warning::ScopeIncomplete { .. }
    )));
}

#[test]
fn stale_temp_is_swept_without_writes() {
    let f = fixture("a", "a");
    f.set_export(&[line("a-1", "first", "open")]);
    f.run().unwrap();
    for rel in [
        "Projects/a/.callisto-x.tmp",
        "Other/.callisto-y.tmp",
        "Other/.callisto-keep.md",
    ] {
        put(&f.vault, rel, "");
    }
    fs::create_dir_all(f.vault.join("Other/.callisto-dir.tmp")).unwrap();
    let target = f.tmp.path().join("outside");
    fs::write(&target, "keep").unwrap();
    std::os::unix::fs::symlink(&target, f.vault.join("Other/.callisto-z.tmp")).unwrap();
    let report = f.run().unwrap();
    assert_eq!(report.unchanged, 1);
    assert!(!f.vault.join("Projects/a/.callisto-x.tmp").exists());
    assert!(!f.vault.join("Other/.callisto-y.tmp").exists());
    assert!(f.vault.join("Other/.callisto-keep.md").exists());
    assert!(f.vault.join("Other/.callisto-dir.tmp").is_dir());
    assert!(fs::symlink_metadata(f.vault.join("Other/.callisto-z.tmp")).is_ok());
    assert_eq!(fs::read_to_string(&target).unwrap(), "keep");
}

#[test]
fn non_utf8_removed_note_is_skipped() {
    let f = fixture("a", "a");
    f.set_export(&[line("a-1", "first", "open"), line("a-2", "second", "open")]);
    f.run().unwrap();
    let mut bytes = fs::read(f.vault.join(FIRST)).unwrap();
    bytes.extend_from_slice(b"\xff\n");
    fs::write(f.vault.join(FIRST), &bytes).unwrap();
    f.set_export(&[line("a-2", "second", "open")]);
    let report = f.run().unwrap();
    assert!(report.removed.is_empty());
    assert_eq!(fs::read(f.vault.join(FIRST)).unwrap(), bytes);
    assert!(
        has_warning(&report, |w| matches!(
            w,
            Warning::NotUtf8 { id, path } if id == "a-1" && path == Path::new(FIRST)
        )),
        "{:?}",
        report.warnings
    );
    let text = report
        .warnings
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("fix its encoding"), "{text}");
}

#[test]
fn removed_duplicate_is_skipped() {
    let f = fixture("a", "a");
    f.set_export(&[line("a-1", "first", "open")]);
    f.run().unwrap();
    fs::copy(
        f.vault.join(FIRST),
        f.vault.join("Projects/a/copy - a-1.md"),
    )
    .unwrap();
    f.set_export(&[]);
    let before = f.snapshot();
    let report = f.run().unwrap();
    assert!(report.removed.is_empty());
    assert_eq!(f.snapshot(), before);
    let dups = report
        .warnings
        .iter()
        .filter(|w| matches!(w, Warning::Duplicate { id, .. } if id == "a-1"))
        .count();
    assert_eq!(dups, 1, "{:?}", report.warnings);
}

#[test]
fn longer_prefix_export_is_refused() {
    let f = fixture("a", "a");
    f.set_export(&[line("a-1", "first", "open")]);
    f.run().unwrap();
    f.set_export(&[line("a-b-1", "theirs", "open")]);
    let before = f.snapshot();
    let scope = Scope {
        prefixes: vec!["a".into(), "a-b".into()],
        unreadable: vec![],
        shared: vec![],
    };
    let err = f.run_with(&scope).unwrap_err();
    assert_eq!(err.stage(), Stage::Export);
    assert!(err.to_string().contains("a-b-1"), "{err}");
    assert_eq!(f.snapshot(), before);
}

const THEIRS: &str = "Projects/a/theirs - a-b-1.md";

/// `a` is configured; `a-b` is not, yet shares `a`'s project folder.
fn overlap() -> (Fixture, Database) {
    let f = fixture("a", "a");
    let ab = f.sibling("a-b", "a-b", "a");
    f.set_export(&[line("a-1", "first", "open")]);
    set_export(&ab, &[line("a-b-1", "theirs", "open")]);
    (f, ab)
}

#[test]
fn prefix_overlap_never_flips() {
    let (f, ab) = overlap();
    let scope = f.scope();
    let first = f.run_database(&ab, &scope).unwrap();
    assert_eq!(first.created, paths(&[THEIRS]));
    let rendered = f.read(THEIRS);
    for _ in 0..2 {
        let a = f.run().unwrap();
        assert!(a.removed.is_empty(), "{:?}", a.removed);
        assert_eq!(f.read(THEIRS), rendered);
        let b = f.run_database(&ab, &scope).unwrap();
        assert!(b.removed.is_empty() && b.updated.is_empty(), "{b:?}");
        assert_eq!(f.read(THEIRS), rendered);
    }
    assert!(f.read(FIRST).contains("\nbeadPrefix: a\n"));
}

#[test]
fn prefix_overlap_still_marks_own_removals() {
    let (f, ab) = overlap();
    let scope = f.scope();
    f.run_database(&ab, &scope).unwrap();
    f.run().unwrap();
    f.set_export(&[]);
    let a = f.run().unwrap();
    assert_eq!(a.removed, paths(&[FIRST]));
    set_export(&ab, &[]);
    let b = f.run_database(&ab, &scope).unwrap();
    assert_eq!(b.removed, paths(&[THEIRS]));
}

fn strip_prefix_line(f: &Fixture, rel: &str) {
    let text = f.read(rel);
    let legacy: String = text
        .split_inclusive('\n')
        .filter(|l| !l.starts_with("beadPrefix:"))
        .collect();
    assert_ne!(legacy, text);
    fs::write(f.vault.join(rel), legacy).unwrap();
}

#[test]
fn legacy_note_falls_back_to_owner() {
    let f = fixture("a", "a");
    f.set_export(&[line("a-1", "first", "open"), line("a-2", "second", "open")]);
    f.run().unwrap();
    strip_prefix_line(&f, FIRST);
    f.set_export(&[line("a-2", "second", "open")]);
    let report = f.run().unwrap();
    assert_eq!(report.removed, paths(&[FIRST]));
    assert!(f.read(FIRST).contains("\nbeadRemoved: true\n"));
}

#[test]
fn legacy_overlap_heals_after_one_rewrite() {
    let (f, ab) = overlap();
    let scope = f.scope();
    f.run_database(&ab, &scope).unwrap();
    strip_prefix_line(&f, THEIRS);
    // Without the field only the configured list decides, and it
    // cannot see a-b.
    assert_eq!(f.run().unwrap().removed, paths(&[THEIRS]));
    let b = f.run_database(&ab, &scope).unwrap();
    assert_eq!(b.updated, paths(&[THEIRS]));
    let healed = f.read(THEIRS);
    assert!(!healed.contains("beadRemoved"), "{healed}");
    assert!(f.run().unwrap().removed.is_empty());
    assert_eq!(f.read(THEIRS), healed);
}

#[test]
fn shared_prefix_skips_removal() {
    let f = fixture("a", "a");
    f.set_export(&[line("a-1", "first", "open")]);
    f.run().unwrap();
    f.set_export(&[line("a-2", "second", "open")]);
    let scope = Scope {
        shared: vec!["a".into()],
        ..f.scope()
    };
    let report = f.run_with(&scope).unwrap();
    assert_eq!(report.created, paths(&["Projects/a/second - a-2.md"]));
    assert!(report.removed.is_empty());
    assert!(!f.read(FIRST).contains("beadRemoved"));
    assert!(
        has_warning(
            &report,
            |w| matches!(w, Warning::SharedPrefix { prefix } if prefix == "a")
        ),
        "{:?}",
        report.warnings
    );
}

#[test]
fn config_project_beats_the_bd_setting() {
    let f = fixture("a", "a");
    put(&f.vault, "Areas/B/B.md", "# B\n");
    fs::write(f.database.dir.join("project"), "a").unwrap();
    f.set_export(&[line("a-1", "first", "open")]);
    let database = f.filed(Filing::Single {
        project: Some("B".into()),
    });
    let report = f.run_database(&database, &f.scope()).unwrap();
    assert_eq!(report.created, paths(&["Areas/B/first - a-1.md"]));
}

#[test]
fn missing_config_project_does_not_fall_back() {
    let f = fixture("a", "a");
    f.set_export(&[line("a-1", "first", "open")]);
    let database = f.filed(Filing::Single {
        project: Some("Gone".into()),
    });
    let report = f.run_database(&database, &f.scope()).unwrap();
    assert!(report.created.is_empty());
    let [Warning::Unmapped(unmapped)] = report.warnings.as_slice() else {
        panic!("{:?}", report.warnings);
    };
    assert_eq!(unmapped.name, "Gone");
}

#[test]
fn bd_project_setting_is_an_override() {
    let f = fixture("a", "a");
    fs::write(f.database.dir.join("project"), "Gone").unwrap();
    f.set_export(&[line("a-1", "first", "open")]);
    let report = f.run().unwrap();
    assert!(report.created.is_empty());
    let [Warning::Unmapped(unmapped)] = report.warnings.as_slice() else {
        panic!("{:?}", report.warnings);
    };
    assert_eq!(unmapped.name, "Gone");
    assert!(unmapped.overridden);
}

/// `line`, carrying `metadata` when given.
fn field_line(id: &str, title: &str, status: &str, metadata: Option<&str>) -> String {
    let base = line(id, title, status);
    match metadata {
        Some(json) => format!(r#"{},"metadata":{json}}}"#, base.strip_suffix('}').unwrap()),
        None => base,
    }
}

/// A database with prefix `zz`, and vault projects `A` and `B`.
fn split() -> Fixture {
    let f = fixture("zz", "repo");
    put(&f.vault, "Projects/A/A.md", "# A\n");
    put(&f.vault, "Projects/B/B.md", "# B\n");
    fs::create_dir_all(f.vault.join("Inbox")).unwrap();
    f
}

const A_AND_B: &[(&str, &str)] = &[("o/a", "A"), ("o/b", "B")];

const ONE: &str = "Projects/A/one - zz-1.md";
const INDEX: &str = "Inbox/Unfiled beads - zz.md";
const REPO_A: Option<&str> = Some(r#"{"repo":"o/a"}"#);

#[test]
fn split_beads_file_under_their_project() {
    let f = split();
    f.set_export(&[
        field_line("zz-1", "one", "open", REPO_A),
        field_line("zz-2", "two", "open", Some(r#"{"repo":"o/b"}"#)),
    ]);
    let report = f.run_split(A_AND_B).unwrap();
    assert_eq!(
        report.created,
        paths(&[ONE, "Projects/B/two - zz-2.md", INDEX])
    );
    let one = f.read(ONE);
    assert!(one.contains("\nprojects:\n  - \"[[A]]\"\n"), "{one}");
    let two = f.read("Projects/B/two - zz-2.md");
    assert!(two.contains("\nprojects:\n  - \"[[B]]\"\n"), "{two}");
    assert!(report.unfiled.is_empty(), "{:?}", report.unfiled);
    assert!(report.conditions.is_empty(), "{:?}", report.conditions);
}

#[test]
fn two_values_map_to_one_project() {
    let f = split();
    f.set_export(&[field_line(
        "zz-1",
        "one",
        "open",
        Some(r#"{"repo":"o/b-old"}"#),
    )]);
    let report = f.run_split(&[("o/b", "B"), ("o/b-old", "B")]).unwrap();
    assert_eq!(report.created, paths(&["Projects/B/one - zz-1.md", INDEX]));
}

#[test]
fn unmapped_value_files_under_its_last_segment() {
    let f = split();
    f.set_export(&[field_line(
        "zz-1",
        "one",
        "open",
        Some(r#"{"repo":"github.com/o/B/"}"#),
    )]);
    let report = f.run_split(&[]).unwrap();
    assert_eq!(report.created, paths(&["Projects/B/one - zz-1.md", INDEX]));
}

#[test]
fn unfiled_bead_gets_no_note() {
    let f = split();
    f.set_export(&[field_line("zz-3", "three", "open", None)]);
    let before = f.snapshot_notes();
    let report = f.run_split(A_AND_B).unwrap();
    assert_eq!(f.snapshot_notes(), before);
    assert_eq!(report.created, paths(&[INDEX]));
    assert_eq!(
        report.unfiled,
        [Unfiled {
            id: "zz-3".into(),
            title: "three".into(),
            status: "open".into(),
            reason: Reason::NoField,
        }]
    );
    assert_eq!(
        report.conditions,
        [Condition::Unfiled {
            ids: vec!["zz-3".into()]
        }]
    );
}

#[test]
fn non_string_field_is_unfiled() {
    let f = split();
    f.set_export(&[
        field_line("zz-1", "one", "open", Some(r#"{"repo":7}"#)),
        field_line("zz-2", "two", "open", Some(r#"{"repo":["o/a"]}"#)),
    ]);
    let before = f.snapshot_notes();
    let report = f.run_split(A_AND_B).unwrap();
    assert_eq!(f.snapshot_notes(), before);
    let reasons: Vec<&Reason> = report.unfiled.iter().map(|u| &u.reason).collect();
    assert_eq!(
        reasons,
        [&Reason::Json("7".into()), &Reason::Json("[\"o/a\"]".into())]
    );
    assert_eq!(
        report.conditions,
        [Condition::Unfiled {
            ids: vec!["zz-1".into(), "zz-2".into()]
        }]
    );
}

#[test]
fn missing_project_is_unfiled() {
    let f = split();
    f.set_export(&[field_line("zz-1", "one", "open", REPO_A)]);
    let before = f.snapshot_notes();
    let report = f.run_split(&[("o/a", "Gone")]).unwrap();
    assert_eq!(f.snapshot_notes(), before);
    assert_eq!(
        report.unfiled[0].reason,
        Reason::MissingProject("Gone".into())
    );
    assert_eq!(
        report.conditions,
        [Condition::MissingProject {
            project: "Gone".into(),
            ids: vec!["zz-1".into()]
        }]
    );
    assert!(
        !has_warning(&report, |w| matches!(w, Warning::Unmapped(_))),
        "{:?}",
        report.warnings
    );
}

#[test]
fn unfiled_existing_note_is_left_alone() {
    let f = split();
    f.set_export(&[field_line("zz-1", "one", "open", REPO_A)]);
    f.run_split(A_AND_B).unwrap();
    let before = f.snapshot_notes();
    f.set_export(&[field_line(
        "zz-1",
        "renamed",
        "open",
        Some(r#"{"repo":"o/z"}"#),
    )]);
    let report = f.run_split(A_AND_B).unwrap();
    assert_eq!(f.snapshot_notes(), before);
    assert!(report.removed.is_empty());
    assert_eq!(report.updated, paths(&[INDEX]));
    assert_eq!(report.unfiled[0].reason, Reason::MissingProject("z".into()));
}

#[test]
fn moved_split_note_updates_in_place() {
    let f = split();
    f.set_export(&[field_line("zz-1", "one", "open", REPO_A)]);
    f.run_split(A_AND_B).unwrap();
    fs::create_dir_all(f.vault.join("Archives")).unwrap();
    fs::rename(f.vault.join(ONE), f.vault.join("Archives/one - zz-1.md")).unwrap();
    f.set_export(&[field_line("zz-1", "renamed", "open", REPO_A)]);
    let report = f.run_split(A_AND_B).unwrap();
    assert_eq!(report.updated, paths(&["Archives/one - zz-1.md"]));
    assert!(report.created.is_empty());
    let names: Vec<_> = fs::read_dir(f.vault.join("Projects/A"))
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, ["A.md"]);
}

#[test]
fn link_to_unfiled_bead_shows_id() {
    let f = split();
    let deps = r#","dependencies":[{"issue_id":"zz-1","depends_on_id":"zz-2","type":"parent-child","created_at":"2026-10-11T02:21:06Z"},{"issue_id":"zz-1","depends_on_id":"zz-2","type":"blocks","created_at":"2026-10-11T02:21:06Z"}]}"#;
    let child = field_line("zz-1", "one", "open", REPO_A);
    let child = format!("{}{deps}", child.strip_suffix('}').unwrap());
    f.set_export(&[child, field_line("zz-2", "first", "open", None)]);
    let report = f.run_split(A_AND_B).unwrap();
    assert_eq!(report.created, paths(&[ONE, INDEX]));
    assert_links_show(&f.read(ONE), "zz-2");
}

#[test]
fn split_removal_marks_absent_beads() {
    let f = split();
    f.set_export(&[field_line("zz-1", "one", "open", REPO_A)]);
    f.run_split(A_AND_B).unwrap();
    f.set_export(&[]);
    let report = f.run_split(A_AND_B).unwrap();
    assert_eq!(report.removed, paths(&[ONE]));
}

#[test]
fn split_ignores_project_override() {
    let f = split();
    fs::write(f.database.dir.join("project"), "A").unwrap();
    f.set_export(&[field_line("zz-1", "one", "open", Some(r#"{"repo":"o/b"}"#))]);
    let report = f.run_split(A_AND_B).unwrap();
    assert_eq!(report.created, paths(&["Projects/B/one - zz-1.md", INDEX]));
}

const VALID_INDEX: &str = "---\ngeneratedBy: callisto\nbeadPrefix: zz\n---\n";

/// Exports filed `zz-1` and unfiled `zz-3`, and runs.
fn index_run(f: &Fixture) -> Report {
    f.set_export(&[
        field_line("zz-1", "one", "open", REPO_A),
        field_line("zz-3", "three", "open", None),
    ]);
    f.run_split(A_AND_B).unwrap()
}

fn has_condition(report: &Report, want: &Condition) -> bool {
    report.conditions.contains(want)
}

#[test]
fn index_lists_unfiled() {
    let f = split();
    let report = index_run(&f);
    assert_eq!(report.created, paths(&[ONE, INDEX]));
    let index = f.read(INDEX);
    assert!(
        index.contains("| zz-3 | three | open | no metadata.repo |"),
        "{index}"
    );
    assert!(!index.contains("zz-1"), "{index}");
}

#[test]
fn index_empties_once_filed() {
    let f = split();
    index_run(&f);
    f.set_export(&[
        field_line("zz-1", "one", "open", REPO_A),
        field_line("zz-3", "three", "open", Some(r#"{"repo":"o/b"}"#)),
    ]);
    let report = f.run_split(A_AND_B).unwrap();
    assert!(report.updated.contains(&PathBuf::from(INDEX)), "{report:?}");
    let index = f.read(INDEX);
    assert!(index.contains("No unfiled beads."), "{index}");
    assert!(f.vault.join("Projects/B/three - zz-3.md").is_file());
}

#[test]
fn unchanged_index_is_not_rewritten() {
    let f = split();
    index_run(&f);
    let before = f.snapshot()[Path::new(INDEX)].clone();
    let report = index_run(&f);
    assert!(report.updated.is_empty(), "{:?}", report.updated);
    assert!(report.created.is_empty(), "{:?}", report.created);
    assert_eq!(f.snapshot()[Path::new(INDEX)], before);
}

#[test]
fn missing_unfiled_folder() {
    let f = split();
    fs::remove_dir(f.vault.join("Inbox")).unwrap();
    let report = index_run(&f);
    assert_eq!(report.created, paths(&[ONE]));
    assert!(!f.vault.join("Inbox").exists());
    assert!(
        has_condition(
            &report,
            &Condition::UnfiledMissing {
                folder: "Inbox".into()
            }
        ),
        "{:?}",
        report.conditions
    );
}

fn assert_index_taken(f: &Fixture, text: &str) {
    put(&f.vault, INDEX, text);
    // zz-9 stays exported so a bead note at INDEX is not marked removed.
    f.set_export(&[
        field_line("zz-3", "three", "open", None),
        field_line("zz-9", "nine", "open", None),
    ]);
    let report = f.run_split(A_AND_B).unwrap();
    assert_eq!(f.read(INDEX), text);
    assert!(
        has_condition(&report, &Condition::IndexTaken { path: INDEX.into() }),
        "{:?}",
        report.conditions
    );
}

#[test]
fn hand_written_index_is_taken() {
    assert_index_taken(&split(), "# mine\n");
}

#[test]
fn bead_note_at_index_is_taken() {
    assert_index_taken(
        &split(),
        "---\nbead: zz-9\ngeneratedBy: callisto\nbeadPrefix: zz\n---\n",
    );
}

#[test]
fn other_prefix_index_is_taken() {
    assert_index_taken(
        &split(),
        "---\ngeneratedBy: callisto\nbeadPrefix: yy\n---\n",
    );
}

#[test]
fn non_utf8_index_is_taken() {
    let f = split();
    let bytes = b"---\ngeneratedBy: callisto\nbeadPrefix: zz\n---\n\xff\n";
    fs::write(f.vault.join(INDEX), bytes).unwrap();
    let report = index_run(&f);
    assert_eq!(fs::read(f.vault.join(INDEX)).unwrap(), bytes);
    assert!(
        has_condition(&report, &Condition::IndexTaken { path: INDEX.into() }),
        "{:?}",
        report.conditions
    );
}

#[test]
fn symlinked_index_is_taken() {
    let f = split();
    put(&f.vault, "Inbox/real.md", VALID_INDEX);
    std::os::unix::fs::symlink(f.vault.join("Inbox/real.md"), f.vault.join(INDEX)).unwrap();
    let report = index_run(&f);
    assert_eq!(f.read("Inbox/real.md"), VALID_INDEX);
    assert!(
        fs::symlink_metadata(f.vault.join(INDEX))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(
        has_condition(&report, &Condition::IndexTaken { path: INDEX.into() }),
        "{:?}",
        report.conditions
    );
}

#[test]
fn single_database_writes_no_index() {
    let f = fixture("a", "a");
    fs::create_dir_all(f.vault.join("Inbox")).unwrap();
    f.set_export(&[line("a-1", "first", "open")]);
    let report = f.run().unwrap();
    assert_eq!(fs::read_dir(f.vault.join("Inbox")).unwrap().count(), 0);
    assert!(report.conditions.is_empty(), "{:?}", report.conditions);
}

#[test]
fn index_is_not_a_bead_note() {
    let f = split();
    index_run(&f);
    let index = Index::scan(&f.vault).unwrap();
    assert!(
        index.notes().iter().all(|n| n.path != Path::new(INDEX)),
        "{:?}",
        index.notes()
    );
}
