//! The YAML block at the head of a generated note: TaskNotes fields,
//! then the bead's own.

use std::collections::BTreeSet;

use jiff::Timestamp;
use jiff::tz::TimeZone;

use crate::bead::Bead;
use crate::graph::Graph;
use crate::render::link::{Links, linkable, wikilink};
use crate::render::yaml::{priority, quote, scalar, status};
use crate::vault::{MARKER_KEY, MARKER_VALUE};

pub struct Context<'a> {
    pub graph: &'a Graph,
    pub links: &'a Links<'a>,
    pub project: &'a str,
    /// The repo's issue prefix, which decides which repo's sync owns
    /// the note.
    pub prefix: &'a str,
    pub tz: &'a TimeZone,
}

pub fn frontmatter(bead: &Bead, cx: &Context) -> String {
    let local = |at: Timestamp| at.to_zoned(cx.tz.clone());
    let link = |b: &Bead| Some(quote(&wikilink(&cx.links.target(b)?)));

    let mut out = String::from("---\ntype: task\n");
    out.push_str(&format!("title: {}\n", quote(&bead.title)));
    out.push_str(&format!("status: {}\n", status(&bead.status)));
    out.push_str(&format!("priority: {}\n", priority(bead.priority)));
    if linkable(cx.project) {
        out.push_str(&format!(
            "projects:\n  - {}\n",
            quote(&wikilink(cx.project))
        ));
    }
    out.push_str(&format!("bead: {}\n", scalar(&bead.id)));
    out.push_str(&format!("beadPrefix: {}\n", scalar(cx.prefix)));
    out.push_str(&format!("beadType: {}\n", scalar(&bead.issue_type)));
    out.push_str(&format!("beadStatus: {}\n", scalar(&bead.status)));
    out.push_str(&format!("beadPriority: {}\n", bead.priority));

    let labels: BTreeSet<&str> = bead.labels.iter().map(String::as_str).collect();
    if !labels.is_empty() {
        out.push_str("beadLabels:\n");
        for label in labels {
            out.push_str(&format!("  - {}\n", quote(label)));
        }
    }

    // A parent or blocker no link can reach shows only in the body.
    let parent = cx
        .graph
        .parents(&bead.id)
        .into_iter()
        .find_map(|r| link(r.note?));
    if let Some(parent) = parent {
        out.push_str(&format!("parent: {parent}\n"));
    }
    let blockers: Vec<String> = cx
        .graph
        .depends_on(&bead.id)
        .into_iter()
        .filter_map(|r| link(r.note?))
        .collect();
    if !blockers.is_empty() {
        out.push_str("blockedBy:\n");
        for b in blockers {
            out.push_str(&format!("  - uid: {b}\n    reltype: FINISHTOSTART\n"));
        }
    }

    let stamp = |at: Timestamp| local(at).strftime("%Y-%m-%dT%H:%M:%S%:z").to_string();
    out.push_str(&format!("dateCreated: {}\n", stamp(bead.created_at)));
    out.push_str(&format!("dateModified: {}\n", stamp(bead.updated_at)));
    if let Some(closed) = bead.closed_at.filter(|_| bead.status == "closed") {
        out.push_str(&format!("completedDate: {}\n", local(closed).date()));
    }
    out.push_str(&format!("created: {}\n", local(bead.created_at).date()));
    out.push_str(&format!("aliases:\n  - {}\n", scalar(&bead.id)));
    out.push_str(&format!("{MARKER_KEY}: {MARKER_VALUE}\n"));
    // Opens the note in Reading view via the Force note view mode plugin.
    out.push_str("obsidianUIMode: preview\n");
    out.push_str("---\n");
    out
}

#[cfg(test)]
mod tests {
    use jiff::tz::Offset;

    use super::*;
    use crate::bead::Dependency;
    use crate::bead::test_support::bead;
    use crate::export::Export;
    use crate::vault::Index;

    fn dep(kind: &str, on: &str) -> Dependency {
        Dependency {
            depends_on_id: on.into(),
            kind: kind.into(),
        }
    }

    /// Renders `id` from a graph of `beads` and `scaffolding` against an
    /// empty vault.
    fn render(id: &str, beads: Vec<Bead>, scaffolding: Vec<Bead>) -> String {
        render_in(id, beads, scaffolding, &[], "ganymede")
    }

    /// Renders `id` into `project` against a vault holding `files`.
    fn render_in(
        id: &str,
        beads: Vec<Bead>,
        scaffolding: Vec<Bead>,
        files: &[(&str, &str)],
        project: &str,
    ) -> String {
        let dir = tempfile::tempdir().unwrap();
        for (rel, text) in files {
            let path = dir.path().join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        let index = Index::scan(dir.path()).unwrap();
        let links = Links::new(&index, Default::default());
        let graph = Graph::new(Export { beads, scaffolding });
        let tz = TimeZone::fixed(Offset::constant(-4));
        let cx = Context {
            graph: &graph,
            links: &links,
            project,
            prefix: id.split('-').next().unwrap(),
            tz: &tz,
        };
        frontmatter(graph.note(id).unwrap(), &cx)
    }

    fn full_graph() -> Vec<Bead> {
        let mut b = bead("ganymede-juj.12", "point references at jossware/ganymede");
        b.status = "closed".into();
        b.closed_at = Some("2026-10-11T15:00:00Z".parse().unwrap());
        b.labels = vec![
            "zeta".into(),
            "has space".into(),
            "alpha".into(),
            "alpha".into(),
        ];
        b.dependencies = vec![
            dep("parent-child", "ganymede-juj"),
            dep("blocks", "ganymede-juj.11"),
            dep("blocks", "ganymede-juj.3"),
        ];
        vec![
            b,
            bead("ganymede-juj", "make the repository public"),
            bead("ganymede-juj.11", "transfer the repository to jossware"),
            bead("ganymede-juj.3", "pick a license"),
        ]
    }

    fn line_of<'a>(block: &'a str, key: &str) -> Option<&'a str> {
        block.lines().find(|l| l.starts_with(&format!("{key}:")))
    }

    #[test]
    fn full() {
        insta::assert_snapshot!(render("ganymede-juj.12", full_graph(), vec![]));
    }

    #[test]
    fn minimal() {
        let mut b = bead("ganymede-x1", "a plain task");
        b.priority = 2;
        insta::assert_snapshot!(render("ganymede-x1", vec![b], vec![]));
    }

    #[test]
    fn statuses() {
        let mut out = String::new();
        for s in [
            "open",
            "pinned",
            "blocked",
            "in_progress",
            "hooked",
            "deferred",
            "closed",
            "review",
        ] {
            let mut b = bead("g-1", "t");
            b.status = s.into();
            let block = render("g-1", vec![b], vec![]);
            out.push_str(line_of(&block, "status").unwrap());
            out.push_str(" / ");
            out.push_str(line_of(&block, "beadStatus").unwrap());
            out.push('\n');
        }
        insta::assert_snapshot!(out);
    }

    #[test]
    fn priorities() {
        let got: Vec<String> = (0..=4)
            .map(|p| {
                let mut b = bead("g-1", "t");
                b.priority = p;
                line_of(&render("g-1", vec![b], vec![]), "priority")
                    .unwrap()
                    .to_string()
            })
            .collect();
        assert_eq!(
            got,
            [
                "priority: high",
                "priority: high",
                "priority: normal",
                "priority: low",
                "priority: low"
            ]
        );
    }

    #[test]
    fn parent_without_note_is_omitted() {
        let mut b = bead("g-1", "t");
        b.dependencies = vec![dep("parent-child", "g-mol-1")];
        let mut mol = bead("g-mol-1", "molecule");
        mol.issue_type = "molecule".into();
        let block = render("g-1", vec![b], vec![mol]);
        assert_eq!(line_of(&block, "parent"), None);
    }

    #[test]
    fn blocked_only_by_beads_without_notes() {
        let mut b = bead("g-1", "t");
        b.dependencies = vec![dep("blocks", "other-x1"), dep("blocks", "g-mol-1")];
        let mut mol = bead("g-mol-1", "molecule");
        mol.issue_type = "molecule".into();
        let block = render("g-1", vec![b], vec![mol]);
        assert_eq!(line_of(&block, "blockedBy"), None);
    }

    fn marked(id: &str) -> String {
        format!("---\nbead: {id}\ngeneratedBy: callisto\n---\n")
    }

    #[test]
    fn notes_named_past_a_link_are_omitted() {
        let mut b = bead("g-1", "t");
        b.dependencies = vec![dep("parent-child", "g-2"), dep("blocks", "g-3")];
        for name in ["x#y", "x|y", "x]]y", "x]"] {
            let (two, three) = (marked("g-2"), marked("g-3"));
            let block = render_in(
                "g-1",
                vec![b.clone(), bead("g-2", "p"), bead("g-3", "q")],
                vec![],
                &[
                    (&format!("P/{name}.md"), &two),
                    (&format!("P/q/{name}.md"), &three),
                ],
                "ganymede",
            );
            assert_eq!(line_of(&block, "parent"), None, "{name}");
            assert_eq!(line_of(&block, "blockedBy"), None, "{name}");
        }
    }

    #[test]
    fn caret_and_bracket_names_stay_linked() {
        let mut b = bead("g-1", "t");
        b.dependencies = vec![dep("parent-child", "g-2")];
        let block = render_in(
            "g-1",
            vec![b, bead("g-2", "p")],
            vec![],
            &[("P/x^[y] - g-2.md", &marked("g-2"))],
            "foo^2",
        );
        assert_eq!(
            line_of(&block, "parent"),
            Some("parent: \"[[x^[y] - g-2]]\"")
        );
        assert!(block.contains("projects:\n  - \"[[foo^2]]\"\n"), "{block}");
    }

    #[test]
    fn parent_skips_one_no_link_reaches() {
        let mut b = bead("g-1", "t");
        b.dependencies = vec![dep("parent-child", "g-2"), dep("parent-child", "g-3")];
        let block = render_in(
            "g-1",
            vec![b, bead("g-2", "p"), bead("g-3", "q")],
            vec![],
            &[("P/x#y - g-2.md", &marked("g-2"))],
            "ganymede",
        );
        assert_eq!(line_of(&block, "parent"), Some("parent: \"[[q - g-3]]\""));
    }

    #[test]
    fn project_named_past_a_link_is_omitted() {
        let block = render_in("g-1", vec![bead("g-1", "t")], vec![], &[], "issue #12");
        assert_eq!(line_of(&block, "projects"), None, "{block}");
        assert!(block.contains("\npriority: high\nbead: g-1\n"), "{block}");
    }

    #[test]
    fn hostile_title() {
        let mut b = bead("g-1", "say \"hi\" \\ [[x]] #tag\nnext");
        b.status = "yes".into();
        insta::assert_snapshot!(render("g-1", vec![b], vec![]));
    }

    #[test]
    fn opens_in_reading_view() {
        for block in [
            render("ganymede-juj.12", full_graph(), vec![]),
            render("g-1", vec![bead("g-1", "t")], vec![]),
        ] {
            assert!(
                block.ends_with("\ngeneratedBy: callisto\nobsidianUIMode: preview\n---\n"),
                "{block}"
            );
        }
    }

    #[test]
    fn records_prefix() {
        let block = render(
            "ganymede-mol-5ed",
            vec![bead("ganymede-mol-5ed", "t")],
            vec![],
        );
        assert!(
            block.contains("\nbead: ganymede-mol-5ed\nbeadPrefix: ganymede\n"),
            "{block}"
        );
    }

    #[test]
    fn renders_are_identical() {
        assert_eq!(
            render("ganymede-juj.12", full_graph(), vec![]),
            render("ganymede-juj.12", full_graph(), vec![])
        );
    }
}
