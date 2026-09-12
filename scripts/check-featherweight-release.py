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

    import re
    consumers = ["embedding", "reactive-screen", "streaming-gateway", "conversation-service"]
    consumer_packages = ["featherweight-external-embedding-check"]
    for name in consumers:
        origin = ROOT / "tests" / ("embedding" if name == "embedding" else f"applications/{name}")
        consumer = stage / name
        shutil.copytree(origin / "src", consumer / "src")
        source = (origin / "Cargo.toml").read_text().replace("[workspace]\n", "")
        source = re.sub(r', path = "[^"]+"', '', source)
        (consumer / "Cargo.toml").write_text(source)
        if name != "embedding":
            consumer_packages.append(tomllib.loads(source)["package"]["name"])
    consumer_args = [arg for name in consumer_packages for arg in ["-p", name]]
    members = [*dirs.values(), *consumers]
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
    run(["cargo", "test", "--offline", "-p", "featherweight-runtime", "-p", "structfs-handles",
         "-p", "structfs-core-store", "-p", "structfs-serde-store", "-p", "structfs-service", "-p", "structfs-state", "-p", "structfs-profiles", *consumer_args], stage, env=env)
    import sys
    run([sys.executable, str(stage / dirs["structfs-serde-store"] /
         "tests/reference_value_v1.py")], stage, env=env)
    run(["cargo", "clippy", "--locked", "--offline", *consumer_args,
         "--all-targets", "--", "-D", "warnings"], stage, env=env)
    run(["cargo", "test", "--locked", "--offline", "-p", "structfs-profiles",
         "--no-default-features"], stage, env=env)
    run(["cargo", "build", "--locked", "--offline", "-p", "structfs-profiles",
         "--no-default-features", "--target", "wasm32-unknown-unknown"], stage, env=env)
    for features in [[], ["--no-default-features"], ["--no-default-features", "--features", "value-codecs"], ["--no-default-features", "--features", "state"], ["--no-default-features", "--features", "profiles"]]:
        run(["cargo", "build", "--locked", "--offline", "-p", "featherweight-guest",
             "--target", "wasm32-unknown-unknown", *features], stage, env=env)
    run(["cargo", "doc", "--locked", "--offline", "--no-deps", "-p", "featherweight-runtime",
         "-p", "structfs-handles", "-p", "structfs-service", "-p", "structfs-state", "-p", "structfs-profiles"], stage, env={**env, "RUSTDOCFLAGS": "-D warnings"})
print("Featherweight package embedding gate passed; nothing published.")
