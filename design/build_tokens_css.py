#!/usr/bin/env python3
"""Generate tokens.css from the TOML token source files.

tokens.css is a generated artifact for the Phase 0 mockups/gallery (plain
HTML+CSS — Aurora itself never uses CSS, this is a review aid only, see
design/README.md). The TOML files are the single source of truth; hand-edit
those, then re-run this script. Do not hand-edit tokens.css.

Usage: python3 build_tokens_css.py > tokens.css
"""

import tomllib
from pathlib import Path

HERE = Path(__file__).parent


def load(path):
    with open(path, "rb") as f:
        return tomllib.load(f)


def resolve_color(ref, palette):
    node = palette
    for p in ref.split("."):
        node = node[p]
    return node


def flatten(prefix, node, out):
    """Flatten a nested TOML table into CSS custom-property names."""
    if isinstance(node, dict):
        for k, v in node.items():
            flatten(f"{prefix}-{k}" if prefix else k, v, out)
    else:
        out[prefix] = node


def css_var_name(dotted):
    return "--" + dotted.replace("_", "-").replace(".", "-")


def main():
    palette = load(HERE / "tokens" / "palette.toml")
    scales = load(HERE / "tokens" / "scales.toml")
    theme = load(HERE / "themes" / "dark.toml")

    lines = [
        "/* GENERATED FILE — do not hand-edit.",
        " * Source: tokens/palette.toml, tokens/scales.toml, themes/dark.toml",
        " * Regenerate: python3 build_tokens_css.py > tokens.css",
        " */",
        ":root {",
    ]

    # --- semantic color tokens, resolved through the theme to the palette ---
    # A colour group can also carry a plain scalar alongside its palette
    # references -- `border.control_opacity` is one (0.0 in every theme but the
    # two High Contrast ones, see tokens/vocabulary.md). Those are emitted
    # as-is; only strings are palette references to resolve.
    for group in ("surface", "text", "icon", "border", "accent"):
        for name, ref in theme[group].items():
            var = css_var_name(f"{group}-{name}")
            value = resolve_color(ref, palette) if isinstance(ref, str) else ref
            lines.append(f"  {var}: {value};")

    for name, val in theme["state"].items():
        if isinstance(val, dict):
            color = resolve_color(val["color"], palette)
            lines.append(f"  {css_var_name(f'state-{name}-color')}: {color};")
            lines.append(f"  {css_var_name(f'state-{name}-alpha')}: {val['alpha']};")
        else:
            # Same rule as the colour groups above: `state.disabled_opacity` is
            # a scalar, everything else here is a palette reference.
            value = resolve_color(val, palette) if isinstance(val, str) else val
            lines.append(f"  {css_var_name(f'state-{name}')}: {value};")

    lines.append("")

    # --- shared scales (theme-independent) ---
    for size, px in scales["type"]["size"].items():
        lines.append(f"  {css_var_name(f'type-size-{size}')}: {px}px;")
    for weight, val in scales["type"]["weight"].items():
        lines.append(f"  {css_var_name(f'type-weight-{weight}')}: {val};")
    for lh, val in scales["type"]["line_height"].items():
        lines.append(f"  {css_var_name(f'type-line-height-{lh}')}: {val};")

    lines.append("")
    for name, val in scales["spacing"].items():
        if name == "density_multiplier":
            continue
        lines.append(f"  {css_var_name(f'spacing-{name}')}: {val}px;")

    lines.append("")
    for name, val in scales["radius"].items():
        lines.append(f"  {css_var_name(f'radius-{name}')}: {val}px;")

    lines.append("")
    for level in ("0", "1", "2"):
        e = scales["elevation"][level]
        lines.append(
            f"  {css_var_name(f'elevation-{level}')}: 0 {e['y']}px {e['blur']}px "
            f"rgba(0,0,0,{e['alpha']});"
        )

    lines.append("")
    for name, ms in scales["motion"]["duration"].items():
        lines.append(f"  {css_var_name(f'motion-duration-{name}')}: {ms}ms;")
    for name, curve in scales["motion"]["easing"].items():
        lines.append(f"  {css_var_name(f'motion-easing-{name}')}: {curve};")

    lines.append("")
    for name, mult in scales["spacing"]["density_multiplier"].items():
        lines.append(f"  {css_var_name(f'density-{name}')}: {mult};")

    lines.append("}")
    lines.append("")
    lines.append('[data-density="compact"] { --density-current: var(--density-compact); }')
    lines.append('[data-density="comfortable"] { --density-current: var(--density-comfortable); }')
    lines.append('[data-density="spacious"] { --density-current: var(--density-spacious); }')

    print("\n".join(lines))


if __name__ == "__main__":
    main()
