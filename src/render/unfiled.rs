//! The index note for beads no table entry claims.

use crate::claim::{Reason, Unclaimed};
use crate::field_path::FieldPath;
use crate::filename::ILLEGAL;
use crate::render::escape::label;
use crate::render::yaml::scalar;
use crate::standing::natural_cmp;
use crate::vault::{MARKER_KEY, MARKER_VALUE};

pub fn file_name(prefix: &str) -> String {
    let safe: String = prefix
        .chars()
        .map(|c| if ILLEGAL.contains(&c) { '-' } else { c })
        .collect();
    format!("Unfiled beads - {safe}.md")
}

pub fn index(prefix: &str, field: &FieldPath, unclaimed: &[Unclaimed]) -> String {
    let mut out = format!(
        "---\n{MARKER_KEY}: {MARKER_VALUE}\nbeadPrefix: {}\nobsidianUIMode: preview\n---\n\n\
         > [!info] Beads no repo claims · {}\n\
         > callisto rewrites this note on every sync. Edits are lost.\n\n",
        scalar(prefix),
        label(prefix),
    );
    if unclaimed.is_empty() {
        out.push_str("No unclaimed beads.\n");
        return out;
    }
    let mut rows: Vec<&Unclaimed> = unclaimed.iter().collect();
    rows.sort_by(|a, b| {
        (a.status == "closed")
            .cmp(&(b.status == "closed"))
            .then_with(|| natural_cmp(&a.id, &b.id))
    });
    out.push_str("| Bead | Title | Status | Reason |\n| --- | --- | --- | --- |\n");
    for u in rows {
        out.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            label(&u.id),
            label(&u.title),
            label(&u.status),
            label(&reason(field, &u.reason)),
        ));
    }
    out
}

fn reason(field: &FieldPath, reason: &Reason) -> String {
    match reason {
        Reason::NoField => format!("no {field}"),
        Reason::Value(v) => format!("{field} is \"{v}\""),
        Reason::Json(j) => format!("{field} is {j}"),
        Reason::MissingProject(p) => format!("project \"{p}\" is not in the vault"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fields;

    fn u(id: &str, status: &str, reason: Reason) -> Unclaimed {
        Unclaimed {
            id: id.into(),
            title: format!("title {id}"),
            status: status.into(),
            reason,
        }
    }

    fn field() -> FieldPath {
        "metadata.repo".parse().unwrap()
    }

    fn five() -> Vec<Unclaimed> {
        vec![
            u("zz-10", "closed", Reason::Value("o/z".into())),
            u("zz-2", "open", Reason::NoField),
            u("zz-9", "in_progress", Reason::Json("7".into())),
            u("zz-1", "closed", Reason::MissingProject("Gone".into())),
            u("zz-3", "blocked", Reason::Value("o/y".into())),
        ]
    }

    #[test]
    fn file_name_maps_illegal() {
        assert_eq!(file_name("zz"), "Unfiled beads - zz.md");
        assert_eq!(file_name("a/b"), "Unfiled beads - a-b.md");
    }

    #[test]
    fn empty() {
        insta::assert_snapshot!(index("zz", &field(), &[]));
    }

    #[test]
    fn every_reason() {
        insta::assert_snapshot!(index("zz", &field(), &five()));
    }

    #[test]
    fn hostile() {
        let mut b = u("zz-1", "open", Reason::NoField);
        b.title = "a | b [[c]] `d`\nnext".into();
        insta::assert_snapshot!(index("zz", &field(), &[b]));
    }

    #[test]
    fn order() {
        let text = index("zz", &field(), &five());
        let at = |id: &str| text.find(&format!("| {id} |")).unwrap();
        let positions: Vec<usize> = ["zz-2", "zz-3", "zz-9", "zz-1", "zz-10"]
            .iter()
            .map(|id| at(id))
            .collect();
        assert!(positions.is_sorted());
    }

    #[test]
    fn no_bead_field() {
        let read = fields::read(&index("zz", &field(), &five()));
        assert!(read.beads.is_empty());
        assert_eq!(read.generated_by.as_deref(), Some("callisto"));
        assert_eq!(read.bead_prefix.as_deref(), Some("zz"));
    }
}
