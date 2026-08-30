#!/usr/bin/env python3
"""Smoke-test a downloaded AgentMesh toolchain bundle (all-target contract).

Validates:
- Phase-0 manifest hashes (when present)
- release-manifest.v0 + pin.generated.toml digest identity
- agentmesh app validate (app manifest + pin)
- agentmesh toolchain install (atomic) + overwrite rejection
- optional agentmesh app run against packaged snapshot fixture (production mode)

Designed for CI matrix jobs and local monitor reruns after unpacking a zip/tar.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path


PRODUCTION_CONTROL_BINARIES = (
    "agentmesh-multica-cli-adapter",
    "agentmesh-local-control-ledger",
    "agentmesh-production-controller-observer",
    "agentmesh-production-authority",
    "agentmesh-production-evaluation-report",
)

PRODUCTION_CONTROL_APPS = (
    "multica-cli-adapter",
    "local-control-ledger",
    "production-controller-observer",
    "production-authority",
    "production-evaluation-report",
)


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def run(cmd: list[str], *, cwd: Path | None = None) -> subprocess.CompletedProcess[str]:
    print("+", " ".join(cmd), flush=True)
    return subprocess.run(cmd, cwd=cwd, text=True, capture_output=True, check=False)


def must_ok(proc: subprocess.CompletedProcess[str], label: str) -> None:
    if proc.returncode != 0:
        sys.stderr.write(proc.stdout)
        sys.stderr.write(proc.stderr)
        raise SystemExit(f"FAIL {label}: rc={proc.returncode}")


def write_json(path: Path, value: dict[str, object]) -> None:
    path.write_text(
        json.dumps(value, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )


def run_pinned_app(
    *,
    cli: Path,
    manifest: Path,
    pin_path: Path,
    input_path: Path,
    sidecar: Path,
    cache: Path,
    label: str,
) -> dict[str, object]:
    proc = run(
        [
            str(cli),
            "app",
            "run",
            "--manifest",
            str(manifest),
            "--toolchain-pin",
            str(pin_path),
            "--input",
            str(input_path),
            "--sidecar-dir",
            str(sidecar),
            "--toolchain-cache",
            str(cache),
            "--mode",
            "production",
        ]
    )
    must_ok(proc, label)
    try:
        envelope = json.loads(proc.stdout.strip().splitlines()[-1])
    except (IndexError, json.JSONDecodeError) as exc:
        raise SystemExit(f"FAIL {label}: invalid host JSON envelope") from exc
    if not isinstance(envelope, dict) or envelope.get("outcome") != "ok":
        print(json.dumps(envelope, indent=2, sort_keys=True), file=sys.stderr)
        raise SystemExit(f"FAIL {label}: host outcome is not ok")
    payload = envelope.get("payload")
    if not isinstance(payload, dict):
        raise SystemExit(f"FAIL {label}: host payload missing")
    return envelope


def assert_payload(
    envelope: dict[str, object],
    *,
    label: str,
    exit_reason: str,
) -> dict[str, object]:
    payload = envelope["payload"]
    if not isinstance(payload, dict):
        raise SystemExit(f"FAIL {label}: host payload missing")
    if payload.get("valid") is not True or payload.get("exit_reason") != exit_reason:
        print(json.dumps(payload, indent=2, sort_keys=True), file=sys.stderr)
        raise SystemExit(f"FAIL {label}: compact plugin result invalid")
    return payload


def find_cli(bundle: Path, target: str) -> Path:
    name = "agentmesh.exe" if "windows" in target else "agentmesh"
    # Prefer flat Phase-0 copy, else bin/
    for candidate in (bundle / name, bundle / "bin" / name):
        if candidate.is_file():
            return candidate.resolve()
    raise SystemExit(f"missing CLI binary {name} under {bundle}")


def load_pin(path: Path) -> dict[str, str]:
    data: dict[str, str] = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        data[key.strip()] = value.strip().strip('"')
    return data


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bundle", required=True, help="Unpacked bundle directory")
    parser.add_argument("--target", required=True)
    parser.add_argument(
        "--skip-app-run",
        action="store_true",
        help="Skip production app run (install/validate only)",
    )
    parser.add_argument(
        "--snapshot",
        default="",
        help="Optional snapshot JSON override (default: bundle testdata or repo fixture)",
    )
    args = parser.parse_args()

    bundle = Path(args.bundle).resolve()
    target = args.target
    if not bundle.is_dir():
        print(f"bundle not found: {bundle}", file=sys.stderr)
        return 2

    release_manifest = bundle / "release-manifest.json"
    pin_path = bundle / "pin.generated.toml"
    app_manifest = bundle / "apps" / "backlog-promoter" / "agentmesh-app.toml"
    production_manifests = [bundle / "apps" / name / "agentmesh-app.toml" for name in PRODUCTION_CONTROL_APPS]
    production_assets = [
        bundle / "docs" / "local-production-control-v0.md",
        bundle / "scripts" / "task-scheduler" / "install-production-controller.ps1",
        bundle / "scripts" / "task-scheduler" / "run-production-controller.ps1",
        bundle / "scripts" / "task-scheduler" / "rollback-production-controller.ps1",
        bundle / "scripts" / "task-scheduler" / "rollback-ledger-parse.ps1",
        bundle / "scripts" / "task-scheduler" / "uninstall-production-controller.ps1",
    ]
    for required in (
        release_manifest,
        pin_path,
        app_manifest,
        *production_manifests,
        *production_assets,
    ):
        if not required.is_file():
            print(f"missing required artifact: {required}", file=sys.stderr)
            return 2

    phase0 = bundle / "manifest.json"
    if phase0.is_file():
        verify = Path(__file__).resolve().parent / "verify_artifact_manifest.py"
        must_ok(run([sys.executable, str(verify), str(phase0)]), "phase0-manifest-verify")

    release = json.loads(release_manifest.read_text(encoding="utf-8"))
    if release.get("schema_version") != "agentmesh-release-manifest.v0":
        print("unexpected release-manifest schema_version", file=sys.stderr)
        return 1
    if release.get("target") != target:
        print(
            f"target mismatch: manifest={release.get('target')} expected={target}",
            file=sys.stderr,
        )
        return 1
    binaries = release.get("binaries")
    if not isinstance(binaries, dict):
        print("release-manifest binaries must be an object", file=sys.stderr)
        return 1
    missing_production = [name for name in PRODUCTION_CONTROL_BINARIES if name not in binaries]
    if missing_production:
        print(f"production-control binaries missing: {missing_production}", file=sys.stderr)
        return 1

    pin = load_pin(pin_path)
    digest = sha256_file(release_manifest)
    if pin.get("release_manifest_sha256") != digest:
        print(
            "pin release_manifest_sha256 does not match release-manifest.json",
            file=sys.stderr,
        )
        return 1
    if pin.get("tag") != release.get("tag") or pin.get("commit_sha") != release.get("commit_sha"):
        print("pin tag/commit mismatch vs release-manifest", file=sys.stderr)
        return 1
    if pin.get("target") != target:
        print("pin target mismatch", file=sys.stderr)
        return 1
    print(f"ok release-manifest digest {digest}")

    cli = find_cli(bundle, target)
    if "windows" not in target:
        cli.chmod(cli.stat().st_mode | 0o111)
        bin_dir = bundle / "bin"
        if bin_dir.is_dir():
            for path in bin_dir.iterdir():
                if path.is_file():
                    path.chmod(path.stat().st_mode | 0o111)

    must_ok(
        run(
            [
                str(cli),
                "app",
                "validate",
                "--manifest",
                str(app_manifest),
                "--toolchain-pin",
                str(pin_path),
            ]
        ),
        "app-validate",
    )
    print("ok app validate")
    for production_manifest in production_manifests:
        must_ok(
            run(
                [
                    str(cli),
                    "app",
                    "validate",
                    "--manifest",
                    str(production_manifest),
                    "--toolchain-pin",
                    str(pin_path),
                ]
            ),
            f"app-validate-{production_manifest.parent.name}",
        )
    print("ok production-control app validate")

    with tempfile.TemporaryDirectory(prefix="agentmesh-smoke-") as tmp:
        tmp_path = Path(tmp)
        cache = tmp_path / "cache"
        sidecar = tmp_path / "sidecars"
        cache.mkdir()
        sidecar.mkdir()

        install = run(
            [
                str(cli),
                "toolchain",
                "install",
                "--bundle",
                str(bundle),
                "--toolchain-cache",
                str(cache),
                "--json",
            ]
        )
        must_ok(install, "toolchain-install")
        report = json.loads(install.stdout.strip().splitlines()[-1])
        if not report.get("ok"):
            print(report, file=sys.stderr)
            return 1
        print("ok toolchain install", report.get("install_dir"))

        second = run(
            [
                str(cli),
                "toolchain",
                "install",
                "--bundle",
                str(bundle),
                "--toolchain-cache",
                str(cache),
            ]
        )
        if second.returncode == 0:
            print("expected immutable overwrite rejection", file=sys.stderr)
            return 1
        print("ok overwrite rejected")

        if not args.skip_app_run:
            snapshot = Path(args.snapshot) if args.snapshot else None
            if snapshot is None:
                candidates = [
                    bundle / "testdata" / "one_candidate.snapshot.json",
                    Path(__file__).resolve().parents[1]
                    / "plugins"
                    / "multica-selector-shadow"
                    / "testdata"
                    / "one_candidate.snapshot.json",
                ]
                snapshot = next((p for p in candidates if p.is_file()), None)
            if snapshot is None:
                print("snapshot fixture missing; pass --snapshot or package testdata", file=sys.stderr)
                return 2
            # Copy snapshot into temp to avoid writing beside readonly installs.
            snap_copy = tmp_path / "input.snapshot.json"
            shutil.copy2(snapshot, snap_copy)
            envelope = run_pinned_app(
                cli=cli,
                manifest=app_manifest,
                pin_path=pin_path,
                input_path=snap_copy,
                sidecar=sidecar,
                cache=cache,
                label="app-run",
            )
            marker = sidecar / "agentmesh-app-run-marker.txt"
            if not marker.is_file() or "app_run_mode=pinned" not in marker.read_text(encoding="utf-8"):
                print("missing pinned app-run marker", file=sys.stderr)
                return 1
            diags = " ".join(d.get("message", "") for d in envelope.get("diagnostics", []))
            if "app_run_mode=pinned" not in diags:
                print("compact diagnostics missing pinned marker", file=sys.stderr)
                return 1
            print("ok app run pinned", envelope.get("run_id"))

            required_home_env = "USERPROFILE" if "windows" in target else "HOME"
            fake_query = (
                "import json,os,sys;"
                f"sys.exit(1) if not os.getenv('{required_home_env}') else "
                "print(json.dumps({'issues':[],'has_more':False,'limit':0,"
                "'offset':0,'total':0}))"
            )
            adapter_input = tmp_path / "multica-cli-adapter-input.json"
            write_json(
                adapter_input,
                {
                    "schema_version": "multica-cli-adapter-input.v0",
                    "operation": "query",
                    "cli_path": str(Path(sys.executable).resolve()),
                    "prefix_args": ["-c", fake_query, "--"],
                    # Exercise the maximum configured CLI timeout through the
                    # actual 120s App host while the fixture exits immediately.
                    "timeout_ms": 85000,
                },
            )
            adapter_envelope = run_pinned_app(
                cli=cli,
                manifest=bundle / "apps" / "multica-cli-adapter" / "agentmesh-app.toml",
                pin_path=pin_path,
                input_path=adapter_input,
                sidecar=sidecar,
                cache=cache,
                label="multica-cli-adapter-run",
            )
            adapter_payload = assert_payload(
                adapter_envelope,
                label="multica-cli-adapter-run",
                exit_reason="query_ok",
            )
            if adapter_payload.get("json_top_level_kind") != "object":
                print("adapter smoke did not return object JSON", file=sys.stderr)
                return 1
            print("ok Multica CLI adapter pinned", adapter_envelope.get("run_id"))

            ledger_path = tmp_path / "production-control-smoke.db"
            ledger_input = tmp_path / "local-control-ledger-input.json"
            write_json(
                ledger_input,
                {
                    "schema_version": "local-control-ledger-input.v0",
                    "operation": "init",
                    "ledger_path": str(ledger_path),
                },
            )
            ledger_envelope = run_pinned_app(
                cli=cli,
                manifest=bundle / "apps" / "local-control-ledger" / "agentmesh-app.toml",
                pin_path=pin_path,
                input_path=ledger_input,
                sidecar=sidecar,
                cache=cache,
                label="production-control-ledger-run",
            )
            ledger_payload = assert_payload(
                ledger_envelope,
                label="production-control-ledger-run",
                exit_reason="ok",
            )
            ledger_data = ledger_payload.get("data")
            if not isinstance(ledger_data, dict) or ledger_data.get("initialized") is not True:
                print("ledger smoke did not initialize database", file=sys.stderr)
                return 1
            print("ok production-control ledger pinned", ledger_envelope.get("run_id"))

            window = {
                "decision_count": 100,
                "token_baseline": 1000.0,
                "token_current": 700.0,
                "failure_rate_baseline_pct": 5.0,
                "failure_rate_current_pct": 6.0,
                "throughput_baseline": 100.0,
                "throughput_current": 95.0,
                "attribution_coverage_pct": 95.0,
                "duplicate_count": 0,
                "unauthorized_count": 0,
            }
            evaluation_input = tmp_path / "production-evaluation-input.json"
            write_json(
                evaluation_input,
                {
                    "schema_version": "production-evaluation-report-input.v0",
                    "operation": "evaluate",
                    "controller_id": "bundle_smoke",
                    "rollback_window": {"window_days": 7, **window},
                    "result_window": {"window_days": 30, **window},
                },
            )
            evaluation_envelope = run_pinned_app(
                cli=cli,
                manifest=bundle
                / "apps"
                / "production-evaluation-report"
                / "agentmesh-app.toml",
                pin_path=pin_path,
                input_path=evaluation_input,
                sidecar=sidecar,
                cache=cache,
                label="production-evaluation-run",
            )
            evaluation_payload = assert_payload(
                evaluation_envelope,
                label="production-evaluation-run",
                exit_reason="evaluation_pass",
            )
            report = evaluation_payload.get("report")
            if not isinstance(report, dict) or report.get("overall_pass") is not True:
                print("evaluation smoke did not pass synthetic gates", file=sys.stderr)
                return 1
            print("ok production evaluation pinned", evaluation_envelope.get("run_id"))

            observer_input = tmp_path / "production-observer-input.json"
            write_json(
                observer_input,
                {
                    "schema_version": "production-controller-observer-input.v0",
                    "operation": "run_once",
                    "controller_id": "bundle_smoke_observer",
                    "authority_mode": "observer",
                    "ledger_path": str(ledger_path),
                    "cli_path": str(Path(sys.executable).resolve()),
                    "now": "2026-08-30T00:00:00Z",
                    "occurrence_id": "bundle-smoke-occurrence",
                    "scope_key": "bundle_smoke_observer",
                    "prefix_args": ["-c", fake_query, "--"],
                    "lease_ttl_seconds": 30,
                    "cli_timeout_ms": 10000,
                },
            )
            observer_envelope = run_pinned_app(
                cli=cli,
                manifest=bundle
                / "apps"
                / "production-controller-observer"
                / "agentmesh-app.toml",
                pin_path=pin_path,
                input_path=observer_input,
                sidecar=sidecar,
                cache=cache,
                label="production-observer-run",
            )
            observer_payload = assert_payload(
                observer_envelope,
                label="production-observer-run",
                exit_reason="observer_success_no_mutation",
            )
            cli_summary = observer_payload.get("cli")
            if (
                observer_payload.get("mutation_performed") is not False
                or not isinstance(cli_summary, dict)
                or cli_summary.get("valid") is not True
                or cli_summary.get("json_top_level_kind") != "object"
            ):
                print(json.dumps(observer_payload, indent=2, sort_keys=True), file=sys.stderr)
                return 1
            print("ok production observer pinned", observer_envelope.get("run_id"))

            authority_input = tmp_path / "production-authority-input.json"
            write_json(
                authority_input,
                {
                    "schema_version": "production-authority-input.v0",
                    "operation": "run_once",
                    "controller_id": "bundle_smoke_authority",
                    "execution_kind": "shadow",
                    "authority_mode": "observer",
                    "ledger_path": str(ledger_path),
                    "cli_path": str(Path(sys.executable).resolve()),
                    "now": "2026-08-30T00:01:00Z",
                    "scope_key": "bundle_smoke_authority",
                    "lease_id": "lease-bundle-smoke-authority",
                    "lease_ttl_seconds": 30,
                    "cli_timeout_ms": 10000,
                },
            )
            authority_envelope = run_pinned_app(
                cli=cli,
                manifest=bundle / "apps" / "production-authority" / "agentmesh-app.toml",
                pin_path=pin_path,
                input_path=authority_input,
                sidecar=sidecar,
                cache=cache,
                label="production-authority-run",
            )
            authority_payload = assert_payload(
                authority_envelope,
                label="production-authority-run",
                exit_reason="observer_success_no_mutation",
            )
            if authority_payload.get("mutation_performed") is not False:
                print("authority smoke performed a mutation", file=sys.stderr)
                return 1
            print("ok production authority pinned", authority_envelope.get("run_id"))

    print("downloaded bundle smoke passed", target)
    return 0


if __name__ == "__main__":
    # Avoid accidental env leakage into plugins during smoke.
    os.environ.pop("AGENTMESH_DEV_PLUGIN", None)
    raise SystemExit(main())
