//! What a wikilink to a bead's note names. Links never carry a path:
//! generated names end in the bead id, so they are unique unless the
//! notes are duplicates, and a path would break when the user moves one.

use std::collections::HashSet;

use crate::bead::Bead;
use crate::filename;
use crate::vault::{Generated, Index};

pub struct Links<'i> {
    index: &'i Index,
    /// Beads whose computed filename belongs to a file callisto did not
    /// write, so a link by that name would reach the wrong note.
    taken: HashSet<String>,
}

impl<'i> Links<'i> {
    pub fn new(index: &'i Index, taken: HashSet<String>) -> Links<'i> {
        Links { index, taken }
    }

    /// The name a wikilink to the bead's note uses, or `None` when no
    /// link can reach that note. An existing note keeps its
    /// creation-time name through retitles, so only a bead without one
    /// gets a name computed from its title.
    pub fn target(&self, bead: &Bead) -> Option<String> {
        let name = match self.index.generated(&bead.id) {
            Generated::One(note) => note.name().to_string(),
            Generated::Duplicate(notes) => notes[0].name().to_string(),
            Generated::Missing if self.taken.contains(&bead.id) => return None,
            Generated::Missing => filename::stem(&bead.title, &bead.id),
        };
        linkable(&name).then_some(name)
    }

    /// Hand-written task notes citing `id`, sorted by path.
    pub fn task_notes(&self, id: &str) -> Vec<String> {
        self.index
            .task_notes(id)
            .iter()
            .map(|n| n.name().to_string())
            .collect()
    }
}

/// Whether a wikilink can name a note called `name`, read against
/// Obsidian 1.14.4. Its parser takes `[[` up to the first `]]`, cuts
/// an alias at the first `|` and a subpath at the first `#`, then
/// trims the target, turns NBSP into a space and drops a trailing
/// `\`. A `^` only means a block after `#`, so it and lone brackets
/// are safe. No markdown link reaches a `#` either: Obsidian decodes
/// its target with `decodeURI`, which leaves `%23` encoded.
pub fn linkable(name: &str) -> bool {
    !name.is_empty()
        && !name.contains(['#', '|', '\u{a0}'])
        && !name.contains("[[")
        && !name.contains("]]")
        && !name.ends_with([']', '\\'])
        && name.trim() == name
        && !name.contains(char::is_control)
}

pub fn wikilink(name: &str) -> String {
    format!("[[{name}]]")
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::fs;
    use std::path::Path;

    use super::*;
    use crate::bead::test_support::bead;

    fn put(root: &Path, rel: &str, text: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    const MARKED: &str = "---\nbead: a-1\ngeneratedBy: callisto\n---\n";

    fn links(index: &Index) -> Links<'_> {
        Links::new(index, HashSet::new())
    }

    #[test]
    fn missing_note_uses_the_computed_stem() {
        let dir = tempfile::tempdir().unwrap();
        let index = Index::scan(dir.path()).unwrap();
        assert_eq!(
            links(&index).target(&bead("a-1", "x/y")).as_deref(),
            Some("x-y - a-1")
        );
    }

    #[test]
    fn existing_note_name_wins() {
        let dir = tempfile::tempdir().unwrap();
        put(dir.path(), "Archives/old title - a-1.md", MARKED);
        let index = Index::scan(dir.path()).unwrap();
        assert_eq!(
            links(&index).target(&bead("a-1", "new title")).as_deref(),
            Some("old title - a-1")
        );
    }

    #[test]
    fn unmarked_note_is_not_a_link_target() {
        let dir = tempfile::tempdir().unwrap();
        put(dir.path(), "hand.md", "---\nbead: a-1\n---\n");
        let index = Index::scan(dir.path()).unwrap();
        assert_eq!(
            links(&index).target(&bead("a-1", "t")).as_deref(),
            Some("t - a-1")
        );
    }

    #[test]
    fn duplicate_links_to_the_first_by_path() {
        let dir = tempfile::tempdir().unwrap();
        put(dir.path(), "B/n - a-1.md", MARKED);
        put(dir.path(), "A/m - a-1.md", MARKED);
        let index = Index::scan(dir.path()).unwrap();
        assert_eq!(
            links(&index).target(&bead("a-1", "t")).as_deref(),
            Some("m - a-1")
        );
    }

    #[test]
    fn taken_bead_has_no_target() {
        let dir = tempfile::tempdir().unwrap();
        let index = Index::scan(dir.path()).unwrap();
        let links = Links::new(&index, HashSet::from(["a-1".to_string()]));
        assert_eq!(links.target(&bead("a-1", "t")), None);
        assert_eq!(links.target(&bead("a-2", "t")).as_deref(), Some("t - a-2"));
    }

    #[test]
    fn renamed_note_with_a_breaking_name_has_no_target() {
        for name in ["x#y - a-1", "x|y - a-1", "x]]y - a-1", "a-1 x]"] {
            let dir = tempfile::tempdir().unwrap();
            put(dir.path(), &format!("{name}.md"), MARKED);
            let index = Index::scan(dir.path()).unwrap();
            assert_eq!(links(&index).target(&bead("a-1", "t")), None, "{name}");
        }
    }

    #[test]
    fn renamed_note_with_brackets_or_a_caret_keeps_its_link() {
        for name in ["fix [WIP] parser - a-1", "x^y - a-1"] {
            let dir = tempfile::tempdir().unwrap();
            put(dir.path(), &format!("{name}.md"), MARKED);
            let index = Index::scan(dir.path()).unwrap();
            assert_eq!(
                links(&index).target(&bead("a-1", "t")).as_deref(),
                Some(name)
            );
        }
    }

    #[test]
    fn breaking_names_are_not_linkable() {
        for name in [
            "", "a#b", "#a", "a|b", "a|", "a[[b", "[[a", "a]]b", "a]]", "a]", "a\nb", "a\u{7f}b",
            " a", "a ", "a\\", "a\u{a0}b",
        ] {
            assert!(!linkable(name), "{name:?}");
        }
    }

    #[test]
    fn names_a_wikilink_reaches_are_linkable() {
        for name in [
            "plain name - a-1",
            "café (x) - a-1",
            "a^b",
            "^a",
            "a[b",
            "[a",
            "a]b",
            "[a]b",
            "fix [WIP] parser",
            "a\\b",
        ] {
            assert!(linkable(name), "{name:?}");
        }
    }

    #[test]
    fn wikilink_wraps() {
        assert_eq!(wikilink("n - a-1"), "[[n - a-1]]");
    }
}
