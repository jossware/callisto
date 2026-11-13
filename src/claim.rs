//! Which table entry claims a bead, by the value of a configured field.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use crate::bead::Bead;
use crate::field_path::FieldValue;
use crate::group::Claim;
use crate::project::{self, Project};
use crate::standing::Condition;

/// Why no note is written for a bead in a table group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reason {
    NoField,
    /// The string value found.
    Value(String),
    /// Compact JSON of a value that is not a string.
    Json(String),
    /// The claiming entry's project is not in the vault.
    MissingProject(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unclaimed {
    pub id: String,
    pub title: String,
    pub status: String,
    pub reason: Reason,
}

/// Never returns [`Reason::MissingProject`]: only the caller can look
/// in the vault.
pub fn claimant<'c>(value: &FieldValue, claims: &'c [Claim]) -> Result<&'c Claim, Reason> {
    let found = match value {
        FieldValue::Absent => return Err(Reason::NoField),
        FieldValue::Other(json) => return Err(Reason::Json(json.clone())),
        FieldValue::String(found) => found,
    };
    claims
        .iter()
        .find(|c| c.values.contains(found))
        .ok_or_else(|| Reason::Value(found.clone()))
}

#[derive(Debug, Default)]
pub struct Partition {
    /// Bead id to the project its note is filed under.
    pub filed: HashMap<String, Project>,
    /// In the order of the beads given.
    pub unclaimed: Vec<Unclaimed>,
}

/// Splits `beads` by whether a claim files them under a project in the
/// vault. `fields` holds each bead's value of the claimed field.
pub fn partition(
    vault: &Path,
    beads: &[Bead],
    fields: &HashMap<String, FieldValue>,
    claims: &[Claim],
) -> Partition {
    partition_by(vault, beads, fields, |value| {
        claimant(value, claims).map(|claim| claim.project.clone())
    })
}

/// Names the project a split database files a bead under: the mapped
/// name when the value is in `projects`, else the value's last path
/// segment.
pub fn split_project(
    value: &FieldValue,
    projects: &BTreeMap<String, String>,
) -> Result<String, Reason> {
    let found = match value {
        FieldValue::Absent => return Err(Reason::NoField),
        FieldValue::Other(json) => return Err(Reason::Json(json.clone())),
        FieldValue::String(found) => found,
    };
    if let Some(project) = projects.get(found) {
        return Ok(project.clone());
    }
    match found.trim_end_matches('/').rsplit('/').next() {
        Some(segment) if !segment.is_empty() => Ok(segment.to_string()),
        _ => Err(Reason::Value(found.clone())),
    }
}

/// Like [`partition`], with the project named by [`split_project`].
pub fn partition_split(
    vault: &Path,
    beads: &[Bead],
    fields: &HashMap<String, FieldValue>,
    projects: &BTreeMap<String, String>,
) -> Partition {
    partition_by(vault, beads, fields, |value| split_project(value, projects))
}

fn partition_by(
    vault: &Path,
    beads: &[Bead],
    fields: &HashMap<String, FieldValue>,
    name_project: impl Fn(&FieldValue) -> Result<String, Reason>,
) -> Partition {
    let mut found: HashMap<String, Option<Project>> = HashMap::new();
    let mut out = Partition::default();
    for bead in beads {
        let value = fields.get(&bead.id).unwrap_or(&FieldValue::Absent);
        let reason = match name_project(value) {
            Ok(name) => {
                let project = found
                    .entry(name.clone())
                    .or_insert_with(|| project::find(vault, &name));
                match project {
                    Some(project) => {
                        out.filed.insert(bead.id.clone(), project.clone());
                        continue;
                    }
                    None => Reason::MissingProject(name),
                }
            }
            Err(reason) => reason,
        };
        out.unclaimed.push(Unclaimed {
            id: bead.id.clone(),
            title: bead.title.clone(),
            status: bead.status.clone(),
            reason,
        });
    }
    out
}

/// One condition for beads no claim matched, then one per missing
/// project in first-seen order.
pub fn conditions(unclaimed: &[Unclaimed]) -> Vec<Condition> {
    let mut ids = Vec::new();
    let mut missing: Vec<(String, Vec<String>)> = Vec::new();
    for u in unclaimed {
        match &u.reason {
            Reason::MissingProject(project) => {
                match missing.iter_mut().find(|(p, _)| p == project) {
                    Some((_, ids)) => ids.push(u.id.clone()),
                    None => missing.push((project.clone(), vec![u.id.clone()])),
                }
            }
            Reason::NoField | Reason::Value(_) | Reason::Json(_) => ids.push(u.id.clone()),
        }
    }
    let mut out = Vec::new();
    if !ids.is_empty() {
        out.push(Condition::Unclaimed { ids });
    }
    out.extend(
        missing
            .into_iter()
            .map(|(project, ids)| Condition::MissingProject { project, ids }),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claim(values: &[&str], project: &str) -> Claim {
        Claim {
            values: values.iter().map(|v| v.to_string()).collect(),
            project: project.into(),
        }
    }

    fn s(value: &str) -> FieldValue {
        FieldValue::String(value.into())
    }

    #[test]
    fn claimant_matches_any_listed_value() {
        let claims = [claim(&["o/a"], "A"), claim(&["o/b", "o/b-old"], "B")];
        assert_eq!(claimant(&s("o/b-old"), &claims).unwrap().project, "B");
        assert_eq!(claimant(&s("o/a"), &claims).unwrap().project, "A");
    }

    #[test]
    fn unclaimed_reasons() {
        let claims = [claim(&["o/a"], "A")];
        assert_eq!(
            claimant(&FieldValue::Absent, &claims).unwrap_err(),
            Reason::NoField
        );
        assert_eq!(
            claimant(&s("o/z"), &claims).unwrap_err(),
            Reason::Value("o/z".into())
        );
        assert_eq!(
            claimant(&FieldValue::Other("[\"o/a\"]".into()), &claims).unwrap_err(),
            Reason::Json("[\"o/a\"]".into())
        );
    }

    #[test]
    fn split_project_prefers_the_map() {
        let map = BTreeMap::from([("github.com/o/api".into(), "API".into())]);
        assert_eq!(
            split_project(&s("github.com/o/api"), &map),
            Ok("API".into())
        );
        assert_eq!(
            split_project(&s("github.com/o/web"), &map),
            Ok("web".into())
        );
        assert_eq!(
            split_project(&s("github.com/o/web/"), &map),
            Ok("web".into())
        );
    }

    #[test]
    fn split_project_unusable_values() {
        let map = BTreeMap::new();
        assert_eq!(
            split_project(&FieldValue::Absent, &map),
            Err(Reason::NoField)
        );
        assert_eq!(split_project(&s(""), &map), Err(Reason::Value("".into())));
        assert_eq!(split_project(&s("/"), &map), Err(Reason::Value("/".into())));
        assert_eq!(
            split_project(&FieldValue::Other("1".into()), &map),
            Err(Reason::Json("1".into()))
        );
    }

    #[test]
    fn partition_split_sends_missing_project_to_unfiled() {
        let vault = tempfile::tempdir().unwrap();
        let home = vault.path().join("Projects/web");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(home.join("web.md"), "").unwrap();
        let beads = [
            crate::bead::test_support::bead("a", "A"),
            crate::bead::test_support::bead("b", "B"),
            crate::bead::test_support::bead("c", "C"),
        ];
        let fields = HashMap::from([("a".to_string(), s("o/web")), ("b".to_string(), s("o/zzz"))]);
        let out = partition_split(vault.path(), &beads, &fields, &BTreeMap::new());
        assert_eq!(out.filed["a"].name, "web");
        let reasons: Vec<_> = out
            .unclaimed
            .iter()
            .map(|u| (&u.id[..], &u.reason))
            .collect();
        assert_eq!(
            reasons,
            [
                ("b", &Reason::MissingProject("zzz".into())),
                ("c", &Reason::NoField)
            ]
        );
    }
}
