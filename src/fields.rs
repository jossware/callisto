//! The few frontmatter fields callisto reads from any note. People and
//! Obsidian write these notes, but only top-level `bead`, `type`,
//! `generatedBy` and `beadPrefix` matter, so this is a line scanner, not
//! a YAML parser.

#[derive(Debug, Default, PartialEq)]
pub struct Fields {
    pub beads: Vec<String>,
    /// `type:`.
    pub kind: Option<String>,
    pub generated_by: Option<String>,
    pub bead_prefix: Option<String>,
}

/// `text` MAY stop right after the closing `---`.
pub fn read(text: &str) -> Fields {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l));
    if lines.next() != Some("---") {
        return Fields::default();
    }
    let mut block = Vec::new();
    for line in lines.by_ref() {
        if line == "---" {
            return fields(&block);
        }
        block.push(line);
    }
    // Without a closing line the leading `---` is a horizontal rule.
    Fields::default()
}

fn fields(block: &[&str]) -> Fields {
    let mut found = Fields::default();
    let (mut bead, mut kind, mut generated_by, mut bead_prefix) = (false, false, false, false);
    for (n, line) in block.iter().enumerate() {
        if line.starts_with([' ', '\t']) {
            continue;
        }
        let Some((key, raw)) = line.split_once(':') else {
            continue;
        };
        let raw = raw.trim();
        match key {
            "bead" if !bead => {
                bead = true;
                found.beads = if raw.is_empty() {
                    block_list(&block[n + 1..])
                } else if let Some(inner) = raw.strip_prefix('[').and_then(|r| r.strip_suffix(']'))
                {
                    inner
                        .split(',')
                        .map(value)
                        .filter(|v| !v.is_empty())
                        .collect()
                } else {
                    Some(value(raw))
                        .filter(|v| !v.is_empty())
                        .into_iter()
                        .collect()
                };
            }
            "type" if !kind => {
                kind = true;
                found.kind = Some(value(raw)).filter(|v| !v.is_empty());
            }
            "generatedBy" if !generated_by => {
                generated_by = true;
                found.generated_by = Some(value(raw)).filter(|v| !v.is_empty());
            }
            "beadPrefix" if !bead_prefix => {
                bead_prefix = true;
                found.bead_prefix = Some(value(raw)).filter(|v| !v.is_empty());
            }
            _ => {}
        }
    }
    found
}

/// YAML lets a block sequence's `-` sit at the key's own column.
fn block_list(rest: &[&str]) -> Vec<String> {
    let mut items = Vec::new();
    for line in rest {
        let trimmed = line.trim_start();
        let Some(item) = trimmed
            .strip_prefix('-')
            .filter(|i| i.is_empty() || i.starts_with(' '))
        else {
            if line.starts_with([' ', '\t']) || trimmed.is_empty() {
                continue;
            }
            break;
        };
        let item = value(item);
        if !item.is_empty() {
            items.push(item);
        }
    }
    items
}

pub(crate) fn value(raw: &str) -> String {
    let raw = raw.trim();
    if let Some(body) = raw.strip_prefix('"') {
        let mut out = String::new();
        let mut chars = body.chars();
        while let Some(c) = chars.next() {
            match c {
                '"' => break,
                '\\' => out.extend(chars.next()),
                c => out.push(c),
            }
        }
        out
    } else if let Some(body) = raw.strip_prefix('\'') {
        let mut out = String::new();
        let mut chars = body.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\'' {
                if chars.peek() == Some(&'\'') {
                    chars.next();
                    out.push('\'');
                } else {
                    break;
                }
            } else {
                out.push(c);
            }
        }
        out
    } else {
        raw.split(" #").next().unwrap_or("").trim().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn beads(text: &str) -> Vec<String> {
        read(text).beads
    }

    #[test]
    fn scalar_bead() {
        assert_eq!(
            read("---\nbead: fx-dg4\ntype: task\n---\nbody\n"),
            Fields {
                beads: vec!["fx-dg4".into()],
                kind: Some("task".into()),
                generated_by: None,
                bead_prefix: None,
            }
        );
    }

    #[test]
    fn bead_prefix() {
        let read_prefix = |text: &str| read(text).bead_prefix;
        assert_eq!(
            read_prefix("---\nbead: a-b-1\nbeadPrefix: a-b\n---\n"),
            Some("a-b".into())
        );
        assert_eq!(
            read_prefix("---\nbeadPrefix: \"a\"\n---\n"),
            Some("a".into())
        );
        assert_eq!(read_prefix("---\nbeadPrefix:\n---\n"), None);
        assert_eq!(read_prefix("---\nmeta:\n  beadPrefix: a\n---\n"), None);
    }

    #[test]
    fn quoted_scalars() {
        assert_eq!(beads("---\nbead: \"a-1\"\n---\n"), ["a-1"]);
        assert_eq!(beads("---\nbead: 'a-2'\n---\n"), ["a-2"]);
    }

    #[test]
    fn comment_after_plain_scalar() {
        assert_eq!(beads("---\nbead: a-1 # old\n---\n"), ["a-1"]);
    }

    #[test]
    fn block_list() {
        let want = ["fx-0pf.2", "fx-0pf.3"];
        assert_eq!(
            beads("---\nbead:\n  - fx-0pf.2\n  - \"fx-0pf.3\"\ntype: task\n---\n"),
            want
        );
        assert_eq!(beads("---\nbead:\n- fx-0pf.2\n- \"fx-0pf.3\"\n---\n"), want);
    }

    #[test]
    fn flow_list() {
        assert_eq!(beads("---\nbead: [a-1, \"a-2\"]\n---\n"), ["a-1", "a-2"]);
    }

    #[test]
    fn marker() {
        assert_eq!(
            read("---\ngeneratedBy: callisto\n---\n").generated_by,
            Some("callisto".into())
        );
    }

    #[test]
    fn view_mode_is_ignored() {
        assert_eq!(
            read(
                "---\nbead: a-1\ntype: task\ngeneratedBy: callisto\nobsidianUIMode: preview\n---\n"
            ),
            Fields {
                beads: vec!["a-1".into()],
                kind: Some("task".into()),
                generated_by: Some("callisto".into()),
                bead_prefix: None,
            }
        );
    }

    #[test]
    fn no_frontmatter() {
        assert_eq!(read("bead: a-1\n"), Fields::default());
    }

    #[test]
    fn unclosed_block_is_no_frontmatter() {
        assert_eq!(read("---\nbead: a-1\nmore\n"), Fields::default());
    }

    #[test]
    fn indented_key_is_ignored() {
        assert!(beads("---\nmeta:\n  bead: a-1\n---\n").is_empty());
    }

    #[test]
    fn body_is_ignored() {
        assert!(beads("---\ntype: task\n---\nbead: a-1\n").is_empty());
    }

    #[test]
    fn crlf_and_bom() {
        assert_eq!(beads("\u{feff}---\r\nbead: a-1\r\n---\r\n"), ["a-1"]);
    }

    #[test]
    fn first_key_wins() {
        assert_eq!(beads("---\nbead: a-1\nbead: a-2\n---\n"), ["a-1"]);
    }

    #[test]
    fn empty_bead_is_no_ids() {
        let f = read("---\nbead:\ntype: task\n---\n");
        assert!(f.beads.is_empty());
        assert_eq!(f.kind, Some("task".into()));
    }
}
