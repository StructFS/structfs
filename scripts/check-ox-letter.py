#!/usr/bin/env python3
"""Test two proposed substitutions against a read-only Ox source checkout.

Optional downstream probe, not part of the independent release gate. Generated
sources, their diff, provenance, and Cargo output stay under local/.
"""
import argparse
import difflib
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("ox", type=Path, help="Ox checkout (read only)")
    parser.add_argument("--sse", action="store_true", help="Probe the supplement's SSE framing tests")
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    source = args.ox.resolve() / "crates/horns-core/src"
    # local/ is gitignored and absent on a fresh clone; create it on demand.
    (root / "local").mkdir(exist_ok=True)
    output = Path(tempfile.mkdtemp(prefix="ox-letter-", dir=root / "local"))
    (output / "src").mkdir()
    hashes = {}
    patches = []
    names = ("subscription.rs", "path_serde.rs", "write.rs")
    if args.sse:
        source = args.ox.resolve() / "crates/ox-gate/src"
        names = ("sse_framing.rs",)
    for name in names:
        original = (source / name).read_text()
        hashes[name] = hashlib.sha256(original.encode()).hexdigest()
        migrated = original
        if args.sse:
            # Test-only shape adapter. Ox replaces malformed UTF-8; StructFS
            # rejects it. This probe does not claim equivalence for that policy.
            migrated = '''#[cfg(test)]
const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;
#[cfg(test)]
#[derive(Debug)]
struct FramingError { frames: Vec<String> }
#[cfg(test)]
struct SseFramer(structfs_http::sse::SseFramer);
#[cfg(test)]
impl Default for SseFramer {
    fn default() -> Self { Self(structfs_http::sse::SseFramer::new(MAX_FRAME_BYTES)) }
}
#[cfg(test)]
impl SseFramer {
    fn push(&mut self, bytes: &[u8]) -> Result<Vec<String>, FramingError> {
        let mut frames = Vec::new();
        for result in self.0.push(bytes) {
            match result {
                Ok(frame) => frames.push(frame.data),
                Err(_) => return Err(FramingError { frames }),
            }
        }
        Ok(frames)
    }
    fn finish(&mut self) -> Vec<String> {
        self.0.finish().into_iter().map(|frame|
            frame.expect("Ox probe fixtures require valid UTF-8").data).collect()
    }
}
''' + original[original.index("#[cfg(test)]"):]
        if name == "subscription.rs":
            begin = migrated.index("                let plen = prefix.len();")
            end = migrated.index("\n            }", begin)
            migrated = (migrated[:begin]
                        + "                structfs_core_store::matches_prefix_suffix(path, prefix, suffix, 1)"
                        + migrated[end:])
            old = ("pub trait AsyncWriter: Send + Sync {\n"
                   "    fn write(&self, path: Path, record: Record) -> BoxFuture<Result<Path, StoreError>>;\n}")
            if migrated.count(old) != 1:
                raise SystemExit("Ox writer shape changed; review the migration probe")
            migrated = migrated.replace(old, "pub use structfs_core_store::SharedWriter as AsyncWriter;")
        (output / "src" / name).write_text(migrated)
        patches.extend(difflib.unified_diff(original.splitlines(True), migrated.splitlines(True),
                                           fromfile=f"a/{name}", tofile=f"b/{name}"))
    (output / "migration.patch").write_text("".join(patches))
    (output / "src/lib.rs").write_text("".join(f"pub mod {Path(name).stem};\n" for name in names))
    core_path = json.dumps(str(root / "packages/core-store"))
    (output / "Cargo.toml").write_text(f'''[package]
name = "ox-letter-adoption-probe"
version = "0.0.0"
edition = "2024"
[workspace]
[dependencies]
structfs-core-store = {{ path = {core_path}, features = ["async"] }}
serde = {{ version = "1", features = ["derive"] }}
serde_json = "1"
tokio = {{ version = "1", features = ["sync", "time", "rt", "rt-multi-thread", "macros"] }}
''')
    if args.sse:
        with (output / "Cargo.toml").open("a") as manifest:
            manifest.write("structfs-http = { path = "
                           + json.dumps(str(root / "packages/http"))
                           + ', default-features = false }\n')
    # Reuse the candidate's dependency versions. Cargo prunes this copied lockfile
    # to the probe's graph; it never changes either repository's lockfile.
    (output / "Cargo.lock").write_bytes((root / "Cargo.lock").read_bytes())
    provenance = {
        "ox_head": subprocess.check_output(["git", "-C", str(source), "rev-parse", "HEAD"], text=True).strip(),
        "source_sha256": hashes,
        "probe": "sse" if args.sse else "subscription",
    }
    print(f"Probe: {output}", flush=True)
    with (output / "cargo-test.log").open("w") as log:
        result = subprocess.run(["cargo", "test", "--offline", "--manifest-path", str(output / "Cargo.toml")],
                                env={**os.environ, "CARGO_TARGET_DIR": str(root / "target")},
                                stdout=log, stderr=subprocess.STDOUT)
    provenance["cargo_test_exit_code"] = result.returncode
    (output / "provenance.json").write_text(json.dumps(provenance, indent=2) + "\n")
    print((output / "cargo-test.log").read_text())
    raise SystemExit(result.returncode)


if __name__ == "__main__":
    main()
