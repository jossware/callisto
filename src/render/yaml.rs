//! Scalars and mapped values for the frontmatter.

/// Bare only when YAML reads the value back as the same string. YAML 1.1
/// and 1.2 read a leading digit as a number or date and these words as
/// booleans or null; Obsidian would then filter on the wrong type.
pub fn scalar(value: &str) -> String {
    const WORDS: [&str; 9] = ["true", "false", "null", "yes", "no", "on", "off", "y", "n"];
    let bytes = value.as_bytes();
    let safe = bytes
        .iter()
        .all(|b| b.is_ascii_alphanumeric() || b"._/:@+~-".contains(b));
    let starts_plain = bytes
        .first()
        .is_some_and(|b| b.is_ascii_alphabetic() || *b == b'/');
    let misread = WORDS.iter().any(|w| value.eq_ignore_ascii_case(w));
    if safe && starts_plain && !misread && bytes.last() != Some(&b':') {
        value.to_string()
    } else {
        quote(value)
    }
}

/// YAML rejects a raw control character even inside double quotes.
pub fn quote(value: &str) -> String {
    let mut out = String::from("\"");
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            c if c.is_control() => out.push_str(&format!("\\x{:02X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// TaskNotes status for a bead status. `blocked` stays `open`: the vault
/// reserves `waiting` for tasks parked on the user.
pub fn status(bead_status: &str) -> &'static str {
    match bead_status {
        "in_progress" | "hooked" => "in-progress",
        "deferred" => "someday-maybe",
        "closed" => "done",
        _ => "open",
    }
}

pub fn priority(p: i64) -> &'static str {
    match p {
        ..=1 => "high",
        2 => "normal",
        _ => "low",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_escapes() {
        assert_eq!(quote("a\"b\\c\nd"), r#""a\"b\\c\x0Ad""#);
    }

    #[test]
    fn scalar_leaves_safe_values_bare() {
        for v in ["ganymede-juj.12", "task", "in_progress", "a/b"] {
            assert_eq!(scalar(v), v);
        }
    }

    #[test]
    fn scalar_quotes_values_yaml_misreads() {
        for v in [
            "yes",
            "No",
            "null",
            "2",
            "2026-10-10",
            "a b",
            "#x",
            "-x",
            "x:",
            "",
        ] {
            assert_eq!(scalar(v), quote(v), "{v}");
        }
    }

    #[test]
    fn status_mapping() {
        for (bead, want) in [
            ("open", "open"),
            ("pinned", "open"),
            ("blocked", "open"),
            ("in_progress", "in-progress"),
            ("hooked", "in-progress"),
            ("deferred", "someday-maybe"),
            ("closed", "done"),
            ("review", "open"),
            ("", "open"),
        ] {
            assert_eq!(status(bead), want, "{bead}");
        }
    }

    #[test]
    fn priority_mapping() {
        for (p, want) in [
            (-1, "high"),
            (0, "high"),
            (1, "high"),
            (2, "normal"),
            (3, "low"),
            (4, "low"),
            (9, "low"),
        ] {
            assert_eq!(priority(p), want, "{p}");
        }
    }
}
