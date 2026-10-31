use std::cmp::Ordering;
use std::collections::HashMap;

use crate::bead::Bead;
use crate::export::Export;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Direction {
    /// The queried bead holds the row.
    Out,
    /// Another bead's row names the queried bead.
    In,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Relation<'g> {
    pub kind: &'g str,
    pub direction: Direction,
    pub id: &'g str,
    /// `Some` only when the related bead gets a note.
    pub note: Option<&'g Bead>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    pub closed: usize,
    pub total: usize,
}

/// One repo's beads and the relationships their dependency rows name.
pub struct Graph {
    /// Beads that get notes, then scaffolding.
    beads: Vec<Bead>,
    notes: usize,
    index: HashMap<String, usize>,
    /// Target id to (holder, row) positions in `beads`.
    incoming: HashMap<String, Vec<(usize, usize)>>,
}

impl Graph {
    pub fn new(export: Export) -> Graph {
        let notes = export.beads.len();
        let beads: Vec<Bead> = export.beads.into_iter().chain(export.scaffolding).collect();
        let mut index = HashMap::new();
        let mut incoming: HashMap<String, Vec<(usize, usize)>> = HashMap::new();
        for (holder, bead) in beads.iter().enumerate() {
            index.entry(bead.id.clone()).or_insert(holder);
            for (row, dep) in bead.dependencies.iter().enumerate() {
                incoming
                    .entry(dep.depends_on_id.clone())
                    .or_default()
                    .push((holder, row));
            }
        }
        Graph {
            beads,
            notes,
            index,
            incoming,
        }
    }

    pub fn notes(&self) -> &[Bead] {
        &self.beads[..self.notes]
    }

    pub fn note(&self, id: &str) -> Option<&Bead> {
        self.index
            .get(id)
            .filter(|&&i| i < self.notes)
            .map(|&i| &self.beads[i])
    }

    pub fn parents(&self, id: &str) -> Vec<Relation<'_>> {
        self.outgoing(id, |kind| kind == "parent-child")
    }

    pub fn children(&self, id: &str) -> Vec<Relation<'_>> {
        self.incoming(id, |kind| kind == "parent-child")
    }

    pub fn child_progress(&self, id: &str) -> Option<Progress> {
        let children = self.children(id);
        if children.is_empty() {
            return None;
        }
        let closed = children
            .iter()
            .filter(|r| self.get(r.id).is_some_and(|b| b.status == "closed"))
            .count();
        Some(Progress {
            closed,
            total: children.len(),
        })
    }

    pub fn depends_on(&self, id: &str) -> Vec<Relation<'_>> {
        self.outgoing(id, |kind| kind == "blocks")
    }

    pub fn blocks(&self, id: &str) -> Vec<Relation<'_>> {
        self.incoming(id, |kind| kind == "blocks")
    }

    pub fn others(&self, id: &str) -> Vec<Relation<'_>> {
        let other = |kind: &str| kind != "parent-child" && kind != "blocks";
        let mut all = self.outgoing(id, other);
        all.extend(self.incoming(id, other));
        tidy(all)
    }

    /// Any bead in the export, scaffolding included.
    pub fn get(&self, id: &str) -> Option<&Bead> {
        self.index.get(id).map(|&i| &self.beads[i])
    }

    fn outgoing(&self, id: &str, wanted: impl Fn(&str) -> bool) -> Vec<Relation<'_>> {
        let Some(bead) = self.get(id) else {
            return Vec::new();
        };
        let relations = bead
            .dependencies
            .iter()
            .filter(|dep| wanted(&dep.kind) && dep.depends_on_id != id)
            .map(|dep| Relation {
                kind: &dep.kind,
                direction: Direction::Out,
                id: &dep.depends_on_id,
                note: self.note(&dep.depends_on_id),
            })
            .collect();
        tidy(relations)
    }

    fn incoming(&self, id: &str, wanted: impl Fn(&str) -> bool) -> Vec<Relation<'_>> {
        let rows = self.incoming.get(id).map(Vec::as_slice).unwrap_or_default();
        let relations = rows
            .iter()
            .map(|&(holder, row)| (&self.beads[holder], &self.beads[holder].dependencies[row]))
            .filter(|(holder, dep)| wanted(&dep.kind) && holder.id != id)
            .map(|(holder, dep)| Relation {
                kind: &dep.kind,
                direction: Direction::In,
                id: &holder.id,
                note: self.note(&holder.id),
            })
            .collect();
        tidy(relations)
    }
}

fn tidy(mut relations: Vec<Relation<'_>>) -> Vec<Relation<'_>> {
    relations.sort_by(|a, b| {
        a.kind
            .cmp(b.kind)
            .then(a.direction.cmp(&b.direction))
            .then_with(|| cmp_ids(a.id, b.id))
    });
    relations.dedup_by(|a, b| (a.kind, a.direction, a.id) == (b.kind, b.direction, b.id));
    relations
}

/// Natural order, so `x-1.2` sorts before `x-1.10`.
pub fn cmp_ids(a: &str, b: &str) -> Ordering {
    let (mut x, mut y) = (runs(a), runs(b));
    loop {
        match (x.next(), y.next()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(p), Some(q)) => {
                let order = if is_digits(p) && is_digits(q) {
                    let (p, q) = (p.trim_start_matches('0'), q.trim_start_matches('0'));
                    p.len().cmp(&q.len()).then_with(|| p.cmp(q))
                } else {
                    p.cmp(q)
                };
                if order != Ordering::Equal {
                    return order;
                }
            }
        }
    }
}

fn is_digits(run: &str) -> bool {
    run.as_bytes()[0].is_ascii_digit()
}

fn runs(id: &str) -> impl Iterator<Item = &str> {
    let mut rest = id;
    std::iter::from_fn(move || {
        let first = rest.chars().next()?;
        let digit = first.is_ascii_digit();
        let end = rest
            .find(|c: char| c.is_ascii_digit() != digit)
            .unwrap_or(rest.len());
        let (run, tail) = rest.split_at(end);
        rest = tail;
        Some(run)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::parse;

    fn issue(id: &str, extra: &str) -> String {
        format!(
            r#"{{"id":"{id}","title":"t","priority":2,"created_at":"2026-10-11T02:21:06Z","updated_at":"2026-10-11T02:21:06Z"{extra}}}"#
        )
    }

    fn dep(kind: &str, on: &str) -> String {
        format!(
            r#"{{"issue_id":"_","depends_on_id":"{on}","type":"{kind}","created_at":"2026-10-11T02:21:06Z"}}"#
        )
    }

    fn deps(list: &[String]) -> String {
        format!(r#","dependencies":[{}]"#, list.join(","))
    }

    fn graph(lines: &[&str]) -> Graph {
        Graph::new(parse(&lines.join("\n")).unwrap())
    }

    fn ids<'a>(relations: &[Relation<'a>]) -> Vec<&'a str> {
        relations.iter().map(|r| r.id).collect()
    }

    fn closed(extra: &str) -> String {
        format!(r#","status":"closed"{extra}"#)
    }

    #[test]
    fn parent_and_children_come_from_rows_not_ids() {
        let g = graph(&[
            &issue("x-xyz", ""),
            &issue("x-abc", &deps(&[dep("parent-child", "x-xyz")])),
            &issue("x-xyz.1", ""),
        ]);
        assert_eq!(ids(&g.children("x-xyz")), ["x-abc"]);
        assert_eq!(ids(&g.parents("x-abc")), ["x-xyz"]);
        assert!(g.parents("x-xyz.1").is_empty());
    }

    #[test]
    fn molecule_parent_has_no_note() {
        let g = graph(&[
            &issue("x-m", r#","issue_type":"molecule""#),
            &issue("x-1", &deps(&[dep("parent-child", "x-m")])),
        ]);
        let p = &g.parents("x-1")[0];
        assert_eq!((p.id, p.note), ("x-m", None));
    }

    #[test]
    fn scaffolding_child_is_listed_without_a_note() {
        let molecule = format!(
            r#","issue_type":"molecule","status":"closed"{}"#,
            deps(&[dep("parent-child", "x-1")])
        );
        let g = graph(&[&issue("x-1", ""), &issue("x-m", &molecule)]);
        let c = &g.children("x-1")[0];
        assert_eq!((c.id, c.note), ("x-m", None));
        assert_eq!(
            g.child_progress("x-1"),
            Some(Progress {
                closed: 1,
                total: 1
            })
        );
    }

    #[test]
    fn cross_repo_dependency_has_no_note() {
        let g = graph(&[&issue("x-1", &deps(&[dep("blocks", "other-9")]))]);
        let d = &g.depends_on("x-1")[0];
        assert_eq!((d.id, d.note), ("other-9", None));
    }

    #[test]
    fn blocks_reads_in_both_directions() {
        let g = graph(&[
            &issue("x-1", ""),
            &issue("x-2", &deps(&[dep("blocks", "x-1")])),
        ]);
        assert_eq!(ids(&g.depends_on("x-2")), ["x-1"]);
        assert_eq!(ids(&g.blocks("x-1")), ["x-2"]);
        assert!(g.depends_on("x-1").is_empty() && g.blocks("x-2").is_empty());
        assert_eq!(g.depends_on("x-2")[0].note.unwrap().id, "x-1");
    }

    #[test]
    fn other_kinds_read_in_both_directions() {
        let g = graph(&[
            &issue("x-1", &deps(&[dep("discovered-from", "x-2")])),
            &issue("x-2", ""),
        ]);
        let out = &g.others("x-1");
        let inn = &g.others("x-2");
        assert_eq!(
            (out[0].kind, out[0].direction, out[0].id),
            ("discovered-from", Direction::Out, "x-2")
        );
        assert_eq!(
            (inn[0].kind, inn[0].direction, inn[0].id),
            ("discovered-from", Direction::In, "x-1")
        );
        assert!(g.depends_on("x-1").is_empty() && g.parents("x-1").is_empty());
    }

    #[test]
    fn child_progress_counts_closed_children() {
        let child = deps(&[dep("parent-child", "x-e")]);
        let g = graph(&[
            &issue("x-e", ""),
            &issue("x-a", &closed(&child)),
            &issue("x-b", &child),
            &issue("x-c", &closed(&child)),
        ]);
        assert_eq!(
            g.child_progress("x-e"),
            Some(Progress {
                closed: 2,
                total: 3
            })
        );
        assert_eq!(g.child_progress("x-a"), None);
    }

    #[test]
    fn ids_sort_naturally() {
        let child = deps(&[dep("parent-child", "x-1")]);
        let g = graph(&[
            &issue("x-1", ""),
            &issue("x-1.10", &child),
            &issue("x-1.2", &child),
            &issue("x-1.1", &child),
        ]);
        assert_eq!(ids(&g.children("x-1")), ["x-1.1", "x-1.2", "x-1.10"]);
        assert_eq!(cmp_ids("x-1.2", "x-1.10"), Ordering::Less);
        assert_eq!(cmp_ids("x-a", "x-b"), Ordering::Less);
        assert_eq!(cmp_ids("x-01", "x-1"), Ordering::Less);
        assert_eq!(cmp_ids("x-1", "x-1"), Ordering::Equal);
    }

    #[test]
    fn others_sort_by_kind_then_direction_then_id() {
        let g = graph(&[
            &issue(
                "x-1",
                &deps(&[dep("related", "x-3"), dep("discovered-from", "x-2")]),
            ),
            &issue("x-2", &deps(&[dep("related", "x-1")])),
            &issue("x-3", ""),
        ]);
        let got: Vec<_> = g
            .others("x-1")
            .iter()
            .map(|r| (r.kind, r.direction, r.id))
            .collect();
        assert_eq!(
            got,
            [
                ("discovered-from", Direction::Out, "x-2"),
                ("related", Direction::Out, "x-3"),
                ("related", Direction::In, "x-2"),
            ]
        );
    }

    #[test]
    fn two_parents_are_both_reported() {
        let g = graph(&[
            &issue("x-a", ""),
            &issue("x-b", ""),
            &issue(
                "x-c",
                &deps(&[dep("parent-child", "x-b"), dep("parent-child", "x-a")]),
            ),
        ]);
        assert_eq!(ids(&g.parents("x-c")), ["x-a", "x-b"]);
    }

    #[test]
    fn duplicate_and_self_edges_are_dropped() {
        let g = graph(&[
            &issue("x-1", ""),
            &issue(
                "x-2",
                &deps(&[
                    dep("blocks", "x-1"),
                    dep("blocks", "x-1"),
                    dep("blocks", "x-2"),
                ]),
            ),
        ]);
        assert_eq!(ids(&g.depends_on("x-2")), ["x-1"]);
        assert_eq!(ids(&g.blocks("x-1")), ["x-2"]);
        assert!(g.blocks("x-2").is_empty());
    }
}
