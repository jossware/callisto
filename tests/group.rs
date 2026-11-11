//! Grouping configured repos by the beads database they use.

use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use callisto::bd::Bd;
use callisto::group::Groups;
use callisto::repo::{self, Repo};

mod common;

const FAKE_BD: &str = r#"#!/bin/sh
d="$PWD/.beads"
[ -f .beads/redirect ] && d=$(cat .beads/redirect)
echo "$*" >> .beads/calls
case "$1" in
context)
  [ -e .beads/context-fails ] && { echo 'no database' >&2; exit 1; }
  printf '{"beads_dir":"%s"}\n' "$d" ;;
config) printf '{"key":"%s","value":"%s"}\n' "$4" "$(cat "$d/prefix" 2>/dev/null)" ;;
*) exit 9 ;;
esac
"#;

struct Fixture {
    tmp: TempDir,
    bd: Bd,
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let bd_path = tmp.path().join("bd");
    common::write_script(&bd_path, FAKE_BD);
    Fixture {
        bd: Bd::new(bd_path),
        tmp,
    }
}

impl Fixture {
    fn path(&self, rel: &str) -> PathBuf {
        self.tmp.path().join(rel)
    }

    /// A checkout `repos/<name>` with its own database using `prefix`.
    fn repo(&self, name: &str, prefix: &str) -> PathBuf {
        let dir = self.checkout(name);
        fs::write(dir.join(".beads/prefix"), prefix).unwrap();
        dir
    }

    /// A checkout `repos/<name>` whose `.beads` redirects to `target`.
    fn redirected(&self, name: &str, target: &Path) -> PathBuf {
        let dir = self.checkout(name);
        fs::write(dir.join(".beads/redirect"), target.display().to_string()).unwrap();
        dir
    }

    /// A database at `shared/.beads`, with `prefix` when given.
    fn shared(&self, prefix: Option<&str>) -> PathBuf {
        let db = self.path("shared/.beads");
        fs::create_dir_all(&db).unwrap();
        if let Some(p) = prefix {
            fs::write(db.join("prefix"), p).unwrap();
        }
        db
    }

    fn checkout(&self, name: &str) -> PathBuf {
        let dir = self.path("repos").join(name);
        fs::create_dir_all(dir.join(".git")).unwrap();
        fs::create_dir_all(dir.join(".beads")).unwrap();
        dir
    }

    fn calls(&self, dir: &Path) -> String {
        fs::read_to_string(dir.join(".beads/calls")).unwrap_or_default()
    }

    fn groups(&self, paths: &[PathBuf]) -> Groups {
        let mut g = Groups::derive(&self.bd, paths);
        g.read_prefixes(&self.bd);
        g
    }
}

fn located(dir: &Path) -> Repo {
    repo::locate(dir).unwrap()
}

#[test]
fn separate_databases_are_separate_groups() {
    let f = fixture();
    let a = f.repo("a", "a");
    let b = f.repo("b", "b");
    let g = f.groups(&[a, b]);
    assert_eq!(g.groups.len(), 2);
    let scope = g.scope();
    assert_eq!(scope.prefixes, ["a", "b"]);
    assert!(scope.unreadable.is_empty(), "{:?}", scope.unreadable);
}

#[test]
fn shared_database_is_one_group_with_one_prefix_read() {
    let f = fixture();
    let db = f.shared(Some("w"));
    let a = f.redirected("a", &db);
    let b = f.redirected("b", &db);
    let g = f.groups(&[a.clone(), b.clone()]);
    assert_eq!(g.groups.len(), 1);
    assert_eq!(g.groups[0].members, [located(&a), located(&b)]);
    assert_eq!(g.groups[0].prefix.as_deref(), Some("w"));
    let calls = f.calls(&a) + &f.calls(&b);
    assert_eq!(calls.matches("config get").count(), 1, "{calls}");
    assert_eq!(g.scope().prefixes, ["w"]);
}

#[test]
fn symlinked_beads_dir_matches() {
    let f = fixture();
    let a = f.repo("a", "a");
    let link = f.path("link");
    symlink(a.join(".beads"), &link).unwrap();
    let b = f.redirected("b", &link);
    let g = f.groups(&[a, b]);
    assert_eq!(g.groups.len(), 1);
    assert_eq!(g.groups[0].members.len(), 2);
}

#[test]
fn worktree_and_main_are_one_member() {
    let f = fixture();
    let a = f.repo("a", "a");
    let wt = f.path("wt");
    let admin = a.join(".git/worktrees/wt");
    fs::create_dir_all(&admin).unwrap();
    fs::write(admin.join("commondir"), "../..\n").unwrap();
    fs::create_dir_all(&wt).unwrap();
    fs::write(wt.join(".git"), format!("gitdir: {}\n", admin.display())).unwrap();
    let g = f.groups(&[a.clone(), wt]);
    assert_eq!(g.groups.len(), 1);
    assert_eq!(g.groups[0].members, [located(&a)]);
    assert!(g.ungrouped.is_empty());
    assert!(g.scope().unreadable.is_empty());
}

#[test]
fn scope_reads_prefixes() {
    let f = fixture();
    let a = f.repo("a", "a");
    let b = f.repo("b", "b");
    let stray = f.path("stray");
    fs::create_dir_all(&stray).unwrap();
    let scope = f.groups(&[a, b, stray.clone()]).scope();
    assert_eq!(scope.prefixes, ["a", "b"]);
    assert_eq!(scope.unreadable, [stray]);
}

#[test]
fn context_failure_is_unreadable() {
    let f = fixture();
    let a = f.repo("a", "a");
    let b = f.repo("b", "b");
    fs::write(b.join(".beads/context-fails"), "").unwrap();
    let g = f.groups(&[a, b.clone()]);
    assert_eq!(g.scope().prefixes, ["a"]);
    assert_eq!(g.ungrouped.len(), 1);
    assert_eq!(g.ungrouped[0].path, b);
    assert_eq!(g.scope().unreadable, [b]);
    let err = g.ungrouped[0].error.as_ref().unwrap().to_string();
    assert!(err.contains("no database"), "{err}");
}

#[test]
fn prefix_failure_marks_every_member_unreadable() {
    let f = fixture();
    let db = f.shared(None);
    let a = f.redirected("a", &db);
    let b = f.redirected("b", &db);
    let g = f.groups(&[a.clone(), b.clone()]);
    assert_eq!(g.groups.len(), 1);
    assert_eq!(g.groups[0].prefix, None);
    let scope = g.scope();
    assert!(scope.prefixes.is_empty());
    assert_eq!(scope.unreadable, [a, b]);
}

#[test]
fn target_member_needs_no_context_call() {
    let f = fixture();
    let a = f.repo("a", "a");
    let b = f.repo("b", "b");
    let g = f.groups(&[a.clone(), b]);
    let found = g.of_target(&f.bd, &located(&a)).unwrap().unwrap();
    assert_eq!(found.members, [located(&a)]);
    let calls = f.calls(&a);
    assert_eq!(calls.matches("context").count(), 1, "{calls}");
}

#[test]
fn unlisted_target_matches_by_database() {
    let f = fixture();
    let db = f.shared(Some("w"));
    let a = f.redirected("a", &db);
    let b = f.redirected("b", &db);
    let c = located(&f.redirected("c", &db));
    let g = f.groups(&[a.clone(), b]);
    let found = g.of_target(&f.bd, &c).unwrap().unwrap();
    assert_eq!(found.database, db.canonicalize().unwrap());
    assert_eq!(found.representative(Some(&c)), &located(&a));
    assert!(g.of_member(&c.root).is_none());
}

#[test]
fn member_target_is_its_own_representative() {
    let f = fixture();
    let db = f.shared(Some("w"));
    let a = f.redirected("a", &db);
    let b = located(&f.redirected("b", &db));
    let g = f.groups(&[a, b.root.clone()]);
    let found = g.of_member(&b.root).unwrap();
    assert_eq!(found.representative(Some(&b)), &b);
}

#[test]
fn unlisted_target_on_another_database_is_none() {
    let f = fixture();
    let a = f.repo("a", "a");
    let c = located(&f.repo("c", "c"));
    let g = f.groups(&[a]);
    assert!(g.of_target(&f.bd, &c).unwrap().is_none());
}

#[test]
fn one_failing_root_is_ungrouped_once() {
    let f = fixture();
    let b = f.repo("b", "b");
    fs::write(b.join(".beads/context-fails"), "").unwrap();
    let sub = b.join("sub");
    fs::create_dir_all(&sub).unwrap();
    let g = f.groups(&[b.clone(), sub.clone()]);
    assert_eq!(g.ungrouped.len(), 1);
    let calls = f.calls(&b);
    assert_eq!(calls.matches("context").count(), 1, "{calls}");
    assert_eq!(g.scope().unreadable, [b, sub]);
}
