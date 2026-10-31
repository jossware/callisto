//! Marking the note of a bead that left beads. The note outlives its
//! bead, so it is kept as last rendered, with two lines changed.

use crate::fields;

pub const KEY: &str = "beadRemoved";

/// `None` when there is nothing to change, or no frontmatter to change.
pub fn mark(text: &str) -> Option<String> {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let bare = |line: &str| line.trim_end_matches(['\n', '\r']).len();
    let first = &lines.first()?[..bare(lines[0])];
    if first.strip_prefix('\u{feff}').unwrap_or(first) != "---" {
        return None;
    }
    let close = 1 + lines[1..].iter().position(|l| &l[..bare(l)] == "---")?;
    let mut out: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    let (mut status, mut flag) = (false, false);
    for (i, line) in lines.iter().enumerate().take(close).skip(1) {
        if line.starts_with([' ', '\t']) {
            continue;
        }
        let (body, ending) = line.split_at(bare(line));
        let Some((key, raw)) = body.split_once(':') else {
            continue;
        };
        if key == "status" && !status {
            status = true;
            // Done and dropped already say the work is over.
            if !matches!(fields::value(raw).as_str(), "done" | "dropped") {
                out[i] = format!("status: dropped{ending}");
            }
        } else if key == KEY && !flag {
            flag = true;
            if fields::value(raw) != "true" {
                out[i] = format!("{KEY}: true{ending}");
            }
        }
    }
    if !flag {
        // The line before the closing one always ends in a newline.
        let prev = lines[close - 1];
        let ending = &prev[bare(prev)..];
        out.insert(close, format!("{KEY}: true{ending}"));
    }
    let marked = out.concat();
    (marked != text).then_some(marked)
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPEN: &str =
        "---\ntype: task\nstatus: open\nbeadStatus: open\ngeneratedBy: callisto\n---\nbody\n";
    const OPEN_MARKED: &str = "---\ntype: task\nstatus: dropped\nbeadStatus: open\ngeneratedBy: callisto\nbeadRemoved: true\n---\nbody\n";

    #[test]
    fn open_becomes_dropped() {
        assert_eq!(mark(OPEN).as_deref(), Some(OPEN_MARKED));
    }

    #[test]
    fn mark_keeps_view_mode() {
        assert_eq!(
            mark("---\nstatus: open\ngeneratedBy: callisto\nobsidianUIMode: preview\n---\nbody\n")
                .as_deref(),
            Some(
                "---\nstatus: dropped\ngeneratedBy: callisto\nobsidianUIMode: preview\nbeadRemoved: true\n---\nbody\n"
            )
        );
    }

    #[test]
    fn done_and_dropped_stay() {
        for status in ["done", "dropped"] {
            let text = format!("---\nstatus: {status}\nbead: a-1\n---\n");
            assert_eq!(
                mark(&text),
                Some(format!(
                    "---\nstatus: {status}\nbead: a-1\nbeadRemoved: true\n---\n"
                ))
            );
        }
    }

    #[test]
    fn already_marked_is_none() {
        assert_eq!(mark(OPEN_MARKED), None);
    }

    #[test]
    fn quoted_status_and_crlf() {
        assert_eq!(
            mark("---\r\nstatus: \"in-progress\"\r\nbead: a-1\r\n---\r\nbody\r\n").as_deref(),
            Some("---\r\nstatus: dropped\r\nbead: a-1\r\nbeadRemoved: true\r\n---\r\nbody\r\n")
        );
    }

    #[test]
    fn indented_keys_are_ignored() {
        assert_eq!(
            mark("---\nmeta:\n  status: open\nstatus: done\n---\n").as_deref(),
            Some("---\nmeta:\n  status: open\nstatus: done\nbeadRemoved: true\n---\n")
        );
    }

    #[test]
    fn body_status_is_ignored() {
        assert_eq!(
            mark("---\nstatus: done\n---\nstatus: open\n").as_deref(),
            Some("---\nstatus: done\nbeadRemoved: true\n---\nstatus: open\n")
        );
    }

    #[test]
    fn no_frontmatter_is_none() {
        assert_eq!(mark("body\n"), None);
        assert_eq!(mark("---\nstatus: open\n"), None);
    }

    #[test]
    fn false_flag_becomes_true() {
        assert_eq!(
            mark("---\nstatus: done\nbeadRemoved: false\nbead: a-1\n---\n").as_deref(),
            Some("---\nstatus: done\nbeadRemoved: true\nbead: a-1\n---\n")
        );
    }
}
