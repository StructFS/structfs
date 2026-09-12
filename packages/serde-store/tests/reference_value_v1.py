#!/usr/bin/env python3
"""Independent C1 encoder for the shared conformance corpus; Python stdlib only.

This validates the tagged document structure and canonicalizes valid inputs.
Rust tests separately exercise rejection categories and native codecs.
"""
import base64
import json
import re
from pathlib import Path


def normalize(node):
    assert isinstance(node, list) and node
    tag = node[0]
    assert tag in {"null", "bool", "int", "float", "string", "bytes", "array", "map"}
    assert len(node) == (1 if tag == "null" else 2)
    if tag == "null":
        return node
    value = node[1]
    if tag == "bool":
        assert type(value) is bool
    elif tag in {"int", "float", "string", "bytes"}:
        assert isinstance(value, str)
        value.encode("utf-8")
        if tag == "int":
            assert re.fullmatch(r"0|-?[1-9][0-9]*", value)
            assert -(2**63) <= int(value) < 2**64
        elif tag == "float":
            assert re.fullmatch(r"[0-9a-f]{16}", value)
            bits = int(value, 16)
            if bits & 0x7FF0000000000000 == 0x7FF0000000000000 and bits & 0xFFFFFFFFFFFFF:
                value = "7ff8000000000000"
        elif tag == "bytes":
            assert base64.b64encode(base64.b64decode(value, validate=True)).decode() == value
    elif tag == "array":
        assert isinstance(value, list)
        value = [normalize(child) for child in value]
    else:
        assert isinstance(value, list)
        entries = {}
        for pair in value:
            assert isinstance(pair, list) and len(pair) == 2
            key, child = pair
            assert isinstance(key, str) and key not in entries
            key.encode("utf-8")
            entries[key] = normalize(child)
        value = [[key, entries[key]] for key in sorted(entries, key=lambda k: k.encode("utf-8"))]
    return [tag, value]


def emit(value):
    if isinstance(value, list):
        return "[" + ",".join(map(emit, value)) + "]"
    if isinstance(value, str):
        escaped = []
        for char in value:
            if char in {'"', "\\"}:
                escaped.append("\\" + char)
            elif ord(char) < 32:
                escaped.append(f"\\u{ord(char):04x}")
            else:
                escaped.append(char)
        return '"' + "".join(escaped) + '"'
    if type(value) is bool:
        return "true" if value else "false"
    assert type(value) is int
    return str(value)


def canonical(document):
    assert isinstance(document, list) and len(document) == 3
    assert document[0] == "structfs-value"
    assert type(document[1]) is int and document[1] == 1
    return emit(["structfs-value", 1, normalize(document[2])])


if __name__ == "__main__":
    fixture = json.loads((Path(__file__).parent / "fixtures/structfs-value-v1.json").read_text())
    cases = fixture["tagged_json"]["accepted"]
    for case in cases:
        assert canonical(json.loads(case["input"])) == case["canonical"], case["name"]
    print(f"Independent Python C1 encoder agrees on {len(cases)} shared vectors.")
