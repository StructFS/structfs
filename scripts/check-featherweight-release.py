#!/usr/bin/env python3
"""Check actual Cargo archives with an independent consumer. Never publishes.
Requires Python 3.11+, cached Cargo dependencies, and wasm32-unknown-unknown.
Run from any directory: python3.12 scripts/check-featherweight-release.py
"""
from pathlib import Path
import json
import os
import shutil
import subprocess
import tarfile
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parent.parent

def run(args, cwd=ROOT, **kwargs):
    print("+ " + " ".join(map(str, args)), flush=True)
    return subprocess.run(args, cwd=cwd, check=True, **kwargs)

# Cross-host fixtures must be packaged without drifting from the browser copy.
fixtures = ROOT / "featherweight/runtime/tests/fixtures"
for source in (ROOT / "featherweight/host/browser/test/fixtures").iterdir():
    assert (fixtures / source.name).read_bytes() == source.read_bytes(), source.name

metadata = json.loads(subprocess.check_output(
    ["cargo", "metadata", "--no-deps", "--format-version=1", "--offline"], cwd=ROOT))
versions = {p["name"]: p["version"] for p in metadata["packages"]}
PACKAGES = [p["name"] for p in metadata["packages"] if p.get("publish") != []]
# Batch packaging uses Cargo's temporary registry for unpublished local versions.
args = ["cargo", "package", "--allow-dirty", "--no-verify", "--offline"]
for package in PACKAGES:
    args += ["-p", package]
run(args)

with tempfile.TemporaryDirectory(prefix="featherweight-packages-") as temporary:
    stage = Path(temporary)
    dirs = {}
    for name in PACKAGES:
        dirname = f"{name}-{versions[name]}"
        archive = Path(metadata["target_directory"]) / "package" / f"{dirname}.crate"
        with tarfile.open(archive) as tar:
            tar.extractall(stage, filter="data")
        # Cargo archives normalize mtimes. With a shared target cache, restoring
        # those old timestamps can incorrectly reuse binaries from a deleted
        # extraction directory, including their embedded CARGO_MANIFEST_DIR.
        for extracted in (stage / dirname).rglob("*"):
            if extracted.is_file():
                os.utime(extracted, None)
        dirs[name] = dirname
        manifest = tomllib.loads((stage / dirname / "Cargo.toml").read_text())
        # Cargo's normalized manifest must contain no source-checkout references.
        for table in [manifest, *manifest.get("target", {}).values()]:
            for section in ["dependencies", "dev-dependencies", "build-dependencies"]:
                for dep in table.get(section, {}).values():
                    assert not isinstance(dep, dict) or "path" not in dep, (name, dep)

    consumer = stage / "consumer"
    shutil.copytree(ROOT / "tests/embedding/src", consumer / "src")
    source = (ROOT / "tests/embedding/Cargo.toml").read_text()
    source = source.replace("[workspace]\n", "")
    import re
    source = re.sub(r', path = "[^"]+"', '', source)
    (consumer / "Cargo.toml").write_text(source)
    members = [*dirs.values(), "consumer"]
    workspace = '[workspace]\nresolver = "2"\nmembers = ' + json.dumps(members) + '\n'
    workspace += '\n[patch.crates-io]\n'
    for name, directory in dirs.items():
        workspace += f'{name} = {{ path = "{directory}" }}\n'
    (stage / "Cargo.toml").write_text(workspace)
    shutil.copy2(ROOT / "Cargo.lock", stage / "Cargo.lock")
    env = dict(os.environ, CARGO_TARGET_DIR=str(ROOT / "target/package-embedding-check"))
    # Verify every publishable archive, not just the runtime dependency subset.
    # Building the extracted workspace avoids Cargo's temporary-registry hash
    # lookup failure during built-in batch verification on Cargo 1.96.
    run(["cargo", "check", "--offline", "--workspace", "--all-targets"], stage, env=env)
    run(["cargo", "test", "--offline", "-p", "featherweight-external-embedding-check",
         "-p", "featherweight-runtime", "-p", "structfs-handles"], stage, env=env)
    run(["cargo", "clippy", "--locked", "--offline", "-p", "featherweight-external-embedding-check",
         "--all-targets", "--", "-D", "warnings"], stage, env=env)
    for features in [[], ["--no-default-features"]]:
        run(["cargo", "build", "--locked", "--offline", "-p", "featherweight-guest",
             "--target", "wasm32-unknown-unknown", *features], stage, env=env)
    run(["cargo", "doc", "--locked", "--offline", "--no-deps", "-p", "featherweight-runtime",
         "-p", "structfs-handles"], stage, env={**env, "RUSTDOCFLAGS": "-D warnings"})
print("Featherweight package embedding gate passed; nothing published.")
