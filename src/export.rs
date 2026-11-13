use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::path::Path;

use serde::Deserialize;
use serde_json::Value;

use crate::bd::{Bd, BdError};
use crate::bead::Bead;
use crate::field_path::{FieldPath, FieldValue};

/// bd scaffolding types that never become notes. Templates, wisps, and
/// memories are recognised by their flags and `_type` instead, so a repo's
/// custom type of the same name still syncs.
const SCAFFOLDING_TYPES: &[&str] = &["molecule", "gate", "message", "agent", "role"];

#[derive(Debug)]
pub struct ParseError {
    /// 1-based line number in the export.
    pub line: usize,
    pub source: serde_json::Error,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.source)
    }
}

impl Error for ParseError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.source)
    }
}

#[derive(Deserialize)]
struct Record {
    #[serde(flatten)]
    bead: Bead,
    #[serde(default)]
    ephemeral: bool,
    #[serde(default)]
    is_template: bool,
}

fn is_scaffolding(record: &Record) -> bool {
    record.ephemeral
        || record.is_template
        || SCAFFOLDING_TYPES.contains(&record.bead.issue_type.as_str())
}

/// One database's export, split by whether a bead gets a note.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Export {
    pub beads: Vec<Bead>,
    /// Never rendered, but their rows still name parents and children of
    /// beads that are.
    pub scaffolding: Vec<Bead>,
    /// Bead id to the requested field's value, for each bead in `beads`.
    /// Empty when no field was requested. Scaffolding has none: it never
    /// becomes a note, so nothing reads its value.
    pub fields: HashMap<String, FieldValue>,
}

/// Parses `bd export` JSONL, keeping export order.
///
/// Unknown fields are ignored: the export carries fields its schema does not
/// list, and bd adds more over time.
pub fn parse(jsonl: &str) -> Result<Export, ParseError> {
    parse_with_field(jsonl, None)
}

/// Like [`parse`], also resolving `field` on every bead that gets a note.
pub fn parse_with_field(jsonl: &str, field: Option<&FieldPath>) -> Result<Export, ParseError> {
    let mut export = Export::default();
    for (i, text) in jsonl.lines().enumerate() {
        if text.trim().is_empty() {
            continue;
        }
        let err = |source| ParseError {
            line: i + 1,
            source,
        };
        let value: Value = serde_json::from_str(text).map_err(err)?;
        // Memory records lack every issue field, so they are dropped before
        // validation rather than failing it.
        if value
            .get("_type")
            .is_some_and(|kind| kind.as_str() != Some("issue"))
        {
            continue;
        }
        let resolved = field.map(|path| path.resolve(&value));
        let record = Record::deserialize(value).map_err(err)?;
        if is_scaffolding(&record) {
            export.scaffolding.push(record.bead);
        } else {
            if let Some(resolved) = resolved {
                export.fields.insert(record.bead.id.clone(), resolved);
            }
            export.beads.push(record.bead);
        }
    }
    Ok(export)
}

#[derive(Debug)]
pub enum ExportError {
    Bd(BdError),
    Parse(ParseError),
}

impl fmt::Display for ExportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExportError::Bd(e) => e.fmt(f),
            ExportError::Parse(e) => write!(f, "bd export: {e}"),
        }
    }
}

impl Error for ExportError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            ExportError::Bd(e) => Some(e),
            ExportError::Parse(e) => Some(e),
        }
    }
}

impl From<BdError> for ExportError {
    fn from(e: BdError) -> ExportError {
        ExportError::Bd(e)
    }
}

impl From<ParseError> for ExportError {
    fn from(e: ParseError) -> ExportError {
        ExportError::Parse(e)
    }
}

/// Exports every bead in the database at `database`. Any failure means
/// the export is not authoritative and the database's sync MUST stop.
pub fn export(bd: &Bd, database: &Path) -> Result<Export, ExportError> {
    export_with_field(bd, database, None)
}

/// Like [`export`], also resolving `field` on every bead that gets a note.
pub fn export_with_field(
    bd: &Bd,
    database: &Path,
    field: Option<&FieldPath>,
) -> Result<Export, ExportError> {
    let jsonl = bd.run_on(database, &["export"])?;
    Ok(parse_with_field(&jsonl, field)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bd::test_support::fake_bd;

    fn line(extra: &str) -> String {
        format!(
            r#"{{"id":"x-1","title":"t","priority":2,"created_at":"2026-10-11T02:21:06Z","updated_at":"2026-10-11T02:21:06Z"{extra}}}"#
        )
    }

    #[test]
    fn parses_minimal_issue_with_defaults() {
        let beads = parse(&line("")).unwrap().beads;
        assert_eq!(beads.len(), 1);
        assert_eq!(beads[0].status, "open");
        assert_eq!(beads[0].issue_type, "task");
        assert!(beads[0].dependencies.is_empty() && beads[0].labels.is_empty());
    }

    #[test]
    fn parses_scheduled_and_due_dates() {
        let extra = r#","defer_until":"2026-11-07T05:00:00Z","due_at":"2026-11-10T14:00:00Z""#;
        let b = &parse(&line(extra)).unwrap().beads[0];
        assert_eq!(b.defer_until, Some("2026-11-07T05:00:00Z".parse().unwrap()));
        assert_eq!(b.due_at, Some("2026-11-10T14:00:00Z".parse().unwrap()));
        let b = &parse(&line("")).unwrap().beads[0];
        assert_eq!((b.defer_until, b.due_at), (None, None));
    }

    #[test]
    fn accepts_custom_status_and_type() {
        let b = &parse(&line(r#","status":"review","issue_type":"incident""#))
            .unwrap()
            .beads[0];
        assert_eq!(
            (b.status.as_str(), b.issue_type.as_str()),
            ("review", "incident")
        );
    }

    #[test]
    fn ignores_unknown_fields() {
        let extra = r#","_type":"issue","dependency_count":3,"future_field":{"a":1}"#;
        assert_eq!(parse(&line(extra)).unwrap().beads.len(), 1);
    }

    #[test]
    fn skips_blank_lines() {
        let text = format!("\n{}\n  \n{}\n\n", line(""), line(""));
        assert_eq!(parse(&text).unwrap().beads.len(), 2);
    }

    #[test]
    fn empty_export_is_empty() {
        assert_eq!(parse("").unwrap(), Export::default());
    }

    #[test]
    fn skips_non_issue_records() {
        let text = format!(
            "{}\n{}",
            r#"{"_type":"memory","key":"k","value":"v"}"#,
            line("")
        );
        let export = parse(&text).unwrap();
        assert_eq!(export.beads.len(), 1);
        assert!(export.scaffolding.is_empty());
    }

    #[test]
    fn separates_scaffolding() {
        for extra in [
            r#","issue_type":"molecule""#,
            r#","issue_type":"gate""#,
            r#","issue_type":"message""#,
            r#","issue_type":"agent""#,
            r#","issue_type":"role""#,
            r#","ephemeral":true"#,
            r#","is_template":true"#,
        ] {
            let export = parse(&line(extra)).unwrap();
            assert_eq!(export.beads, vec![], "{extra}");
            assert_eq!(export.scaffolding.len(), 1, "{extra}");
        }
    }

    #[test]
    fn keeps_custom_types_named_like_scaffolding() {
        for kind in ["template", "memory", "wisp"] {
            let extra = format!(r#","issue_type":"{kind}""#);
            assert_eq!(parse(&line(&extra)).unwrap().beads.len(), 1, "{kind}");
        }
    }

    #[test]
    fn reports_the_bad_line() {
        let text = format!("{}\n{{not json\n", line(""));
        assert_eq!(parse(&text).unwrap_err().line, 2);
    }

    #[test]
    fn missing_required_field_is_an_error() {
        let err = parse(
            r#"{"id":"x-1","title":"t","created_at":"2026-10-11T02:21:06Z","updated_at":"2026-10-11T02:21:06Z"}"#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("priority"), "{err}");
    }

    #[test]
    fn wrong_field_type_is_an_error() {
        assert!(parse(&line(r#","labels":"alpha""#)).is_err());
        assert!(parse(&line(r#","closed_at":"yesterday""#)).is_err());
        assert!(parse(&line(r#","due_at":"soon""#)).is_err());
    }

    fn fake_export(dir: &std::path::Path, stdout: &str) -> Bd {
        Bd::new(fake_bd(dir, &format!("cat <<'EOF'\n{stdout}\nEOF")))
    }

    #[test]
    fn export_parses_bd_output() {
        let tmp = tempfile::tempdir().unwrap();
        let bd = fake_export(tmp.path(), &format!("{}\n{}", line(""), line("")));
        assert_eq!(export(&bd, tmp.path()).unwrap().beads.len(), 2);
    }

    #[test]
    fn export_fails_on_fresh_clone() {
        let tmp = tempfile::tempdir().unwrap();
        let bd = Bd::new(fake_bd(
            tmp.path(),
            "echo 'Error: no beads database found' >&2; exit 1",
        ));
        assert!(matches!(
            export(&bd, tmp.path()),
            Err(ExportError::Bd(BdError::Failed { .. }))
        ));
    }

    #[test]
    fn export_fails_on_bad_line() {
        let tmp = tempfile::tempdir().unwrap();
        let bd = fake_export(tmp.path(), &format!("{}\ngarbage", line("")));
        match export(&bd, tmp.path()) {
            Err(ExportError::Parse(e)) => assert_eq!(e.line, 2),
            other => panic!("expected a parse error, got {other:?}"),
        }
    }

    #[test]
    fn parses_dependencies_and_comments() {
        let extra = r#","dependencies":[{"issue_id":"x-1","depends_on_id":"x-0","type":"parent-child","created_at":"2026-10-11T02:21:06Z","metadata":"{}"}],"comments":[{"id":"c","issue_id":"x-1","author":"a","text":"hi\nthere","created_at":"2026-10-11T02:21:07Z"}]"#;
        let b = &parse(&line(extra)).unwrap().beads[0];
        assert_eq!(b.dependencies[0].depends_on_id, "x-0");
        assert_eq!(b.dependencies[0].kind, "parent-child");
        assert_eq!(b.comments[0].text, "hi\nthere");
    }

    fn two_lines(first: &str) -> String {
        format!("{}\n{}", line(first), line("").replace("x-1", "x-2"))
    }

    fn repo_path() -> FieldPath {
        "metadata.repo".parse().unwrap()
    }

    #[test]
    fn field_values_are_recorded_per_bead() {
        let text = two_lines(r#","metadata":{"repo":"github.com/o/p"}"#);
        let export = parse_with_field(&text, Some(&repo_path())).unwrap();
        assert_eq!(
            export.fields["x-1"],
            FieldValue::String("github.com/o/p".into())
        );
        assert_eq!(export.fields["x-2"], FieldValue::Absent);
        assert_eq!(export.fields.len(), 2);
    }

    #[test]
    fn non_string_field_keeps_the_bead() {
        let export =
            parse_with_field(&line(r#","metadata":{"repo":42}"#), Some(&repo_path())).unwrap();
        assert_eq!(export.beads.len(), 1);
        assert_eq!(export.fields["x-1"], FieldValue::Other("42".into()));
    }

    #[test]
    fn scaffolding_gets_no_field_value() {
        let extra = r#","issue_type":"molecule","metadata":{"repo":"r"}"#;
        let export = parse_with_field(&line(extra), Some(&repo_path())).unwrap();
        assert_eq!(export.scaffolding.len(), 1);
        assert!(export.fields.is_empty());
    }

    #[test]
    fn no_field_means_no_values() {
        let text = two_lines(r#","metadata":{"repo":"r"}"#);
        let plain = parse_with_field(&text, None).unwrap();
        assert!(plain.fields.is_empty());
        assert_eq!(plain, parse(&text).unwrap());
        let with = parse_with_field(&text, Some(&repo_path())).unwrap();
        assert_eq!(with.beads, parse(&text).unwrap().beads);
    }

    #[test]
    fn export_with_field_resolves() {
        let tmp = tempfile::tempdir().unwrap();
        let bd = fake_export(tmp.path(), &line(r#","metadata":{"repo":"r"}"#));
        let export = export_with_field(&bd, tmp.path(), Some(&repo_path())).unwrap();
        assert_eq!(export.fields["x-1"], FieldValue::String("r".into()));
    }
}
