#!/usr/bin/env python3
"""Stage 2 on macOS: the same scripted input as stage2.py, frames timed by the Metal HUD.

NOT RUN YET. Written on Linux; the Mac reachable from the dev box has no GUI
session. Run it at the Mac, logged in, from a Terminal that has Accessibility
permission (System Settings > Privacy & Security > Accessibility), with
``pip install pyobjc-framework-Quartz``.

    mac_stage2.py --app aurora --bin target/release/aurora-app --file X.psd --out DIR
    mac_stage2.py --app photocraft --bin ~/pc-work/target/release/photocraft --file X.psd --out DIR

Each run launches the app with ``MTL_HUD_ENABLED=1 MTL_HUD_LOG_ENABLED=1``, records the
unified log of the Metal HUD (``log stream``) for the whole run, injects the same
pan / zoom / brush input as the Linux harness (CGEvent), and writes the raw log
and the scenario time windows to DIR. ``--parse`` then turns the HUD's per-frame
lines into the same statistics as stage2.py. The HUD log's line format has not
been checked on this macOS version: look at ``hud.log`` first and adjust
``FRAME_RE`` if needed (see ../README.md, "Stage 2 on the Mac").
"""
import argparse, json, math, os, re, signal, statistics, subprocess, sys, time
from pathlib import Path

WIN_W, WIN_H = 1280, 800
# The canvas centre in window coordinates (points), as on Linux; check with a screenshot.
CANVAS = {'aurora': (550, 440), 'photocraft': (640, 400)}
# Assumed HUD log content: a frame interval in ms. VERIFY on the first run.
FRAME_RE = re.compile(r'(?i)frame\s*(?:interval|time)[^0-9]*([0-9.]+)\s*ms')


def q():
    import Quartz
    return Quartz


def post(ev):
    Q = q(); Q.CGEventPost(Q.kCGHIDEventTap, ev)


def move(x, y, button=None):
    Q = q()
    kind, b = Q.kCGEventMouseMoved, Q.kCGMouseButtonLeft
    if button == 'left': kind = Q.kCGEventLeftMouseDragged
    if button == 'middle': kind, b = Q.kCGEventOtherMouseDragged, Q.kCGMouseButtonCenter
    post(Q.CGEventCreateMouseEvent(None, kind, (x, y), b))


def press(x, y, button, down):
    Q = q()
    if button == 'left':
        kind = Q.kCGEventLeftMouseDown if down else Q.kCGEventLeftMouseUp; b = Q.kCGMouseButtonLeft
    else:
        kind = Q.kCGEventOtherMouseDown if down else Q.kCGEventOtherMouseUp; b = Q.kCGMouseButtonCenter
    post(Q.CGEventCreateMouseEvent(None, kind, (x, y), b))


KEYS = {'1': 18, '0': 29, 'b': 11, 'return': 36}


def key(name, cmd=False):
    Q = q()
    for down in (True, False):
        e = Q.CGEventCreateKeyboardEvent(None, KEYS[name], down)
        if cmd: Q.CGEventSetFlags(e, Q.kCGEventFlagMaskCommand)
        post(e); time.sleep(0.03)


def wheel(up, option):
    Q = q()
    e = Q.CGEventCreateScrollWheelEvent(None, Q.kCGScrollEventUnitLine, 1, 1 if up else -1)
    if option: Q.CGEventSetFlags(e, Q.kCGEventFlagMaskAlternate)
    post(e)


def place_window(pid):
    script = (f'tell application "System Events" to tell (first process whose unix id is {pid}) '
              f'to set {{position, size}} of front window to {{{{0, 25}}, {{{WIN_W}, {WIN_H}}}}}')
    subprocess.run(['osascript', '-e', script], capture_output=True)


def scenario(name, app, args, ox, oy):
    cx, cy = CANVAS[app]; cx += ox; cy += oy
    move(cx, cy)
    key('1' if name in ('pan', 'brush') else '0', cmd=True)
    if name == 'brush': key('b')
    time.sleep(2)
    t0 = time.time(); dt = 1 / args.rate
    if name in ('pan', 'brush'):
        R, period, btn = (250, 1.0, 'middle') if name == 'pan' else (150, 2.0, 'left')
        press(cx + R, cy, btn, True)
        k = 0
        while k * dt < args.duration:
            while time.time() < t0 + k * dt: time.sleep(0.0002)
            a = 2 * math.pi * k * dt / period
            move(cx + R * math.cos(a), cy + R * math.sin(a), btn); k += 1
        t1 = time.time()
        press(cx + R, cy, btn, False)
    else:
        k = 0; ndt = 1 / args.wheel_rate
        while k * ndt < args.duration:
            while time.time() < t0 + k * ndt: time.sleep(0.0002)
            wheel((k // 6) % 2 == 0, option=(app == 'photocraft')); k += 1
        t1 = time.time()
    time.sleep(1.0)
    return {'scenario': name, 'start': t0, 'end': t1}


def run(args):
    out = Path(args.out); out.mkdir(parents=True, exist_ok=True)
    env = dict(os.environ, MTL_HUD_ENABLED='1', MTL_HUD_LOG_ENABLED='1')
    if args.app == 'aurora':
        env.update(AURORA_OPEN=args.file, AURORA_FIRST_RUN='0', AURORA_PRESENT_MODE=args.present)
        cmd = [args.bin]
    else:
        env.update(PHOTOCRAFT_PRESENT_MODE=args.present)
        cmd = [args.bin, '--new-instance', '--', args.file]
    log = open(out / 'hud.log', 'w')
    ls = subprocess.Popen(['log', 'stream', '--style', 'ndjson', '--level', 'debug',
                           '--predicate', 'subsystem == "com.apple.Metal"'], stdout=log, stderr=subprocess.DEVNULL)
    p = subprocess.Popen(cmd, env=env, stdout=open(out / 'app.log', 'w'), stderr=subprocess.STDOUT)
    time.sleep(args.open_wait)          # no outside 'ready' signal here: wait long enough, check the screenshot
    place_window(p.pid)
    key('return'); time.sleep(3)        # each app's post-open dialog default, as on Linux
    subprocess.run(['screencapture', '-x', str(out / 'ready.png')])
    windows = [scenario(s, args.app, args, 0, 25) for s in args.scenarios.split(',')]
    subprocess.run(['screencapture', '-x', str(out / 'end.png')])
    p.send_signal(signal.SIGTERM); time.sleep(2); p.kill(); ls.terminate()
    (out / 'windows.json').write_text(json.dumps({'app': args.app, 'file': args.file, 'windows': windows}, indent=1))


def parse(args):
    out = Path(args.out)
    wins = json.loads((out / 'windows.json').read_text())['windows']
    frames = []
    for line in (out / 'hud.log').read_text(errors='replace').splitlines():
        try: rec = json.loads(line)
        except ValueError: continue
        m = FRAME_RE.search(rec.get('eventMessage', ''))
        if m:
            # 'timestamp' is local time with microseconds; convert to epoch seconds.
            ts = time.mktime(time.strptime(rec['timestamp'][:19], '%Y-%m-%d %H:%M:%S')) + float('0.' + rec['timestamp'][20:26])
            frames.append((ts, float(m.group(1))))
    for w in wins:
        iv = sorted(ms for ts, ms in frames if w['start'] <= ts <= w['end'] + 0.25)
        if not iv:
            print(w['scenario'], 'no HUD frame lines matched; check hud.log and FRAME_RE'); continue
        pick = lambda q_: iv[min(len(iv) - 1, int(q_ * (len(iv) - 1)))]
        print(f"{w['scenario']}: frames {len(iv)} fps {len(iv) / (w['end'] - w['start']):.1f} mean {statistics.fmean(iv):.2f} "
              f"p50 {pick(.5):.2f} p95 {pick(.95):.2f} p99 {pick(.99):.2f} max {iv[-1]:.1f} "
              f"over {100 * sum(v > 1000 / 60 for v in iv) / len(iv):.1f}%")


if __name__ == '__main__':
    ap = argparse.ArgumentParser()
    ap.add_argument('--app', choices=['aurora', 'photocraft']); ap.add_argument('--bin'); ap.add_argument('--file')
    ap.add_argument('--out', required=True); ap.add_argument('--scenarios', default='pan,zoom,brush')
    ap.add_argument('--duration', type=float, default=6); ap.add_argument('--rate', type=float, default=240)
    ap.add_argument('--wheel-rate', type=float, default=60); ap.add_argument('--present', default='novsync')
    ap.add_argument('--open-wait', type=float, default=30); ap.add_argument('--parse', action='store_true')
    a = ap.parse_args()
    parse(a) if a.parse else run(a)
