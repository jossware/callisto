//! Grouping configured repos by the beads database they use.

use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use callisto::bd::Bd;
use callisto::config::{RepoEntry, TableEntry};
use callisto::group::{Claim, FieldClaims, Groups, Ownership, Placed, Unresolvable};
use callisto::remote::Git;
use callisto::repo::{self, Repo};
use callisto::standing::Condition;

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

/// Reads the origin remote from `.git/origin` in the `-C` directory.
const FAKE_GIT: &str = r#"#!/bin/sh
cat "$2/.git/origin" 2>/dev/null || { echo "error: No such remote 'origin'" >&2; exit 2; }
"#;

struct Fixture {
    tmp: TempDir,
    bd: Bd,
    git: Git,
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let bd_path = tmp.path().join("bd");
    common::write_script(&bd_path, FAKE_BD);
    let git_path = tmp.path().join("git");
    common::write_script(&git_path, FAKE_GIT);
    Fixture {
        bd: Bd::new(bd_path),
        git: Git::new(git_path),
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
        let entries: Vec<RepoEntry> = paths.iter().cloned().map(RepoEntry::Path).collect();
        let mut g = Groups::derive(&self.bd, &entries);
        g.read_prefixes(&self.bd);
        g
    }

    fn entries(&self, entries: &[RepoEntry]) -> Groups {
        let mut g = Groups::derive(&self.bd, entries);
        g.read_prefixes(&self.bd);
        g.resolve(&self.git);
        g
    }
}

fn table(path: &Path, value: Option<&[&str]>, project: Option<&str>) -> RepoEntry {
    table_on("metadata.repo", path, value, project)
}

fn table_on(field: &str, path: &Path, value: Option<&[&str]>, project: Option<&str>) -> RepoEntry {
    RepoEntry::Table(TableEntry {
        path: path.to_path_buf(),
        field: field.to_string(),
        value: value.map(|v| v.iter().map(|s| s.to_string()).collect()),
        project: project.map(str::to_string),
    })
}

fn claim(values: &[&str], project: &str) -> Claim {
    Claim {
        values: values.iter().map(|s| s.to_string()).collect(),
        project: project.to_string(),
    }
}

fn field_claims(claims: Vec<Claim>) -> Ownership {
    Ownership::Field(FieldClaims {
        field: "metadata.repo".parse().unwrap(),
        claims,
    })
}

fn broken(ownership: &Ownership) -> &str {
    match ownership {
        Ownership::Broken(reason) => reason,
        other => panic!("not broken: {other:?}"),
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

#[test]
fn table_group_resolves_explicit_claims() {
    let f = fixture();
    let db = f.shared(Some("w"));
    let a = f.redirected("a", &db);
    let b = f.redirected("b", &db);
    let entries = [
        table(&a, Some(&["o/a"]), Some("A")),
        table(&b, Some(&["o/b", "o/b-old"]), None),
    ];
    let g = f.entries(&entries);
    assert_eq!(g.groups.len(), 1);
    assert_eq!(
        g.groups[0].entries,
        [
            Placed {
                repo: located(&a),
                entry: entries[0].clone(),
            },
            Placed {
                repo: located(&b),
                entry: entries[1].clone(),
            },
        ]
    );
    assert_eq!(
        g.groups[0].ownership,
        field_claims(vec![claim(&["o/a"], "A"), claim(&["o/b", "o/b-old"], "b")])
    );
    assert!(!g.degraded());
    assert!(g.conditions().is_empty());
}

#[test]
fn defaulted_value_reads_origin() {
    let f = fixture();
    let db = f.shared(Some("w"));
    let a = f.redirected("a", &db);
    fs::write(a.join(".git/origin"), "git@GitHub.com:o/a.git\n").unwrap();
    let g = f.entries(&[table(&a, None, Some("A"))]);
    assert_eq!(
        g.groups[0].ownership,
        field_claims(vec![claim(&["github.com/o/a"], "A")])
    );
    assert!(!g.degraded());
}

#[test]
fn missing_origin_is_unresolvable() {
    let f = fixture();
    let db = f.shared(Some("w"));
    let a = f.redirected("a", &db);
    let b = f.redirected("b", &db);
    let g = f.entries(&[table(&a, Some(&["o/a"]), None), table(&b, None, None)]);
    assert_eq!(g.unresolvable.len(), 1, "{:?}", g.unresolvable);
    assert_eq!(g.unresolvable[0].path, b);
    let reason = &g.unresolvable[0].reason;
    assert!(reason.contains("No such remote"), "{reason}");
    assert_eq!(
        g.groups[0].ownership,
        field_claims(vec![claim(&["o/a"], "a")])
    );
    assert!(g.degraded());
    assert_eq!(
        g.conditions(),
        [Condition::Unresolvable {
            path: b,
            reason: reason.clone(),
        }]
    );
    assert!(g.scope().unreadable.is_empty());
}

#[test]
fn absent_table_checkout_is_unresolvable_but_scope_complete() {
    let f = fixture();
    let home = f.repo("home", "h");
    let missing = f.path("missing");
    let g = f.entries(&[RepoEntry::Path(home), table(&missing, Some(&["o/x"]), None)]);
    assert_eq!(
        g.unresolvable,
        [Unresolvable {
            path: missing,
            reason: "not in a repository".to_string(),
        }]
    );
    let scope = g.scope();
    assert_eq!(scope.prefixes, ["h"]);
    assert!(scope.unreadable.is_empty(), "{:?}", scope.unreadable);
}

#[test]
fn failed_context_table_entry_is_unresolvable_but_scope_complete() {
    let f = fixture();
    let a = f.repo("a", "a");
    fs::write(a.join(".beads/context-fails"), "").unwrap();
    let g = f.entries(&[table(&a, Some(&["o/a"]), None)]);
    assert_eq!(g.unresolvable.len(), 1);
    let reason = &g.unresolvable[0].reason;
    assert!(reason.contains("no database"), "{reason}");
    assert!(g.scope().unreadable.is_empty());
}

#[test]
fn unreadable_prefix_table_entry_is_in_scope_unreadable() {
    let f = fixture();
    let db = f.shared(None);
    let a = f.redirected("a", &db);
    let g = f.entries(&[table(&a, Some(&["o/a"]), None)]);
    assert_eq!(
        g.unresolvable,
        [Unresolvable {
            path: a.clone(),
            reason: "cannot read the issue prefix".to_string(),
        }]
    );
    assert_eq!(g.scope().unreadable, [a]);
}

#[test]
fn mixed_group_is_broken() {
    let f = fixture();
    let home = f.repo("home", "h");
    let db = f.shared(Some("w"));
    let a = f.redirected("a", &db);
    let b = f.redirected("b", &db);
    let g = f.entries(&[
        RepoEntry::Path(home),
        RepoEntry::Path(a),
        table(&b, Some(&["o/b"]), None),
    ]);
    assert_eq!(g.groups[0].ownership, Ownership::Prefix);
    let reason = broken(&g.groups[1].ownership);
    assert!(
        reason.contains("path and table entries share one database"),
        "{reason}"
    );
}

#[test]
fn different_fields_is_broken() {
    let f = fixture();
    let home = f.repo("home", "h");
    let db = f.shared(Some("w"));
    let a = f.redirected("a", &db);
    let b = f.redirected("b", &db);
    let g = f.entries(&[
        RepoEntry::Path(home),
        table(&a, Some(&["o/a"]), None),
        table_on("metadata.other", &b, Some(&["o/b"]), None),
    ]);
    assert_eq!(g.groups[0].ownership, Ownership::Prefix);
    let reason = broken(&g.groups[1].ownership);
    assert!(
        reason.contains("table entries use different fields (metadata.repo, metadata.other)"),
        "{reason}"
    );
}

#[test]
fn repeated_root_is_broken() {
    let f = fixture();
    let home = f.repo("home", "h");
    let db = f.shared(Some("w"));
    let a = f.redirected("a", &db);
    let sub = a.join("sub");
    fs::create_dir_all(&sub).unwrap();
    let g = f.entries(&[
        RepoEntry::Path(home),
        table(&a, Some(&["o/a"]), None),
        table(&sub, Some(&["o/b"]), None),
    ]);
    assert_eq!(g.groups[0].ownership, Ownership::Prefix);
    let entries = &g.groups[1].entries;
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].repo, entries[1].repo);
    let reason = broken(&g.groups[1].ownership);
    let want = format!("{} is listed twice", located(&a).root.display());
    assert!(reason.contains(&want), "{reason}");
}

#[test]
fn shared_value_is_broken() {
    let f = fixture();
    let c = f.repo("c", "c");
    let db = f.shared(Some("w"));
    let a = f.redirected("a", &db);
    let b = f.redirected("b", &db);
    let g = f.entries(&[
        table(&c, Some(&["o/c"]), None),
        table(&a, Some(&["o/a", "o/x"]), None),
        table(&b, Some(&["o/x"]), None),
    ]);
    assert_eq!(
        g.groups[0].ownership,
        field_claims(vec![claim(&["o/c"], "c")])
    );
    let reason = broken(&g.groups[1].ownership);
    assert!(
        reason.contains(r#"value "o/x" is claimed by more than one entry"#),
        "{reason}"
    );
}

#[test]
fn path_only_config_is_unchanged() {
    let f = fixture();
    let a = f.repo("a", "a");
    let b = f.repo("b", "b");
    let g = f.entries(&[RepoEntry::Path(a), RepoEntry::Path(b)]);
    assert_eq!(g.groups.len(), 2);
    assert!(g.groups.iter().all(|g| g.ownership == Ownership::Prefix));
    assert!(g.unresolvable.is_empty());
    assert!(!g.degraded());
    let scope = g.scope();
    assert_eq!(scope.prefixes, ["a", "b"]);
    assert!(scope.unreadable.is_empty(), "{:?}", scope.unreadable);
}
