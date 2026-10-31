//! A new note's filename. It is fixed at creation: an existing note is
//! found by `bead:` and keeps its name when the title changes.

/// Leaves room under the 255-byte name limit for the id, ` - `, `.md`
/// and a Syncthing `.sync-conflict-…` suffix.
pub const TITLE_MAX_BYTES: usize = 120;

/// Illegal in a filename on some platform, or breaks an Obsidian link.
const ILLEGAL: &[char] = &[
    '/', '\\', ':', '*', '?', '"', '<', '>', '|', '#', '^', '[', ']',
];

pub fn filename(title: &str, id: &str) -> String {
    stem(title, id) + ".md"
}

/// The name without `.md`, which is also what a wikilink to the note
/// names.
pub fn stem(title: &str, id: &str) -> String {
    let mapped: String = title
        .chars()
        .map(|c| match c {
            c if ILLEGAL.contains(&c) => '-',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    let collapsed = mapped.split_whitespace().collect::<Vec<_>>().join(" ");
    // The vault index skips dot-named files, so a hidden note would never
    // be found again and every sync would create another.
    let title = collapsed.trim_start_matches(|c: char| c == '.' || c.is_whitespace());
    let mut cut = title.len().min(TITLE_MAX_BYTES);
    while !title.is_char_boundary(cut) {
        cut -= 1;
    }
    let title = title[..cut].trim_end();
    if title.is_empty() {
        id.to_string()
    } else {
        format!("{title} - {id}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_example() {
        assert_eq!(
            filename("point references at jossware/ganymede", "ganymede-juj.12"),
            "point references at jossware-ganymede - ganymede-juj.12.md"
        );
    }

    #[test]
    fn each_illegal_character_is_replaced() {
        for c in "/\\:*?\"<>|#^[]".chars() {
            assert_eq!(stem(&format!("a{c}b"), "x-1"), "a-b - x-1", "{c}");
        }
    }

    #[test]
    fn control_characters_become_spaces() {
        assert_eq!(stem("a\nb\tc", "x-1"), "a b c - x-1");
    }

    #[test]
    fn whitespace_collapses() {
        assert_eq!(stem("  a   b  ", "x-1"), "a b - x-1");
    }

    #[test]
    fn leading_dots_are_stripped() {
        assert_eq!(stem(".envrc is ignored", "x-1"), "envrc is ignored - x-1");
        assert_eq!(stem("...", "x-1"), "x-1");
        assert_eq!(stem(". .foo", "x-1"), "foo - x-1");
    }

    #[test]
    fn empty_title_is_the_id() {
        assert_eq!(stem("", "x-1"), "x-1");
        assert_eq!(stem("  ", "x-1"), "x-1");
    }

    #[test]
    fn non_ascii_is_kept() {
        assert_eq!(
            stem("café ünïcode 日本語", "x-1"),
            "café ünïcode 日本語 - x-1"
        );
    }

    #[test]
    fn long_title_keeps_the_id() {
        assert_eq!(
            stem(&"a".repeat(300), "x-1"),
            format!("{} - x-1", "a".repeat(TITLE_MAX_BYTES))
        );
    }

    #[test]
    fn long_non_ascii_title_cuts_on_a_char_boundary() {
        assert_eq!(
            stem(&"日".repeat(100), "x-1"),
            format!("{} - x-1", "日".repeat(40))
        );
    }

    #[test]
    fn cut_trims_trailing_space() {
        let title = format!("{} bbbb", "a".repeat(119));
        assert_eq!(stem(&title, "x-1"), format!("{} - x-1", "a".repeat(119)));
    }
}
