# ICC profile fixtures

- **`sRGB.icc`**, **`ECI-RGBv2.icc`** — from the `colord-data` Debian
  package (`/usr/share/color/icc/colord/`), **CC0-licensed**
  (`data/profiles/*` in `colord`'s own upstream copyright file — confirmed
  by reading it directly, not assumed from the package name). Small
  (15–20 KB) and unambiguously redistributable, so committed normally
  rather than gitignored — same provenance and reasoning as
  `spike/raw-icc/reference/icc-profiles/`, copied here so `aurora-color`
  (a real crate) has its own fixtures rather than reaching into `spike/`,
  which is deliberately kept isolated from real code.

Used by `crates/aurora-color`'s tests (`include_bytes!`) to load and
transform through real, independently-verified profiles rather than
synthetic ones — the same discipline `spike/raw-icc/FINDINGS.md` used to
cross-validate `lcms2` against `moxcms`.
