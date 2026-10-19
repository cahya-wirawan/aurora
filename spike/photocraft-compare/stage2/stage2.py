#!/usr/bin/env python3
"""Stage 2: interactive frame rate of Aurora and PhotoCraft, measured from outside.

Linux only. Both apps run in the same private X server (Xvfb), in a window of
the same size, driven by the same synthetic input (XTEST), and their presented
frames are timed by the same outside observer: an X DAMAGE monitor on the app's
window, in this process. Neither app's own frame counter is used.

    stage2.py --aurora BIN --photocraft BIN --files A.psd B.png --out DIR [--runs 3]
              [--modes idle,loaded] [--scenarios pan,zoom,brush] [--duration 8]

Needs python-xlib (``pip install python-xlib``) and an ``Xvfb`` binary
(``--xvfb``). See ../README.md, "Stage 2", for the method and its limits.
"""
import argparse, json, os, shutil, signal, statistics, subprocess, sys, tempfile, threading, time
from pathlib import Path

from Xlib import X, XK, display as xdisplay
from Xlib.ext import damage, xtest

CLK_TCK = os.sysconf('SC_CLK_TCK')
WIN_W, WIN_H = 1280, 800          # Aurora's window (logical px, scale 1 on Xvfb)
BUDGET_MS = 1000.0 / 60.0

# Per-app gesture mapping. Each app's own documented gesture for the same action.
APPS = {
    'aurora': {
        'zoom_mod': None,            # plain wheel zooms the canvas
        'canvas': (550, 440),        # centre of the canvas area at 1280 x 800
    },
    'photocraft': {
        'zoom_mod': 'Alt_L',         # Alt + wheel zooms (plain wheel scrolls, Photoshop's default)
        'canvas': None,              # measured from the window at start (see find_canvas_centre)
    },
}


def now_ns():
    return time.monotonic_ns()


class DamageMonitor(threading.Thread):
    """Records the monotonic time of every presented frame of one window.

    DamageReportNonEmpty sends one event when the window's damage goes from
    empty to non-empty; the handler subtracts it at once, so each present
    gives one event (two presents closer together than one round trip,
    well under a millisecond here, would count as one)."""

    def __init__(self, disp_name, wid):
        super().__init__(daemon=True)
        self.d = xdisplay.Display(disp_name)
        self.win = self.d.create_resource_object('window', wid)
        self.dmg = self.win.damage_create(damage.DamageReportNonEmpty)
        self.d.flush()
        self.stamps = []
        self.stop = False
        self.ev = self.d.extension_event.DamageNotify

    def run(self):
        while not self.stop:
            try:
                e = self.d.next_event()
            except Exception:
                return
            if e.type == self.ev:
                self.stamps.append(now_ns())
                self.d.damage_subtract(self.dmg, 0, 0)
                self.d.flush()

    def last(self):
        return self.stamps[-1] if self.stamps else 0


class Input:
    def __init__(self, disp_name):
        self.d = xdisplay.Display(disp_name)
        self.sent = []

    def kc(self, name):
        return self.d.keysym_to_keycode(XK.string_to_keysym(name))

    def move(self, x, y):
        xtest.fake_input(self.d, X.MotionNotify, x=int(x), y=int(y))
        self.d.flush(); self.sent.append(now_ns())

    def button(self, b, down):
        xtest.fake_input(self.d, X.ButtonPress if down else X.ButtonRelease, b)
        self.d.flush()

    def key(self, name, down):
        xtest.fake_input(self.d, X.KeyPress if down else X.KeyRelease, self.kc(name))
        self.d.flush()

    def chord(self, mods, key):
        for m in mods: self.key(m, True)
        self.key(key, True); self.key(key, False)
        for m in reversed(mods): self.key(m, False)

    def wheel(self, up):
        b = 4 if up else 5
        xtest.fake_input(self.d, X.ButtonPress, b); xtest.fake_input(self.d, X.ButtonRelease, b)
        self.d.flush(); self.sent.append(now_ns())


def toplevels_of(d, pid):
    atom = d.intern_atom('_NET_WM_PID')
    out = []
    for w in d.screen().root.query_tree().children:
        try:
            p = w.get_full_property(atom, X.AnyPropertyType)
            if p and p.value and int(p.value[0]) == pid and w.get_attributes().map_state == X.IsViewable:
                out.append(w)
        except Exception:
            pass
    return out


def wait_window(disp_name, pid, timeout=60):
    d = xdisplay.Display(disp_name)
    t0 = time.monotonic()
    while time.monotonic() - t0 < timeout:
        ws = toplevels_of(d, pid)
        if ws:
            # The largest mapped toplevel is the main window.
            w = max(ws, key=lambda w: w.get_geometry().width * w.get_geometry().height)
            return d, w
        time.sleep(0.1)
    return d, None


def wait_quiet(mon, quiet_s, min_s, timeout_s, t0=None):
    """Waits until no frame has been presented for quiet_s (after min_s)."""
    t0 = t0 or now_ns()
    while True:
        t = now_ns()
        if (t - t0) / 1e9 > timeout_s:
            return False, (t - t0) / 1e9
        if (t - t0) / 1e9 >= min_s and (t - max(mon.last(), t0)) / 1e9 >= quiet_s:
            return True, (max(mon.last(), t0) - t0) / 1e9
        time.sleep(0.05)


def proc_cpu_ticks(pid):
    try:
        f = open(f'/proc/{pid}/stat').read().rsplit(')', 1)[1].split()
        return int(f[11]) + int(f[12])
    except Exception:
        return None


class GpuSampler(threading.Thread):
    def __init__(self):
        super().__init__(daemon=True)
        self.samples = []; self.stop = False; self.p = None

    def run(self):
        try:
            self.p = subprocess.Popen(['nvidia-smi', '--query-gpu=utilization.gpu', '--format=csv,noheader,nounits', '-lms', '100'],
                                      stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
        except OSError:
            return
        for line in self.p.stdout:
            if self.stop: break
            try: self.samples.append(float(line.strip()))
            except ValueError: pass

    def finish(self):
        self.stop = True
        if self.p: self.p.terminate()


def pct(sorted_vals, q):
    if not sorted_vals: return None
    k = (len(sorted_vals) - 1) * q
    lo, hi = int(k), min(int(k) + 1, len(sorted_vals) - 1)
    return sorted_vals[lo] + (sorted_vals[hi] - sorted_vals[lo]) * (k - lo)


def frame_stats(stamps, t_start, t_end, inputs):
    fr = [s for s in stamps if t_start <= s <= t_end]
    iv = sorted((b - a) / 1e6 for a, b in zip(fr, fr[1:]))
    dur = (t_end - t_start) / 1e9
    lat = []
    j = 0
    for t in inputs:
        while j < len(fr) and fr[j] < t: j += 1
        if j < len(fr) and fr[j] - t < 250e6: lat.append((fr[j] - t) / 1e6)
    lat.sort()
    r = {'frames': len(fr), 'seconds': dur, 'fps': len(fr) / dur if dur else None, 'inputs': len(inputs)}
    if iv:
        r.update({'mean_ms': statistics.fmean(iv), 'p50_ms': pct(iv, .5), 'p95_ms': pct(iv, .95),
                  'p99_ms': pct(iv, .99), 'max_ms': iv[-1],
                  'over_budget_pct': 100.0 * sum(1 for v in iv if v > BUDGET_MS) / len(iv)})
    if lat:
        r.update({'input_to_frame_p50_ms': pct(lat, .5), 'input_to_frame_p95_ms': pct(lat, .95)})
    return r


def screenshot(disp_name, path, w=WIN_W, h=WIN_H):
    try:
        import zlib, struct
        d = xdisplay.Display(disp_name)
        img = d.screen().root.get_image(0, 0, w, h, X.ZPixmap, 0xffffffff).data
        out = bytearray()
        for y in range(h):
            row = img[y * w * 4:(y + 1) * w * 4]; px = bytearray(w * 3)
            px[0::3] = row[2::4]; px[1::3] = row[1::4]; px[2::3] = row[0::4]
            out.append(0); out += px
        def chunk(t, b): return struct.pack('>I', len(b)) + t + b + struct.pack('>I', zlib.crc32(t + b) & 0xffffffff)
        Path(path).write_bytes(b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', w, h, 8, 2, 0, 0, 0))
                              + chunk(b'IDAT', zlib.compress(bytes(out), 1)) + chunk(b'IEND', b''))
    except Exception as e:
        print('screenshot failed', e, file=sys.stderr)


def launch(app, args, file, home):
    env = dict(os.environ)
    for k in ('XDG_CONFIG_HOME', 'XDG_CACHE_HOME', 'XDG_STATE_HOME', 'XDG_DATA_HOME', 'XDG_RUNTIME_DIR', 'WAYLAND_DISPLAY'):
        env.pop(k, None)
    rt = home / 'run'; rt.mkdir(mode=0o700)
    tmp = home / 'tmp'; tmp.mkdir()
    env.update({'DISPLAY': args.display, 'HOME': str(home), 'TMPDIR': str(tmp), 'XDG_RUNTIME_DIR': str(rt),
                'XDG_CONFIG_HOME': str(home / '.config'), 'XDG_CACHE_HOME': str(home / '.cache'),
                'XDG_DATA_HOME': str(home / '.local/share'), 'XDG_STATE_HOME': str(home / '.local/state')})
    if args.threads:
        env['RAYON_NUM_THREADS'] = str(args.threads)
    if app == 'aurora':
        env.update({'AURORA_OPEN': str(file), 'AURORA_FIRST_RUN': '0', 'RUST_LOG': 'info'})
        if args.present: env['AURORA_PRESENT_MODE'] = args.present
        cmd = [args.aurora]
    else:
        if args.present: env['PHOTOCRAFT_PRESENT_MODE'] = args.present
        cmd = [args.photocraft, '--new-instance', '--', str(file)]
    log = open(home / 'app.log', 'w')
    p = subprocess.Popen(cmd, env=env, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
    return p


def stop_app(p):
    if p.poll() is None:
        try: os.killpg(p.pid, signal.SIGTERM)
        except ProcessLookupError: pass
        try: p.wait(5)
        except subprocess.TimeoutExpired:
            try: os.killpg(p.pid, signal.SIGKILL)
            except ProcessLookupError: pass
            p.wait()


def run_scenario(name, app, inp, mon, pid, args, cx, cy):
    """One timed scenario. Returns its stats."""
    rate = args.rate
    dt = 1.0 / rate
    zoom_mod = APPS[app]['zoom_mod']
    # Setup (untimed): the same zoom command in both apps.
    inp.move(cx, cy)
    if name in ('pan', 'brush'):
        inp.chord(['Control_L'], '1')        # 100 %
    else:
        inp.chord(['Control_L'], '0')        # fit on screen
    if name == 'brush':
        inp.chord([], 'b')
    wait_quiet(mon, 1.0, 1.0, 30)
    inp.sent.clear()
    gpu = GpuSampler(); gpu.start()
    c0 = proc_cpu_ticks(pid)
    t_start = now_ns()
    if name in ('pan', 'brush'):
        # A circle of radius R at one turn per `period` s, sampled at `rate` events/s.
        import math
        R, period = (args.pan_radius, args.pan_period) if name == 'pan' else (args.brush_radius, args.brush_period)
        inp.move(cx + R, cy)
        inp.button(2 if name == 'pan' else 1, True)
        k = 0
        while True:
            t = k * dt
            if t >= args.duration: break
            target = t_start + int(t * 1e9)
            while now_ns() < target: time.sleep(0.0002)
            a = 2 * math.pi * t / period
            inp.move(cx + R * math.cos(a), cy + R * math.sin(a))
            k += 1
        inp.button(2 if name == 'pan' else 1, False)
    else:
        # Wheel: alternating bursts of `zoom_burst` notches in, then out.
        if zoom_mod: inp.key(zoom_mod, True)
        k = 0; ndt = 1.0 / args.wheel_rate
        while True:
            t = k * ndt
            if t >= args.duration: break
            target = t_start + int(t * 1e9)
            while now_ns() < target: time.sleep(0.0002)
            inp.wheel(up=(k // args.zoom_burst) % 2 == 0)
            k += 1
        if zoom_mod: inp.key(zoom_mod, False)
    t_end = now_ns()
    # Frames presented in response to the last inputs still count: wait for quiet.
    _, _ = wait_quiet(mon, 0.5, 0.0, 10, t0=t_end)
    t_tail = max(t_end, mon.last())
    c1 = proc_cpu_ticks(pid)
    gpu.finish()
    st = frame_stats(mon.stamps, t_start, t_tail, inp.sent)
    st['input_seconds'] = (t_end - t_start) / 1e9
    st['tail_seconds'] = (t_tail - t_end) / 1e9
    if c0 is not None and c1 is not None and st['seconds']:
        st['cpu_pct_of_one_core'] = 100.0 * (c1 - c0) / CLK_TCK / st['seconds']
    if gpu.samples:
        st['gpu_util_mean'] = statistics.fmean(gpu.samples); st['gpu_util_max'] = max(gpu.samples)
    return st


def find_canvas_centre(app, win):
    c = APPS[app]['canvas']
    if c: return c
    g = win.get_geometry()
    return (g.width // 2, g.height // 2)


def one_launch(app, file, args, outdir, tag):
    home = Path(tempfile.mkdtemp(prefix=f'{app}-', dir=args.scratch))
    rec = {'app': app, 'file': str(file), 'tag': tag}
    p = launch(app, args, file, home)
    t_launch = now_ns()
    try:
        d, win = wait_window(args.display, p.pid)
        if win is None:
            rec['error'] = 'no window'; return rec
        rec['window_seconds'] = (now_ns() - t_launch) / 1e9
        win.configure(x=0, y=0, width=WIN_W, height=WIN_H); d.sync()
        win.set_input_focus(X.RevertToParent, X.CurrentTime); d.sync()
        mon = DamageMonitor(args.display, win.id); mon.start()
        ok, settled = wait_quiet(mon, args.quiet, args.min_open, args.open_timeout, t0=t_launch)
        rec['open_settled'] = ok
        rec['launch_to_last_frame_s'] = settled
        g = win.get_geometry(); rec['window'] = [g.width, g.height]
        cx, cy = find_canvas_centre(app, win)
        inp = Input(args.display)
        rec['scenarios'] = {}
        for sc in args.scenarios:
            if p.poll() is not None:
                rec['error'] = f'exited {p.returncode} before {sc}'; break
            win.set_input_focus(X.RevertToParent, X.CurrentTime); d.sync()
            rec['scenarios'][sc] = run_scenario(sc, app, inp, mon, p.pid, args, cx, cy)
            if args.screens:
                screenshot(args.display, outdir / f'{tag}-{sc}.png')
        mon.stop = True
    finally:
        stop_app(p)
        try: rec['log_tail'] = (home / 'app.log').read_text(errors='replace')[-1500:]
        except OSError: pass
        shutil.rmtree(home, ignore_errors=True)
    return rec


def start_load(n):
    if n <= 0: return None
    me = os.getpid()
    script = f'i=0; while [ $i -lt {n} ]; do yes > /dev/null & i=$((i+1)); done; while kill -0 {me} 2>/dev/null; do sleep 1; done; kill $(jobs -p)'
    return subprocess.Popen(['sh', '-c', script], start_new_session=True)


def stop_load(p):
    if p:
        try: os.killpg(p.pid, signal.SIGTERM)
        except ProcessLookupError: pass


def machine_state():
    s = {'loadavg': open('/proc/loadavg').read().split()[:3]}
    try:
        s['cargo_rustc'] = subprocess.run(['pgrep', '-c', '-f', 'rustc|cargo'], capture_output=True, text=True).stdout.strip()
    except OSError:
        pass
    return s


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--aurora', required=True); ap.add_argument('--photocraft', required=True)
    ap.add_argument('--files', nargs='+', required=True); ap.add_argument('--out', required=True)
    ap.add_argument('--display', default=':99'); ap.add_argument('--xvfb', default='Xvfb')
    ap.add_argument('--no-xvfb', action='store_true', help='use an existing X server at --display')
    ap.add_argument('--apps', default='aurora,photocraft'); ap.add_argument('--runs', type=int, default=3)
    ap.add_argument('--modes', default='idle,loaded'); ap.add_argument('--load', type=int, default=8)
    ap.add_argument('--scenarios', default='pan,zoom,brush'); ap.add_argument('--duration', type=float, default=8.0)
    ap.add_argument('--rate', type=float, default=240.0, help='pointer events per second (pan, brush)')
    ap.add_argument('--wheel-rate', type=float, default=60.0, help='wheel notches per second (zoom)')
    ap.add_argument('--zoom-burst', type=int, default=6)
    ap.add_argument('--pan-radius', type=float, default=250.0); ap.add_argument('--pan-period', type=float, default=1.0)
    ap.add_argument('--brush-radius', type=float, default=150.0); ap.add_argument('--brush-period', type=float, default=2.0)
    ap.add_argument('--present', default='novsync', help="'' leaves each app's default")
    ap.add_argument('--threads', type=int, default=0)
    ap.add_argument('--quiet', type=float, default=3.0); ap.add_argument('--min-open', type=float, default=4.0)
    ap.add_argument('--open-timeout', type=float, default=240.0)
    ap.add_argument('--screens', action='store_true')
    ap.add_argument('--scratch', default=None)
    args = ap.parse_args()
    args.scenarios = [s for s in args.scenarios.split(',') if s]
    out = Path(args.out); out.mkdir(parents=True, exist_ok=True)
    args.scratch = args.scratch or str(out / 'scratch'); Path(args.scratch).mkdir(exist_ok=True)
    xvfb = None
    if not args.no_xvfb:
        xvfb = subprocess.Popen([args.xvfb, args.display, '-screen', '0', '2560x1440x24', '-nolisten', 'tcp'],
                                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
        time.sleep(2)
    apps = args.apps.split(',')
    results = {'started': time.strftime('%Y-%m-%d %H:%M:%S'), 'args': {k: v for k, v in vars(args).items()}, 'runs': []}
    resfile = out / 'results.json'
    try:
        for mode in args.modes.split(','):
            load = start_load(args.load) if mode == 'loaded' else None
            time.sleep(1 if load else 0)
            try:
                for r in range(args.runs):
                    for fi, f in enumerate(args.files):
                        order = apps if (r + fi) % 2 == 0 else list(reversed(apps))
                        for app in order:
                            tag = f'{mode}-r{r}-{Path(f).stem}-{app}'
                            before = machine_state()
                            print(f'[{time.strftime("%H:%M:%S")}] {tag} load={before}', flush=True)
                            rec = one_launch(app, Path(f), args, out, tag)
                            rec.update({'mode': mode, 'round': r, 'machine_before': before, 'machine_after': machine_state()})
                            results['runs'].append(rec)
                            resfile.write_text(json.dumps(results, indent=1))
                            for sc, st in rec.get('scenarios', {}).items():
                                print(f'    {sc}: fps {st.get("fps", 0):.1f} mean {st.get("mean_ms", 0):.2f} p95 {st.get("p95_ms", 0):.2f} '
                                      f'max {st.get("max_ms", 0):.1f} over {st.get("over_budget_pct", 0):.1f}% cpu {st.get("cpu_pct_of_one_core", 0):.0f}%', flush=True)
                            if 'error' in rec: print('    ERROR', rec['error'], flush=True)
            finally:
                stop_load(load)
    finally:
        if xvfb:
            os.killpg(xvfb.pid, signal.SIGTERM)
    print('results:', resfile)


if __name__ == '__main__':
    main()
