use agentmesh_local_tracker_adapter::adapt_request_input as adapt_local_request_input;
use agentmesh_markdown_request_validator::validate_request_input;
use agentmesh_non_multica_request_adapter::adapt_request_input;
use serde_json::{json, Value};

fn fixed_request_markdown() -> &'static str {
    r#"---
title: "Add adapter evidence digest parity"
ready_for_multica: true
status: ready
project_key: agentmesh-private
issue_type: AFK
request_kind: app
source_prd: "4_Project/OSS/agentmesh-private/Requests/App/2026-07-26-add-an-adapter-evidence-digest-seam.md"
source_design: 4_Project/OSS/agentmesh-private/Docs/agentmesh-request-operations-v1.md
source_roadmap: 4_Project/OSS/agentmesh-private/ROADMAP.md
sequence_index: 1
sequence_total: 1
blocked_by: []
unblocks: []
---
# Add adapter evidence digest parity

## What to build
Build deterministic adapter evidence digest parity.

## Acceptance criteria
- Digest matches across materializers.

## Blocked by
- None.

## User stories covered
- As a maintainer, I can compare adapter evidence.

## Notes
- Tool-neutral.
"#
}

#[test]
fn evidence_digest_matches_across_request_materializers() {
    let markdown = fixed_request_markdown();
    let markdown_output = validate_request_input(&json!({
        "schema_version": "markdown-request-validator-input.v0",
        "markdown": markdown,
    }));
    let non_multica_output = adapt_request_input(&json!({
        "schema_version": "non-multica-request-adapter-input.v0",
        "markdown": markdown,
    }));
    let local_output = adapt_local_request_input(&json!({
        "schema_version": "local-tracker-adapter-input.v0",
        "markdown": markdown,
        "adapter": {"passthrough": {"lane": "parity"}},
    }));

    assert_eq!(markdown_output["valid"], true);
    assert_eq!(non_multica_output["valid"], true);
    assert_eq!(local_output["valid"], true);

    let markdown_digest = &markdown_output["evidence_digest"];
    assert_eq!(markdown_digest, &non_multica_output["evidence_digest"]);
    assert_eq!(markdown_digest, &local_output["evidence_digest"]);
    assert_contract_order(markdown_digest);
}

fn assert_contract_order(digest: &Value) {
    assert_eq!(
        digest["section_order"],
        json!(["identity", "sources", "routing"])
    );
    let sections = digest["sections"].as_array().expect("sections array");
    let actual_sections: Vec<_> = sections
        .iter()
        .map(|section| section["key"].as_str().expect("section key"))
        .collect();
    assert_eq!(actual_sections, ["identity", "sources", "routing"]);

    let actual_fields: Vec<Vec<_>> = sections
        .iter()
        .map(|section| {
            section["fields"]
                .as_array()
                .expect("fields array")
                .iter()
                .map(|field| field["key"].as_str().expect("field key"))
                .collect()
        })
        .collect();
    assert_eq!(
        actual_fields,
        [
            vec![
                "title",
                "request_kind",
                "issue_type",
                "status",
                "project_key"
            ],
            vec!["source_prd", "source_design", "source_roadmap"],
            vec![
                "ready_for_multica",
                "sequence_index",
                "sequence_total",
                "blocked_by",
                "unblocks"
            ],
        ]
    );

    for section in sections {
        assert!(section["rationale"]
            .as_str()
            .is_some_and(|value| !value.is_empty()));
        for field in section["fields"].as_array().expect("fields array") {
            assert!(field["rationale"]
                .as_str()
                .is_some_and(|value| !value.is_empty()));
        }
    }
}
