#!/usr/bin/env python3
"""Reject hardcoded style values in widget code (PRD FR-027 acceptance
criterion 3: "Zero hardcoded style values in widget code; CI lints for
literal colours and pixel sizes outside the token definitions" —
invariant §7.3.10: every widget resolves colour, spacing, size, radius,
and duration from `aurora-theme`'s design tokens, never a literal).

Scans `aurora-widgets`/`aurora-ui` source for the two patterns that show
up when someone reaches for a quick literal instead of a real token: a
bare hex colour string, and a `length(...)` call (this codebase's one
real entry point for a taffy pixel size — confirmed by grepping the
whole tree before writing this) whose argument is a numeric literal
rather than a token/variable expression.

Deliberately narrow, not exhaustive: `length(<identifier or
expression>)` (e.g. `length(scales.spacing.md)`, `length(width)`) is
real, resolved-from-a-token code and passes; `length(<number>)` does
not. `percent(...)` is a proportional/structural layout choice (fill N%
of the parent), not an absolute style value the token scale governs, so
it's out of scope here. Catching every conceivable hardcoded literal
(e.g. a bare number reaching a style field through some other helper)
would need real type information a text scan doesn't have — narrower
but reliable beats broader but noisy, the same trade-off
`check_layering.py` already makes.

Run via `python3 scripts/check_no_hardcoded_style.py`; CI runs it on
every push.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SCANNED_CRATES = ["aurora-widgets", "aurora-ui"]

# Ordered longest-first so an 8-digit run is never mistaken for a 6-digit
# one followed by two stray hex characters.
HEX_COLOR = re.compile(r"#[0-9a-fA-F]{8}\b|#[0-9a-fA-F]{6}\b|#[0-9a-fA-F]{3}\b")
LITERAL_LENGTH_CALL = re.compile(r"\blength\(\s*-?\d")


def source_files(crate: str) -> list[Path]:
    """Every `.rs` file in `crate`'s `src/` — `rglob` from `src/` alone
    already excludes a sibling `tests/` directory (integration tests,
    inherently test-only regardless of `#[cfg(test)]`)."""
    return sorted((ROOT / "crates" / crate / "src").rglob("*.rs"))


def production_text(path: Path) -> str:
    """`path`'s own text, truncated at its first `#[cfg(test)]` — this
    codebase's own consistent convention puts that module last in a
    file, so everything after it is test-only. Not a real Rust parser
    (deliberately, see this script's own module doc comment); a file
    that ever breaks this convention needs a real fix here, not a
    workaround.
    """
    text = path.read_text()
    marker = text.find("#[cfg(test)]")
    return text if marker == -1 else text[:marker]


def main() -> int:
    errors: list[str] = []

    for crate in SCANNED_CRATES:
        for path in source_files(crate):
            rel = path.relative_to(ROOT)
            for lineno, line in enumerate(production_text(path).splitlines(), start=1):
                if line.strip().startswith("//"):
                    continue
                for match in HEX_COLOR.finditer(line):
                    errors.append(
                        f"{rel}:{lineno}: hardcoded colour {match.group()!r} "
                        "— resolve from a real aurora-theme token instead"
                    )
                if LITERAL_LENGTH_CALL.search(line):
                    errors.append(
                        f"{rel}:{lineno}: length(<literal>) — resolve the "
                        "size from aurora_theme::Scales instead"
                    )

    if errors:
        print("Hardcoded style values found:\n", file=sys.stderr)
        for e in errors:
            print(f"  {e}", file=sys.stderr)
        print(
            f"\n{len(errors)} violation(s). PRD FR-027 / invariant §7.3.10: "
            "every widget resolves colour, spacing, size, radius, and "
            "duration from aurora-theme's design tokens, never a literal.",
            file=sys.stderr,
        )
        return 1

    scanned = sum(len(source_files(c)) for c in SCANNED_CRATES)
    print(f"No hardcoded style values found ({scanned} files scanned).")
    return 0


if __name__ == "__main__":
    sys.exit(main())
