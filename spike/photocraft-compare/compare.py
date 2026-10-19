#!/usr/bin/env python3
"""Run Aurora and PhotoCraft over the same files and compare (python3 stdlib only).

Every timed run is "open the file, flatten it, write a PNG", timed from outside
with the same ``/usr/bin/time`` measure for both tools (``-v`` on Linux, ``-l``
on macOS), plus this script's own wall clock. Runs are interleaved (the tool
order rotates each round), one warm-up run per tool and file is reported
separately, and nothing is dropped: a crash, a timeout or a refused file is a
row with its reason.

Pictures are checked (idle mode) by ``pngread.py``, an independent decoder:

* PhotoCraft's warm-up PNG, and Aurora's 16-bit PNG from one extra, untimed
  verification run (``--depth 16``; the timed runs write the 8-bit dithered
  PNG Aurora's Save As writes) are compared with each other and, for a PSD with
  a real merged image, with Photoshop's merged image (Aurora's
  ``--reference-out``, the psd_fidelity_audit's reference), over white;
* Aurora's own dither floor (its timed 8-bit output against its 16-bit output)
  is reported next to it;
* every timed run's output (idle and loaded) is checked by SHA-256 against the
  idle warm-up output of the same tool.

See README.md for the commands and the fairness notes.
"""

import argparse
import atexit
import csv
import datetime
import fcntl
import hashlib
import json
import os
import platform
import re
import shutil
import signal
import statistics
import struct
import subprocess
import sys
import time
import zlib

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import pngread  # noqa: E402

REPO = os.path.abspath(os.path.join(HERE, "..", ".."))

# The representative subset (relative to the repo root); missing files are
# reported, not skipped silently.
REPRESENTATIVE = [
    "corpora/psd/reference/psd-tools-fixtures/1layer.psd",
    "corpora/psd/reference/psd-tools-fixtures/16bit5x5.psd",
    "corpora/psd/reference/psd-tools-fixtures/group.psd",
    "corpora/psd/reference/psd-tools-fixtures/clipping-mask.psd",
    "corpora/psd/reference/psd-tools-fixtures/layer_mask_data.psd",
    "corpora/psd/reference/psd-tools-fixtures/layer_effects.psd",
    "corpora/psd/reference/psd-tools-fixtures/adjustments/curves_rgb.psd",
    "corpora/psd/reference/psd-tools-fixtures/adjustments/huesaturation_colorize_rgb.psd",
    "corpora/psd/reference/psd-tools-fixtures/blend-modes/rgb-blend-modes.psd",
    "corpora/psd/reference/psd-tools-fixtures/broken-groups.psd",
    "corpora/psd/reference/psd-tools-fixtures/pen-text.psb",
    "images/curve-02.jpg",
    "images/instagram-01.png",
    "images/flat-design-nyepi-template/6739968.psd",
]

LOAD_GROUPS = []


def kill_load():
    while LOAD_GROUPS:
        p = LOAD_GROUPS.pop()
        try:
            os.killpg(p.pid, signal.SIGKILL)
        except OSError:
            pass
        try:
            p.wait(timeout=5)
        except Exception:
            pass


def on_signal(signum, _frame):
    kill_load()
    sys.exit(128 + signum)


atexit.register(kill_load)
signal.signal(signal.SIGTERM, on_signal)
signal.signal(signal.SIGINT, on_signal)
if hasattr(signal, "SIGHUP"):
    signal.signal(signal.SIGHUP, on_signal)


def start_load(k):
    """K busy `yes` processes, each in its own process group with a watchdog
    that kills it within a second if this script dies (even by SIGKILL)."""
    me = os.getpid()
    script = ("yes >/dev/null & y=$!; "
              "while kill -0 %d 2>/dev/null && kill -0 $y 2>/dev/null; do sleep 1; done; "
              "kill $y 2>/dev/null" % me)
    for _ in range(k):
        LOAD_GROUPS.append(subprocess.Popen(["sh", "-c", script], stdout=subprocess.DEVNULL,
                                            stderr=subprocess.DEVNULL, start_new_session=True))
    time.sleep(1.0)


def sh(cmd):
    try:
        return subprocess.run(cmd, capture_output=True, text=True, timeout=30).stdout.strip()
    except Exception:
        return ""


def machine():
    m = {"system": platform.system(), "release": platform.release(), "machine": platform.machine(),
         "python": platform.python_version(), "cpus": os.cpu_count()}
    if m["system"] == "Linux":
        try:
            with open("/proc/cpuinfo") as f:
                for line in f:
                    if line.startswith("model name"):
                        m["cpu"] = line.split(":", 1)[1].strip()
                        break
            with open("/proc/meminfo") as f:
                m["ram_gib"] = round(int(f.readline().split()[1]) / 1048576, 1)
        except OSError:
            pass
        gpu = sh(["nvidia-smi", "--query-gpu=name,driver_version", "--format=csv,noheader"])
        if gpu:
            m["gpu"] = gpu
    elif m["system"] == "Darwin":
        m["cpu"] = sh(["sysctl", "-n", "machdep.cpu.brand_string"])
        mem = sh(["sysctl", "-n", "hw.memsize"])
        if mem.isdigit():
            m["ram_gib"] = round(int(mem) / 2**30, 1)
        m["macos"] = sh(["sw_vers", "-productVersion"])
    return m


# ---------------------------------------------------------------- one run


def time_prefix():
    if platform.system() == "Darwin":
        return ["/usr/bin/time", "-l"]
    return ["/usr/bin/time", "-v"]


def parse_time(stderr):
    """Peak RSS in KiB and the elapsed seconds /usr/bin/time reported."""
    rss = None
    elapsed = None
    m = re.search(r"Maximum resident set size \(kbytes\):\s*(\d+)", stderr)
    if m:
        rss = int(m.group(1))
    m = re.search(r"Elapsed \(wall clock\) time.*:\s*(?:(\d+):)?(\d+):(\d+(?:\.\d+)?)\s*$", stderr, re.M)
    if m:
        elapsed = int(m.group(1) or 0) * 3600 + int(m.group(2)) * 60 + float(m.group(3))
    m = re.search(r"^\s*(\d+)\s+maximum resident set size", stderr, re.M)
    if m:  # macOS reports bytes
        rss = int(m.group(1)) // 1024
    m = re.search(r"^\s*(\d+(?:\.\d+)?)\s+real", stderr, re.M)
    if m:
        elapsed = float(m.group(1))
    return rss, elapsed


def tool_command(tool, args, src, out, ref=None, depth16=False):
    env = {}
    if args.threads:
        # Both tools get the same rayon size; Aurora's --threads also sizes
        # its PSD encode pool and its pool composite.
        env["RAYON_NUM_THREADS"] = str(args.threads)
    if tool.startswith("aurora"):
        cmd = [args.aurora, src, "--out", out, "--scratch", args.scratch]
        if tool == "aurora-pool":
            cmd += ["--composite", "pool"]
        elif tool == "aurora-gpu":
            cmd += ["--gpu"]
        if args.threads:
            cmd += ["--threads", str(args.threads)]
        if depth16:
            cmd += ["--depth", "16"]
        if ref:
            cmd += ["--reference-out", ref]
        return cmd, env
    return [args.photocraft, "convert", src, out], env


def run_one(tool, args, src, out, ref=None, depth16=False):
    cmd, extra_env = tool_command(tool, args, src, out, ref, depth16)
    env = dict(os.environ)
    env.update(extra_env)
    if os.path.exists(out):
        os.remove(out)
    started = time.monotonic()
    row = {"tool": tool, "cmd": " ".join(cmd)}
    try:
        p = subprocess.Popen(time_prefix() + cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                             env=env, text=True, start_new_session=True)
        try:
            stdout, stderr = p.communicate(timeout=args.timeout)
        except subprocess.TimeoutExpired:
            os.killpg(p.pid, signal.SIGKILL)
            stdout, stderr = p.communicate()
            row.update(status="timeout", wall_s=round(time.monotonic() - started, 3),
                       error="no result within %d s" % args.timeout)
            return row
    except OSError as e:
        row.update(status="error", error=str(e))
        return row
    row["wall_s"] = round(time.monotonic() - started, 3)
    rss, elapsed = parse_time(stderr)
    row["peak_rss_mib"] = round(rss / 1024, 1) if rss is not None else None
    row["time_elapsed_s"] = elapsed
    row["exit"] = p.returncode
    if tool.startswith("aurora"):
        line = next((l for l in stdout.splitlines() if l.startswith("{")), None)
        if line:
            try:
                row["aurora"] = json.loads(line)
            except ValueError:
                pass
    tail = [l for l in stderr.splitlines() if l.strip() and not l.startswith("\t")
            and "Command being timed" not in l]
    row["stderr_tail"] = tail[-4:]
    if p.returncode != 0:
        row["status"] = "crash" if p.returncode < 0 or p.returncode > 128 else "failed"
        err = row.get("aurora", {}).get("error") if tool.startswith("aurora") else None
        row["error"] = err or (" | ".join(tail[-2:]) if tail else "exit %d" % p.returncode)
    elif not os.path.exists(out):
        row.update(status="failed", error="exit 0 but no output file")
    else:
        row["status"] = "ok"
        row["output_bytes"] = os.path.getsize(out)
        with open(out, "rb") as f:
            row["sha256"] = hashlib.sha256(f.read()).hexdigest()
    return row


def has_real_merged_data(path):
    """psd_audit_tests.rs `audit_has_real_merged_data`: resource 1057's byte.

    Reads the header and the whole image resource section (no size cap)."""
    try:
        with open(path, "rb") as f:
            head = f.read(30)
            if len(head) < 30:
                return True
            mode_len = int.from_bytes(head[26:30], "big")
            f.seek(26 + 4 + mode_len)
            res_len = int.from_bytes(f.read(4), "big")
            res = f.read(res_len)
        at = 0
        while at + 8 <= len(res) and res[at:at + 4] == b"8BIM":
            rid = int.from_bytes(res[at + 4:at + 6], "big")
            name_len = res[at + 6]
            data_at = at + 6 + ((1 + name_len + 1) & ~1)
            if data_at + 4 > len(res):
                break
            length = int.from_bytes(res[data_at:data_at + 4], "big")
            body = data_at + 4
            if rid == 1057:
                return body + 4 >= len(res) or res[body + 4] != 0
            at = body + length + (length & 1)
        return True
    except OSError:
        return True


def png_header(path):
    """Dimensions, bit depth, colour type and colour space of a PNG, without decoding it."""
    try:
        with open(path, "rb") as f:
            data = f.read(1 << 20)
        w, h, depth, colour = struct.unpack(">IIBB", data[16:26])
        space, desc = "srgb", None
        i = data.find(b"iCCP")
        if i > 0 and data.find(b"sRGB", 0, i) < 0:
            (n,) = struct.unpack(">I", data[i - 4:i])
            desc = pngread.icc_description(data[i + 4:i + 4 + n])
            if "srgb" not in desc.lower():
                space = "embedded"
        return {"width": w, "height": h, "bit_depth": depth, "colour_type": colour, "colour": space, "icc": desc}
    except (OSError, struct.error, zlib.error):
        return None


def over_white(rgba):
    out = bytearray(len(rgba))
    for i in range(0, len(rgba), 4):
        a = rgba[i + 3]
        for c in range(3):
            out[i + c] = (rgba[i + c] * a + 255 * (255 - a) + 127) // 255
        out[i + 3] = 255
    return out


def sample(rgba, step):
    if step <= 1:
        return rgba
    out = bytearray()
    for i in range(0, len(rgba), 4 * step):
        out += rgba[i:i + 4]
    return out


def decode(path, args):
    w, h, info, rgba = pngread.read_png_rgba8(path)
    step = max(1, -(-w * h // args.verify_max_pixels))
    return {"w": w, "h": h, "info": info, "rgba": sample(rgba, step), "step": step}


def compare_images(a, b):
    if (a["w"], a["h"]) != (b["w"], b["h"]):
        return {"dimensions_differ": [[a["w"], a["h"]], [b["w"], b["h"]]]}
    if a["step"] != b["step"]:
        return {"error": "different sampling"}
    c = pngread.compare_rgba8(a["rgba"], b["rgba"])
    if a["info"]["colour"] != b["info"]["colour"]:
        c["colour_spaces_differ"] = [a["info"]["colour"], b["info"]["colour"]]
    return c


def against_reference(img, refs):
    """Score against the reference in the colour space the image is in."""
    space = img["info"]["colour"]
    ref = refs.get(space) or refs.get("srgb")
    if (img["w"], img["h"]) != (ref["w"], ref["h"]):
        return {"dimensions_differ": [[img["w"], img["h"]], [ref["w"], ref["h"]]]}
    c = pngread.compare_rgba8(over_white(img["rgba"]), ref["rgba"])
    c["score"] = round(sum(c["mean"][:3]) / 3, 4)
    c["reference_space"] = space if space in refs else "srgb"
    return c


def aurora_conversion(res):
    """What Aurora's open converted: (known, source description or None).

    From the run's own `colour_converted_from`; for results written before
    that field existed, a PSD's stored-profile reference (written only when
    the open converted) stands in, and anything else is unknown."""
    rows = [r for r in res.get("verify_runs", []) + res.get("warmup", []) if r.get("aurora", {}).get("ok")]
    for r in rows:
        if "colour_converted_from" in r["aurora"]:
            return True, r["aurora"]["colour_converted_from"]
    for r in rows:
        if r["aurora"].get("reference_written"):
            return True, ("(the file's profile)" if r["aurora"].get("reference_stored_written") else None)
    return False, None


def apply_colour_policy(v, conversion):
    """Colour notes, and whether the A-vs-P pair is comparable.

    PhotoCraft keeps a file's profile and embeds it. When Aurora converted
    the file to sRGB the two outputs are in different spaces (not compared);
    when Aurora did not convert (a grey profile, which it does not apply, or
    a profile it ignored) both hold the file's own values and are compared."""
    known, converted = conversion
    v["notes"] = [n for n in v.get("notes", []) if not n.startswith("colour:")]
    pc_head = v["headers"].get("photocraft") or {}
    if pc_head.get("colour") != "embedded":
        return
    pair = v.get("pair") or {}
    if known and not converted:
        pair.pop("colour_spaces_differ", None)
        v["notes"].append("colour: PhotoCraft tags '%s'; Aurora did not convert (profile not applied), "
                          "so both hold the same unconverted values" % pc_head.get("icc"))
    elif known:
        v["notes"].append("colour: Aurora converted %s to sRGB in its timed open; PhotoCraft kept '%s'"
                          % (converted if converted.startswith("(") else "'%s'" % converted, pc_head.get("icc")))
    else:
        v["notes"].append("colour: PhotoCraft tags '%s'; whether Aurora converted is not recorded"
                          % pc_head.get("icc"))


def verify(args, warm, au16, ref, conversion=(False, None)):
    """warm: {tool: its warm-up PNG}; au16: Aurora's 16-bit PNG (or None);
    conversion: `aurora_conversion` of this file."""
    v = {"headers": {t: png_header(p) for t, p in warm.items()}, "notes": []}
    images = {}

    def load(key, path):
        try:
            images[key] = decode(path, args)
        except Exception as e:  # noqa: BLE001
            v.setdefault("decode_errors", {})[key] = str(e)

    a8 = warm.get("aurora-export")
    if au16:
        v["headers"]["aurora (16-bit verify run)"] = png_header(au16)
        load("aurora16", au16)
    if a8:
        load("aurora8", a8)
    if "photocraft" in warm:
        load("photocraft", warm["photocraft"])
    # Other Aurora modes: bit-identical to aurora-export's PNG, or compared.
    sha = lambda p: hashlib.sha256(open(p, "rb").read()).hexdigest()  # noqa: E731
    for t, p in warm.items():
        if t.startswith("aurora") and t != "aurora-export" and a8:
            same = sha(p) == sha(a8)
            v.setdefault("same_bytes_as_aurora_export", {})[t] = same
            if not same and "aurora16" in images:
                try:
                    v.setdefault("vs_aurora16", {})[t] = compare_images(decode(p, args), images["aurora16"])
                except Exception as e:  # noqa: BLE001
                    v.setdefault("decode_errors", {})[t] = str(e)
    if "aurora16" in images and "aurora8" in images:
        v["dither_floor"] = compare_images(images["aurora8"], images["aurora16"])
    au = images.get("aurora16") or images.get("aurora8")
    if au and "photocraft" in images:
        v["pair"] = compare_images(au, images["photocraft"])
        v["pair"]["aurora_side"] = "16-bit" if "aurora16" in images else "8-bit dithered"
    apply_colour_policy(v, conversion)
    if ref and os.path.exists(ref):
        refs = {}
        try:
            refs["srgb"] = decode(ref, args)
            stored = ref[:-4] + ".stored.png"
            if os.path.exists(stored):
                refs["embedded"] = decode(stored, args)
        except Exception as e:  # noqa: BLE001
            v.setdefault("decode_errors", {})["reference"] = str(e)
        if "srgb" in refs:
            r = v["reference"] = {}
            for key, label in (("aurora16", "aurora 16-bit"), ("aurora8", "aurora 8-bit dithered (timed)"),
                               ("photocraft", "photocraft")):
                if key in images:
                    r[label] = against_reference(images[key], refs)
    return v


def run_file(args, src, mode, tools, outdir, state):
    name = re.sub(r"[^A-Za-z0-9._-]+", "_", display(src))
    res = {"file": display(src), "path": src, "mode": mode, "runs": [], "warmup": [], "verify_runs": []}
    if not os.path.exists(src):
        res["missing"] = True
        return res
    res["bytes"] = os.path.getsize(src)
    is_psd = src.lower().endswith((".psd", ".psb"))
    synthetic = os.path.abspath(src).startswith(os.path.join(os.path.abspath(args.work), "synth") + os.sep)
    warm = {}
    for tool in tools:  # warm-up, reported separately
        out = os.path.join(outdir, tool, name + ".warm.png")
        os.makedirs(os.path.dirname(out), exist_ok=True)
        row = run_one(tool, args, src, out)
        res["warmup"].append(row)
        if row.get("status") == "ok":
            warm[tool] = out
            if mode == "idle":
                state["idle_hash"][(src, tool)] = row["sha256"]
    au16 = ref = None
    au_tool = "aurora-export" if "aurora-export" in tools else next((t for t in tools if t.startswith("aurora")), None)
    if mode == "idle" and au_tool:
        # Untimed verification run: the same pipeline, written as 16 bits.
        if is_psd and not synthetic and has_real_merged_data(src):
            ref = os.path.join(outdir, "ref", name + ".png")
            os.makedirs(os.path.dirname(ref), exist_ok=True)
        au16 = os.path.join(outdir, au_tool, name + ".verify16.png")
        row = run_one(au_tool, args, src, au16, ref=ref, depth16=True)
        res["verify_runs"].append(row)
        if row.get("status") != "ok":
            au16 = None
    for r in range(args.runs):
        order = tools[r % len(tools):] + tools[:r % len(tools)]
        for tool in order:
            out = os.path.join(outdir, tool, "%s.run%d.png" % (name, r))
            row = run_one(tool, args, src, out)
            row["round"] = r
            res["runs"].append(row)
            if os.path.exists(out):
                os.remove(out)
    res["reference"] = bool(ref and os.path.exists(ref))
    if mode == "idle" and warm:
        res["verify"] = verify(args, warm, au16, ref, aurora_conversion(res))
    # Every timed run (and this mode's warm-up) against the idle warm-up output.
    for tool in tools:
        idle = state["idle_hash"].get((src, tool))
        rows = [x for x in res["warmup"] + res["runs"] if x["tool"] == tool and x.get("status") == "ok"]
        if not rows:
            res.setdefault("hash_check", {})[tool] = "no output"
        elif idle is None:
            res.setdefault("hash_check", {})[tool] = "no idle output"
        else:
            bad = sum(1 for x in rows if x["sha256"] != idle)
            res.setdefault("hash_check", {})[tool] = "match" if bad == 0 else "%d differ" % bad
    if not args.keep_outputs:
        for p in list(warm.values()) + [au16]:
            if p and os.path.exists(p):
                os.remove(p)
    return res


def collect_inputs(args):
    files = []
    if args.set in ("representative", "all"):
        files += [os.path.join(REPO, p) for p in REPRESENTATIVE]
        synth = os.path.join(args.work, "synth")
        files += [os.path.join(synth, "synthetic-%smp-%s" % (mp, kind))
                  for mp in args.synthetic_sizes.split(",") if mp
                  for kind in ("layers.psd", "flat.png")]
    if args.set in ("corpus", "all"):
        for root, _dirs, names in os.walk(os.path.join(REPO, "corpora", "psd")):
            for n in names:
                if n.lower().endswith((".psd", ".psb")):
                    files.append(os.path.join(root, n))
    files += args.files
    seen = set()
    out = []
    for f in sorted(files) if args.set == "corpus" else files:
        if f not in seen:
            seen.add(f)
            out.append(f)
    return out


def display(path):
    return os.path.relpath(path, REPO).replace("corpora/psd/reference/psd-tools-fixtures/", "psd-tools/") \
        if path.startswith(REPO) else os.path.basename(path)


def summarize_runs(rows, key):
    vals = [r[key] for r in rows if r.get("status") == "ok" and r.get(key) is not None]
    if not vals:
        return None
    return {"min": min(vals), "median": statistics.median(vals), "max": max(vals), "n": len(vals)}


def fmt_range(s, unit=""):
    if not s:
        return "-"
    if s["n"] == 1:
        return "%s%s (n=1)" % (fmt(s["min"]), unit)
    return "%s-%s%s (n=%d)" % (fmt(s["min"]), fmt(s["max"]), unit, s["n"])


def fmt(x):
    if isinstance(x, float):
        return "%.3g" % x if x < 10 else "%.0f" % x if x >= 100 else "%.1f" % x
    return str(x)


CAVEATS = """## Read these before the numbers

* **Not the same work per run.** Aurora's timed run is its Save As PNG path: an 8-bit
  *dithered* encode, then a safe write (a temp file, `fsync`, a re-decode of the file, a
  rename and a directory `fsync`). PhotoCraft writes its PNG directly, with its own encoder and
  bit depth (see the bit-depth and byte columns). Aurora's phases (open, composite, encode,
  write) are in the phase table. At 60 MP the 8-bit dithered encode is most of Aurora's time.
* **`aurora-export` composites on one thread**, as the app's export does today; `aurora-pool`
  runs the same fold on `--threads` threads. `--threads` sizes Aurora's PSD encode pool and its
  pool composite; both tools also get `RAYON_NUM_THREADS`.
* **Colour conversion.** Aurora converts a tagged file to sRGB inside its timed open; PhotoCraft
  keeps the source profile and embeds it. Files where this happens carry a note. On the Nyepi
  template it is most of Aurora's open (about 10 s of 11.5 s, measured on a copy with the
  profile resource renamed).
* **Warm page cache.** One warm-up run per tool comes first (reported separately), so the
  measured runs read the input from the OS page cache.
* **Peak RSS** includes each tool's caches: Aurora's tile store keeps up to a quarter of RAM
  (at most 8 GiB) resident, plus its effects coverage cache; `aurora-pool` also holds every
  composited tile until it assembles the image.
* **Pixel checks.** Aurora is checked from a 16-bit output of the same pipeline (one extra,
  untimed run; `--depth 16`), because its timed 8-bit output is dithered; the dither floor
  (timed 8-bit against 16-bit) is shown next to it, and the timed outputs are tied to the
  checked pictures by SHA-256 (`hash`). PhotoCraft is checked from its timed output (its
  warm-up run). The Photoshop reference is the PSD's own merged image, decoded **by Aurora**
  (the psd_fidelity_audit's method), and exists only for files Aurora opens: files Aurora
  refuses have no fidelity score for either tool. An output with no ICC profile, or one whose
  ICC description names sRGB, is treated as sRGB (the harness reads the description, not the
  profile); other profiles are scored against the merged image as stored.
* **Never a single sample.** Cells give min-max over the measured runs (n is shown).
"""


def check_cell(res, idle_res, tools):
    """A short pixel-check marker for a timing row."""
    parts = []
    v = (idle_res or {}).get("verify") or {}
    ref = v.get("reference") or {}
    a, p = ref.get("aurora 16-bit"), ref.get("photocraft")
    if a or p:
        parts.append("vs PS: A %s, P %s" % (("%.2f" % a["score"]) if a and "score" in a else "-",
                                            ("%.2f" % p["score"]) if p and "score" in p else "-"))
    pair = v.get("pair")
    if pair and "mean" in pair:
        if pair.get("colour_spaces_differ"):
            parts.append("A~P: colour spaces differ")
        else:
            parts.append("A~P: mean %.2f max %d" % (max(pair["mean"][:3]), max(pair["max"])))
    elif pair and "dimensions_differ" in pair:
        parts.append("A~P: SIZE DIFFERS")
    if not v:
        parts.append("unverified")
    hc = res.get("hash_check", {})
    bad = [t for t in tools if hc.get(t) not in ("match",)]
    parts.append("hash ok" if not bad else "hash: " + ", ".join("%s %s" % (t, hc.get(t)) for t in bad))
    return "; ".join(parts)


def bucket(s):
    return "<1" if s < 1 else "<3" if s < 3 else "<10" if s < 10 else ">=10"


def derived_counts(results):
    """The aggregate counts quoted in FINDINGS.md, computed here so they are reproducible."""
    idle = [r for r in results if r["mode"] == "idle" and r.get("verify")]
    pairs = [r["verify"]["pair"] for r in idle if r["verify"].get("pair")]
    if not pairs:
        return []
    dims = sum(1 for c in pairs if "dimensions_differ" in c)
    differ = [c for c in pairs if c.get("colour_spaces_differ")]
    same = [c for c in pairs if "mean" in c and not c.get("colour_spaces_differ")]
    notes = [n for r in idle for n in r["verify"].get("notes", [])]
    unconverted = sum(1 for n in notes if "did not convert" in n)
    unknown = sum(1 for n in notes if "not recorded" in n)
    L = ["### Derived counts (idle)", "",
         "- files with both tools' output: %d; dimensions agree on %d, differ on %d"
         % (len(pairs), len(pairs) - dims, dims),
         "- pairs in different colour spaces (Aurora converted, PhotoCraft kept the profile), not compared: %d"
         % len(differ),
         "- pairs where PhotoCraft tags a profile Aurora did not apply (same values, compared): %d" % unconverted]
    if unknown:
        L.append("- pairs where PhotoCraft tags a profile and Aurora's conversion is not recorded: %d" % unknown)
    L.append("- compared pairs with more than 1 %% of pixels more than 2 levels apart: %d of %d"
             % (sum(1 for c in same if c["share_over_2"] > 0.01), len(same)))
    a, p = {}, {}
    for r in idle:
        ref = r["verify"].get("reference") or {}
        if "score" in (ref.get("aurora 16-bit") or {}):
            a[r["file"]] = ref["aurora 16-bit"]
        if "score" in (ref.get("photocraft") or {}):
            p[r["file"]] = ref["photocraft"]
    both = sorted(set(a) & set(p))
    if both:
        L.append("- vs Photoshop, both scored on %d files: PhotoCraft closer by more than 0.5 level on %d, "
                 "Aurora (16-bit) on %d, within 0.5 on %d"
                 % (len(both), sum(1 for f in both if p[f]["score"] < a[f]["score"] - 0.5),
                    sum(1 for f in both if a[f]["score"] < p[f]["score"] - 0.5),
                    sum(1 for f in both if abs(a[f]["score"] - p[f]["score"]) <= 0.5)))
        for name, m in (("Aurora 16-bit", a), ("PhotoCraft", p)):
            L.append("- %s: score 0.0000 at 4 decimals (mean error under 0.00005 level) on %d files; "
                     "pixel-identical to the reference (max 0) on %d"
                     % (name, sum(1 for f in both if m[f]["score"] == 0),
                        sum(1 for f in both if max(m[f]["max"][:3]) == 0)))
    L.append("")
    return L


def write_reports(args, meta, results):
    os.makedirs(args.out, exist_ok=True)
    with open(os.path.join(args.out, "results.json"), "w") as f:
        json.dump({"meta": meta, "results": results}, f, indent=1)
    with open(os.path.join(args.out, "runs.csv"), "w", newline="") as f:
        w = csv.writer(f)
        w.writerow(["file", "mode", "tool", "kind", "round", "status", "wall_s", "peak_rss_mib", "output_bytes",
                    "aurora_open_ms", "aurora_composite_ms", "aurora_encode_ms", "aurora_write_ms",
                    "aurora_threads", "sha256", "error"])
        for res in results:
            for kind in ("warmup", "verify_runs", "runs"):
                for row in res.get(kind, []):
                    a = row.get("aurora", {}).get("ms", {})
                    w.writerow([res["file"], res["mode"], row["tool"], kind, row.get("round", ""),
                                row.get("status"), row.get("wall_s"), row.get("peak_rss_mib"),
                                row.get("output_bytes"), a.get("open"), a.get("composite"), a.get("encode"),
                                a.get("write"), row.get("aurora", {}).get("threads", ""),
                                row.get("sha256", ""), row.get("error", "")])
    idle = {r["path"]: r for r in results if r["mode"] == "idle"}
    tools = meta["tool_list"]
    L = ["# Aurora vs PhotoCraft: headless open -> flatten -> PNG", ""]
    L.append("Generated %s by `spike/photocraft-compare/compare.py` (harness commit %s)."
             % (meta["date"], meta.get("harness_commit", "?")))
    L += ["", "## Machine and tools", ""]
    for k, v in meta["machine"].items():
        L.append("- %s: %s" % (k, v))
    for k in ("aurora", "photocraft"):
        L.append("- %s: %s" % (k, meta["tools"].get(k)))
    L.append("- threads: %s; runs per file and mode: %d (+1 warm-up per tool); timeout %d s; load: %d `yes`"
             % (args.threads or "each tool's default", args.runs, args.timeout, args.load))
    L += ["", CAVEATS]
    for mode in meta["modes"]:
        L.append("## %s: wall time (s) and peak RSS (MiB), min-max over measured runs" % mode)
        L.append("")
        L.append("| file | size | px |" + "".join(" %s wall | %s RSS |" % (t, t) for t in tools) + " pixel check |")
        L.append("|" + "---|" * (4 + 2 * len(tools)))
        for res in results:
            if res["mode"] != mode:
                continue
            px = "-"
            for row in res["runs"] + res["warmup"]:
                a = row.get("aurora")
                if a and a.get("ok"):
                    px = "%dx%d" % (a["width"], a["height"])
                    break
            cells = ""
            for t in tools:
                rows = [r for r in res["runs"] if r["tool"] == t]
                bad = [r for r in rows if r.get("status") != "ok"]
                wall = fmt_range(summarize_runs(rows, "wall_s"))
                if bad:
                    wall += " **%d %s**" % (len(bad), bad[0].get("status"))
                cells += " %s | %s |" % (wall, fmt_range(summarize_runs(rows, "peak_rss_mib")))
            size = "%.1f MB" % (res.get("bytes", 0) / 1e6) if "bytes" in res else "missing"
            L.append("| %s | %s | %s |%s %s |" % (res["file"], size, px, cells,
                                                 check_cell(res, idle.get(res["path"]), tools)))
        L.append("")
        L.append("### %s: Aurora's own phases (ms, min-max over measured runs)" % mode)
        L.append("")
        L.append("| file | tool | open | composite | encode | write | layer tiles |")
        L.append("|---|---|---|---|---|---|---|")
        for res in results:
            if res["mode"] != mode:
                continue
            for t in [t for t in tools if t.startswith("aurora")]:
                rows = [r for r in res["runs"] if r["tool"] == t and r.get("aurora", {}).get("ok")]
                if not rows:
                    continue
                ph = lambda k: fmt_range(summarize_runs([{**r, "status": "ok", "v": r["aurora"]["ms"][k]} for r in rows], "v"))  # noqa: E731,E501
                L.append("| %s | %s | %s | %s | %s | %s | %s |" % (
                    res["file"], t, ph("open"), ph("composite"), ph("encode"), ph("write"),
                    rows[0]["aurora"].get("layer_tiles")))
        L.append("")
    L += ["## Pixel checks (idle)", "",
          "Levels are 8-bit, colour premultiplied; `share>2` = pixels with any channel more than 2 levels off. "
          "A = Aurora's 16-bit verification output, P = PhotoCraft's output. Scores against Photoshop are "
          "the mean of the R, G, B mean errors over white (the audit's score); [space] = which reference.", "",
          "| file | Aurora timed PNG bits/bytes | PhotoCraft PNG bits/bytes | A vs P mean RGB / max / share>2 | Aurora dither floor (mean) | "
          "A vs PS | A 8-bit timed vs PS | P vs PS | notes |",
          "|---|---|---|---|---|---|---|---|---|"]
    for res in results:
        v = res.get("verify")
        if not v or res["mode"] != "idle":
            continue
        h = v.get("headers", {})
        bytes_of = lambda t: next((r.get("output_bytes") for r in res["warmup"] if r["tool"] == t and r.get("status") == "ok"), None)  # noqa: E731,E501
        ab = "%s / %s" % ((h.get("aurora-export") or {}).get("bit_depth", "-"), bytes_of("aurora-export"))
        pb = "%s / %s" % ((h.get("photocraft") or {}).get("bit_depth", "-"), bytes_of("photocraft"))
        pair = v.get("pair") or {}
        if "mean" in pair:
            ps = "%s / %s / %.4f" % ([round(x, 3) for x in pair["mean"][:3]], max(pair["max"]), pair["share_over_2"])
            if pair.get("colour_spaces_differ"):
                ps += " (colour spaces differ: not comparable)"
        elif "dimensions_differ" in pair:
            ps = "**dimensions differ %s**" % pair["dimensions_differ"]
        else:
            ps = "-"
        df = v.get("dither_floor") or {}
        dfs = "%.3f" % max(df["mean"][:3]) if "mean" in df else "-"
        ref = v.get("reference") or {}
        sc = lambda k: ("%.3f [%s]" % (ref[k]["score"], ref[k]["reference_space"]) if k in ref and "score" in ref[k] else ("**size differs**" if k in ref else "-"))  # noqa: E731,E501
        notes = list(v.get("notes", []))
        for t, same in (v.get("same_bytes_as_aurora_export") or {}).items():
            notes.append("%s %s aurora-export's PNG" % (t, "bit-identical to" if same else "DIFFERS from"))
        for k, e in (v.get("decode_errors") or {}).items():
            notes.append("%s undecodable: %s" % (k, e))
        L.append("| %s | %s | %s | %s | %s | %s | %s | %s | %s |" % (
            res["file"], ab, pb, ps, dfs, sc("aurora 16-bit"), sc("aurora 8-bit dithered (timed)"),
            sc("photocraft"), "; ".join(notes)))
    L.append("")
    scores = {"aurora 16-bit": [], "aurora 8-bit dithered (timed)": [], "photocraft": []}
    for res in results:
        ref = (res.get("verify") or {}).get("reference") or {}
        for k in scores:
            if k in ref and "score" in ref[k]:
                scores[k].append(ref[k]["score"])
    if any(scores.values()):
        L += ["### Scores against Photoshop, all files with a reference", "",
              "| output | n | median | <1 | <3 | <10 | >=10 |", "|---|---|---|---|---|---|---|"]
        for k, vals in scores.items():
            if vals:
                b = {x: sum(1 for s in vals if bucket(s) == x) for x in ("<1", "<3", "<10", ">=10")}
                L.append("| %s | %d | %.3f | %d | %d | %d | %d |" % (k, len(vals), statistics.median(vals),
                                                                b["<1"], b["<3"], b["<10"], b[">=10"]))
        L.append("")
    L += derived_counts(results)
    L += ["## Failures (every non-ok run, warm-up and verification runs included)", ""]
    any_fail = False
    for res in results:
        if res.get("missing"):
            L.append("- %s: input file missing" % res["file"])
            any_fail = True
        for kind in ("warmup", "verify_runs", "runs"):
            for row in res.get(kind, []):
                if row.get("status") != "ok":
                    any_fail = True
                    what = {"warmup": "warm-up", "verify_runs": "16-bit verify run"}.get(kind) or "round %d" % row["round"]
                    L.append("- %s [%s, %s, %s]: %s: %s" % (res["file"], res["mode"], row["tool"], what,
                                                          row.get("status"), (row.get("error") or "")[:300]))
        for t, hc in res.get("hash_check", {}).items():
            if hc not in ("match", "no output"):
                any_fail = True
                L.append("- %s [%s, %s]: output hash vs idle: %s" % (res["file"], res["mode"], t, hc))
    if not any_fail:
        L.append("- none")
    L.append("")
    with open(os.path.join(args.out, "summary.md"), "w") as f:
        f.write("\n".join(L))


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--rewrite", help="only regenerate summary.md/runs.csv from this results.json")
    ap.add_argument("--aurora", help="aurora-headless binary")
    ap.add_argument("--photocraft", help="photocraft-cli binary")
    ap.add_argument("--work", help="work dir (synthetic inputs in WORK/synth)")
    ap.add_argument("--out", required=True, help="results directory")
    ap.add_argument("--set", default="representative", choices=["representative", "corpus", "all", "none"])
    ap.add_argument("--files", nargs="*", default=[], help="extra input files")
    ap.add_argument("--tools", default="aurora-export,aurora-pool,photocraft")
    ap.add_argument("--modes", default="idle,loaded")
    ap.add_argument("--runs", type=int, default=3)
    ap.add_argument("--load", type=int, default=8, help="number of `yes` processes in the loaded mode")
    ap.add_argument("--timeout", type=int, default=300)
    ap.add_argument("--threads", type=int, default=os.cpu_count(),
                    help="threads for both tools (RAYON_NUM_THREADS, and Aurora's --threads); 0 = defaults")
    ap.add_argument("--synthetic-sizes", default="24,60")
    ap.add_argument("--verify-max-pixels", type=int, default=6_000_000,
                    help="compare every k-th pixel above this many (the decode is always whole)")
    ap.add_argument("--keep-outputs", action="store_true")
    ap.add_argument("--aurora-version", default="")
    ap.add_argument("--photocraft-version", default="")
    ap.add_argument("--harness-commit", default="")
    args = ap.parse_args()
    if args.rewrite:
        with open(args.rewrite) as f:
            data = json.load(f)
        old = data["meta"].get("args", {})
        for k in ("threads", "runs", "timeout", "load"):
            if k in old:
                setattr(args, k, old[k])
        for res in data["results"]:
            if res.get("verify"):
                apply_colour_policy(res["verify"], aurora_conversion(res))
        write_reports(args, data["meta"], data["results"])
        print(os.path.join(args.out, "summary.md"))
        return
    if not (args.aurora and args.photocraft and args.work):
        ap.error("--aurora, --photocraft and --work are required")
    os.makedirs(args.work, exist_ok=True)
    # One harness at a time per work directory: two would share outputs and
    # scratch, and would load each other's timings.
    lock = open(os.path.join(args.work, ".compare.lock"), "w")
    try:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except OSError:
        sys.exit("another compare.py is running in %s" % args.work)
    args.scratch = os.path.join(args.work, "aurora-scratch")
    os.makedirs(args.scratch, exist_ok=True)
    tools = [t for t in args.tools.split(",") if t]
    modes = [m for m in args.modes.split(",") if m]
    meta = {"date": datetime.datetime.now().isoformat(timespec="seconds"), "machine": machine(),
            "tools": {"aurora": args.aurora_version, "photocraft": args.photocraft_version},
            "harness_commit": args.harness_commit, "tool_list": tools, "modes": modes, "argv": sys.argv,
            "args": {"threads": args.threads, "runs": args.runs, "timeout": args.timeout, "load": args.load}}
    inputs = collect_inputs(args)
    outdir = os.path.join(args.work, "outputs")
    results = []
    state = {"idle_hash": {}}
    for mode in modes:
        if mode == "loaded":
            start_load(args.load)
        try:
            for i, src in enumerate(inputs):
                print("[%s %d/%d] %s" % (mode, i + 1, len(inputs), display(src)), file=sys.stderr, flush=True)
                results.append(run_file(args, src, mode, tools, outdir, state))
                write_reports(args, meta, results)
        finally:
            kill_load()
    write_reports(args, meta, results)
    shutil.rmtree(args.scratch, ignore_errors=True)
    print(os.path.join(args.out, "summary.md"))


if __name__ == "__main__":
    main()
