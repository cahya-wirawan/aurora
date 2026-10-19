#!/usr/bin/env python3
"""Tables from a stage-2 results.json: min-max over the measured runs, never one sample.

    summarize.py RESULTS_DIR/results.json > RESULTS_DIR/summary.md
"""
import json, sys
from collections import defaultdict
from pathlib import Path


def rng(vals, fmt):
    vals = [v for v in vals if v is not None]
    if not vals:
        return '-'
    lo, hi = min(vals), max(vals)
    return fmt.format(lo) if fmt.format(lo) == fmt.format(hi) else f'{fmt.format(lo)}-{fmt.format(hi)}'


def main():
    res = json.loads(Path(sys.argv[1]).read_text())
    runs = res['runs']
    groups = defaultdict(list)
    opens = defaultdict(list)
    errors = []
    for r in runs:
        name = Path(r['file']).name
        opens[(r['mode'], name, r['app'])].append(r)
        if 'error' in r:
            errors.append(f"- {r['mode']} round {r['round']} {name} {r['app']}: {r['error']}")
        for sc, st in r.get('scenarios', {}).items():
            groups[(r['mode'], sc, name, r['app'])].append(st)
    a = res['args']
    print('# Stage 2: interactive frames, Aurora vs PhotoCraft (Linux, Xvfb)\n')
    print(f"Started {res['started']}. Scenario {a['duration']} s of input; pointer events {a['rate']}/s "
          f"(pan: middle-button drag, circle r={a['pan_radius']} px, {a['pan_period']} s a turn; brush: "
          f"left drag, circle r={a['brush_radius']} px, {a['brush_period']} s a turn), wheel {a['wheel_rate']} "
          f"notches/s in bursts of {a['zoom_burst']}; present mode `{a['present']}`; window 1280 x 800.\n")
    print('Frame times are intervals between presented frames seen by an X DAMAGE monitor outside both apps. '
          'Ranges are min-max over the runs (one value per run). `over` = % of intervals above 16.7 ms. '
          'CPU = process CPU time / scenario wall time (100 % = one core). GPU = whole-GPU utilisation '
          '(nvidia-smi, 100 ms samples, other processes included).\n')
    for mode in sorted({k[0] for k in groups}, key=lambda m: (m != 'idle', m)):
        for sc in ('pan', 'zoom', 'brush'):
            keys = sorted(k for k in groups if k[0] == mode and k[1] == sc)
            if not keys:
                continue
            print(f'\n## {mode}: {sc}\n')
            print('| file | app | n | fps | mean ms | p50 | p95 | p99 | max | over % | input->frame p50 | CPU % | GPU % mean |')
            print('|---|---|---|---|---|---|---|---|---|---|---|---|---|')
            for k in keys:
                sts = groups[k]
                g = lambda f: [s.get(f) for s in sts]
                print(f"| {k[2]} | {k[3]} | {len(sts)} | {rng(g('fps'), '{:.0f}')} | {rng(g('mean_ms'), '{:.1f}')} | "
                      f"{rng(g('p50_ms'), '{:.1f}')} | {rng(g('p95_ms'), '{:.1f}')} | {rng(g('p99_ms'), '{:.0f}')} | "
                      f"{rng(g('max_ms'), '{:.0f}')} | {rng(g('over_budget_pct'), '{:.0f}')} | "
                      f"{rng(g('input_to_frame_p50_ms'), '{:.1f}')} | {rng(g('cpu_pct_of_one_core'), '{:.0f}')} | "
                      f"{rng(g('gpu_util_mean'), '{:.0f}')} |")
    print('\n## Open (launch to the window titled with the file, then quiet)\n')
    print('| mode | file | app | n | launch -> titled s | launch -> ready s | settled | load avg before |')
    print('|---|---|---|---|---|---|---|---|')
    for k in sorted(opens):
        rs = opens[k]
        print(f"| {k[0]} | {k[1]} | {k[2]} | {len(rs)} | {rng([r.get('launch_to_title_s') for r in rs], '{:.1f}')} | "
              f"{rng([r.get('launch_to_ready_s') for r in rs], '{:.1f}')} | "
              f"{sum(1 for r in rs if r.get('after_return_settled'))}/{len(rs)} | "
              f"{rng([float(r['machine_before']['loadavg'][0]) for r in rs], '{:.1f}')} |")
    print('\n## Zoom state after each scenario\'s setup (window title)\n')
    seen = set()
    for (mode, sc, name, app), sts in sorted(groups.items()):
        if app != 'photocraft':
            continue
        t = sorted({s.get('title_after_setup', '') for s in sts})
        if (sc, name) not in seen:
            seen.add((sc, name)); print(f'- {sc} {name} photocraft: {t}')
    print('\n## Errors\n')
    print('\n'.join(errors) if errors else 'none')


if __name__ == '__main__':
    main()
