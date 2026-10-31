//! Relationships in a real export.

use std::fs;

use callisto::export::parse;
use callisto::graph::{Graph, Progress};

fn ganymede() -> Graph {
    let text = fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/export/ganymede.jsonl"
    ))
    .unwrap();
    Graph::new(parse(&text).unwrap())
}

#[test]
fn ganymede_epic_progress() {
    let g = ganymede();
    let kids: Vec<_> = g.children("ganymede-juj").iter().map(|r| r.id).collect();
    assert_eq!(kids.len(), 12);
    assert_eq!(
        kids[..3],
        ["ganymede-juj.1", "ganymede-juj.2", "ganymede-juj.3"]
    );
    assert_eq!(
        g.child_progress("ganymede-juj"),
        Some(Progress {
            closed: 12,
            total: 12
        })
    );
}

#[test]
fn ganymede_molecule_child() {
    let g = ganymede();
    let kids = g.children("ganymede-yl3");
    assert_eq!(kids.len(), 1);
    assert_eq!((kids[0].id, kids[0].note), ("ganymede-mol-bhp", None));
    let step = g
        .notes()
        .iter()
        .find(|b| g.parents(&b.id).iter().any(|p| p.id == "ganymede-mol-bhp"))
        .unwrap();
    assert_eq!(g.parents(&step.id)[0].note, None);
}
