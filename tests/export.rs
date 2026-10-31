//! Parsing against real `bd export` output.

use std::fs;

use callisto::export::parse;

fn fixture(name: &str) -> String {
    fs::read_to_string(format!(
        "{}/tests/fixtures/export/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

#[test]
fn ganymede_export_parses() {
    let export = parse(&fixture("ganymede.jsonl")).unwrap();
    assert_eq!(export.scaffolding.len(), 9);
    let beads = export.beads;
    // Non-molecule lines in the captured file; recount if recaptured.
    assert_eq!(beads.len(), 166);
    assert!(beads.iter().all(|b| b.issue_type != "molecule"));
    let b = beads.iter().find(|b| b.id == "ganymede-juj.12").unwrap();
    assert_eq!(b.title, "Bead ganymede-juj.12");
    assert!(
        b.dependencies
            .iter()
            .any(|d| d.kind == "parent-child" && d.depends_on_id == "ganymede-juj")
    );
}

#[test]
fn synthetic_export_keeps_what_callisto_renders() {
    let export = parse(&fixture("synthetic.jsonl")).unwrap();
    // The molecule and the wisp.
    assert_eq!(export.scaffolding.len(), 2);
    let beads = export.beads;
    let titles: Vec<_> = beads.iter().map(|b| b.title.as_str()).collect();
    // The molecule and the wisp are scaffolding.
    assert_eq!(beads.len(), 4, "{titles:?}");
    let a = beads.iter().find(|b| b.priority == 0).unwrap();
    assert_eq!(a.comments.len(), 2);
    assert_eq!(a.labels, ["alpha", "beta"]);
    assert!(beads.iter().any(|b| b.status == "review"));
    assert!(beads.iter().any(|b| b.issue_type == "incident"));
    assert!(
        beads
            .iter()
            .any(|b| b.closed_at.is_some() && b.close_reason.is_some())
    );
}
