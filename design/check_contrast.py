#!/usr/bin/env python3
"""WCAG 2.1 contrast check over a theme's resolved token pairs.

Usage: python3 check_contrast.py [themes/dark.toml]

Resolves every token pair in PAIRS against tokens/palette.toml + the given
theme file, computes the WCAG contrast ratio, and checks it against the
floor from PRD FR-027 acceptance criterion 1 (4.5:1 body text, 3:1 large
text / UI boundaries). This is a Phase 0 design-review tool, not the CI lint
described in PLAN.md M1.6 (that one runs against aurora-theme's Rust token
types once they exist, over every built-in theme).
"""

import sys
import tomllib
from pathlib import Path

HERE = Path(__file__).parent

# (label, foreground token path, background token path, required ratio, note)
PAIRS = [
    ("text.primary on surface.canvas", "text.primary", "surface.canvas", 4.5, "body text"),
    ("text.primary on surface.app", "text.primary", "surface.app", 4.5, "body text"),
    ("text.primary on surface.panel", "text.primary", "surface.panel", 4.5, "body text"),
    ("text.primary on surface.raised", "text.primary", "surface.raised", 4.5, "body text"),
    ("text.primary on surface.overlay", "text.primary", "surface.overlay", 4.5, "body text"),
    ("text.primary on surface.sunken", "text.primary", "surface.sunken", 4.5, "body text"),
    ("text.secondary on surface.panel", "text.secondary", "surface.panel", 4.5, "body text"),
    ("text.secondary on surface.raised", "text.secondary", "surface.raised", 4.5, "body text"),
    ("text.secondary on surface.canvas", "text.secondary", "surface.canvas", 4.5, "body text"),
    ("text.on_accent on accent.primary", "text.on_accent", "accent.primary", 4.5, "body text"),
    ("text.on_accent on accent.primary_hover", "text.on_accent", "accent.primary_hover", 4.5, "body text"),
    ("border.strong on surface.panel", "border.strong", "surface.panel", 3.0, "UI boundary"),
    ("border.strong on surface.app", "border.strong", "surface.app", 3.0, "UI boundary"),
    ("border.focus on surface.panel", "border.focus", "surface.panel", 3.0, "UI boundary"),
    ("border.focus on surface.canvas", "border.focus", "surface.canvas", 3.0, "UI boundary"),
    ("border.focus on surface.raised", "border.focus", "surface.raised", 3.0, "UI boundary"),
    ("accent.primary on surface.panel", "accent.primary", "surface.panel", 3.0, "UI boundary"),
]

# Not gated (informational only). (label, fg, bg, reason)
INFORMATIONAL = [
    ("text.disabled on surface.panel", "text.disabled", "surface.panel",
     "not gated — disabled content"),
    ("icon.disabled on surface.panel", "icon.disabled", "surface.panel",
     "not gated — disabled content"),
    # border.default is decorative (subtle dividers, default input outlines);
    # WCAG 1.4.11 non-text contrast applies to boundaries that are the *only*
    # way a component is identified, and panels/inputs here are also
    # identified by spacing, labels, and surface-color changes. border.strong
    # and border.focus carry the gated cases instead, since they signal real
    # state (selection, focus) rather than decoration.
    ("border.default on surface.panel", "border.default", "surface.panel",
     "not gated — decorative, see comment in source"),
    ("border.default on surface.app", "border.default", "surface.app",
     "not gated — decorative, see comment in source"),
]


def load_palette():
    with open(HERE / "tokens" / "palette.toml", "rb") as f:
        return tomllib.load(f)


def load_theme(path):
    with open(path, "rb") as f:
        return tomllib.load(f)


def resolve(ref, palette):
    """'neutral.100' -> hex string, by walking the palette table."""
    parts = ref.split(".")
    node = palette
    for p in parts:
        node = node[p]
    if not isinstance(node, str):
        raise TypeError(f"{ref} did not resolve to a color string (got {node!r})")
    return node


def resolve_token(path, theme, palette):
    """'text.primary' -> hex, by walking the theme table then the palette."""
    parts = path.split(".")
    node = theme
    for p in parts:
        node = node[p]
    if isinstance(node, dict):
        # {color = "...", alpha = ...} overlay tokens aren't opaque colors;
        # not used in PAIRS, but guard clearly if misused.
        raise TypeError(f"{path} is a translucent overlay token, not a solid color")
    return resolve(node, palette)


def hex_to_srgb(hex_str):
    hex_str = hex_str.lstrip("#")
    r, g, b = (int(hex_str[i : i + 2], 16) / 255.0 for i in (0, 2, 4))
    return r, g, b


def channel_to_linear(c):
    return c / 12.92 if c <= 0.04045 else ((c + 0.055) / 1.055) ** 2.4


def relative_luminance(hex_str):
    r, g, b = hex_to_srgb(hex_str)
    r, g, b = (channel_to_linear(c) for c in (r, g, b))
    return 0.2126 * r + 0.7152 * g + 0.0722 * b


def contrast_ratio(hex_a, hex_b):
    la, lb = relative_luminance(hex_a), relative_luminance(hex_b)
    lighter, darker = max(la, lb), min(la, lb)
    return (lighter + 0.05) / (darker + 0.05)


def main():
    theme_path = Path(sys.argv[1]) if len(sys.argv) > 1 else HERE / "themes" / "dark.toml"
    palette = load_palette()
    theme = load_theme(theme_path)

    print(f"Contrast check: {theme_path.relative_to(HERE) if theme_path.is_relative_to(HERE) else theme_path}\n")

    failures = 0
    for label, fg_path, bg_path, floor, note in PAIRS:
        fg_hex = resolve_token(fg_path, theme, palette)
        bg_hex = resolve_token(bg_path, theme, palette)
        ratio = contrast_ratio(fg_hex, bg_hex)
        ok = ratio >= floor
        if not ok:
            failures += 1
        status = "PASS" if ok else "FAIL"
        print(f"  [{status}] {label:45s} {ratio:5.2f}:1  (floor {floor:.1f}:1, {note})")

    print()
    for label, fg_path, bg_path, reason in INFORMATIONAL:
        fg_hex = resolve_token(fg_path, theme, palette)
        bg_hex = resolve_token(bg_path, theme, palette)
        ratio = contrast_ratio(fg_hex, bg_hex)
        print(f"  [info] {label:45s} {ratio:5.2f}:1  ({reason})")

    print()
    if failures:
        print(f"{failures} pair(s) below the WCAG AA floor.")
        return 1
    print("All gated pairs pass WCAG 2.1 AA.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
