//! Which project a split database files a bead under, by the value of
//! its `split.by` field.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use crate::bead::Bead;
use crate::field_path::FieldValue;
use crate::project::{self, Project};
use crate::standing::Condition;

/// Why no note is written for a bead in a split database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reason {
    NoField,
    /// The string value found.
    Value(String),
    /// Compact JSON of a value that is not a string.
    Json(String),
    /// The named project is not in the vault.
    MissingProject(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unfiled {
    pub id: String,
    pub title: String,
    pub status: String,
    pub reason: Reason,
}

#[derive(Debug, Default)]
pub struct Partition {
    /// Bead id to the project its note is filed under.
    pub filed: HashMap<String, Project>,
    /// In the order of the beads given.
    pub unfiled: Vec<Unfiled>,
}

/// Names the project a split database files a bead under: the mapped
/// name when the value is in `projects`, else the value's last path
/// segment. Never returns [`Reason::MissingProject`]: only the caller can
/// look in the vault.
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

/// Splits `beads` by whether [`split_project`] names a project in the
/// vault for them. `fields` holds each bead's `split.by` value.
pub fn partition_split(
    vault: &Path,
    locations: &[PathBuf],
    beads: &[Bead],
    fields: &HashMap<String, FieldValue>,
    projects: &BTreeMap<String, String>,
) -> Partition {
    let mut found: HashMap<String, Option<Project>> = HashMap::new();
    let mut out = Partition::default();
    for bead in beads {
        let value = fields.get(&bead.id).unwrap_or(&FieldValue::Absent);
        let reason = match split_project(value, projects) {
            Ok(name) => {
                let project = found
                    .entry(name.clone())
                    .or_insert_with(|| project::find(vault, locations, &name));
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
        out.unfiled.push(Unfiled {
            id: bead.id.clone(),
            title: bead.title.clone(),
            status: bead.status.clone(),
            reason,
        });
    }
    out
}

/// One condition for beads with no usable value, then one per missing
/// project in first-seen order.
pub fn conditions(unfiled: &[Unfiled]) -> Vec<Condition> {
    let mut ids = Vec::new();
    let mut missing: Vec<(String, Vec<String>)> = Vec::new();
    for u in unfiled {
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
        out.push(Condition::Unfiled { ids });
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

    fn s(value: &str) -> FieldValue {
        FieldValue::String(value.into())
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
        std::fs::create_dir_all(vault.path().join("Projects/web")).unwrap();
        let beads = [
            crate::bead::test_support::bead("a", "A"),
            crate::bead::test_support::bead("b", "B"),
            crate::bead::test_support::bead("c", "C"),
        ];
        let fields = HashMap::from([("a".to_string(), s("o/web")), ("b".to_string(), s("o/zzz"))]);
        let out = partition_split(
            vault.path(),
            &project::DEFAULT_LOCATIONS.map(PathBuf::from),
            &beads,
            &fields,
            &BTreeMap::new(),
        );
        assert_eq!(out.filed["a"].name, "web");
        let reasons: Vec<_> = out.unfiled.iter().map(|u| (&u.id[..], &u.reason)).collect();
        assert_eq!(
            reasons,
            [
                ("b", &Reason::MissingProject("zzz".into())),
                ("c", &Reason::NoField)
            ]
        );
    }
}
