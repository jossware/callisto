//! Which table entry claims a bead, by the value of a configured field.

use std::collections::HashMap;
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
    /// The value found, or compact JSON when it is not a string.
    Value(String),
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
        FieldValue::Other(json) => return Err(Reason::Value(json.clone())),
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
    let mut found: HashMap<&str, Option<Project>> = HashMap::new();
    let mut out = Partition::default();
    for bead in beads {
        let value = fields.get(&bead.id).unwrap_or(&FieldValue::Absent);
        let reason = match claimant(value, claims) {
            Ok(claim) => {
                let project = found
                    .entry(&claim.project)
                    .or_insert_with(|| project::find(vault, &claim.project));
                match project {
                    Some(project) => {
                        out.filed.insert(bead.id.clone(), project.clone());
                        continue;
                    }
                    None => Reason::MissingProject(claim.project.clone()),
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
            Reason::NoField | Reason::Value(_) => ids.push(u.id.clone()),
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
            Reason::Value("[\"o/a\"]".into())
        );
    }
}
