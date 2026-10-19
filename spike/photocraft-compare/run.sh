#!/usr/bin/env bash
# Build both tools (release), generate the synthetic inputs and run the
# comparison. bash + python3 stdlib only; runs on Linux and macOS.
#
#   spike/photocraft-compare/run.sh [--work DIR] [--commit SHA] [--skip-build] [-- compare.py args...]
#
# WORK (default: ../aurora-photocraft-work beside the repo, i.e. outside it)
# holds the PhotoCraft clone, its target dir, the synthetic inputs, the
# outputs and Aurora's tile scratch. Delete it when done.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
WORK="$(cd "$REPO/.." && pwd)/aurora-photocraft-work"
COMMIT=""
SKIP_BUILD=0
PC_URL="https://github.com/storytold/photocraft"
while [ $# -gt 0 ]; do
  case "$1" in
    --work) WORK="$2"; shift 2 ;;
    --commit) COMMIT="$2"; shift 2 ;;
    --skip-build) SKIP_BUILD=1; shift ;;
    --) shift; break ;;
    *) echo "unknown argument $1" >&2; exit 2 ;;
  esac
done
mkdir -p "$WORK"
WORK="$(cd "$WORK" && pwd)"
case "$WORK/" in "$REPO/"*) echo "WORK must be outside the repository" >&2; exit 2 ;; esac

# Free space: both release builds plus the 60 MP inputs need room.
avail_kb=$(df -Pk "$WORK" | awk 'NR==2 {print $4}')
if [ "$avail_kb" -lt $((25 * 1024 * 1024)) ]; then
  echo "less than 25 GiB free under $WORK; free space first" >&2; exit 1
fi

# PhotoCraft needs Rust >= 1.95 (its rust-version); use Aurora's pinned 1.97.
TOOLCHAIN="${PHOTOCRAFT_TOOLCHAIN:-1.97}"
if [ "$SKIP_BUILD" -eq 0 ]; then
  if [ ! -d "$WORK/photocraft/.git" ]; then
    git clone --quiet "$PC_URL" "$WORK/photocraft"
  fi
  if [ -n "$COMMIT" ]; then
    git -C "$WORK/photocraft" fetch --quiet origin
    git -C "$WORK/photocraft" checkout --quiet "$COMMIT"
  fi
  (cd "$WORK/photocraft" && CARGO_TARGET_DIR="$WORK/target" cargo "+$TOOLCHAIN" build --release -p photocraft-cli)
  (cd "$REPO" && cargo build --release -p aurora-app --bin aurora-headless)
fi
PC_BIN="$WORK/target/release/photocraft-cli"
AU_BIN="$REPO/target/release/aurora-headless"
PC_SHA="$(git -C "$WORK/photocraft" rev-parse HEAD)"
PC_VER="$("$PC_BIN" --version 2>/dev/null | head -1 || true) commit $PC_SHA, rustc $(rustc "+$TOOLCHAIN" --version | cut -d' ' -f2)"
AU_VER="$(grep -m1 '^version' "$REPO/Cargo.toml" | cut -d'"' -f2) commit $(git -C "$REPO" rev-parse HEAD)$(git -C "$REPO" diff --quiet HEAD -- crates || echo ' (dirty)')"

python3 "$HERE/gen_synthetic.py" "$WORK/synth" --sizes "${SYNTHETIC_SIZES:-24,60}"

STAMP="$(date +%Y%m%d-%H%M%S)"
OUT="${RESULTS_DIR:-$WORK/results-$STAMP}"
python3 "$HERE/compare.py" --aurora "$AU_BIN" --photocraft "$PC_BIN" --work "$WORK" --out "$OUT" \
  --aurora-version "$AU_VER" --photocraft-version "$PC_VER" \
  --harness-commit "$(git -C "$REPO" rev-parse --short HEAD)$(git -C "$REPO" diff --quiet HEAD -- spike/photocraft-compare || echo '-dirty')" \
  --synthetic-sizes "${SYNTHETIC_SIZES:-24,60}" "$@"
