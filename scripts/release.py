#!/usr/bin/env python3
"""Plan, validate and publish workspace releases. Python 3.9+, no dependencies."""
import argparse
import json
from pathlib import Path
import subprocess
import sys
import time
import urllib.error
import urllib.request

ROOT = Path(__file__).resolve().parent.parent


def run(*args, capture=False):
    return subprocess.run(args, cwd=ROOT, check=True, text=True,
                          stdout=subprocess.PIPE if capture else None).stdout


def publishable(package):
    registries = package.get("publish")
    return registries is None or "crates-io" in registries


def plan(metadata, only=(), exclude=()):
    members = set(metadata["workspace_members"])
    packages = {p["name"]: p for p in metadata["packages"]
                if p["id"] in members and publishable(p)}
    unknown = (set(only) | set(exclude)) - packages.keys()
    if unknown:
        raise ValueError(f"unknown or nonpublishable crates: {sorted(unknown)}")
    selected = (set(only) if only else set(packages)) - set(exclude)
    if not selected:
        raise ValueError("no crates selected")
    ordered, visiting, visited = [], set(), set()

    def visit(name):
        if name in visited:
            return
        if name in visiting:
            raise ValueError(f"dependency cycle at {name}")
        visiting.add(name)
        for dep in packages[name]["dependencies"]:
            # Versioned dev dependencies must also resolve when Cargo packages
            # each crate. Reject cycles rather than publish a partial graph.
            if dep.get("path") and not (dep.get("kind") == "dev" and dep.get("req") == "*"):
                if dep["name"] not in packages:
                    raise ValueError(f"{name} depends on nonpublishable {dep['name']}")
                if dep["name"] in selected:
                    visit(dep["name"])
        visiting.remove(name)
        visited.add(name)
        ordered.append(packages[name])

    for name in sorted(selected):
        visit(name)
    return ordered


def registry_versions(name):
    # Exact sparse-index entries, including older and yanked releases. Search
    # results and local tags are not evidence of publication.
    key = name.lower()
    prefix = "1" if len(key) == 1 else "2" if len(key) == 2 else (
        f"3/{key[0]}" if len(key) == 3 else f"{key[:2]}/{key[2:4]}")
    request = urllib.request.Request(f"https://index.crates.io/{prefix}/{key}",
                                     headers={"User-Agent": "StructFS-release-preflight"})
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            return {entry["vers"]: entry for entry in
                    (json.loads(line) for line in response if line.strip())}
    except urllib.error.HTTPError as error:
        if error.code == 404:
            return {}
        raise RuntimeError(f"registry lookup failed for {name}: HTTP {error.code}") from error
    except (urllib.error.URLError, TimeoutError) as error:
        raise RuntimeError(f"registry lookup failed for {name}: {error}") from error


def check_external_dependencies(packages, metadata, lookup):
    selected = {p["name"] for p in packages}
    local = {p["name"]: p for p in metadata["packages"]}
    for package in packages:
        for dep in package["dependencies"]:
            if (not dep.get("path") or dep["name"] in selected
                    or (dep.get("kind") == "dev" and dep.get("req") == "*")):
                continue
            expected = local[dep["name"]]["version"]
            entry = lookup(dep["name"]).get(expected)
            if not entry or entry.get("yanked"):
                raise ValueError(f"{package['name']} needs omitted dependency {dep['name']} "
                                 f"{expected}; include it in the coordinated release")


def wait_for_version(name, version, timeout):
    deadline = time.monotonic() + timeout
    while True:
        entry = registry_versions(name).get(version)
        if entry:
            if entry.get("yanked"):
                raise RuntimeError(f"{name} {version} is yanked")
            return
        if time.monotonic() >= deadline:
            raise RuntimeError(f"registry did not expose {name} {version}; rerun preflight before retrying")
        time.sleep(min(5, max(0, deadline - time.monotonic())))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    modes = parser.add_mutually_exclusive_group()
    modes.add_argument("--plan", action="store_true", help="print local plan without network or mutation")
    modes.add_argument("--dry-run", action="store_true", help="verify registry and actual archives; no publishing or tags")
    parser.add_argument("--crate", action="append", default=[])
    parser.add_argument("--exclude", action="append", default=[])
    parser.add_argument("--skip-gates", action="store_true", help="use only after gates passed for this exact tree")
    parser.add_argument("--propagation-delay", type=int, default=120,
                        help="maximum seconds to wait for each published version (default 120)")
    args = parser.parse_args()
    if args.propagation_delay < 1:
        parser.error("--propagation-delay must be positive")
    metadata = json.loads(run("cargo", "metadata", "--locked", "--offline", "--no-deps",
                              "--format-version=1", capture=True))
    packages = plan(metadata, args.crate, args.exclude)
    for package in packages:
        print(f"{package['name']} {package['version']}", flush=True)
    if args.plan:
        return
    if not args.dry_run:
        if run("git", "status", "--porcelain", capture=True).strip():
            raise ValueError("publishing requires a clean working tree")
        if run("git", "branch", "--show-current", capture=True).strip() != "main":
            raise ValueError("publishing requires main")
    head = run("git", "rev-parse", "HEAD", capture=True).strip()
    cache = {}

    def lookup(name):
        if name not in cache:
            cache[name] = registry_versions(name)
        return cache[name]

    check_external_dependencies(packages, metadata, lookup)
    pending = []
    for package in packages:
        name, version = package["name"], package["version"]
        tag = f"{name}/v{version}"
        tagged = subprocess.run(["git", "rev-parse", "--verify", f"refs/tags/{tag}^{{commit}}"],
                                cwd=ROOT, text=True, capture_output=True)
        entry = lookup(name).get(version)
        if entry:
            if entry.get("yanked"):
                raise ValueError(f"{name} {version} is yanked; choose a new version")
            print(f"Already published: {name} {version}; skipping (tag is not publication evidence)")
            continue
        if tagged.returncode == 0 and tagged.stdout.strip() != head:
            raise ValueError(f"unpublished {tag} points to a different commit")
        pending.append((package, tag, tagged.returncode == 0))
    if not args.skip_gates:
        run(str(ROOT / "scripts/check-release.sh"))
    if args.dry_run:
        print("Dry run complete: registry checked and release gates passed. Nothing published or tagged."
              if not args.skip_gates else "Registry preflight complete; gates explicitly skipped. Nothing published or tagged.")
        return
    if run("git", "rev-parse", "HEAD", capture=True).strip() != head:
        raise ValueError("HEAD changed during release verification")
    if run("git", "status", "--porcelain", capture=True).strip():
        raise ValueError("working tree changed during release verification")
    for package, tag, tag_exists in pending:
        # Cargo validates dependency availability and verifies the package again.
        run("cargo", "publish", "--locked", "-p", package["name"])
        wait_for_version(package["name"], package["version"], args.propagation_delay)
        if not tag_exists:
            run("git", "tag", "-a", tag, "-m", f"{package['name']} v{package['version']}")
        print(f"Published and verified {tag}; push this tag after reviewing the release.", flush=True)


if __name__ == "__main__":
    try:
        main()
    except (ValueError, RuntimeError, subprocess.CalledProcessError) as error:
        print(f"Release aborted: {error}", file=sys.stderr)
        sys.exit(1)
