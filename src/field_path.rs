use std::error::Error;
use std::fmt;
use std::str::FromStr;

use serde_json::Value;

/// A dotted path into a bead's raw JSON, such as `metadata.repo`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldPath {
    raw: String,
    segments: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidFieldPath {
    pub path: String,
}

impl fmt::Display for InvalidFieldPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid field path \"{}\": empty segment", self.path)
    }
}

impl Error for InvalidFieldPath {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldValue {
    Absent,
    String(String),
    /// Compact JSON text of a non-string leaf.
    Other(String),
}

impl FromStr for FieldPath {
    type Err = InvalidFieldPath;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let segments: Vec<String> = s.split('.').map(str::to_owned).collect();
        if segments.iter().any(String::is_empty) {
            return Err(InvalidFieldPath { path: s.to_owned() });
        }
        Ok(FieldPath {
            raw: s.to_owned(),
            segments,
        })
    }
}

impl fmt::Display for FieldPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.raw)
    }
}

impl FieldPath {
    pub fn resolve(&self, value: &Value) -> FieldValue {
        let mut current = value;
        for segment in &self.segments {
            match current {
                Value::Object(map) => match map.get(segment) {
                    Some(next) => current = next,
                    None => return FieldValue::Absent,
                },
                _ => return FieldValue::Absent,
            }
        }
        match current {
            Value::String(s) => FieldValue::String(s.clone()),
            other => {
                FieldValue::Other(serde_json::to_string(other).expect("a Value always serializes"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json as j;

    fn p(s: &str) -> FieldPath {
        s.parse::<FieldPath>().unwrap()
    }

    #[test]
    fn parses_valid_paths() {
        p("title");
        p("a.b.c");
        assert_eq!(p("metadata.repo").to_string(), "metadata.repo");
    }

    #[test]
    fn rejects_empty_segments() {
        for s in ["", ".", ".a", "a.", "a..b"] {
            let err = s.parse::<FieldPath>().unwrap_err();
            assert_eq!(err.path, s);
            assert!(err.to_string().contains(&format!("\"{s}\"")));
        }
    }

    #[test]
    fn resolves_nested_string() {
        let v = j!({"metadata":{"repo":"github.com/o/p"}});
        assert_eq!(
            p("metadata.repo").resolve(&v),
            FieldValue::String("github.com/o/p".into())
        );
    }

    #[test]
    fn resolves_top_level_string() {
        assert_eq!(
            p("title").resolve(&j!({"title":"t"})),
            FieldValue::String("t".into())
        );
    }

    #[test]
    fn empty_string_is_present() {
        assert_eq!(
            p("metadata.repo").resolve(&j!({"metadata":{"repo":""}})),
            FieldValue::String(String::new())
        );
    }

    #[test]
    fn missing_keys_are_absent() {
        assert_eq!(p("metadata.repo").resolve(&j!({})), FieldValue::Absent);
        assert_eq!(
            p("metadata.repo").resolve(&j!({"metadata":{}})),
            FieldValue::Absent
        );
    }

    #[test]
    fn non_object_intermediate_is_absent() {
        for m in [j!("x"), j!([{"repo":"x"}]), j!(null), j!(3)] {
            assert_eq!(
                p("metadata.repo").resolve(&j!({ "metadata": m })),
                FieldValue::Absent
            );
        }
    }

    #[test]
    fn json_string_is_not_decoded() {
        assert_eq!(
            p("metadata.repo").resolve(&j!({"metadata":"{\"repo\":\"x\"}"})),
            FieldValue::Absent
        );
    }

    #[test]
    fn segments_are_literal() {
        assert_eq!(
            p("metadata. repo").resolve(&j!({"metadata":{"repo":"x"}})),
            FieldValue::Absent
        );
    }

    #[test]
    fn non_strings_keep_json_text() {
        let cases = [
            (j!(1), "1"),
            (j!(true), "true"),
            (j!(null), "null"),
            (j!([1, "a"]), "[1,\"a\"]"),
            (j!({"a":1}), "{\"a\":1}"),
        ];
        for (leaf, text) in cases {
            assert_eq!(
                p("x").resolve(&j!({ "x": leaf })),
                FieldValue::Other(text.into())
            );
        }
    }
}
