#!/usr/bin/env python3
"""Generate CHANGELOG.md from git history.

Deterministic, standard library only.  Usage:

    python3 scripts/gen_changelog.py            # rewrite CHANGELOG.md from `main`
    python3 scripts/gen_changelog.py --stdout   # print instead of writing
    python3 scripts/gen_changelog.py --ref HEAD # use another ref
    python3 scripts/gen_changelog.py --check    # exit 1 if CHANGELOG.md is stale

How a version's entry is found (the workspace version in the root
`Cargo.toml` is the single source of truth for which versions exist):

1. every commit reachable from the ref is read, with the `version` of the root
   `Cargo.toml` as of that commit;
2. a version's subject is the oldest non-merge commit whose subject ends with
   `(x.y.z)`; failing that, a merge commit naming it in a range such as
   `(0.162.0-0.163.0)`; failing that, the oldest commit that carries that
   version in `Cargo.toml`;
3. its date is that commit's date (`YYYY-MM-DD`, the committer date);
4. versions are listed newest first by version number.  Merge order can
   differ from version order (a patch such as 0.186.1 landed on `main` after
   0.187.0), but the commit dates still fall in version order (checked).

The full account of each version lives in PLAN.md; this file is an index.
"""

import argparse
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUTPUT = ROOT / "CHANGELOG.md"

INTRO = """\
# Changelog

All notable changes to Aurora are listed here, newest version first, in the
spirit of [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and
[Semantic Versioning](https://semver.org/) (Aurora is 0.x: a minor version is
one task-level unit of work, a patch version is a bug fix).

This file is **generated** by `scripts/gen_changelog.py` from git history: one
line per version, taken from that version's commit subject, dated with the
commit date. Do not edit it by hand; change the script or the commit subjects.
The full account of every version (design decisions, measurements,
disclosed weaknesses) is in [PLAN.md](PLAN.md); where Aurora stands today is in
[STATUS.md](STATUS.md). Versions are ordered by version number, so a patch
merged late (for example 0.186.1, merged to `main` after 0.187.0) sits in
version position, and each date is the commit date of that version's own
commit, not of the merge.

## [Unreleased]

Work on a branch that has not been merged to `main` is not listed here until
it is merged.
"""

VERSION_RE = re.compile(r"\((\d+\.\d+\.\d+)\)\s*$")
RANGE_RE = re.compile(r"\((\d+\.\d+\.\d+)\s*[–—-]\s*(\d+\.\d+\.\d+)\)\s*$")
CARGO_VERSION_RE = re.compile(r'^version\s*=\s*"([^"]+)"', re.M)
MERGE_PREFIX_RE = re.compile(r"^Merge (?:branch )?[\w./-]+?:\s*")
FIXED_RE = re.compile(
    r"^(fix|correct|repair|revert|stop|harden|close the|make .* (?:wait|robust))\b", re.I
)
ADDED_RE = re.compile(
    r"^(add|build|implement|introduce|create|draw|start|prototype|extend|wire|port|"
    r"support|write|new|bring|ship|open|read|show|enable|apply|land|record)\b",
    re.I,
)


def git(*args, cwd=ROOT):
    return subprocess.run(
        ["git", *args], cwd=cwd, capture_output=True, text=True, check=True
    ).stdout


def version_key(v):
    return tuple(int(p) for p in v.split("."))


def commits(ref):
    out = git("log", ref, "--format=%H%x1f%cs%x1f%P%x1f%s%x1e")
    rows = []
    for rec in out.split("\x1e"):
        rec = rec.strip("\n")
        if not rec:
            continue
        h, date, parents, subject = rec.split("\x1f", 3)
        rows.append((h, date, len(parents.split()), subject))
    return rows  # newest first


def cargo_versions(hashes):
    """Workspace version at each commit, through one `git cat-file --batch`."""
    spec = "".join(f"{h}:Cargo.toml\n" for h in hashes)
    proc = subprocess.run(
        ["git", "cat-file", "--batch"],
        cwd=ROOT,
        input=spec.encode(),
        capture_output=True,
        check=True,
    )
    data = proc.stdout
    versions = {}
    pos = 0
    for h in hashes:
        eol = data.index(b"\n", pos)
        header = data[pos:eol].decode().split()
        pos = eol + 1
        if len(header) == 3 and header[1] == "blob":
            size = int(header[2])
            body = data[pos : pos + size].decode("utf-8", "replace")
            pos += size + 1
            m = CARGO_VERSION_RE.search(body)
            versions[h] = m.group(1) if m else None
        else:
            versions[h] = None
    return versions


def clean_subject(subject):
    s = subject.strip()
    s = RANGE_RE.sub("", s)
    s = VERSION_RE.sub("", s)
    s = MERGE_PREFIX_RE.sub("", s)
    s = s.strip().rstrip(".:;,- ").strip()
    if s:
        s = s[0].upper() + s[1:]
    return s


def group_of(version, subject):
    if FIXED_RE.match(subject):
        return "Fixed"
    if ADDED_RE.match(subject):
        return "Added"
    if version_key(version)[2] != 0:
        return "Fixed"  # a patch version is a bug fix by the versioning rules
    return "Changed"


def collect(ref):
    rows = commits(ref)
    by_cargo = cargo_versions([r[0] for r in rows])
    oldest_first = list(reversed(rows))
    entries = {}  # version -> (rank, date, subject)

    def offer(version, rank, date, subject):
        if version not in entries or rank < entries[version][0]:
            entries[version] = (rank, date, subject)
        # same rank: keep the oldest (first offered, since we walk oldest first)

    for h, date, nparents, subject in oldest_first:
        m = VERSION_RE.search(subject)
        if m and nparents < 2:
            offer(m.group(1), 0, date, subject)
        r = RANGE_RE.search(subject)
        if r:
            lo, hi = r.group(1), r.group(2)
            for v in set(by_cargo.values()):
                if v and version_key(lo) <= version_key(v) <= version_key(hi):
                    offer(v, 1, date, subject)
        v = by_cargo.get(h)
        if v:
            offer(v, 2, date, subject)
    return entries


def render(entries):
    out = [INTRO]
    for version in sorted(entries, key=version_key, reverse=True):
        _rank, date, subject = entries[version]
        text = clean_subject(subject) or "Release"
        group = group_of(version, text)
        out.append(f"## [{version}] - {date}\n")
        out.append(f"### {group}\n")
        out.append(f"- {text}\n")
    return "\n".join(out)


def main(argv):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--ref", default="main")
    ap.add_argument("--stdout", action="store_true")
    ap.add_argument("--check", action="store_true")
    args = ap.parse_args(argv)
    text = render(collect(args.ref))
    if args.stdout:
        sys.stdout.write(text)
        return 0
    if args.check:
        current = OUTPUT.read_text() if OUTPUT.exists() else ""
        if current != text:
            print("CHANGELOG.md is stale: run python3 scripts/gen_changelog.py")
            return 1
        return 0
    OUTPUT.write_text(text)
    print(f"wrote {OUTPUT.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
