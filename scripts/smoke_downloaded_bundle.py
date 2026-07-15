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
    for required in (release_manifest, pin_path, app_manifest):
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
            app_run = run(
                [
                    str(cli),
                    "app",
                    "run",
                    "--manifest",
                    str(app_manifest),
                    "--toolchain-pin",
                    str(pin_path),
                    "--input",
                    str(snap_copy),
                    "--sidecar-dir",
                    str(sidecar),
                    "--toolchain-cache",
                    str(cache),
                    "--mode",
                    "production",
                ]
            )
            must_ok(app_run, "app-run")
            envelope = json.loads(app_run.stdout.strip().splitlines()[-1])
            if envelope.get("outcome") != "ok":
                print(json.dumps(envelope, indent=2), file=sys.stderr)
                return 1
            marker = sidecar / "agentmesh-app-run-marker.txt"
            if not marker.is_file() or "app_run_mode=pinned" not in marker.read_text(encoding="utf-8"):
                print("missing pinned app-run marker", file=sys.stderr)
                return 1
            diags = " ".join(d.get("message", "") for d in envelope.get("diagnostics", []))
            if "app_run_mode=pinned" not in diags:
                print("compact diagnostics missing pinned marker", file=sys.stderr)
                return 1
            print("ok app run pinned", envelope.get("run_id"))

    print("downloaded bundle smoke passed", target)
    return 0


if __name__ == "__main__":
    # Avoid accidental env leakage into plugins during smoke.
    os.environ.pop("AGENTMESH_DEV_PLUGIN", None)
    raise SystemExit(main())
