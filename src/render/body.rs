//! The note below the frontmatter, in Layout B: relationships first,
//! then the bead's prose.

use jiff::Timestamp;

use crate::bead::Bead;
use crate::graph::{Direction, Relation, cmp_ids};
use crate::render::escape::{label, markdown};
use crate::render::frontmatter::Context;
use crate::render::link::{linkable, wikilink};

pub fn body(bead: &Bead, cx: &Context) -> String {
    let mut out = callout(bead, cx);
    let fields = [
        ("Description", &bead.description),
        ("Design", &bead.design),
        ("Acceptance criteria", &bead.acceptance_criteria),
        ("Notes", &bead.notes),
    ];
    let mut sections: Vec<(&str, String)> = fields
        .into_iter()
        .filter_map(|(name, text)| Some((name, prose(text.as_deref()?)?)))
        .collect();
    if let Some(comments) = comments(bead, cx) {
        sections.push(("Comments", comments));
    }
    for (name, text) in sections {
        out.push_str(&format!("\n## {name}\n\n{text}\n"));
    }
    out
}

/// `None` when nothing but whitespace is left.
fn prose(text: &str) -> Option<String> {
    let escaped = markdown(text);
    let trimmed = escaped.trim_end();
    let first = trimmed.find(|c: char| !c.is_whitespace())?;
    // Keep the first line's indent: it may be what makes it code.
    let start = trimmed[..first].rfind('\n').map_or(0, |i| i + 1);
    Some(trimmed[start..].to_string())
}

fn comments(bead: &Bead, cx: &Context) -> Option<String> {
    let mut sorted: Vec<_> = bead.comments.iter().collect();
    sorted.sort_by_key(|c| c.created_at);
    let rendered: Vec<String> = sorted
        .into_iter()
        .map(|c| {
            let author = label(&c.author);
            let at = stamp(c.created_at, cx);
            let head = if author.is_empty() {
                at
            } else {
                format!("**{author}** · {at}")
            };
            match prose(&c.text) {
                Some(text) => format!("{head}\n\n{text}"),
                None => head,
            }
        })
        .collect();
    (!rendered.is_empty()).then(|| rendered.join("\n\n"))
}

pub fn glyph(status: &str) -> &'static str {
    match status {
        "in_progress" => "◐",
        "blocked" => "●",
        "closed" => "✓",
        "deferred" => "◌",
        _ => "○",
    }
}

pub(crate) fn callout(bead: &Bead, cx: &Context) -> String {
    let mut out = format!(
        "> [!info] {} · {} · P{} · {}\n",
        label(&bead.id),
        label(&bead.issue_type),
        bead.priority,
        label(&bead.status)
    );
    let rows = rows(bead, cx);
    // A Markdown table needs a header row; an empty one keeps the names
    // in the first column.
    if !rows.is_empty() {
        out.push_str("> | | |\n> | --- | --- |\n");
    }
    for (name, cell) in rows {
        out.push_str(&format!("> | {name} | {} |\n", cell.join("<br>")));
    }
    out
}

fn rows(bead: &Bead, cx: &Context) -> Vec<(String, Vec<String>)> {
    let g = cx.graph;
    let id = bead.id.as_str();
    let items = |mut rels: Vec<Relation>| -> Vec<String> {
        rels.sort_by(|a, b| order(a, cx).cmp(&order(b, cx)).then(cmp_ids(a.id, b.id)));
        rels.iter().map(|r| item(r, cx)).collect()
    };
    let mut rows = vec![
        ("Parent".to_string(), items(g.parents(id))),
        ("Depends on".to_string(), items(g.depends_on(id))),
        ("Blocks".to_string(), items(g.blocks(id))),
    ];

    let mut children = items(g.children(id));
    if bead.issue_type == "epic"
        && let Some(p) = g.child_progress(id)
    {
        children.insert(0, format!("{} / {} closed", p.closed, p.total));
    }
    rows.push(("Children".to_string(), children));

    for (name, rels) in others(g.others(id)) {
        rows.push((name, items(rels)));
    }

    let text = |value: &Option<String>| -> Vec<String> {
        value
            .as_deref()
            .filter(|v| !v.trim().is_empty())
            .map(label)
            .into_iter()
            .collect()
    };
    rows.push(("Assignee".to_string(), text(&bead.assignee)));
    rows.push(("Owner".to_string(), text(&bead.owner)));
    rows.push(("Created by".to_string(), text(&bead.created_by)));
    let started = bead
        .started_at
        .map(|at| stamp(at, cx))
        .into_iter()
        .collect();
    rows.push(("Started".to_string(), started));
    if bead.status == "closed" {
        rows.push(("Close reason".to_string(), text(&bead.close_reason)));
    }
    let notes = cx
        .links
        .task_notes(id)
        .iter()
        .map(|n| if linkable(n) { wikilink(n) } else { label(n) })
        .collect();
    rows.push(("Task note".to_string(), notes));

    rows.retain(|(_, cell)| !cell.is_empty());
    rows
}

/// Open work first, so a long row reads from what is left to do. Each
/// status sorts with the TaskNotes status it maps to. A bead outside the
/// export has no status and goes last.
fn order(r: &Relation, cx: &Context) -> (u8, i64) {
    let Some(b) = cx.graph.get(r.id) else {
        return (4, 0);
    };
    let group = match b.status.as_str() {
        "in_progress" | "hooked" => 0,
        "blocked" | "deferred" => 2,
        "closed" => 3,
        _ => 1,
    };
    (group, b.priority)
}

/// TaskNotes draws its own status widget beside a link to a task note,
/// so a glyph there would show the status twice. A bead outside the
/// export (another repo, a deleted one) has no status to show.
fn item(r: &Relation, cx: &Context) -> String {
    let target = r.note.and_then(|b| cx.links.target(b));
    match (target, cx.graph.get(r.id)) {
        (Some(name), _) => wikilink(&name),
        (None, Some(b)) => format!("{} {}", glyph(&b.status), label(r.id)),
        (None, None) => label(r.id),
    }
}

/// Groups by row name, keeping the graph's kind-then-direction order.
/// Symmetric kinds share a name across directions and kinds, so a bead
/// can arrive twice.
fn others(rels: Vec<Relation<'_>>) -> Vec<(String, Vec<Relation<'_>>)> {
    let mut rows: Vec<(String, Vec<Relation>)> = Vec::new();
    for r in rels {
        let name = row_name(r.kind, r.direction);
        match rows.iter_mut().find(|(n, _)| *n == name) {
            Some((_, group)) => group.push(r),
            None => rows.push((name, vec![r])),
        }
    }
    for (_, group) in &mut rows {
        group.sort_by(|a, b| cmp_ids(a.id, b.id));
        group.dedup_by(|a, b| a.id == b.id);
    }
    rows
}

/// A row `X -> Y` of `kind` reads from X's side as Out.
fn row_name(kind: &str, direction: Direction) -> String {
    let out = direction == Direction::Out;
    let known = match kind {
        "discovered-from" => Some(if out { "Discovered from" } else { "Discovered" }),
        // bd prints "X is superseded by Y" for X's row.
        "supersedes" => Some(if out { "Superseded by" } else { "Supersedes" }),
        "duplicates" => Some(if out { "Duplicate of" } else { "Duplicated by" }),
        "related" | "relates-to" => Some("Related"),
        "tracks" => Some(if out { "Tracks" } else { "Tracked by" }),
        "caused-by" => Some(if out { "Caused by" } else { "Caused" }),
        "validates" => Some(if out { "Validates" } else { "Validated by" }),
        "waits-for" => Some(if out { "Waits for" } else { "Waited on by" }),
        _ => None,
    };
    if let Some(name) = known {
        return name.to_string();
    }
    let spaced = kind.replace('-', " ");
    let mut chars = spaced.chars();
    let human: String = chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default();
    let name = label(&human);
    if out {
        name
    } else {
        format!("{name} (reverse)")
    }
}

fn stamp(at: Timestamp, cx: &Context) -> String {
    at.to_zoned(cx.tz.clone())
        .strftime("%Y-%m-%d %H:%M")
        .to_string()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use jiff::tz::{Offset, TimeZone};

    use super::*;
    use crate::bead::test_support::bead;
    use crate::bead::{Bead, Comment, Dependency};
    use crate::export::Export;
    use crate::graph::Graph;
    use crate::render::frontmatter::Context;
    use crate::render::link::Links;
    use crate::vault::Index;

    fn dep(kind: &str, on: &str) -> Dependency {
        Dependency {
            depends_on_id: on.into(),
            kind: kind.into(),
        }
    }

    fn with(id: &str, deps: &[(&str, &str)]) -> Bead {
        let mut b = bead(id, "t");
        b.dependencies = deps.iter().map(|(k, on)| dep(k, on)).collect();
        b
    }

    fn molecule(id: &str, status: &str) -> Bead {
        let mut b = bead(id, "molecule");
        b.issue_type = "molecule".into();
        b.status = status.into();
        b
    }

    /// Renders `id` with `f` against a vault holding `files`.
    pub(super) fn render_with(
        f: fn(&Bead, &Context) -> String,
        id: &str,
        beads: Vec<Bead>,
        scaffolding: Vec<Bead>,
        files: &[(&str, &str)],
    ) -> String {
        let dir = tempfile::tempdir().unwrap();
        for (rel, text) in files {
            let path = dir.path().join(rel);
            fs::create_dir_all(path.parent().unwrap_or(Path::new(""))).unwrap();
            fs::write(path, text).unwrap();
        }
        let index = Index::scan(dir.path()).unwrap();
        let links = Links::new(&index, Default::default());
        let graph = Graph::new(Export { beads, scaffolding });
        let tz = TimeZone::fixed(Offset::constant(-4));
        let cx = Context {
            graph: &graph,
            links: &links,
            project: "ganymede",
            prefix: id.split('-').next().unwrap(),
            tz: &tz,
        };
        f(graph.note(id).unwrap(), &cx)
    }

    fn callout_of(id: &str, beads: Vec<Bead>, scaffolding: Vec<Bead>) -> String {
        render_with(callout, id, beads, scaffolding, &[])
    }

    fn row<'a>(callout: &'a str, name: &str) -> Option<&'a str> {
        callout
            .lines()
            .find(|l| l.starts_with(&format!("> | {name} | ")))
    }

    #[test]
    fn glyphs() {
        let got: Vec<&str> = [
            "open",
            "in_progress",
            "blocked",
            "closed",
            "deferred",
            "pinned",
            "hooked",
            "review",
        ]
        .into_iter()
        .map(glyph)
        .collect();
        assert_eq!(got, ["○", "◐", "●", "✓", "◌", "○", "○", "○"]);
    }

    #[test]
    fn header_only_without_rows() {
        assert_eq!(
            callout_of("g-1", vec![bead("g-1", "t")], vec![]),
            "> [!info] g-1 · task · P1 · open\n"
        );
    }

    #[test]
    fn header_escapes_type_and_status() {
        let mut b = bead("g-1", "t");
        b.status = "in_progress".into();
        b.issue_type = "a|b".into();
        assert_eq!(
            callout_of("g-1", vec![b], vec![]),
            "> [!info] g-1 · a\\|b · P1 · in\\_progress\n"
        );
    }

    #[test]
    fn plain_ids_for_beads_without_notes() {
        let b = with("g-1", &[("parent-child", "g-mol-1"), ("blocks", "other-9")]);
        let out = callout_of("g-1", vec![b], vec![molecule("g-mol-1", "closed")]);
        assert_eq!(row(&out, "Parent"), Some("> | Parent | ✓ g-mol-1 |"));
        assert_eq!(row(&out, "Depends on"), Some("> | Depends on | other-9 |"));
    }

    #[test]
    fn linked_rows_carry_no_glyph() {
        let mut blocker = bead("g-2", "first");
        blocker.status = "closed".into();
        let mut blocked = with("g-3", &[("blocks", "g-1")]);
        blocked.status = "blocked".into();
        let b = with("g-1", &[("blocks", "g-2"), ("parent-child", "g-e")]);
        let out = callout_of(
            "g-1",
            vec![b, blocker, blocked, bead("g-e", "epic")],
            vec![],
        );
        assert_eq!(
            out,
            "> [!info] g-1 · task · P1 · open\n\
             > | | |\n\
             > | --- | --- |\n\
             > | Parent | [[epic - g-e]] |\n\
             > | Depends on | [[first - g-2]] |\n\
             > | Blocks | [[t - g-3]] |\n"
        );
    }

    #[test]
    fn related_both_ways_lists_once() {
        let out = callout_of(
            "g-1",
            vec![
                with("g-1", &[("related", "g-2"), ("relates-to", "g-3")]),
                with("g-2", &[("related", "g-1")]),
                bead("g-3", "t"),
            ],
            vec![],
        );
        assert_eq!(
            row(&out, "Related"),
            Some("> | Related | [[t - g-2]]<br>[[t - g-3]] |")
        );
        assert_eq!(out.matches("Related").count(), 1);
    }

    #[test]
    fn other_kinds_are_labelled() {
        let out = callout_of(
            "g-1",
            vec![
                with(
                    "g-1",
                    &[
                        ("discovered-from", "g-2"),
                        ("supersedes", "g-3"),
                        ("until", "g-4"),
                    ],
                ),
                with("g-2", &[]),
                bead("g-3", "t"),
                bead("g-4", "t"),
                with(
                    "g-5",
                    &[
                        ("discovered-from", "g-1"),
                        ("supersedes", "g-1"),
                        ("until", "g-1"),
                    ],
                ),
            ],
            vec![],
        );
        let names: Vec<&str> = out
            .lines()
            .skip(3)
            .map(|l| l.split(" | ").nth(1).unwrap())
            .collect();
        assert_eq!(
            names,
            [
                "Discovered from",
                "Discovered",
                "Superseded by",
                "Supersedes",
                "Until",
                "Until (reverse)"
            ]
        );
        assert_eq!(
            row(&out, "Supersedes"),
            Some("> | Supersedes | [[t - g-5]] |")
        );
    }

    #[test]
    fn rows_put_open_work_first() {
        let child = |id: &str, status: &str, priority: i64| {
            let mut b = with(id, &[("parent-child", "g-e")]);
            b.status = status.into();
            b.priority = priority;
            b
        };
        let mut epic = with("g-e", &[("related", "other-9"), ("related", "g-1")]);
        epic.issue_type = "epic".into();
        let mut mol = molecule("g-mol-1", "pinned");
        mol.priority = 0;
        mol.dependencies = vec![dep("parent-child", "g-e")];
        let out = callout_of(
            "g-e",
            vec![
                epic,
                child("g-1", "closed", 0),
                child("g-2", "blocked", 1),
                child("g-3", "open", 2),
                child("g-4", "open", 1),
                child("g-5", "in_progress", 3),
                child("g-6", "deferred", 1),
                child("g-7", "hooked", 2),
            ],
            vec![mol],
        );
        assert_eq!(
            row(&out, "Children"),
            Some(
                "> | Children | 1 / 8 closed<br>[[t - g-7]]<br>[[t - g-5]]<br>○ g-mol-1<br>\
                 [[t - g-4]]<br>[[t - g-3]]<br>[[t - g-2]]<br>[[t - g-6]]<br>[[t - g-1]] |"
            )
        );
        assert_eq!(
            row(&out, "Related"),
            Some("> | Related | [[t - g-1]]<br>other-9 |")
        );
    }

    #[test]
    fn children_count_only_for_epics() {
        let child = [("parent-child", "g-e")];
        let mut a = with("g-a", &child);
        a.status = "closed".into();
        let mut c = with("g-c", &child);
        c.status = "closed".into();
        let mut epic = bead("g-e", "epic");
        epic.issue_type = "epic".into();
        let out = callout_of(
            "g-e",
            vec![epic.clone(), a.clone(), with("g-b", &child), c.clone()],
            vec![],
        );
        assert_eq!(
            row(&out, "Children"),
            Some("> | Children | 2 / 3 closed<br>[[t - g-b]]<br>[[t - g-a]]<br>[[t - g-c]] |")
        );
        epic.issue_type = "task".into();
        let out = callout_of("g-e", vec![epic, a], vec![]);
        assert_eq!(row(&out, "Children"), Some("> | Children | [[t - g-a]] |"));
    }

    #[test]
    fn cell_values_cannot_break_the_table() {
        let mut b = bead("g-1", "t");
        b.assignee = Some("a|b\n[[x]]".into());
        b.owner = Some("  ".into());
        b.close_reason = Some("not closed yet".into());
        let out = callout_of("g-1", vec![b], vec![]);
        assert_eq!(
            row(&out, "Assignee"),
            Some(r"> | Assignee | a\|b \[\[x\]\] |")
        );
        assert_eq!(row(&out, "Owner"), None);
        assert_eq!(row(&out, "Close reason"), None);
        assert_eq!(out.lines().count(), 4);
    }

    #[test]
    fn people_times_and_close_reason() {
        let mut b = bead("g-1", "t");
        b.status = "closed".into();
        b.owner = Some("ryan@example.com".into());
        b.created_by = Some("ryane".into());
        b.started_at = Some("2026-10-11T02:30:00Z".parse().unwrap());
        b.close_reason = Some("done:\nshipped".into());
        let out = callout_of("g-1", vec![b], vec![]);
        assert_eq!(
            out,
            "> [!info] g-1 · task · P1 · closed\n\
             > | | |\n\
             > | --- | --- |\n\
             > | Owner | ryan@example.com |\n\
             > | Created by | ryane |\n\
             > | Started | 2026-10-10 22:30 |\n\
             > | Close reason | done: shipped |\n"
        );
    }

    #[test]
    fn task_note_row() {
        let task = "---\ntype: task\nbead: g-1\n---\n";
        let out = render_with(
            callout,
            "g-1",
            vec![bead("g-1", "t")],
            vec![],
            &[
                ("P/b.md", task),
                ("P/a.md", task),
                ("P/plan.md", "---\nbead: g-1\n---\n"),
            ],
        );
        assert_eq!(
            row(&out, "Task note"),
            Some("> | Task note | [[a]]<br>[[b]] |")
        );
    }

    #[test]
    fn task_notes_named_past_a_link_render_as_text() {
        let task = "---\ntype: task\nbead: g-1\n---\n";
        let out = render_with(
            callout,
            "g-1",
            vec![bead("g-1", "t")],
            vec![],
            &[
                ("P/x|y.md", task),
                ("P/x^y.md", task),
                ("P/x]y.md", task),
                ("P/x[y.md", task),
                ("P/x#y.md", task),
                ("P/ok.md", task),
            ],
        );
        assert_eq!(
            row(&out, "Task note"),
            Some(r"> | Task note | [[ok]]<br>x\#y<br>[[x[y]]<br>[[x]y]]<br>[[x^y]]<br>x\|y |")
        );
    }

    #[test]
    fn note_renamed_past_a_link_shows_its_id() {
        let marked = "---\nbead: g-2\ngeneratedBy: callisto\n---\n";
        let out = render_with(
            callout,
            "g-1",
            vec![with("g-1", &[("blocks", "g-2")]), bead("g-2", "t")],
            vec![],
            &[("P/x#y - g-2.md", marked)],
        );
        assert_eq!(row(&out, "Depends on"), Some("> | Depends on | ○ g-2 |"));
    }

    fn comment(author: &str, at: &str, text: &str) -> Comment {
        Comment {
            author: author.into(),
            text: text.into(),
            created_at: at.parse().unwrap(),
        }
    }

    fn note_of(
        id: &str,
        beads: Vec<Bead>,
        scaffolding: Vec<Bead>,
        files: &[(&str, &str)],
    ) -> String {
        render_with(crate::render::note, id, beads, scaffolding, files)
    }

    #[test]
    fn epic_with_children() {
        let child = [("parent-child", "g-e")];
        let mut epic = bead("g-e", "build the thing");
        epic.issue_type = "epic".into();
        epic.description = Some("Ship it in pieces.".into());
        let mut done = with("g-e.1", &child);
        done.title = "first piece".into();
        done.status = "closed".into();
        let mut open = with("g-e.2", &child);
        open.title = "second piece".into();
        let mut mol = molecule("g-mol-1", "closed");
        mol.dependencies = vec![dep("parent-child", "g-e")];
        insta::assert_snapshot!(note_of("g-e", vec![epic, done, open], vec![mol], &[]));
    }

    fn every_row_beads() -> Vec<Bead> {
        let mut b = with(
            "g-1",
            &[
                ("parent-child", "g-e"),
                ("blocks", "g-2"),
                ("discovered-from", "g-4"),
                ("supersedes", "g-6"),
            ],
        );
        b.title = "the bead".into();
        b.status = "closed".into();
        b.closed_at = Some("2026-10-12T15:00:00Z".parse().unwrap());
        b.assignee = Some("ryane".into());
        b.owner = Some("ryan@example.com".into());
        b.created_by = Some("Ryan".into());
        b.started_at = Some("2026-10-11T03:00:00Z".parse().unwrap());
        b.close_reason = Some("Shipped in 0320dca.".into());
        b.description = Some("What to do.\n\n- one\n- two".into());
        b.design = Some("# Plan\n\n## Steps\n\n```rust\nfn main() {}\n```".into());
        b.acceptance_criteria = Some("Tests pass.".into());
        b.notes = Some("Found `x` on the way.".into());
        let mut blocker = bead("g-2", "blocker");
        blocker.status = "closed".into();
        let blocked = with("g-3", &[("blocks", "g-1")]);
        let child = with("g-1.1", &[("parent-child", "g-1")]);
        let found = with("g-5", &[("discovered-from", "g-1")]);
        vec![
            b,
            bead("g-e", "the epic"),
            blocker,
            blocked,
            child,
            bead("g-4", "origin"),
            found,
            bead("g-6", "older"),
        ]
    }

    const TASK_NOTE: (&str, &str) = ("P/the bead.md", "---\ntype: task\nbead: g-1\n---\n");

    #[test]
    fn every_row() {
        insta::assert_snapshot!(note_of("g-1", every_row_beads(), vec![], &[TASK_NOTE]));
    }

    #[test]
    fn no_rows() {
        insta::assert_snapshot!(note_of("g-1", vec![bead("g-1", "bare")], vec![], &[]));
    }

    #[test]
    fn comments() {
        let mut b = bead("g-1", "t");
        b.comments = vec![
            comment(
                "bob",
                "2026-10-12T10:00:00Z",
                "Second, with #tag\nand a second line.",
            ),
            comment("al|ce", "2026-10-11T10:00:00Z", "First."),
            comment("carol", "2026-10-13T10:00:00Z", "  \n"),
        ];
        insta::assert_snapshot!(note_of("g-1", vec![b], vec![], &[]));
    }

    #[test]
    fn blank_comment_author_shows_the_stamp_alone() {
        let mut b = bead("g-1", "t");
        b.comments = vec![comment(" \t", "2026-10-11T10:00:00Z", "Hi.")];
        let note = note_of("g-1", vec![b], vec![], &[]);
        assert!(
            note.contains("## Comments\n\n2026-10-11 06:00\n\nHi."),
            "{note}"
        );
    }

    const HOSTILE: &str = "# Heading\nsee #tag and %%hidden%%\n<div>raw</div>\na\n---\n> [!warning] fake\n[[link]]\n```sh\nunclosed <x>";

    #[test]
    fn hostile() {
        let mut b = bead("g-1", "t");
        b.description = Some(HOSTILE.into());
        b.design = Some(HOSTILE.into());
        b.acceptance_criteria = Some(HOSTILE.into());
        b.notes = Some(HOSTILE.into());
        b.comments = vec![comment("x", "2026-10-11T10:00:00Z", HOSTILE)];
        insta::assert_snapshot!(note_of("g-1", vec![b], vec![], &[]));
    }

    #[test]
    fn blank_sections_are_omitted() {
        let mut b = bead("g-1", "t");
        b.description = Some("  \n".into());
        b.notes = Some("\n\nkept\n\n".into());
        let out = render_with(body, "g-1", vec![b], vec![], &[]);
        assert!(!out.contains("## Description"), "{out}");
        assert!(out.ends_with("\n\n## Notes\n\nkept\n"), "{out}");
    }

    #[test]
    fn note_ends_with_one_newline() {
        let out = note_of("g-1", every_row_beads(), vec![], &[TASK_NOTE]);
        assert!(out.ends_with('\n') && !out.ends_with("\n\n"));
        let bare = note_of("g-1", vec![bead("g-1", "t")], vec![], &[]);
        assert!(bare.ends_with("· open\n"), "{bare}");
    }

    #[test]
    fn blank_line_before_callout() {
        for out in [
            note_of("g-1", vec![bead("g-1", "t")], vec![], &[]),
            note_of("g-1", every_row_beads(), vec![], &[TASK_NOTE]),
        ] {
            assert_eq!(out.matches("\n---\n\n> [!info] ").count(), 1, "{out}");
            assert!(!out.contains("---\n\n\n"), "{out}");
        }
    }

    #[test]
    fn renders_are_identical() {
        assert_eq!(
            note_of("g-1", every_row_beads(), vec![], &[TASK_NOTE]),
            note_of("g-1", every_row_beads(), vec![], &[TASK_NOTE])
        );
    }
}
