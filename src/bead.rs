use jiff::Timestamp;
use serde::Deserialize;

/// One issue from `bd export`. Only the fields callisto renders are kept.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Bead {
    pub id: String,
    pub title: String,
    #[serde(default = "default_status")]
    pub status: String,
    pub priority: i64,
    #[serde(default = "default_issue_type")]
    pub issue_type: String,
    pub description: Option<String>,
    pub design: Option<String>,
    pub acceptance_criteria: Option<String>,
    pub notes: Option<String>,
    pub close_reason: Option<String>,
    pub assignee: Option<String>,
    pub owner: Option<String>,
    pub created_by: Option<String>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub started_at: Option<Timestamp>,
    pub closed_at: Option<Timestamp>,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub dependencies: Vec<Dependency>,
    #[serde(default)]
    pub comments: Vec<Comment>,
}

/// An edge from the bead that holds it to `depends_on_id`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Dependency {
    pub depends_on_id: String,
    #[serde(rename = "type")]
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Comment {
    pub author: String,
    pub text: String,
    pub created_at: Timestamp,
}

// bd's own defaults when it omits the field.
fn default_status() -> String {
    "open".into()
}

fn default_issue_type() -> String {
    "task".into()
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::Bead;

    /// An open P1 task with fixed timestamps and nothing optional.
    pub fn bead(id: &str, title: &str) -> Bead {
        Bead {
            id: id.into(),
            title: title.into(),
            status: "open".into(),
            priority: 1,
            issue_type: "task".into(),
            description: None,
            design: None,
            acceptance_criteria: None,
            notes: None,
            close_reason: None,
            assignee: None,
            owner: None,
            created_by: None,
            created_at: "2026-10-11T02:21:06Z".parse().unwrap(),
            updated_at: "2026-10-11T02:21:40Z".parse().unwrap(),
            started_at: None,
            closed_at: None,
            labels: vec![],
            dependencies: vec![],
            comments: vec![],
        }
    }
}
