#!/usr/bin/env python3
"""Export an auditable source snapshot for embedding without a sibling checkout.

Requires Python 3.11+. Usage: export-featherweight.py OUTPUT_DIRECTORY [--embedded]
Production source is copied verbatim; manifests materialize workspace settings
and omit development-only dependencies. SHA256SUMS pins every exported file.
"""
import hashlib
import json
import os
from pathlib import Path
import shutil
import sys
import tomllib

ROOT = Path(__file__).resolve().parent.parent
PACKAGES = ["namecode", "packages/ll-store", "packages/path-validation", "packages/path-macro", "packages/core-store", "packages/serde-store", "packages/handles", "featherweight/runtime", "featherweight/guest"]
workspace = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]
out = Path(sys.argv[1]).resolve()
out.mkdir(parents=True, exist_ok=True)

def scalar(v):
    if isinstance(v, bool): return str(v).lower()
    if isinstance(v, str): return json.dumps(v)
    if isinstance(v, list): return "[" + ", ".join(scalar(i) for i in v) + "]"
    return str(v)

def toml(table, prefix=""):
    lines = [f"[{prefix}]"] if prefix else []
    for key, value in table.items():
        if not isinstance(value, dict): lines.append(f"{json.dumps(key)} = {scalar(value)}")
    for key, value in table.items():
        if isinstance(value, dict): lines += [""] + toml(value, f"{prefix}.{json.dumps(key)}" if prefix else json.dumps(key))
    return lines

for package in PACKAGES:
    src = ROOT / package
    dest = out / package
    dest.mkdir(parents=True, exist_ok=True)
    shutil.copytree(src / "src", dest / "src", dirs_exist_ok=True)
    for name in ["README.md", "LICENSE"]:
        if (src / name).exists(): shutil.copy2(src / name, dest / name)
    data = tomllib.loads((src / "Cargo.toml").read_text())
    data.pop("dev-dependencies", None)
    for key, value in list(data["package"].items()):
        if isinstance(value, dict) and value.get("workspace"):
            data["package"][key] = workspace["package"][key]
    for name, dep in list(data.get("dependencies", {}).items()):
        if isinstance(dep, dict) and dep.get("workspace"):
            inherited = workspace["dependencies"][name]
            inherited = {"version": inherited} if isinstance(inherited, str) else inherited.copy()
            features = list(dict.fromkeys(inherited.get("features", []) + dep.get("features", [])))
            inherited.update({k:v for k,v in dep.items() if k not in ["workspace", "features"]})
            if features: inherited["features"] = features
            if "path" in inherited:
                inherited["path"] = os.path.relpath(out / inherited["path"], dest)
            data["dependencies"][name] = inherited
    (dest / "Cargo.toml").write_text("\n".join(toml(data)) + "\n")
if "--embedded" not in sys.argv[2:]:
    (out / "Cargo.toml").write_text("[workspace]\nresolver = \"2\"\nmembers = " + scalar(PACKAGES) + "\n")
for name in ["README.md", "LICENSE"]: shutil.copy2(ROOT / name, out / name)
files = sorted(p for p in out.rglob("*") if p.is_file() and p.name != "SHA256SUMS")
(out / "SHA256SUMS").write_text("".join(f"{hashlib.sha256(p.read_bytes()).hexdigest()}  {p.relative_to(out)}\n" for p in files))
