#!/usr/bin/env bash
# Fetches the PSD/PSB test corpus used for round-trip and structural-coverage
# testing (PLAN 0.7, PRD §13 Step 6).
#
# Not committed (49 MB, PLAN 0.7's "corpora are gitignored" rule) -- this
# script plus manifest.txt is the source of truth for exactly which files
# and why. See README.md for provenance and honest scope caveats.
set -euo pipefail
cd "$(dirname "$0")"

# Pinned commit, not `main` -- reproducible fetches, and this is someone
# else's actively-developed test suite, not an archive maintained for us.
SHA="ad89f315777866c832bf82e0377226cb13250c36"
BASE="https://raw.githubusercontent.com/psd-tools/psd-tools/${SHA}/tests/psd_files"
OUT="psd-tools-fixtures"

mkdir -p "$OUT"
total=$(wc -l < manifest.txt)
n=0
while IFS= read -r path; do
  n=$((n + 1))
  out="$OUT/$path"
  if [ -f "$out" ]; then
    echo "  [$n/$total] already have $path"
    continue
  fi
  mkdir -p "$(dirname "$out")"
  echo "  [$n/$total] fetching $path"
  curl -sL --max-time 60 -o "$out" "$BASE/$path"
done < manifest.txt

echo "Done. $(find "$OUT" -type f | wc -l) files in $OUT/"
echo "Run: python3 inventory.py"
