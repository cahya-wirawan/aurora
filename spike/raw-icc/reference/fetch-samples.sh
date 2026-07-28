#!/usr/bin/env bash
# Re-fetches the real camera RAW samples used by `cargo run -- raw`.
#
# Not committed (multi-MB each, PLAN 0.7's "corpora are gitignored" rule) --
# this script is the source of truth for exactly which files and why. See
# reference/README.md for provenance.
set -euo pipefail
cd "$(dirname "$0")/raw-samples"

fetch() {
  local out="$1" url="$2"
  if [ -f "$out" ]; then
    echo "  already have $out"
  else
    echo "  fetching $out"
    curl -sL --max-time 60 -o "$out" "$url"
  fi
}

fetch canon-eos-m200.cr3 "https://raw.pixls.us/data/Canon/Canon%20EOS%20M200/_MG_2233.CR3"
fetch nikon-1-j1.nef "https://raw.pixls.us/data/Nikon/1%20J1/_DSC8140.NEF"
fetch sony-dsc-rx1.arw "https://raw.pixls.us/data/Sony/DSC-RX1/_DSC0812.ARW"

echo "Done. Run: cargo run -- raw"
