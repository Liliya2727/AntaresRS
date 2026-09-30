#!/usr/bin/env python3
"""Check the Rust charging-node table against the C original, entry for entry.

Run from the repo root:  python3 scripts/check_charging_nodes.py
Exits non-zero on any mismatch (count, name, path, on_val, off_val, order).
"""
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
C_SRC = ROOT / "archdaemon/jni/src/BypassCharge/ChargingNodes.c"
RUST_SRC = ROOT / "crates/azenith-daemon/src/bypass_charge.rs"

# The C table is a list of {"NAME", "path", "on", "off"} initialisers. Paths are
# split across adjacent string literals in at least one entry, so join them
# before matching rather than assuming one literal per field.
ENTRY = re.compile(
    r'\{\s*"(?P<name>[A-Z][A-Z0-9_]*)"\s*,\s*'
    r'(?P<path>"(?:[^"\\]|\\.)*"(?:\s*"(?:[^"\\]|\\.)*")*)\s*,\s*'
    r'"(?P<on>[^"]*)"\s*,\s*"(?P<off>[^"]*)"\s*,?\s*\}'
)


def c_entries(text: str):
    out = []
    for m in ENTRY.finditer(text):
        path = "".join(re.findall(r'"((?:[^"\\]|\\.)*)"', m.group("path")))
        out.append((m.group("name"), path, m.group("on"), m.group("off")))
    return out


def rust_entries(text: str):
    """Parse the CHARGING_NODES table only.

    Whitespace-tolerant (rustfmt may break entries across lines) and scoped to
    the table body so the struct definition and the unit-test fixtures, which
    use the same literal shape, are not counted as entries.
    """
    marker = "pub static CHARGING_NODES: &[ChargingNode] = &["
    body = text.split(marker, 1)
    if len(body) != 2:
        raise SystemExit(f"FAIL: could not locate `{marker}` in Rust source")
    # The table closes with a `];` in column 0; the first one after the marker
    # is the table's own terminator (everything deeper is a test fixture).
    end = body[1].find("\n];")
    if end == -1:
        raise SystemExit("FAIL: could not find the table terminator `];`")
    body = body[1][:end]

    out = []
    for m in re.finditer(
        r'ChargingNode\s*\{\s*name:\s*"(?P<name>[A-Z][A-Z0-9_]*)",\s*'
        r'vendor:\s*"[^"]*",\s*'
        r'path:\s*"(?P<path>(?:[^"\\]|\\.)*)",\s*'
        r'on_val:\s*"(?P<on>[^"]*)",\s*'
        r'off_val:\s*"(?P<off>[^"]*)",?\s*\},',
        body,
    ):
        out.append(
            (m.group("name"), m.group("path"), m.group("on"), m.group("off"))
        )
    return out


def main() -> int:
    c = c_entries(C_SRC.read_text())
    r = rust_entries(RUST_SRC.read_text())

    errors = []
    if len(c) != len(r):
        errors.append(f"entry count differs: C={len(c)} Rust={len(r)}")

    for i, (ce, re_) in enumerate(zip(c, r)):
        if ce != re_:
            errors.append(f"[{i}] C={ce!r}\n     Rust={re_!r}")

    cn = {e[0] for e in c}
    rn = {e[0] for e in r}
    for name in sorted(cn - rn):
        errors.append(f"missing from Rust: {name}")
    for name in sorted(rn - cn):
        errors.append(f"not in C: {name}")

    if errors:
        print(f"FAIL: {len(errors)} mismatch(es)")
        for e in errors:
            print(" ", e)
        return 1

    print(f"OK: {len(r)} charging nodes match C exactly (names, paths, values, order)")
    return 0


if __name__ == "__main__":
    sys.exit(main())