//! Any Decision Record lifecycle metadata shared by source and graph readers.
//!
//! Decision records are caller-owned Markdown.  AgentMesh only normalizes the
//! bounded frontmatter it reads; it never writes a promoted status back to the
//! canonical source.

use serde_yaml::{Mapping, Value};
use std::collections::BTreeMap;
use thiserror::Error;

/// Lifecycle states for an Any Decision Record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DecisionStatus {
    /// Ambiguous AI inference awaiting optional review.
    Candidate,
    /// Explicitly captured current decision.
    Adopted,
    /// Explicitly not selected.
    Rejected,
    /// Explicitly waiting for a later decision.
    Deferred,
    /// Replaced by a later record.
    Superseded,
}

impl DecisionStatus {
    /// Parse the canonical status spelling and supported legacy aliases.
    pub fn parse(value: &str) -> Result<Self, DecisionMetadataError> {
        match normalize_token(value).as_str() {
            "candidate" | "ready" | "proposed" | "draft" => Ok(Self::Candidate),
            "adopted" | "approved" | "accepted" => Ok(Self::Adopted),
            "rejected" => Ok(Self::Rejected),
            "deferred" => Ok(Self::Deferred),
            "superseded" => Ok(Self::Superseded),
            other => Err(DecisionMetadataError::Invalid(format!(
                "invalid decision status: {other}"
            ))),
        }
    }

    /// Canonical serialized spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Candidate => "candidate",
            Self::Adopted => "adopted",
            Self::Rejected => "rejected",
            Self::Deferred => "deferred",
            Self::Superseded => "superseded",
        }
    }
}

/// Retrieval scope for Decision evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecisionScope {
    /// Current adopted decisions only.
    Current,
    /// Candidate and deferred decisions for optional review.
    Review,
    /// All lifecycle states, including rejected and superseded history.
    Historical,
}

impl DecisionScope {
    /// Parse the CLI/contract spelling.
    pub fn parse(value: &str) -> Result<Self, DecisionMetadataError> {
        match normalize_token(value).as_str() {
            "current" => Ok(Self::Current),
            "review" => Ok(Self::Review),
            "historical" => Ok(Self::Historical),
            other => Err(DecisionMetadataError::Invalid(format!(
                "invalid decision scope: {other}"
            ))),
        }
    }

    /// Canonical serialized spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Review => "review",
            Self::Historical => "historical",
        }
    }

    /// Whether a status participates in this scope.
    pub const fn allows(self, status: DecisionStatus) -> bool {
        match self {
            Self::Current => matches!(status, DecisionStatus::Adopted),
            Self::Review => matches!(status, DecisionStatus::Candidate | DecisionStatus::Deferred),
            Self::Historical => true,
        }
    }
}

/// Normalized Any Decision Record metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionMetadata {
    /// Normalized lifecycle status.
    pub decision_status: DecisionStatus,
    /// Optional controlled decision kind.
    pub decision_kind: Option<String>,
    /// Actor that recorded the decision.
    pub recorded_by: String,
    /// Human review state, independent of adoption.
    pub review_status: String,
    /// How adoption was obtained.
    pub adoption_mode: String,
    /// Execution-risk classification.
    pub impact: String,
    /// Source links supplied by the recorder.
    pub source_refs: Vec<String>,
    /// Older records replaced by this record.
    pub supersedes: Vec<String>,
}

/// Errors from bounded Decision Record metadata parsing.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum DecisionMetadataError {
    /// Frontmatter is present but malformed.
    #[error("invalid decision frontmatter: {0}")]
    Frontmatter(String),
    /// A lifecycle field violates the contract.
    #[error("invalid decision metadata: {0}")]
    Invalid(String),
}

const DECISION_KINDS: &[&str] = &[
    "technical",
    "product",
    "process",
    "tool",
    "naming",
    "non_goal",
];
const RECORDED_BY: &[&str] = &["ai", "human", "system"];
const REVIEW_STATUSES: &[&str] = &["unreviewed", "reviewed", "corrected"];
const ADOPTION_MODES: &[&str] = &["auto", "human", "candidate"];
const IMPACTS: &[&str] = &[
    "normal",
    "security",
    "legal",
    "paid",
    "production",
    "irreversible",
];

/// Parse a Markdown YAML frontmatter mapping.
pub fn parse_frontmatter(text: &str) -> Result<BTreeMap<String, Value>, DecisionMetadataError> {
    let normalized = text.replace("\r\n", "\n");
    let normalized = normalized.strip_prefix('\u{feff}').unwrap_or(&normalized);
    let Some(rest) = normalized.strip_prefix("---\n") else {
        return Ok(BTreeMap::new());
    };
    let Some(end) = rest.find("\n---\n") else {
        return Err(DecisionMetadataError::Frontmatter(
            "closing YAML fence is missing".into(),
        ));
    };
    let mapping: Mapping = serde_yaml::from_str(&rest[..end])
        .map_err(|error| DecisionMetadataError::Frontmatter(error.to_string()))?;
    let mut output = BTreeMap::new();
    for (key, value) in mapping {
        let Value::String(key) = key else {
            return Err(DecisionMetadataError::Frontmatter(
                "frontmatter keys must be strings".into(),
            ));
        };
        output.insert(key.to_ascii_lowercase(), value);
    }
    Ok(output)
}

/// Normalize and validate Decision Record frontmatter.
pub fn normalize_decision_metadata(
    frontmatter: &BTreeMap<String, Value>,
) -> Result<DecisionMetadata, DecisionMetadataError> {
    normalize_decision_metadata_inner(frontmatter, true)
}

/// Normalize metadata already copied into a derived graph node.
///
/// Graph nodes retain lifecycle/provenance actor fields, but older exporters
/// do not copy the complete `source_refs` list. Canonical Markdown remains the
/// authority for that required AI provenance, so graph validation must not
/// reject such a node before the source is reread.
pub fn normalize_decision_metadata_for_graph(
    frontmatter: &BTreeMap<String, Value>,
) -> Result<DecisionMetadata, DecisionMetadataError> {
    normalize_decision_metadata_inner(frontmatter, false)
}

fn normalize_decision_metadata_inner(
    frontmatter: &BTreeMap<String, Value>,
    require_source_refs: bool,
) -> Result<DecisionMetadata, DecisionMetadataError> {
    let decision_status = match non_empty(frontmatter, "decision_status") {
        Some(value) => DecisionStatus::parse(&value)?,
        None => match non_empty(frontmatter, "status") {
            Some(value) => DecisionStatus::parse(&value).unwrap_or(DecisionStatus::Adopted),
            None => DecisionStatus::Adopted,
        },
    };

    let decision_kind = non_empty(frontmatter, "decision_kind");
    if let Some(kind) = decision_kind.as_deref() {
        if !DECISION_KINDS.contains(&kind) {
            return Err(DecisionMetadataError::Invalid(format!(
                "invalid decision_kind: {kind}"
            )));
        }
    }

    let recorded_by = non_empty(frontmatter, "recorded_by").unwrap_or_else(|| "human".into());
    if !RECORDED_BY.contains(&recorded_by.as_str()) {
        return Err(DecisionMetadataError::Invalid(format!(
            "invalid recorded_by: {recorded_by}"
        )));
    }
    let review_status = non_empty(frontmatter, "review_status").unwrap_or_else(|| {
        if matches!(recorded_by.as_str(), "ai" | "system") {
            "unreviewed".into()
        } else {
            "reviewed".into()
        }
    });
    if !REVIEW_STATUSES.contains(&review_status.as_str()) {
        return Err(DecisionMetadataError::Invalid(format!(
            "invalid review_status: {review_status}"
        )));
    }

    let adoption_mode = non_empty(frontmatter, "adoption_mode").unwrap_or_else(|| {
        if decision_status == DecisionStatus::Candidate {
            "candidate".into()
        } else if matches!(recorded_by.as_str(), "ai" | "system") {
            "auto".into()
        } else {
            "human".into()
        }
    });
    if !ADOPTION_MODES.contains(&adoption_mode.as_str()) {
        return Err(DecisionMetadataError::Invalid(format!(
            "invalid adoption_mode: {adoption_mode}"
        )));
    }
    if decision_status == DecisionStatus::Candidate && adoption_mode != "candidate" {
        return Err(DecisionMetadataError::Invalid(
            "candidate Decision Record must use adoption_mode: candidate".into(),
        ));
    }
    if decision_status == DecisionStatus::Adopted && adoption_mode == "candidate" {
        return Err(DecisionMetadataError::Invalid(
            "adopted Decision Record cannot use adoption_mode: candidate".into(),
        ));
    }

    let impact = non_empty(frontmatter, "impact").unwrap_or_else(|| "normal".into());
    if !IMPACTS.contains(&impact.as_str()) {
        return Err(DecisionMetadataError::Invalid(format!(
            "invalid impact: {impact}"
        )));
    }
    let source_refs = string_list(frontmatter.get("source_refs"));
    if require_source_refs && recorded_by == "ai" && source_refs.is_empty() {
        return Err(DecisionMetadataError::Invalid(
            "AI Decision Record requires source_refs".into(),
        ));
    }

    Ok(DecisionMetadata {
        decision_status,
        decision_kind,
        recorded_by,
        review_status,
        adoption_mode,
        impact,
        source_refs,
        supersedes: string_list(frontmatter.get("supersedes")),
    })
}

fn non_empty(frontmatter: &BTreeMap<String, Value>, key: &str) -> Option<String> {
    scalar_text(frontmatter.get(key)).map(|value| normalize_token(&value))
}

fn scalar_text(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(value) => (!value.trim().is_empty()).then(|| value.trim().to_owned()),
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        Value::Null | Value::Sequence(_) | Value::Mapping(_) => None,
        Value::Tagged(value) => scalar_text(Some(&value.value)),
    }
}

fn string_list(value: Option<&Value>) -> Vec<String> {
    let values = match value {
        Some(Value::Sequence(values)) => values.iter().collect::<Vec<_>>(),
        Some(value) => vec![value],
        None => Vec::new(),
    };
    values
        .into_iter()
        .filter_map(|value| match value {
            Value::Mapping(mapping) => ["value", "target", "path"]
                .iter()
                .find_map(|key| mapping.get(Value::String((*key).to_owned())))
                .and_then(|value| scalar_text(Some(value))),
            _ => scalar_text(Some(value)),
        })
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .collect()
}

fn normalize_token(value: &str) -> String {
    value.trim().to_ascii_lowercase().replace(['-', ' '], "_")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_ai_decision_is_adopted_without_review_gate() {
        let frontmatter = parse_frontmatter(
            "---\ntype: Decision\ndecision_status: adopted\ndecision_kind: technical\nrecorded_by: ai\nsource_refs:\n  - plan.md\n---\n# Decision\n",
        )
        .unwrap();
        let metadata = normalize_decision_metadata(&frontmatter).unwrap();

        assert_eq!(metadata.decision_status, DecisionStatus::Adopted);
        assert_eq!(metadata.review_status, "unreviewed");
        assert_eq!(metadata.adoption_mode, "auto");
        assert_eq!(metadata.source_refs, ["plan.md"]);
        assert!(DecisionScope::Current.allows(metadata.decision_status));
    }

    #[test]
    fn ambiguous_ai_decision_stays_candidate_and_is_review_only() {
        let frontmatter = parse_frontmatter(
            "---\ndecision_status: candidate\nrecorded_by: ai\nsource_refs: [chat.md]\n---\n",
        )
        .unwrap();
        let metadata = normalize_decision_metadata(&frontmatter).unwrap();

        assert_eq!(metadata.decision_status, DecisionStatus::Candidate);
        assert_eq!(metadata.adoption_mode, "candidate");
        assert!(!DecisionScope::Current.allows(metadata.decision_status));
        assert!(DecisionScope::Review.allows(metadata.decision_status));
    }

    #[test]
    fn legacy_status_defaults_to_adopted_but_explicit_unknown_lifecycle_is_invalid() {
        let legacy = parse_frontmatter("---\nstatus: accepted\n---\n").unwrap();
        assert_eq!(
            normalize_decision_metadata(&legacy)
                .unwrap()
                .decision_status,
            DecisionStatus::Adopted
        );

        let invalid = parse_frontmatter("---\ndecision_status: mystery\n---\n").unwrap();
        assert!(normalize_decision_metadata(&invalid).is_err());
    }

    #[test]
    fn malformed_ai_record_without_source_refs_is_rejected() {
        let frontmatter = parse_frontmatter("---\nrecorded_by: ai\n---\n").unwrap();
        let error = normalize_decision_metadata(&frontmatter).unwrap_err();
        assert!(error.to_string().contains("source_refs"));
    }

    #[test]
    fn supersedes_and_scope_statuses_are_deterministic() {
        let frontmatter = parse_frontmatter(
            "---\ndecision_status: adopted\nsupersedes:\n  - old.md\n  - value: older.md\n---\n",
        )
        .unwrap();
        let metadata = normalize_decision_metadata(&frontmatter).unwrap();
        assert_eq!(metadata.supersedes, ["old.md", "older.md"]);
        assert!(DecisionScope::Historical.allows(DecisionStatus::Superseded));
        assert!(!DecisionScope::Review.allows(DecisionStatus::Rejected));
    }
}
