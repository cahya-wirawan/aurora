#!/usr/bin/env python3
"""Enforce the crate layering rule from PRD §7.2.

Dependencies point downward only. A lower crate never depends on a higher one,
and no crate depends on anything outside its declared allowance.

This is enforced mechanically rather than by convention because the layering is
what keeps `aurora-widgets` document-agnostic and `aurora-cli` headless — both
of which quietly stop being true the first time someone adds a convenient
import. Run via `python3 scripts/check_layering.py`; CI runs it on every push.
"""

from __future__ import annotations

import json
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
ALLOWED: dict[str, list[str]] = json.loads((ROOT / "scripts" / "layering.json").read_text())


def declared_deps(manifest: Path) -> set[str]:
    """Internal crates this manifest depends on, across all dependency kinds."""
    data = tomllib.loads(manifest.read_text())
    found: set[str] = set()
    for table in ("dependencies", "dev-dependencies", "build-dependencies"):
        found |= {name for name in data.get(table, {}) if name.startswith("aurora-")}
    for target in data.get("target", {}).values():
        for table in ("dependencies", "dev-dependencies", "build-dependencies"):
            found |= {name for name in target.get(table, {}) if name.startswith("aurora-")}
    return found


def transitively_allowed(crate: str, seen: set[str] | None = None) -> set[str]:
    """Everything `crate` may reach, so a legal direct dep is never flagged."""
    seen = seen or set()
    for dep in ALLOWED.get(crate, []):
        if dep not in seen:
            seen.add(dep)
            transitively_allowed(dep, seen)
    return seen


def main() -> int:
    errors: list[str] = []

    crate_dirs = sorted(p for p in (ROOT / "crates").iterdir() if p.is_dir())
    on_disk = {p.name for p in crate_dirs}
    declared = set(ALLOWED)

    if on_disk != declared:
        for missing in sorted(declared - on_disk):
            errors.append(f"{missing}: declared in layering.json but no crate directory")
        for extra in sorted(on_disk - declared):
            errors.append(f"{extra}: crate exists but is not in layering.json — add it with its allowed deps")

    for crate_dir in crate_dirs:
        crate = crate_dir.name
        if crate not in ALLOWED:
            continue

        manifest = crate_dir / "Cargo.toml"
        if not manifest.is_file():
            errors.append(f"{crate}: missing Cargo.toml")
            continue

        permitted = transitively_allowed(crate)
        for dep in sorted(declared_deps(manifest)):
            if dep == crate:
                errors.append(f"{crate}: depends on itself")
            elif dep not in permitted:
                # Is this an upward dependency (the layering violation) or just undeclared?
                if crate in transitively_allowed(dep):
                    errors.append(
                        f"{crate} -> {dep}: UPWARD dependency. {dep} already depends on {crate}; "
                        f"this creates a cycle and inverts the layering (PRD §7.2)."
                    )
                else:
                    errors.append(
                        f"{crate} -> {dep}: not permitted. Either move the shared code down "
                        f"the stack, or amend scripts/layering.json deliberately."
                    )

    if errors:
        print("Crate layering violations:\n", file=sys.stderr)
        for e in errors:
            print(f"  {e}", file=sys.stderr)
        print(
            f"\n{len(errors)} violation(s). The layering rule is in PRD §7.2; "
            "changing it is an architecture decision, not a build fix.",
            file=sys.stderr,
        )
        return 1

    print(f"Crate layering OK ({len(crate_dirs)} crates).")
    return 0


if __name__ == "__main__":
    sys.exit(main())
