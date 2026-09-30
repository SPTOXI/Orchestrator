//! What the GitHub client returns (ADR-0017). Built from the API's JSON
//! with tolerant accessors: a field GitHub leaves out becomes empty, never
//! a parse error.

use serde::Serialize;
use serde_json::Value;

/// The account the token belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub login: String,
    pub name: Option<String>,
    pub url: String,
    /// Scopes of a classic token (`X-OAuth-Scopes`); empty for fine-grained
    /// tokens, which do not report them.
    pub scopes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoInfo {
    pub owner: String,
    pub name: String,
    pub full_name: String,
    pub url: String,
    pub default_branch: String,
    pub private: bool,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PullSummary {
    pub number: u64,
    pub title: String,
    /// `open`, `closed` or `merged`.
    pub state: String,
    pub draft: bool,
    pub author: String,
    /// Branch the changes come from (`owner:branch` when from a fork).
    pub head: String,
    pub head_sha: String,
    pub base: String,
    pub url: String,
    pub created_at: String,
    pub updated_at: String,
}

/// State of the CI of a commit: check runs and commit statuses together.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Checks {
    /// `success`, `failure`, `pending` or `none` (nothing ran).
    pub state: String,
    pub total: u32,
    pub passed: u32,
    pub failed: u32,
    pub pending: u32,
    pub items: Vec<CheckItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckItem {
    pub name: String,
    /// `success`, `failure`, `pending` or `skipped`.
    pub state: String,
    /// `check` (GitHub Actions and apps) or `status` (commit status).
    pub kind: String,
    pub url: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Review {
    pub author: String,
    /// `approved`, `changes_requested`, `commented` or `dismissed`.
    pub state: String,
    pub body: String,
    pub submitted_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Comment {
    pub author: String,
    pub body: String,
    pub url: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PullDetail {
    #[serde(flatten)]
    pub summary: PullSummary,
    pub body: String,
    pub merged: bool,
    pub merged_at: Option<String>,
    /// `None` while GitHub is still computing it.
    pub mergeable: Option<bool>,
    /// `clean`, `blocked`, `behind`, `dirty` (conflict), `unstable`, …
    pub mergeable_state: Option<String>,
    pub commits: u64,
    pub additions: u64,
    pub deletions: u64,
    pub changed_files: u64,
    pub checks: Checks,
    /// The latest decisive review of each reviewer.
    pub reviews: Vec<Review>,
    /// The most recent comments, oldest first.
    pub comments: Vec<Comment>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeResult {
    pub number: u64,
    pub title: String,
    pub url: String,
    pub merged: bool,
    pub sha: String,
    pub message: String,
    pub branch_deleted: bool,
    /// Why the branch was not deleted, when that was asked and failed.
    pub branch_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueSummary {
    pub number: u64,
    pub title: String,
    /// `open` or `closed`.
    pub state: String,
    pub author: String,
    pub labels: Vec<String>,
    pub comment_count: u64,
    pub url: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueDetail {
    #[serde(flatten)]
    pub summary: IssueSummary,
    pub body: String,
    pub comments: Vec<Comment>,
}

pub(crate) fn text(value: &Value, pointer: &str) -> String {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

pub(crate) fn opt_text(value: &Value, pointer: &str) -> Option<String> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

pub(crate) fn number(value: &Value, pointer: &str) -> u64 {
    value.pointer(pointer).and_then(Value::as_u64).unwrap_or(0)
}

pub(crate) fn flag(value: &Value, pointer: &str) -> bool {
    value
        .pointer(pointer)
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

impl RepoInfo {
    pub(crate) fn from_api(v: &Value) -> Self {
        Self {
            owner: text(v, "/owner/login"),
            name: text(v, "/name"),
            full_name: text(v, "/full_name"),
            url: text(v, "/html_url"),
            default_branch: text(v, "/default_branch"),
            private: flag(v, "/private"),
            description: opt_text(v, "/description"),
        }
    }
}

impl PullSummary {
    pub(crate) fn from_api(v: &Value, base_owner: &str) -> Self {
        let merged = flag(v, "/merged") || v.pointer("/merged_at").is_some_and(|m| !m.is_null());
        let state = match text(v, "/state").as_str() {
            "closed" if merged => "merged".to_owned(),
            other => other.to_owned(),
        };
        let head_owner = text(v, "/head/repo/owner/login");
        let head_ref = text(v, "/head/ref");
        let head = if head_owner.is_empty() || head_owner.eq_ignore_ascii_case(base_owner) {
            head_ref
        } else {
            format!("{head_owner}:{head_ref}")
        };
        Self {
            number: number(v, "/number"),
            title: text(v, "/title"),
            state,
            draft: flag(v, "/draft"),
            author: text(v, "/user/login"),
            head,
            head_sha: text(v, "/head/sha"),
            base: text(v, "/base/ref"),
            url: text(v, "/html_url"),
            created_at: text(v, "/created_at"),
            updated_at: text(v, "/updated_at"),
        }
    }
}

impl Comment {
    pub(crate) fn from_api(v: &Value) -> Self {
        Self {
            author: text(v, "/user/login"),
            body: text(v, "/body"),
            url: text(v, "/html_url"),
            created_at: text(v, "/created_at"),
        }
    }
}

impl IssueSummary {
    pub(crate) fn from_api(v: &Value) -> Self {
        Self {
            number: number(v, "/number"),
            title: text(v, "/title"),
            state: text(v, "/state"),
            author: text(v, "/user/login"),
            labels: v
                .get("labels")
                .and_then(Value::as_array)
                .map(|labels| {
                    labels
                        .iter()
                        .filter_map(|l| l.get("name").and_then(Value::as_str).map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default(),
            comment_count: number(v, "/comments"),
            url: text(v, "/html_url"),
            created_at: text(v, "/created_at"),
            updated_at: text(v, "/updated_at"),
        }
    }
}

/// The latest decisive review of each reviewer (a later "commented" does
/// not hide an approval); pending reviews are left out.
pub(crate) fn latest_reviews(reviews: &[Value]) -> Vec<Review> {
    let mut latest: Vec<Review> = Vec::new();
    for v in reviews {
        let state = text(v, "/state").to_ascii_lowercase();
        if state == "pending" || state.is_empty() {
            continue;
        }
        let review = Review {
            author: text(v, "/user/login"),
            state,
            body: text(v, "/body"),
            submitted_at: opt_text(v, "/submitted_at"),
        };
        match latest.iter_mut().find(|r| r.author == review.author) {
            Some(existing) => {
                if review.state != "commented" || existing.state == "commented" {
                    *existing = review;
                }
            }
            None => latest.push(review),
        }
    }
    latest
}

/// Check runs and commit statuses of one commit, as one state.
pub(crate) fn summarize_checks(check_runs: &[Value], statuses: &[Value]) -> Checks {
    let mut items = Vec::new();
    for run in check_runs {
        let state = if text(run, "/status") != "completed" {
            "pending"
        } else {
            match text(run, "/conclusion").as_str() {
                "success" => "success",
                "neutral" | "skipped" => "skipped",
                _ => "failure",
            }
        };
        items.push(CheckItem {
            name: text(run, "/name"),
            state: state.to_owned(),
            kind: "check".to_owned(),
            url: opt_text(run, "/html_url").or_else(|| opt_text(run, "/details_url")),
            description: opt_text(run, "/output/title"),
        });
    }
    for status in statuses {
        let state = match text(status, "/state").as_str() {
            "success" => "success",
            "pending" => "pending",
            _ => "failure",
        };
        items.push(CheckItem {
            name: text(status, "/context"),
            state: state.to_owned(),
            kind: "status".to_owned(),
            url: opt_text(status, "/target_url"),
            description: opt_text(status, "/description"),
        });
    }
    let count = |wanted: &[&str]| {
        items
            .iter()
            .filter(|i| wanted.contains(&i.state.as_str()))
            .count() as u32
    };
    let (passed, failed, pending) = (
        count(&["success", "skipped"]),
        count(&["failure"]),
        count(&["pending"]),
    );
    let total = items.len() as u32;
    let state = if failed > 0 {
        "failure"
    } else if pending > 0 {
        "pending"
    } else if total > 0 {
        "success"
    } else {
        "none"
    };
    Checks {
        state: state.to_owned(),
        total,
        passed,
        failed,
        pending,
        items,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn checks_add_up_to_one_state() {
        let runs = vec![
            json!({"name": "build", "status": "completed", "conclusion": "success", "html_url": "https://ci/1"}),
            json!({"name": "lint", "status": "completed", "conclusion": "skipped"}),
            json!({"name": "e2e", "status": "in_progress", "conclusion": null}),
        ];
        let statuses =
            vec![json!({"context": "deploy", "state": "success", "target_url": "https://d"})];
        let checks = summarize_checks(&runs, &statuses);
        assert_eq!(
            (
                checks.state.as_str(),
                checks.total,
                checks.passed,
                checks.pending,
                checks.failed
            ),
            ("pending", 4, 3, 1, 0)
        );
        assert_eq!(checks.items[3].kind, "status");

        let failing = summarize_checks(
            &[json!({"name": "t", "status": "completed", "conclusion": "timed_out"})],
            &[json!({"context": "x", "state": "pending"})],
        );
        assert_eq!(failing.state, "failure");
        assert_eq!(summarize_checks(&[], &[]).state, "none");
        assert_eq!(summarize_checks(&runs[..2], &[]).state, "success");
    }

    #[test]
    fn a_later_comment_does_not_hide_an_approval() {
        let reviews = vec![
            json!({"user": {"login": "ana"}, "state": "CHANGES_REQUESTED", "body": "falta teste"}),
            json!({"user": {"login": "bia"}, "state": "COMMENTED", "body": "boa"}),
            json!({"user": {"login": "ana"}, "state": "APPROVED", "body": ""}),
            json!({"user": {"login": "ana"}, "state": "COMMENTED", "body": "nit"}),
            json!({"user": {"login": "caio"}, "state": "PENDING"}),
        ];
        let latest = latest_reviews(&reviews);
        assert_eq!(latest.len(), 2);
        assert_eq!(
            (latest[0].author.as_str(), latest[0].state.as_str()),
            ("ana", "approved")
        );
        assert_eq!(
            (latest[1].author.as_str(), latest[1].state.as_str()),
            ("bia", "commented")
        );
    }

    #[test]
    fn pull_states_and_fork_heads() {
        let merged = json!({
            "number": 7, "title": "T", "state": "closed", "merged_at": "2026-09-30T00:00:00Z",
            "head": {"ref": "feat", "sha": "abc", "repo": {"owner": {"login": "eu"}}},
            "base": {"ref": "main"}, "user": {"login": "eu"}
        });
        let pull = PullSummary::from_api(&merged, "time");
        assert_eq!(
            (pull.state.as_str(), pull.head.as_str()),
            ("merged", "eu:feat")
        );
        let open = json!({"number": 8, "state": "open", "head": {"ref": "x", "repo": {"owner": {"login": "Time"}}}});
        let pull = PullSummary::from_api(&open, "time");
        assert_eq!(
            (pull.state.as_str(), pull.head.as_str(), pull.draft),
            ("open", "x", false)
        );
    }
}
