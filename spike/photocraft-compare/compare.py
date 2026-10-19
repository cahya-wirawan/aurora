#!/usr/bin/env python3
"""Run Aurora and PhotoCraft over the same files and compare (python3 stdlib only).

Every run is "open the file, flatten it, write a PNG", timed from outside with
the same ``/usr/bin/time`` measure for both tools (``-v`` on Linux, ``-l`` on
macOS), plus each tool's own wall clock from this script. Runs are interleaved
(the tool order rotates each round), one warm-up run per tool and file is
reported separately, and nothing is dropped: a crash, a timeout or a refused
file is a row with its reason.

Outputs are verified: each tool's first measured PNG is decoded by
``pngread.py`` (an independent decoder) and compared with the other tool's and,
for a PSD with a real merged image, with Photoshop's merged image (written by
Aurora's ``--reference-out``, i.e. the psd_fidelity_audit's reference, compared
over white like the audit). Later runs' outputs are checked by SHA-256 only.

See README.md for the commands and the fairness notes.
"""

import argparse
import atexit
import csv
import datetime
import hashlib
import json
import os
import platform
import re
import shutil
import signal
import statistics
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import pngread  # noqa: E402

REPO = os.path.abspath(os.path.join(HERE, "..", ".."))
FIXTURES = os.path.join(REPO, "corpora", "psd", "reference", "psd-tools-fixtures")
IMAGES = os.path.join(REPO, "images")

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

LOAD_PROCS = []


def kill_load():
    while LOAD_PROCS:
        p = LOAD_PROCS.pop()
        try:
            p.kill()
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
    for _ in range(k):
        LOAD_PROCS.append(subprocess.Popen(["yes"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL))
    time.sleep(1.0)


# ---------------------------------------------------------------- machine


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


def tool_command(tool, args, src, out, ref):
    if tool.startswith("aurora"):
        cmd = [args.aurora, src, "--out", out, "--scratch", args.scratch]
        if tool == "aurora-pool":
            cmd += ["--composite", "pool"]
        elif tool == "aurora-gpu":
            cmd += ["--gpu"]
        if args.threads:
            cmd += ["--threads", str(args.threads)]
        if ref:
            cmd += ["--reference-out", ref]
        return cmd, {}
    env = {}
    if args.threads:
        env["RAYON_NUM_THREADS"] = str(args.threads)
    return [args.photocraft, "convert", src, out], env


def run_one(tool, args, src, out, ref=None):
    cmd, extra_env = tool_command(tool, args, src, out, ref)
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
        with open(out, "rb") as f:
            row["sha256"] = hashlib.sha256(f.read()).hexdigest()
    return row


# ---------------------------------------------------------------- verification


def has_real_merged_data(path):
    """psd_audit_tests.rs `audit_has_real_merged_data`: resource 1057's byte."""
    try:
        with open(path, "rb") as f:
            head = f.read(1 << 20)
        be32 = lambda at: int.from_bytes(head[at:at + 4], "big") if at + 4 <= len(head) else None  # noqa: E731
        mode_len = be32(26)
        if mode_len is None:
            return True
        res_at = 30 + mode_len
        res_len = be32(res_at)
        if res_len is None:
            return True
        at = res_at + 4
        end = min(at + res_len, len(head))
        while at + 8 <= end and head[at:at + 4] == b"8BIM":
            rid = int.from_bytes(head[at + 4:at + 6], "big")
            name_len = head[at + 6]
            data_at = at + 6 + ((1 + name_len + 1) & ~1)
            length = be32(data_at)
            if length is None:
                break
            body = data_at + 4
            if rid == 1057:
                return body + 4 >= len(head) or head[body + 4] != 0
            at = body + length + (length & 1)
        return True
    except OSError:
        return True


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


def verify(args, outputs, ref):
    """Decode each tool's PNG, compare tools pairwise and with the reference."""
    decoded = {}
    v = {"tools": {}}
    for tool, path in outputs.items():
        try:
            w, h, info, rgba = pngread.read_png_rgba8(path)
            step = max(1, -(-w * h // args.verify_max_pixels))
            decoded[tool] = (w, h, sample(rgba, step), step)
            v["tools"][tool] = {"width": w, "height": h, **info, "sample_step": step}
        except Exception as e:  # noqa: BLE001
            v["tools"][tool] = {"decode_error": str(e)}
    names = sorted(decoded)
    v["pairs"] = {}
    for i, a in enumerate(names):
        for b in names[i + 1:]:
            (wa, ha, ra, sa), (wb, hb, rb, sb) = decoded[a], decoded[b]
            key = "%s vs %s" % (a, b)
            if (wa, ha) != (wb, hb):
                v["pairs"][key] = {"dimensions_differ": [[wa, ha], [wb, hb]]}
            else:
                v["pairs"][key] = pngread.compare_rgba8(ra, rb)
    if ref and os.path.exists(ref):
        try:
            w, h, _info, rr = pngread.read_png_rgba8(ref)
            v["reference"] = {"width": w, "height": h}
            for tool, (wt, ht, rt, st) in decoded.items():
                if (wt, ht) != (w, h):
                    v["reference"][tool] = {"dimensions_differ": [[wt, ht], [w, h]]}
                    continue
                c = pngread.compare_rgba8(over_white(rt), sample(rr, st))
                c["score"] = round(sum(c["mean"][:3]) / 3, 4)
                v["reference"][tool] = c
        except Exception as e:  # noqa: BLE001
            v["reference"] = {"decode_error": str(e)}
    return v


# ---------------------------------------------------------------- driver


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


def run_file(args, src, mode, tools, outdir, ref_cache):
    name = re.sub(r"[^A-Za-z0-9._-]+", "_", display(src))
    res = {"file": display(src), "path": src, "mode": mode, "runs": [], "warmup": []}
    if not os.path.exists(src):
        res["missing"] = True
        return res
    res["bytes"] = os.path.getsize(src)
    is_psd = src.lower().endswith((".psd", ".psb"))
    ref = None
    if is_psd and mode == "idle" and has_real_merged_data(src):
        ref = os.path.join(outdir, "ref", name + ".png")
        os.makedirs(os.path.dirname(ref), exist_ok=True)
    keep = {}
    for tool in tools:  # warm-up, reported separately
        out = os.path.join(outdir, tool, name + ".warm.png")
        os.makedirs(os.path.dirname(out), exist_ok=True)
        row = run_one(tool, args, src, out, ref if tool.startswith("aurora") and ref else None)
        res["warmup"].append(row)
        if row.get("status") == "ok":
            keep.setdefault(tool, out)
            if tool.startswith("aurora") and ref:
                ref_cache[src] = ref
    for r in range(args.runs):
        order = tools[r % len(tools):] + tools[:r % len(tools)]
        for tool in order:
            out = os.path.join(outdir, tool, "%s.run%d.png" % (name, r))
            row = run_one(tool, args, src, out)
            row["round"] = r
            res["runs"].append(row)
            if os.path.exists(out) and out != keep.get(tool):
                os.remove(out)
    ref = ref_cache.get(src)
    if mode == "idle" and keep:
        res["verify"] = verify(args, keep, ref if ref and os.path.exists(ref) else None)
    res["reference"] = bool(ref and os.path.exists(ref))
    # Same output every run?
    for tool in tools:
        hashes = {row.get("sha256") for row in res["warmup"] + res["runs"]
                  if row["tool"] == tool and row.get("status") == "ok"}
        res.setdefault("deterministic", {})[tool] = len(hashes) <= 1
    if not args.keep_outputs:
        for p in keep.values():
            if os.path.exists(p):
                os.remove(p)
    return res


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


def write_reports(args, meta, results):
    os.makedirs(args.out, exist_ok=True)
    with open(os.path.join(args.out, "results.json"), "w") as f:
        json.dump({"meta": meta, "results": results}, f, indent=1)
    with open(os.path.join(args.out, "runs.csv"), "w", newline="") as f:
        w = csv.writer(f)
        w.writerow(["file", "mode", "tool", "kind", "round", "status", "wall_s", "peak_rss_mib",
                    "aurora_open_ms", "aurora_composite_ms", "aurora_encode_ms", "aurora_write_ms",
                    "aurora_threads", "error"])
        for res in results:
            for kind in ("warmup", "runs"):
                for row in res.get(kind, []):
                    a = row.get("aurora", {}).get("ms", {})
                    w.writerow([res["file"], res["mode"], row["tool"], kind, row.get("round", ""),
                                row.get("status"), row.get("wall_s"), row.get("peak_rss_mib"),
                                a.get("open"), a.get("composite"), a.get("encode"), a.get("write"),
                                row.get("aurora", {}).get("threads", ""), row.get("error", "")])
    lines = ["# Aurora vs PhotoCraft: headless open -> flatten -> PNG", ""]
    lines.append("Generated %s by `spike/photocraft-compare/compare.py`." % meta["date"])
    lines.append("")
    lines.append("## Machine and tools")
    lines.append("")
    for k, v in meta["machine"].items():
        lines.append("- %s: %s" % (k, v))
    for k in ("aurora", "photocraft"):
        lines.append("- %s: %s" % (k, meta["tools"].get(k)))
    lines.append("- threads: %s; runs per file and mode: %d (+1 warm-up per tool); timeout %d s; load: %d `yes`"
                 % (args.threads or "each tool's default", args.runs, args.timeout, args.load))
    lines.append("")
    tools = meta["tool_list"]
    for mode in meta["modes"]:
        lines.append("## %s: wall time (s) and peak RSS (MiB), min-max over measured runs" % mode)
        lines.append("")
        head = "| file | size | px |" + "".join(" %s wall | %s RSS |" % (t, t) for t in tools)
        lines.append(head)
        lines.append("|" + "---|" * (3 + 2 * len(tools)))
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
            lines.append("| %s | %s | %s |%s" % (res["file"], size, px, cells))
        lines.append("")
        if mode == "idle":
            lines.append("### idle: Aurora's own phases (ms, min-max over measured runs)")
            lines.append("")
            lines.append("| file | tool | open | composite | encode | write | layer tiles |")
            lines.append("|---|---|---|---|---|---|---|")
            for res in results:
                if res["mode"] != mode:
                    continue
                for t in [t for t in tools if t.startswith("aurora")]:
                    rows = [r for r in res["runs"] if r["tool"] == t and r.get("aurora", {}).get("ok")]
                    if not rows:
                        continue
                    ph = lambda k: fmt_range(summarize_runs([{**r, "v": r["aurora"]["ms"][k]} for r in rows], "v"))  # noqa: E731,E501
                    lines.append("| %s | %s | %s | %s | %s | %s | %s |" % (
                        res["file"], t, ph("open"), ph("composite"), ph("encode"), ph("write"),
                        rows[0]["aurora"].get("layer_tiles")))
            lines.append("")
    lines.append("## Pixel checks (idle, first output of each tool)")
    lines.append("")
    lines.append("Mean/max |difference| per channel in 8-bit levels (colour premultiplied), share of pixels "
                 "with any channel off by more than 2. Reference = the PSD's own merged image (Photoshop's), "
                 "tool output over white, like Aurora's psd_fidelity_audit; score = mean of R, G, B means.")
    lines.append("")
    lines.append("| file | pair | dims | mean R,G,B,A | max R,G,B,A | share >2 |")
    lines.append("|---|---|---|---|---|---|")
    for res in results:
        v = res.get("verify")
        if not v:
            continue
        for key, c in v.get("pairs", {}).items():
            if "dimensions_differ" in c:
                lines.append("| %s | %s | **differ %s** | | | |" % (res["file"], key, c["dimensions_differ"]))
            else:
                lines.append("| %s | %s | same | %s | %s | %.4f |" % (res["file"], key, c["mean"], c["max"],
                                                                   c["share_over_2"]))
        ref = v.get("reference", {})
        for t in tools:
            c = ref.get(t)
            if not c:
                continue
            if "dimensions_differ" in c:
                lines.append("| %s | %s vs Photoshop | **differ %s** | | | |" % (res["file"], t, c["dimensions_differ"]))
            else:
                lines.append("| %s | %s vs Photoshop (score %.3f) | same | %s | %s | %.4f |" % (
                    res["file"], t, c["score"], c["mean"][:3], c["max"][:3], c["share_over_2"]))
        for t, info in v.get("tools", {}).items():
            if "decode_error" in info:
                lines.append("| %s | %s output | **undecodable: %s** | | | |" % (res["file"], t, info["decode_error"]))
    lines.append("")
    lines.append("## Failures (every non-ok run, warm-up included)")
    lines.append("")
    any_fail = False
    for res in results:
        if res.get("missing"):
            lines.append("- %s: input file missing" % res["file"])
            any_fail = True
        for row in res.get("warmup", []) + res.get("runs", []):
            if row.get("status") != "ok":
                any_fail = True
                lines.append("- %s [%s, %s, %s]: %s: %s" % (
                    res["file"], res["mode"], row["tool"], "warm-up" if "round" not in row else
                    "round %d" % row["round"], row.get("status"), (row.get("error") or "")[:300]))
        for t, same in res.get("deterministic", {}).items():
            if not same:
                lines.append("- %s [%s, %s]: output bytes differ between runs" % (res["file"], res["mode"], t))
    if not any_fail:
        lines.append("- none")
    lines.append("")
    with open(os.path.join(args.out, "summary.md"), "w") as f:
        f.write("\n".join(lines))


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--aurora", required=True, help="aurora-headless binary")
    ap.add_argument("--photocraft", required=True, help="photocraft-cli binary")
    ap.add_argument("--work", required=True, help="work dir (synthetic inputs in WORK/synth)")
    ap.add_argument("--out", required=True, help="results directory")
    ap.add_argument("--set", default="representative", choices=["representative", "corpus", "all", "none"])
    ap.add_argument("--files", nargs="*", default=[], help="extra input files")
    ap.add_argument("--tools", default="aurora-export,aurora-pool,photocraft")
    ap.add_argument("--modes", default="idle,loaded")
    ap.add_argument("--runs", type=int, default=3)
    ap.add_argument("--load", type=int, default=8, help="number of `yes` processes in the loaded mode")
    ap.add_argument("--timeout", type=int, default=300)
    ap.add_argument("--threads", type=int, default=os.cpu_count(),
                    help="threads for both tools (Aurora --threads, PhotoCraft RAYON_NUM_THREADS); 0 = defaults")
    ap.add_argument("--synthetic-sizes", default="24,60")
    ap.add_argument("--verify-max-pixels", type=int, default=6_000_000,
                    help="compare every k-th pixel above this many (the decode is always whole)")
    ap.add_argument("--keep-outputs", action="store_true")
    ap.add_argument("--aurora-version", default="")
    ap.add_argument("--photocraft-version", default="")
    args = ap.parse_args()
    args.scratch = os.path.join(args.work, "aurora-scratch")
    os.makedirs(args.scratch, exist_ok=True)
    tools = [t for t in args.tools.split(",") if t]
    modes = [m for m in args.modes.split(",") if m]
    meta = {"date": datetime.datetime.now().isoformat(timespec="seconds"), "machine": machine(),
            "tools": {"aurora": args.aurora_version, "photocraft": args.photocraft_version},
            "tool_list": tools, "modes": modes, "argv": sys.argv}
    inputs = collect_inputs(args)
    outdir = os.path.join(args.work, "outputs")
    results = []
    ref_cache = {}
    for mode in modes:
        if mode == "loaded":
            start_load(args.load)
        try:
            for i, src in enumerate(inputs):
                print("[%s %d/%d] %s" % (mode, i + 1, len(inputs), display(src)), file=sys.stderr, flush=True)
                results.append(run_file(args, src, mode, tools, outdir, ref_cache))
                write_reports(args, meta, results)
        finally:
            kill_load()
    write_reports(args, meta, results)
    shutil.rmtree(args.scratch, ignore_errors=True)
    print(os.path.join(args.out, "summary.md"))


if __name__ == "__main__":
    main()
