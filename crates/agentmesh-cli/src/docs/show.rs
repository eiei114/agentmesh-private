//! `agentmesh docs show <name>` JSON contract.

use super::EMBEDDED_DOCS;
use serde::Serialize;
use std::process::ExitCode;

const SHOW_SCHEMA_VERSION: &str = "agentmesh-docs-show.v0";
const ERROR_SCHEMA_VERSION: &str = "agentmesh-docs-error.v0";

#[derive(Debug, Serialize)]
struct DocsShowOutput<'a> {
    schema_version: &'a str,
    name: &'a str,
    description: &'a str,
    source: &'a str,
    content: &'a str,
}

#[derive(Debug, Serialize)]
struct DocsError<'a> {
    code: &'a str,
    message: String,
    name: &'a str,
}

#[derive(Debug, Serialize)]
struct DocsErrorOutput<'a> {
    schema_version: &'a str,
    error: DocsError<'a>,
}

/// Emit one embedded document or the deterministic not-found contract.
pub fn docs_show_command(name: &str) -> ExitCode {
    let (payload, code) = render_docs_show_json(name);
    println!("{payload}");
    code
}

fn render_docs_show_json(name: &str) -> (String, ExitCode) {
    if let Some(document) = EMBEDDED_DOCS.iter().find(|document| document.name == name) {
        let payload = DocsShowOutput {
            schema_version: SHOW_SCHEMA_VERSION,
            name: document.name,
            description: document.description,
            source: document.source,
            content: document.content,
        };
        return (
            serde_json::to_string(&payload).expect("serialize docs show JSON"),
            ExitCode::SUCCESS,
        );
    }

    let payload = DocsErrorOutput {
        schema_version: ERROR_SCHEMA_VERSION,
        error: DocsError {
            code: "document_not_found",
            message: format!("Unknown document name: {name}"),
            name,
        },
    };
    (
        serde_json::to_string(&payload).expect("serialize docs error JSON"),
        ExitCode::from(2),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn show_returns_exact_embedded_content() {
        let document = &EMBEDDED_DOCS[0];
        let (json, code) = render_docs_show_json(document.name);
        let payload: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(code, ExitCode::SUCCESS);
        assert_eq!(payload["content"], document.content);
        assert_eq!(payload["source"], document.source);
    }

    #[test]
    fn lookup_is_exact_and_never_path_like() {
        for name in [
            "PROTOCOL-V0",
            "../docs/protocol-v0.md",
            "docs/protocol-v0.md",
        ] {
            let (json, code) = render_docs_show_json(name);
            let payload: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
            assert_eq!(code, ExitCode::from(2));
            assert_eq!(payload["schema_version"], ERROR_SCHEMA_VERSION);
            assert_eq!(payload["error"]["code"], "document_not_found");
            assert_eq!(payload["error"]["name"], name);
        }
    }

    #[test]
    fn unicode_and_multiline_content_serialize_without_truncation() {
        let content = "# 日本語\n\nline one\nline two\n";
        let payload = DocsShowOutput {
            schema_version: SHOW_SCHEMA_VERSION,
            name: "unicode",
            description: "説明",
            source: "docs/unicode.md",
            content,
        };
        let json = serde_json::to_string(&payload).expect("serialize Unicode Markdown");
        let decoded: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(decoded["description"], "説明");
        assert_eq!(decoded["content"], content);
    }

    #[test]
    fn empty_content_serializes_as_an_empty_json_string() {
        let payload = DocsShowOutput {
            schema_version: SHOW_SCHEMA_VERSION,
            name: "empty",
            description: "empty fixture",
            source: "docs/empty.md",
            content: "",
        };
        let json = serde_json::to_string(&payload).expect("serialize empty Markdown");
        let decoded: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(decoded["content"], "");
    }
}
