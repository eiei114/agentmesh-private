//! Canonical request adapter evidence digest contract.
//!
//! The digest is intentionally adapter-neutral: request materializers can attach
//! it next to their compact payloads so fixtures can compare evidence parity
//! without duplicating adapter-specific output formatting.

use serde_json::{json, Value};

/// Schema version for the adapter-neutral evidence digest.
pub const EVIDENCE_DIGEST_SCHEMA_VERSION: &str = "agentmesh-adapter-evidence-digest.v0";
/// Stable request schema covered by this digest.
pub const REQUEST_SCHEMA_VERSION: &str = "agentmesh-request.v0";

/// Convert a request title into the bounded ASCII slug shared by local adapters.
///
/// The exact normalization is part of the adapter-neutral identity contract: Unicode
/// case-folding is reduced to ASCII alphanumerics, runs of punctuation become one
/// separator, and the result is capped at 64 bytes.
pub fn request_slug(title: &str) -> String {
    let mut out = String::new();
    let mut previous_dash = false;
    for character in title.chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() {
            out.push(character);
            previous_dash = false;
        } else if !previous_dash && !out.is_empty() {
            out.push('-');
            previous_dash = true;
        }
        if out.len() >= 64 {
            break;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "untitled".to_string()
    } else {
        out
    }
}

/// Stable request fields that request materializers expose as evidence.
#[derive(Debug, Default)]
pub struct RequestEvidenceFields {
    /// Request title.
    pub title: Option<String>,
    /// Stable request kind, such as `app` for App supply or `repair` for maintenance follow-up.
    pub request_kind: Option<String>,
    /// Work item issue type such as `AFK`.
    pub issue_type: Option<String>,
    /// Whether the source request is ready for Multica import/operation.
    pub ready_for_multica: Option<bool>,
    /// Source request status.
    pub status: Option<String>,
    /// Project key that owns the request.
    pub project_key: Option<String>,
    /// Source PRD path.
    pub source_prd: Option<String>,
    /// Source design path.
    pub source_design: Option<String>,
    /// Source roadmap path.
    pub source_roadmap: Option<String>,
    /// Stable blocking dependencies.
    pub blocked_by: Vec<String>,
    /// Stable downstream unblocks.
    pub unblocks: Vec<String>,
    /// Sequence index for ordered request batches.
    pub sequence_index: Option<u64>,
    /// Sequence total for ordered request batches.
    pub sequence_total: Option<u64>,
}

/// Build the canonical adapter-neutral evidence digest.
///
/// Section and field order are part of the contract. Optional scalar fields are
/// serialized as JSON null and dependency fields as arrays, making fixture
/// comparisons deterministic across Markdown, local, and non-Multica adapters.
pub fn adapter_evidence_digest(fields: &RequestEvidenceFields) -> Value {
    json!({
        "schema_version": EVIDENCE_DIGEST_SCHEMA_VERSION,
        "request_schema_version": REQUEST_SCHEMA_VERSION,
        "serialization": {
            "format": "json",
            "object_key_order": "lexicographic",
            "array_order": "contract-defined",
            "null_policy": "missing optional scalar fields serialize as null; dependency fields serialize as arrays"
        },
        "section_order": ["identity", "sources", "routing"],
        "sections": [
            section(
                "identity",
                "Fields that identify the request independently of any tracker adapter.",
                vec![
                    field("title", json!(fields.title), "Primary human-readable request identifier."),
                    field("request_kind", json!(fields.request_kind), "Stable AgentMesh request kind used for adapter routing."),
                    field("issue_type", json!(fields.issue_type), "Work classification retained from the request contract."),
                    field("status", json!(fields.status), "Source lifecycle status before adapter materialization."),
                    field("project_key", json!(fields.project_key), "Owning project key used to compare adapter parity."),
                ],
            ),
            section(
                "sources",
                "Document references that let reviewers trace the request contract.",
                vec![
                    field("source_prd", json!(fields.source_prd), "PRD path captured from stable request metadata."),
                    field("source_design", json!(fields.source_design), "Design document path captured from stable request metadata."),
                    field("source_roadmap", json!(fields.source_roadmap), "Roadmap path captured from stable request metadata."),
                ],
            ),
            section(
                "routing",
                "Adapter-neutral scheduling and dependency facts.",
                vec![
                    field("ready_for_multica", json!(fields.ready_for_multica), "Readiness flag preserved as evidence, not as adapter authority."),
                    field("sequence_index", json!(fields.sequence_index), "1-based order inside a request sequence when provided."),
                    field("sequence_total", json!(fields.sequence_total), "Total requests in the sequence when provided."),
                    field("blocked_by", json!(fields.blocked_by), "Stable upstream dependency identifiers in source order."),
                    field("unblocks", json!(fields.unblocks), "Stable downstream dependency identifiers in source order."),
                ],
            ),
        ],
    })
}

fn section(key: &str, rationale: &str, fields: Vec<Value>) -> Value {
    json!({"key": key, "rationale": rationale, "fields": fields})
}

fn field(key: &str, value: Value, rationale: &str) -> Value {
    json!({"key": key, "value": value, "rationale": rationale})
}

#[cfg(test)]
mod tests {
    use super::request_slug;

    #[test]
    fn request_slug_is_bounded_and_deterministic() {
        assert_eq!(request_slug("---"), "untitled");
        assert_eq!(request_slug("  Repair: AgentMesh!  "), "repair-agentmesh");
        assert_eq!(request_slug(&"a".repeat(80)).len(), 64);
        assert_eq!(request_slug("日本語"), "untitled");
    }
}
