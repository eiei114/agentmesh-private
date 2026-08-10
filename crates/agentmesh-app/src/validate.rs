//! Combined manifest + toolchain pin validation.

use crate::manifest::AppManifest;
use crate::pin::ToolchainPin;
use agentmesh_proto::PROTOCOL_VERSION;
use std::path::Path;
use thiserror::Error;

/// Validation failure categories.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ValidationError {
    /// IO / parse / structural failure.
    #[error("{0}")]
    Invalid(String),
}

/// Successful validation report (secret-free).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationReport {
    /// App name from manifest.
    pub app_name: String,
    /// Logical plugin name.
    pub plugin_logical_name: String,
    /// Pin tag.
    pub pin_tag: String,
    /// Pin commit SHA.
    pub pin_commit_sha: String,
    /// Pin target triple.
    pub pin_target: String,
    /// Host protocol version checked against the app.
    pub protocol_version: String,
}

/// Validate an app manifest together with a version-controlled toolchain pin.
pub fn validate_app_bundle(
    manifest_path: &Path,
    pin_path: &Path,
) -> Result<ValidationReport, ValidationError> {
    let manifest = AppManifest::load(manifest_path).map_err(ValidationError::Invalid)?;
    manifest
        .validate_structure()
        .map_err(ValidationError::Invalid)?;

    let pin = ToolchainPin::load(pin_path).map_err(ValidationError::Invalid)?;
    pin.validate_structure().map_err(ValidationError::Invalid)?;

    if manifest.protocol_version != PROTOCOL_VERSION {
        return Err(ValidationError::Invalid(format!(
            "protocol_version mismatch: app=`{}` host=`{PROTOCOL_VERSION}`",
            manifest.protocol_version
        )));
    }

    // Reject shell-command-like leftover keys by scanning raw TOML again for forbidden keys.
    reject_forbidden_manifest_keys(manifest_path)?;
    reject_forbidden_pin_keys(pin_path)?;

    // If schema refs exist, ensure they resolve under the app root without escape.
    let app_root = manifest_path
        .parent()
        .ok_or_else(|| ValidationError::Invalid("manifest path has no parent".into()))?;
    for (field, rel) in [
        ("schemas.input", manifest.schemas.input.as_deref()),
        ("schemas.output", manifest.schemas.output.as_deref()),
    ] {
        if let Some(rel) = rel {
            let joined = app_root.join(rel);
            let canonical_root =
                std::fs::canonicalize(app_root).unwrap_or_else(|_| app_root.to_path_buf());
            // Soft existence check: warn-as-error for validate when file missing.
            if !joined.exists() {
                return Err(ValidationError::Invalid(format!(
                    "{field} path does not exist: {rel}"
                )));
            }
            let canonical = std::fs::canonicalize(&joined).map_err(|e| {
                ValidationError::Invalid(format!("{field} canonicalize failed: {e}"))
            })?;
            if !canonical.starts_with(&canonical_root) {
                return Err(ValidationError::Invalid(format!(
                    "{field} escapes app root"
                )));
            }
        }
    }

    Ok(ValidationReport {
        app_name: manifest.name,
        plugin_logical_name: manifest.plugin.logical_name,
        pin_tag: pin.tag,
        pin_commit_sha: pin.commit_sha,
        pin_target: pin.target,
        protocol_version: PROTOCOL_VERSION.to_string(),
    })
}

fn reject_forbidden_manifest_keys(path: &Path) -> Result<(), ValidationError> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| ValidationError::Invalid(format!("manifest read error: {e}")))?;
    let value: toml::Value = toml::from_str(&text)
        .map_err(|e| ValidationError::Invalid(format!("manifest parse error: {e}")))?;
    let forbidden = [
        "command",
        "shell",
        "exec",
        "script",
        "run",
        "args",
        "cwd",
        "install_path",
        "plugin_path",
    ];
    scan_forbidden(&value, "", &forbidden)
}

fn reject_forbidden_pin_keys(path: &Path) -> Result<(), ValidationError> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| ValidationError::Invalid(format!("pin read error: {e}")))?;
    let value: toml::Value = toml::from_str(&text)
        .map_err(|e| ValidationError::Invalid(format!("pin parse error: {e}")))?;
    let forbidden = [
        "command",
        "shell",
        "exec",
        "install_path",
        "cache_path",
        "local_path",
        "path",
        "plugin_path",
    ];
    scan_forbidden(&value, "", &forbidden)
}

fn scan_forbidden(
    value: &toml::Value,
    prefix: &str,
    forbidden: &[&str],
) -> Result<(), ValidationError> {
    let Some(table) = value.as_table() else {
        return Ok(());
    };
    for (key, child) in table {
        let path = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        if forbidden.iter().any(|f| key == f) {
            return Err(ValidationError::Invalid(format!(
                "forbidden key `{path}` (shell/path fields are rejected)"
            )));
        }
        scan_forbidden(child, &path, forbidden)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use tempfile::tempdir;

    #[test]
    fn version_controlled_app_manifests_validate() {
        let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let apps_dir = workspace_root.join("apps");
        let example_pin = workspace_root.join("toolchains/agentmesh-pin.v0.example.toml");
        assert!(
            example_pin.is_file(),
            "missing example toolchain pin at {}",
            example_pin.display()
        );

        let mut manifests = Vec::new();
        for entry in std::fs::read_dir(&apps_dir).expect("apps directory") {
            let path = entry.expect("apps entry").path();
            if !path.is_dir() {
                continue;
            }
            let manifest = path.join("agentmesh-app.toml");
            if manifest.is_file() {
                manifests.push(manifest);
            }
        }

        assert!(
            !manifests.is_empty(),
            "expected at least one app manifest under apps/"
        );
        for manifest in manifests {
            let report = validate_app_bundle(&manifest, &example_pin).unwrap_or_else(|e| {
                panic!("validate {}: {e}", manifest.display());
            });
            assert!(
                !report.app_name.is_empty(),
                "app name missing for {}",
                manifest.display()
            );
            assert!(
                !report.plugin_logical_name.is_empty(),
                "plugin logical_name missing for {}",
                manifest.display()
            );
        }
    }

    #[test]
    fn readme_lists_all_version_controlled_apps() {
        let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let readme =
            std::fs::read_to_string(workspace_root.join("README.md")).expect("README.md readable");
        let apps_dir = workspace_root.join("apps");

        let mut listed = 0;
        for entry in std::fs::read_dir(&apps_dir).expect("apps directory") {
            let path = entry.expect("apps entry").path();
            if !path.is_dir() {
                continue;
            }
            let manifest = path.join("agentmesh-app.toml");
            if !manifest.is_file() {
                continue;
            }
            let name = path
                .file_name()
                .expect("app directory name")
                .to_string_lossy();
            let expected = format!("apps/{name}/");
            assert!(
                readme.contains(&expected),
                "README.md missing workspace app entry for `{expected}`"
            );
            listed += 1;
        }

        assert!(listed > 0, "expected at least one app manifest under apps/");
    }

    #[test]
    fn readme_default_members_match_cargo_toml() {
        let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let cargo_toml = std::fs::read_to_string(workspace_root.join("Cargo.toml"))
            .expect("Cargo.toml readable");
        let readme =
            std::fs::read_to_string(workspace_root.join("README.md")).expect("README.md readable");

        let default_members = parse_default_members(&cargo_toml);
        let readme_members = parse_readme_default_members(&readme);

        assert_eq!(
            readme_members, default_members,
            "README Production (`default-members`) must match root Cargo.toml default-members"
        );
    }

    fn parse_default_members(cargo_toml: &str) -> Vec<String> {
        let section = cargo_toml
            .split("default-members")
            .nth(1)
            .and_then(|rest| rest.split(']').next())
            .expect("default-members section");
        let mut members = section
            .split('"')
            .filter(|part| part.starts_with("crates/") || part.starts_with("plugins/"))
            .map(|path| {
                path.rsplit('/')
                    .next()
                    .expect("workspace member path")
                    .to_string()
            })
            .collect::<Vec<_>>();
        members.sort();
        members
    }

    fn parse_readme_default_members(readme: &str) -> Vec<String> {
        let workspace_section = readme
            .split_once("## Workspace crates")
            .map(|(_, section)| section)
            .expect("README.md workspace crates section");
        let production_section = workspace_section
            .split_once("Production (`default-members`):")
            .and_then(|(_, section)| {
                section.split_once("\n\nAdditional production workspace crates")
            })
            .map(|(section, _)| section)
            .or_else(|| {
                workspace_section
                    .split_once("Production (`default-members`):")
                    .and_then(|(_, section)| section.split_once("\n\nApps / packaging"))
                    .map(|(section, _)| section)
            })
            .expect("README.md Production (`default-members`) section");

        let mut members = production_section
            .lines()
            .filter_map(|line| {
                let trimmed = line.trim_start();
                trimmed
                    .strip_prefix("- `agentmesh-")
                    .and_then(|rest| rest.split('`').next())
                    .map(|name| format!("agentmesh-{name}"))
            })
            .collect::<Vec<_>>();
        members.sort();
        members
    }

    #[test]
    fn readme_lists_all_workspace_crates() {
        let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let readme =
            std::fs::read_to_string(workspace_root.join("README.md")).expect("README.md readable");
        let workspace_section = readme
            .split_once("## Workspace crates")
            .map(|(_, section)| {
                section
                    .split_once("\n## ")
                    .map_or(section, |(body, _)| body)
            })
            .expect("README.md workspace crates section");
        let crates_dir = workspace_root.join("crates");

        let mut listed = 0;
        for entry in std::fs::read_dir(&crates_dir).expect("crates directory") {
            let path = entry.expect("crates entry").path();
            if !path.is_dir() {
                continue;
            }
            let manifest = path.join("Cargo.toml");
            if !manifest.is_file() {
                continue;
            }
            let name = path
                .file_name()
                .expect("crate directory name")
                .to_string_lossy();
            let expected = format!("- `{name}`");
            assert!(
                workspace_section
                    .lines()
                    .any(|line| line.trim_start().starts_with(&expected)),
                "README.md missing workspace crate entry for {expected}"
            );
            listed += 1;
        }

        assert!(listed > 0, "expected at least one crate under crates/");
    }

    fn write(dir: &Path, name: &str, body: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn validate_ok_with_schema_ref() {
        let dir = tempdir().unwrap();
        let schema = write(dir.path(), "input.schema.json", "{}\n");
        let _ = schema;
        let manifest = write(
            dir.path(),
            "agentmesh-app.toml",
            r#"
schema_version = "agentmesh-app.v0"
name = "backlog-promoter"
protocol_version = "2026-07-15"

[plugin]
logical_name = "agentmesh-multica-selector-shadow"

[schemas]
input = "input.schema.json"

[conformance]
cargo_package = "agentmesh-multica-selector-shadow"
"#,
        );
        let pin = write(
            dir.path(),
            "pin.toml",
            r#"
schema_version = "agentmesh-toolchain-pin.v0"
tag = "v0.2.0-dev.1"
commit_sha = "376f849893654a8d2de868e79f0c5d0aefb4308c"
target = "x86_64-pc-windows-msvc"
release_manifest_sha256 = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
"#,
        );
        let report = validate_app_bundle(&manifest, &pin).unwrap();
        assert_eq!(report.app_name, "backlog-promoter");
        assert_eq!(report.pin_tag, "v0.2.0-dev.1");
    }

    #[test]
    fn reject_command_key() {
        let dir = tempdir().unwrap();
        let manifest = write(
            dir.path(),
            "agentmesh-app.toml",
            r#"
schema_version = "agentmesh-app.v0"
name = "backlog-promoter"
protocol_version = "2026-07-15"
command = "rm -rf /"

[plugin]
logical_name = "agentmesh-multica-selector-shadow"
"#,
        );
        let pin = write(
            dir.path(),
            "pin.toml",
            r#"
schema_version = "agentmesh-toolchain-pin.v0"
tag = "v0.2.0-dev.1"
commit_sha = "376f849893654a8d2de868e79f0c5d0aefb4308c"
target = "x86_64-pc-windows-msvc"
release_manifest_sha256 = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
"#,
        );
        let err = validate_app_bundle(&manifest, &pin).unwrap_err();
        assert!(err.to_string().contains("command"));
    }
}
