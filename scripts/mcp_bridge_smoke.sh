#!/usr/bin/env bash
# Manual smoke test of the MCP bridge (task 160): starts aurora-app with
# --mcp-bridge under Xvfb, drives it through `aurora-cli mcp --bridge`, and
# checks a ui_screenshot / ui_inspect / command_run round trip.
#
#   scripts/mcp_bridge_smoke.sh [path-to-aurora-app path-to-aurora-cli]
#
# Needs Xvfb (or a real DISPLAY: set SMOKE_USE_DISPLAY=1). Kills only the
# processes it started. Exit 0 = every check passed.
set -u
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP="${1:-$ROOT/target/debug/aurora-app}"
CLI="${2:-$ROOT/target/debug/aurora-cli}"
WORK="$(mktemp -d)"
PIDS=()
cleanup() {
  for pid in "${PIDS[@]}"; do kill "$pid" 2>/dev/null; done
  for pid in "${PIDS[@]}"; do wait "$pid" 2>/dev/null; done
  rm -rf "$WORK"
}
trap cleanup EXIT

if [ -z "${SMOKE_USE_DISPLAY:-}" ]; then
  if ! command -v Xvfb >/dev/null; then
    echo "SKIPPED: Xvfb is not installed (set SMOKE_USE_DISPLAY=1 to use \$DISPLAY)"
    exit 0
  fi
  DISP=":$((90 + RANDOM % 9))"
  Xvfb "$DISP" -screen 0 1400x900x24 >"$WORK/xvfb.log" 2>&1 &
  PIDS+=($!)
  export DISPLAY="$DISP"
  sleep 2
fi

mkdir -p "$WORK/files" "$WORK/bridge"
export AURORA_BRIDGE_DIR="$WORK/bridge"
export HOME="$WORK/home"; mkdir -p "$HOME"
"$APP" --mcp-bridge --mcp-bridge-read-root "$WORK/files" --mcp-bridge-write-root "$WORK/files" >"$WORK/app.log" 2>&1 &
PIDS+=($!)
for _ in $(seq 1 60); do
  FILE="$(ls "$WORK"/bridge/aurora-bridge-*.json 2>/dev/null | head -1)"
  [ -n "$FILE" ] && break
  sleep 0.5
done
[ -n "${FILE:-}" ] || { echo "FAIL: no discovery file"; cat "$WORK/app.log"; exit 1; }
echo "discovery: $FILE ($(stat -c %a "$FILE" 2>/dev/null || stat -f %Lp "$FILE"))"
sleep 3   # let the first frame draw

python3 - "$CLI" "$FILE" "$WORK/files" <<'PY'
import base64, json, subprocess, sys
cli, discovery, files = sys.argv[1:4]
p = subprocess.Popen([cli, "mcp", "--bridge", "--discovery", discovery,
                      "--read-root", files, "--write-root", files],
                     stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
n = 0
def rpc(method, params=None, notify=False):
    global n
    msg = {"jsonrpc": "2.0", "method": method, "params": params or {}}
    if not notify:
        n += 1; msg["id"] = n
    p.stdin.write(json.dumps(msg) + "\n"); p.stdin.flush()
    if notify: return None
    while True:
        r = json.loads(p.stdout.readline())
        if r.get("id") == n: return r
def tool(name, args=None):
    return rpc("tools/call", {"name": name, "arguments": args or {}})
ok = True
def check(label, cond, detail=""):
    global ok
    print(("PASS " if cond else "FAIL ") + label, detail if not cond else "")
    ok &= bool(cond)
rpc("initialize", {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "smoke", "version": "1"}})
rpc("notifications/initialized", notify=True)
tools = {t["name"] for t in rpc("tools/list")["result"]["tools"]}
check("ui tools are listed", {"ui_screenshot", "ui_inspect", "ui_pointer", "ui_key", "ui_menu", "ui_dialog"} <= tools)
shot = tool("ui_screenshot", {"max_side": 800})
img = shot["result"]["content"][0]
check("screenshot is a PNG", img.get("type") == "image" and base64.b64decode(img["data"]).startswith(b"\x89PNG"), shot)
insp = json.loads(tool("ui_inspect")["result"]["content"][0]["text"])
check("ui_inspect lists widgets", len(insp["nodes"]) > 10, insp.get("window"))
def jtext(r):
    return json.loads(r["result"]["content"][0]["text"])
# A crash-recovery dialog from an earlier killed run may be open: it is
# modal, so everything but ui_dialog is refused until a person (here the
# script, through ui_dialog) answers it.
dlg = jtext(tool("ui_dialog", {"action": "get"}))
if dlg.get("open"):
    acts = [a["id"] for a in dlg.get("actions", []) if a.get("controller_may_choose", True)]
    print("dialog open:", dlg.get("title"), acts)
    ans = tool("ui_dialog", {"action": "choose", "id": acts[0]}) if acts else None
    check("a harmless dialog can be answered", ans is not None and ans["result"].get("isError") is not True, ans)
ctx = tool("command_run", {"id": "layer.new"})
check("command_run layer.new", ctx["result"].get("isError") is not True, ctx)
doc = json.loads(tool("doc_inspect")["result"]["content"][0]["text"])
check("doc_inspect sees the live document", doc["document"]["layer_count"] >= 2, doc)
quit_ = tool("command_run", {"id": "app.quit"})
check("app.quit is refused", quit_["result"].get("isError") is True)
# The route the review found: click File, then "Quit Aurora" in the in-window
# menu bar. With a clean document that used to exit the app.
def find(nodes, name, role=None):
    for nd in nodes:
        if nd.get("label") == name and (role is None or nd.get("role") == role):
            return nd
nodes = jtext(tool("ui_inspect"))["nodes"]
file_btn = find(nodes, "File", "Button")
check("menu bar has a File button", file_btn is not None)
if file_btn is None:
    print("buttons:", sorted({nd.get("label") for nd in nodes if nd.get("role") == "Button"} - {None})[:40])
if file_btn:
    tool("ui_pointer", {"action": "click", "id": file_btn["id"]})
    nodes = jtext(tool("ui_inspect"))["nodes"]
    quit_item = find(nodes, "Quit Aurora")
    if quit_item is None:
        print("labels with Quit/Open/New:", sorted({str(nd.get("label")) for nd in nodes if any(k in str(nd.get("label")) for k in ("Quit", "Open", "New", "Save"))}))
        after = tool("ui_screenshot", {"max_side": 800})["result"]["content"][0]
        open("/tmp/mcp_bridge_smoke_menu.png", "wb").write(base64.b64decode(after["data"]))
    check("File menu lists Quit Aurora", quit_item is not None)
    if quit_item:
        tool("ui_pointer", {"action": "click", "id": quit_item["id"]})
    alive = tool("ui_screenshot", {"max_side": 800})
    try:
        open("/tmp/mcp_bridge_smoke_after_quit.png", "wb").write(base64.b64decode(alive["result"]["content"][0]["data"]))
    except Exception:
        pass
    check("the app is still alive after clicking Quit Aurora",
          alive.get("result", {}).get("isError") is not True and p.poll() is None, alive)
    status = jtext(tool("ui_inspect")).get("status", "")
    print("status after the quit click:", status)
open("/tmp/mcp_bridge_smoke.png", "wb").write(base64.b64decode(img["data"]))
print("screenshot written to /tmp/mcp_bridge_smoke.png")
p.stdin.close(); p.wait(timeout=10)
sys.exit(0 if ok else 1)
PY
