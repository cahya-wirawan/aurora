# Reference fixtures

## `raw-samples/` (gitignored — see `fetch-samples.sh`)

One real, unedited camera RAW file per major vendor, from
[raw.pixls.us](https://raw.pixls.us) — the public sample archive
`rawspeed`/`darktable`/`RawTherapee` use for exactly this kind of testing.
Not synthesized.

- **`canon-eos-m200.cr3`** — Canon EOS M200, CR3 (Canon's current container
  format, ISO-BMFF-based — a harder, newer target than the older CR2, and
  the more representative "what does Aurora need to support today" choice).
- **`nikon-1-j1.nef`** — Nikon 1 J1, NEF.
- **`sony-dsc-rx1.arw`** — Sony DSC-RX1, ARW.

Multi-MB each (8–25 MB), so gitignored per PLAN 0.7's corpus rule rather
than committed. Re-fetch with `./fetch-samples.sh`.

## `icc-profiles/`

- **`sRGB.icc`**, **`ECI-RGBv2.icc`** — from the `colord-data` Debian
  package (`/usr/share/color/icc/colord/`), **CC0-licensed**
  (`data/profiles/*` in `colord`'s own upstream copyright file — confirmed
  by reading it directly, not assumed from the package name). Already
  compatible with Aurora's own `deny.toml` allow-list. Small (15–20 KB) and
  unambiguously redistributable, so committed normally, unlike the RAW
  samples above.
