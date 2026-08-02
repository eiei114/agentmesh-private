#!/usr/bin/env python3
"""Package AgentMesh toolchain bundle for one target (CLI + bundled plugins + docs/apps).

Produces the layout consumed by `agentmesh toolchain install`:

  <out>/
    release-manifest.json          # agentmesh-release-manifest.v0
    bin/agentmesh[.exe]
    bin/agentmesh-multica-selector-shadow[.exe]
    bin/agentmesh-markdown-request-validator[.exe]
    bin/agentmesh-public-0x-readiness-report[.exe]
    bin/agentmesh-public-0x-rollback-replay[.exe]
    apps/<app>/...
    docs/agentmesh-app-v0.md
    docs/public-0x-readiness-gate.md
    docs/public-0x-readiness-report.md
    README.bundle.md

Also writes a Phase-0-compatible flat `manifest.json` (artifact smoke) listing the
binaries at the bundle root copies for CI unzip+run compatibility when requested.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import shutil
import sys
from pathlib import Path


SUPPORTED_TARGETS = (
    "x86_64-pc-windows-msvc",
    "x86_64-unknown-linux-gnu",
    "aarch64-apple-darwin",
)

RELEASE_MANIFEST_SCHEMA = "agentmesh-release-manifest.v0"
PROTOCOL_VERSION = "2026-07-15"


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def exe_suffix(target: str) -> str:
    return ".exe" if "windows" in target else ""


def logical_bin_name(stem: str, target: str) -> str:
    return f"{stem}{exe_suffix(target)}"


def copy_tree(src: Path, dst: Path) -> None:
    if dst.exists():
        shutil.rmtree(dst)
    shutil.copytree(src, dst)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", required=True, help="Output bundle directory")
    parser.add_argument("--target", required=True, choices=SUPPORTED_TARGETS)
    parser.add_argument("--tag", required=True, help="Prerelease tag, e.g. v0.2.0-dev.1")
    parser.add_argument("--commit", required=True, help="Full 40-char commit SHA")
    parser.add_argument(
        "--bin-dir",
        required=True,
        help="Directory containing release binaries (e.g. target/<triple>/release)",
    )
    parser.add_argument(
        "--repo-root",
        default=str(Path(__file__).resolve().parents[1]),
        help="AgentMesh repository root (for apps/docs)",
    )
    parser.add_argument("--protocol-version", default=PROTOCOL_VERSION)
    parser.add_argument("--rust-toolchain", default="1.97.0")
    parser.add_argument(
        "--also-flat-phase0-manifest",
        action="store_true",
        help="Also write Phase-0 manifest.json + flat binary copies for legacy CI smoke",
    )
    parser.add_argument(
        "--include-echo",
        action="store_true",
        help="Also package agentmesh-fixture-echo for Phase-0 host smoke",
    )
    args = parser.parse_args()

    if len(args.commit) != 40 or any(c for c in args.commit if c not in "0123456789abcdef"):
        print("error: --commit must be lowercase 40-char hex SHA", file=sys.stderr)
        return 2

    out = Path(args.out)
    bin_dir = Path(args.bin_dir)
    repo = Path(args.repo_root)
    if out.exists():
        shutil.rmtree(out)
    staged_bin = out / "bin"
    staged_bin.mkdir(parents=True)

    cli_name = logical_bin_name("agentmesh", args.target)
    multica_plugin_name = logical_bin_name("agentmesh-multica-selector-shadow", args.target)
    markdown_plugin_name = logical_bin_name("agentmesh-markdown-request-validator", args.target)
    readiness_report_plugin_name = logical_bin_name(
        "agentmesh-public-0x-readiness-report", args.target
    )
    rollback_replay_plugin_name = logical_bin_name(
        "agentmesh-public-0x-rollback-replay", args.target
    )
    required = [
        cli_name,
        multica_plugin_name,
        markdown_plugin_name,
        readiness_report_plugin_name,
        rollback_replay_plugin_name,
    ]
    if args.include_echo:
        required.append(logical_bin_name("agentmesh-fixture-echo", args.target))

    binaries_meta: dict[str, dict[str, str]] = {}
    for name in required:
        src = bin_dir / name
        if not src.is_file():
            print(f"error: missing binary {src}", file=sys.stderr)
            return 2
        dst = staged_bin / name
        shutil.copy2(src, dst)
        if "windows" not in args.target:
            dst.chmod(0o755)
        logical = name[: -len(exe_suffix(args.target))] if exe_suffix(args.target) else name
        if name.endswith(".exe"):
            logical = name[:-4]
        binaries_meta[logical] = {
            "relative_path": f"bin/{name}",
            "sha256": sha256_file(dst),
        }

    # Apps + docs for dogfood consumers.
    app_src = repo / "apps"
    if app_src.is_dir():
        copy_tree(app_src, out / "apps")
    docs_dst = out / "docs"
    docs_dst.mkdir(parents=True, exist_ok=True)
    for doc_name in [
        "agentmesh-app-v0.md",
        "public-0x-readiness-gate.md",
        "public-0x-readiness-report.md",
    ]:
        docs_src = repo / "docs" / doc_name
        if docs_src.is_file():
            shutil.copy2(docs_src, docs_dst / doc_name)
    snapshot_schema = repo / "schemas" / "backlog-promoter-snapshot-v0.schema.json"
    if snapshot_schema.is_file():
        schemas_dst = out / "schemas"
        schemas_dst.mkdir(parents=True, exist_ok=True)
        shutil.copy2(snapshot_schema, schemas_dst / snapshot_schema.name)

    fixture = (
        repo
        / "plugins"
        / "multica-selector-shadow"
        / "testdata"
        / "one_candidate.snapshot.json"
    )
    if fixture.is_file():
        testdata = out / "testdata"
        testdata.mkdir(parents=True, exist_ok=True)
        shutil.copy2(fixture, testdata / fixture.name)

    release_manifest = {
        "schema_version": RELEASE_MANIFEST_SCHEMA,
        "tag": args.tag,
        "commit_sha": args.commit,
        "target": args.target,
        "protocol_version": args.protocol_version,
        "binaries": binaries_meta,
    }
    release_path = out / "release-manifest.json"
    release_path.write_text(
        json.dumps(release_manifest, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    release_sha = sha256_file(release_path)

    readme = out / "README.bundle.md"
    readme.write_text(
        "\n".join(
            [
                f"# AgentMesh toolchain bundle ({args.target})",
                "",
                f"- tag: `{args.tag}`",
                f"- commit: `{args.commit}`",
                f"- protocol: `{args.protocol_version}`",
                f"- release-manifest sha256: `{release_sha}`",
                "",
                "## Install",
                "",
                "```bash",
                "agentmesh toolchain install --bundle <this-dir> --toolchain-cache ~/.agentmesh/toolchains",
                "```",
                "",
                "Directories under the cache are immutable: existing `<tag>/<target>/` is never overwritten.",
                "",
            ]
        )
        + "\n",
        encoding="utf-8",
    )

    pin_example = out / "pin.generated.toml"
    pin_example.write_text(
        "\n".join(
            [
                'schema_version = "agentmesh-toolchain-pin.v0"',
                f'tag = "{args.tag}"',
                f'commit_sha = "{args.commit}"',
                f'target = "{args.target}"',
                f'release_manifest_sha256 = "{release_sha}"',
                "",
            ]
        ),
        encoding="utf-8",
    )

    if args.also_flat_phase0_manifest:
        # Flat copies so legacy `verify_artifact_manifest.py` + echo smoke can run.
        flat_bins = []
        for name in required:
            shutil.copy2(staged_bin / name, out / name)
            flat_bins.append(
                {
                    "name": name,
                    "sha256": sha256_file(out / name),
                    "size": (out / name).stat().st_size,
                }
            )
        echo_input = repo / "examples" / "echo-input.json"
        if echo_input.is_file() and args.include_echo:
            shutil.copy2(echo_input, out / "echo-input.json")
        phase0 = {
            "schema_version": args.protocol_version,
            "commit": args.commit,
            "protocol_version": args.protocol_version,
            "host_version": "0.1.0",
            "plugin_version": "0.1.0",
            "rust_toolchain": args.rust_toolchain,
            "target": args.target,
            "binaries": flat_bins,
        }
        (out / "manifest.json").write_text(
            json.dumps(phase0, indent=2) + "\n", encoding="utf-8"
        )

    print(f"wrote toolchain bundle {out}")
    print(f"release_manifest_sha256={release_sha}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
