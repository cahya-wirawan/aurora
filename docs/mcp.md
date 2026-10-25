# The Aurora MCP server

`aurora-cli mcp` serves the [Model Context Protocol](https://modelcontextprotocol.io)
over standard input and output, so a model or a script can open a document,
inspect it, run editor commands, look at it, check a PSD save and write files,
**without a window or a GPU**. It is headless only: driving the running desktop
app (screenshots, pointer, menus) is [bridge mode](#bridge-mode-driving-the-running-app-task-160). The design and the reasons for
its dependencies are in [ADR 0014](adr/0014-mcp-server.md); the command
descriptions come from the [command registry](command-registry.md).

## Running it

```sh
cargo run -p aurora-cli -- mcp --read-root ~/Pictures --write-root ~/Pictures/out
```

| Option | Meaning |
|---|---|
| `--read-root DIR` | A directory the server may read from (repeatable). |
| `--write-root DIR` | A directory the server may create new files in (repeatable). |
| `--max-open-mb N` | Largest file `doc_open` reads (default 512; an ordinary photographer's PSD fits). |
| `--max-open-mpx N` | Largest canvas `doc_open` accepts, in megapixels, read from the file's header before anything is decoded (default 100; a flat image costs about 12 bytes a pixel to open, so 100 Mpx is about 1.2 GB). It also bounds preview and export composites. |
| `--max-export-mb N` | Largest exported file (default 1024), enforced while it is written. |
| `--scratch-dir DIR` | Where the private scratch directory is made, for staged copies, exports in progress **and the document's tile store** (default: the system temporary directory). |

**Without a root there is no file access at all**: `doc_open` and `doc_save`
say so. A relative path is taken from the *first* root of its kind; an absolute
path must lie inside one of them. stdout carries only protocol lines; logs go to
stderr (`RUST_LOG=info` for more).

A client configuration (Claude Desktop style):

```json
{
  "mcpServers": {
    "aurora": {
      "command": "/path/to/aurora-cli",
      "args": ["mcp", "--read-root", "/work/photos", "--write-root", "/work/out"]
    }
  }
}
```

## Tools

Every tool has a title and all four annotation hints. **Hints are honest
descriptions, not permissions.**

| Tool | Does | readOnly | destructive | idempotent |
|---|---|---|---|---|
| `command_list` | The registry's commands with parameters, `headless` (can this server run it) and `enabled` / `disabled_reason` now. Filters: `filter`, `enabled_only`, `headless_only`. | yes | no | yes |
| `command_run` | One command: `{"id": "select.expand", "args": {"amount": 4}}`. | no | yes | no |
| `command_batch` | Commands in order, `stop_on_error` (default true), at most 32 steps. **Not atomic** (below). | no | yes | no |
| `doc_open` | Opens PNG, JPEG, TIFF, PSD, PSB or `.aur` from the read root as the session's document, **replacing** the previous one (its unsaved in-memory edits are lost). | no | yes | no |
| `doc_inspect` | Canvas size, layer tree (id, name, kind, visibility, opacity, fill, blend mode, bounds, clipping, mask, active), undo history with the undone steps, selection, saved selections, open-time notes. `{"document": null}` with none open. | yes | no | yes |
| `render_preview` | The flattened document as a PNG image content block (`max_side` default 1024, at most 2048) plus a text note with sizes. | yes | no | yes |
| `psd_check` | What saving as PSD would keep, change or not save: the grouped report of File > Check PSD Compatibility (`needs_attention`, `not_saved`, `changed`, `from_open`, `kept`), written to no file. Not opened in Photoshop. | yes | no | yes |
| `doc_save` | Writes the document to a **new** file in the write root; the extension picks the format (`png`, `jpg`, `tif`, `psd`, `aur`). | no | no | no |

Resources: `aurora://document` (live `doc_inspect`) and `aurora://commands`
(live `command_list`).

### Errors

* A bad argument is a JSON-RPC **`-32602`** error, returned before anything
  runs: an unknown tool argument, a wrong type, a missing field, an unknown
  command id, an unknown command argument, a value of the wrong type or out
  of range (the registry's `validate_args`), a bad `max_side`, a batch of 0 or
  more than 32 steps. A batch is validated **as a whole** before its first
  step: one bad step means nothing runs.
* Everything else that goes wrong is a tool result with `isError: true` and a
  sentence: a path outside the roots, a missing file, a disabled command (with
  its reason), a command that needs a window, no document open.
* A line that is not JSON, or not UTF-8, gets `-32700` (`id: null`); the next
  line is served. A JSON value that is not a request object (`[]`, `42`, a
  batch array) or has no `method` gets `-32600` with `id: null` (the request's
  own id when it has one). A request whose id is still in flight is rejected
  with `-32600` and the original keeps running. A line over 1 MiB ends the
  session (and is never executed).

### What runs headless

`command_list` marks each command. Runnable here: **Undo, Redo, New Layer, New
Group, Delete Layer, Select All, Deselect, Inverse Selection, Feather / Expand
/ Contract / Smooth / Border Selection, Check PSD Compatibility**, and **Open**
(`file.open`, `path`) and **Save As** (`file.save`, `path`; use `doc_save` for
`accept_losses`), which the server runs itself because they touch files. Every
edit is one undo step, with the same labels the History panel shows.

**Not available headless** (a clear `isError`): everything that needs a window
or app state: tool selection and strokes (`tool.*`), colours, zoom and fit,
panels and workspaces, the clipboard (`edit.copy`, `paste`, ...), Quick Mask,
Crop, the Pen and path commands, saved-selection commands, Select Colour Range,
adjustment layers (`layer.new_curves`, ...), clipping masks, output profile
commands, document switching and closing, `app.quit`, and the dynamic families
(`workspace.switch/...`, `document.switch....`, `select.load:...`). Several of
these have a document-level implementation that is simply not wired here yet;
the list will grow.

### Concurrency: what is atomic

* A command's **enabled check and its run happen in one hold of the session
  lock**, and the host re-checks its own preconditions (it refuses, it does not
  silently do nothing), so two requests cannot both pass a check that only one
  of them can satisfy.
* A `command_batch` whose steps are all host commands (everything except
  `file.open` and `file.save`) runs **in one lock hold**: no other request
  runs between its steps (`"interleaved_by_other_requests": false`). A batch
  containing `file.open` or `file.save` takes the lock in pieces; each step is
  atomic, other requests may run between steps
  (`"interleaved_by_other_requests": true`).
* Requests are still served concurrently by the SDK and calls on the document
  queue behind each other in no guaranteed order; wait for a reply before
  sending a request that depends on it.

### `command_batch` is not atomic

There is **no rollback**. Steps that ran before a failure stay applied. The
reply says `completed`, `failed`, `not_run` and `"atomic": false`, and lists
each step's result. To recover, run `edit.undo` once per applied edit step and
inspect `history` with `doc_inspect` as you go. With `stop_on_error: true`
(default) the batch stops at the first failing step; with `false` it runs on.
Undo does not remove files already written.

### `doc_save` never overwrites

The target must not exist: an existing file, a symbolic link to one, a dangling
link and a path outside the write root are all errors, and the check is the
file system's own exclusive create. The file is produced in a private scratch
file first (using the application's safe writers and verification) and copied
in, so a failed or cancelled save leaves nothing at the target. The folder
must already exist. A **PSD that would lose or change something** is not
written unless `accept_losses: true`; the grouped report is returned either way
(`saved: false, needs_confirmation: true`), mirroring the app's "Save With
Changes?" dialog.

### Progress and cancellation

A `doc_save` request carrying `_meta.progressToken` gets
`notifications/progress` (`progress` of `total: 1000`, strictly increasing, at
most ten a second). `notifications/cancelled` stops the save: it is checked
between tiles of the composite and during the copy, a partial file at the
target is removed, and the request gets no response. A running PSD or `.aur`
write is not interruptible (cancel is noticed afterwards, before the copy).
`doc_open` and `render_preview` honour cancel too, between tiles / before
decoding; a decode in progress finishes.

## Security model

* **No authority without a root.** Roots are the only directories the server
  can reach; `Roots::open` is the one place that takes ambient authority.
* **Confinement is the open.** Paths are opened through `cap-std` directory
  handles: `..` is refused on the text, a symbolic link (or a link in a
  folder) leading out of a root is refused by the kernel-assisted open, not by
  a prior check. Control characters and paths over 4096 bytes are refused.
* **Headers are read before pixels.** The size a PNG, JPEG, TIFF, BigTIFF,
  PSD, PSB or `.aur` header declares is checked against `--max-open-mpx`
  before the file is copied or decoded, and checked again on the private copy
  that the decoder will open, so a file modified in place between the check and
  the copy cannot slip past the cap (by content, not by extension: a PSB
  called `.png` is checked as a PSB). A hostile 3.5 MB PNG that says 30000 x
  30000 is refused in milliseconds without allocating; a file with a cut or
  inconsistent header is refused. The `.aur` manifest is read by the app's
  own reader, which also refuses zip entries that claim sizes they do not
  have. Tested with PNG, JPEG, TIFF, PSD and PSB headers claiming up to
  300000 x 300000, a PSD claiming 32,767 layers over no data and `.aur`
  files with a lying zip entry. Not covered: layer-count and nesting bombs
  that fit inside the file-size cap are bounded by the app's own reader caps
  (PSD record limits, group depth, tile budget), not by this server.
* **The decoder never sees your path.** A read file is copied (bounded by
  `--max-open-mb`) into a private scratch directory and opened from
  there; it cannot change under the decoder and no caller path is re-resolved.
  The copy is removed afterwards. Only regular files open (no FIFOs, devices;
  the open itself is non-blocking, so a name swapped for a FIFO between the
  check and the open cannot hang the server).
* **Scratch is private and tidy.** The scratch directory is `0700`, its files
  `0600`, whatever the umask. Its name is `aurora-mcp-` plus exactly eight
  random characters, it holds a marker file (`.aurora-mcp-scratch`) and a lock
  file held for the life of the process, and it is removed on exit. A
  `kill -9` leaves it; the next server started in the same parent directory
  removes such leftovers, but **only** a real directory (a symbolic link is
  never followed) with exactly that name shape, with the marker file, whose
  lock file exists and can be taken. Anything else is left alone: a person's
  own `aurora-mcp-notes/`, a directory without the marker, a marked directory
  whose lock file is missing (it may be a live server whose lock a temp cleaner
  removed). There is no age-based cleanup, so such a directory is never
  reclaimed automatically.
* **Writes create, never replace.** See `doc_save`. A failed save removes
  only the file it created: if the name was meanwhile replaced by a different
  file (device and inode compared, Unix), that file is left alone.
* **stdout is the protocol, and a stray print cannot reach it.** On Unix the
  protocol writes to a private duplicate of file descriptor 1 made at start,
  and descriptor 1 is pointed at stderr, so a `println!` in the application or
  a dependency lands in the log. (On other platforms the stream is the
  process's stdout and nothing enforces this: a documented gap; the code
  reachable from `mcp` prints nothing to stdout.)
* **The runtime is never blocked.** The document session lock is only ever
  taken on blocking threads, never on an async worker, so while a long preview
  or export runs, `ping`, cancellation and every other request are still
  served (tested with two runtime workers).
* **Bounds.** Request 1 MiB, tool reply 4 MiB, preview side 2048 and PNG
  6 MiB, batch 32 steps, 2000 layers in `doc_inspect`, 100 Mpx (`--max-open-mpx`)
  for opening and for any composite, 1 GiB exported file (`--max-export-mb`)
  enforced **while the export is written** (a monitor cancels it as soon as its
  directory outgrows the limit), a progress token at most 256 bytes (it is
  echoed in every progress notification), at most 256 requests in flight. An
  export whose worst-case size clearly exceeds the scratch disk's free space is
  refused before it starts (Unix).
* **No panics.** The workspace denies `unwrap`/`expect`/`panic`/indexing; a
  hostile argument or file is an error. (A release build aborts on a panic by
  the workspace profile; none is known.)
* **Not a sandbox for the decoders.** The image, PSD and `.aur` decoders run
  in the server process on files you allowed it to read.

## Limits and known gaps

* One document per session; `doc_open` replaces it.
* Requests sent without waiting for replies may run out of order (see
  Concurrency). Wait for each reply (every real client does).
* Memory: a preview or a PNG/JPEG/TIFF export composites the whole document
  into one f16 RGBA buffer, **8 bytes a pixel** (800 MB at 100 Mpx), plus
  conversion and encode copies on top (the 8-bit dithered image, the encoded
  bytes, and the verification re-decode of an export), so budget several times
  that for an export at the cap; a PSD or `.aur` export streams from the tile
  store instead. Opening costs about 12 bytes a pixel (measured). Lower
  `--max-open-mpx` on a small machine.
* The preview composites the whole document on the CPU (one thread) and
  downsamples it: a 24 Mpx document takes seconds. The last preview is cached
  by document revision and size, so asking again for the same size of an
  unchanged document is free; a different `max_side` recomposites. No pyramid
  shortcut yet.
* Cancelling is noticed between composite tiles and during copies; PSD and
  `.aur` writers and a decode in progress finish first.
* The red team's live path-escape battery (and PSD/`.aur`/zip bombs beyond the
  tests listed above) has not been run by anyone but the builder's own tests.
* Only Linux was exercised. Windows drive-prefixed absolute paths and macOS
  are untested; file-copy staging doubles the I/O of a large PSD open.
* Colour: exports use the sRGB working space with no output profile; an
  `.aur`'s output profile is kept in `.aur` saves only.
* Saved selections of the document are written to a PSD as alpha channels and
  listed in the report; the active selection and paths are not.

## Bridge mode: driving the running app (task 160)

The same tools can drive the **running desktop app** instead of a headless
document, so a session can look at the real window (on the Mac too) and
operate it. Design and threat model: [ADR 0015](adr/0015-mcp-bridge.md).

**It is off unless you turn it on.** Without the flag nothing listens and no
file is written.

### Enable it

```sh
aurora-app --mcp-bridge --mcp-bridge-read-root ~/Pictures/work --mcp-bridge-write-root ~/Pictures/work/out    # or AURORA_MCP_BRIDGE=1
```

| Option | Meaning |
|---|---|
| `--mcp-bridge` | Turn the bridge on (also `AURORA_MCP_BRIDGE=1`). |
| `--mcp-bridge-read-root DIR` | A directory the file tools may read from (repeatable; also `AURORA_MCP_BRIDGE_ROOTS`, a path list). `--mcp-bridge-root DIR` is the same thing: **read-only**. None = `doc_open` is refused. |
| `--mcp-bridge-write-root DIR` | A directory `doc_save` may create NEW files in (repeatable; also `AURORA_MCP_BRIDGE_WRITE_ROOTS`). Writing is never granted by a read root. None = `doc_save` is refused. |
| `--mcp-bridge-max-mpx N` | Largest canvas the bridge opens, previews or exports, in Mpx (default 50). Preview and export run on the UI thread. |

The app listens on `127.0.0.1` only, makes a 256-bit token and writes a
discovery file (mode `0600`, in a `0700` per-user directory):

| Platform | Default directory |
|---|---|
| Linux | `$XDG_RUNTIME_DIR/aurora`, else `<temp>/aurora-bridge-<uid>` |
| macOS | `$TMPDIR/aurora-bridge-<uid>` (`$TMPDIR` is per-user) |
| Windows | `%LOCALAPPDATA%\Aurora\bridge` |

`AURORA_BRIDGE_DIR` overrides it (for both the app and the proxy).

### Connect

```json
{
  "mcpServers": {
    "aurora-live": {
      "command": "/path/to/aurora-cli",
      "args": ["mcp", "--bridge", "--read-root", "/Users/me/Pictures/work", "--write-root", "/Users/me/Pictures/work"]
    }
  }
}
```

`--discovery FILE` names a specific discovery file; the default is the newest
live app in the directory above. Give the proxy roots that lie **inside**
the app's read and write roots respectively: the app enforces its own roots again, and a path outside
them is refused there. A relative path means the first root of its kind in the
proxy; the app is always sent an absolute path.

On a Mac and a Linux machine the config is the same apart from paths; start
`aurora-app` on the machine whose window you want to see, and run the proxy
as the same user.

### Tools in bridge mode

`command_list`, `command_run`, `command_batch`, `doc_open`, `doc_inspect`,
`render_preview`, `psd_check`, `doc_save` keep their names, argument
validation (`-32602` before anything is sent), hints and replies, but act on
the **live** document and run through the same `activate_command` /
`run_activated` the palette and menus use. Almost every registry command is
runnable (`command_list` says `headless: true` for ones this server will send;
read it as "runnable here"). Never run: `app.quit`, `automation.disconnect`,
`file.open` / `file.save` (use `doc_open` / `doc_save`) and any command with a
file-path parameter.

| Tool | Does | readOnly |
|---|---|---|
| `ui_inspect` | Widget tree: id, role, name, value, state, bounds in logical window pixels; focus, tool, open dialog / palette / menu. At most 1500 widgets. `include_hidden`. | yes |
| `ui_screenshot` | PNG of Aurora's own rendered frame (`max_side` 16 to 4096, default 1600), with the physical size and scale factor. Waits for the next drawn frame; an occluded or hidden window times out. | yes |
| `ui_pointer` | `move`, `click`, `double_click`, `drag` (`to_x`, `to_y`, `steps`), `scroll` (`dx`, `dy` lines), at `x`,`y` (logical window pixels) or the centre of a widget `id`; `button`, `ctrl`/`shift`/`alt`/`meta`. | no |
| `ui_key` | A key (`Enter`, `Escape`, `z`, `F5`...) with modifiers and/or typed `text` (at most 256 characters), through the real key handler. | no |
| `ui_menu` | `list` the menus (command ids, labels, hints, enabled) or `activate` an enabled item by command id. | no |
| `ui_dialog` | `get` the open dialog (title, message, actions with ids) or `choose` an action. The only way to answer a dialog. | no |

`doc_open` replaces nothing destructively: the app opens the file in a new
document tab. `doc_save` is the usual never-overwrite save; it is an export
and does not mark the live document as saved.

### Safety rules

* **A person is told.** While a controller is connected the window title ends
  with "[automation connected]" and the status bar ends with "Automation
  connected". The palette command **Disconnect Automation** closes it and
  turns the bridge off until Aurora is restarted.
* **A controller can never quit Aurora.** Whatever a tool does while a
  request is running is synthetic input, and quitting is refused at its one
  central place (`request_quit`) for it: the File > Quit Aurora menu item
  (clicked with `ui_pointer` or driven with `ui_key`), the Quit command, any
  key chord and a window-close request all end with a calm status-bar notice
  and nothing else. The same gate refuses switching the bridge off
  (`automation.disconnect` is the person's lever) and any native file dialog
  (it would block the UI thread until a person answered; use `doc_open` /
  `doc_save`). A real user's own quit, close or Disconnect works normally
  while a controller is connected: only a request's own input is gated.
* **A controller can never discard work.** `ui_dialog` may
  acknowledge a report and **cancel** a question, but it may not choose
  Save, Don't Save, Quit, Replace or Save Anyway in the unsaved-changes,
  quit, workspace-replace and lossy-PSD dialogs; those need a person
  (the table is in ADR 0015). `file.close` and a window-close shortcut can
  *open* the close dialog; the reply says so and the controller can only
  cancel it. A click or key that opens a dialog stops the rest of that call.
* **A modal dialog blocks everything** but reads (`doc_inspect`,
  `ui_inspect`, `ui_screenshot`, `render_preview`, `psd_check`,
  `command_list`) and `ui_dialog`. That includes the unsaved-changes
  questions: they are answered only with `ui_dialog`, visibly. With the
  command palette open, commands and file tools are refused (drive the
  palette with `ui_key`). The tools can start a close, but quitting is not a
  tool and the close dialogs are the normal review.
* One controller at a time; one request at a time per connection; 16 requests
  can wait for the UI thread; burst 32 / 20 per second per connection; a
  request line is at most 1 MiB; a controller silent for 5 minutes is dropped
  (the proxy reconnects). Read-only calls are retried once on a fresh
  connection; anything that changes state is never resent after a transport
  failure (the error says it may have completed).
* Five failed handshakes lock new connections out for 1 s, doubling to 60 s.
  Only a well-formed handshake with a wrong token counts; HTTP requests (a
  web page), binary junk, a TLS hello and any other first line are refused
  or dropped and never count.
* Every tool call is logged at `info` (method only, no pixel data).

### What is not protected

Any process running as the same user can read the discovery file and use the
bridge: that is the design (ADR 0015). The commands that touch the document
are exactly those a person at the window could run. Windows file permissions
are not verified by code; Windows and macOS are untested. The app does not
compare a stale discovery file's port with a live process beyond its lock, so
an unrelated process that reuses a dead app's port would only ever receive a
token that no longer works.

### Limits and gaps

* Preview, `doc_save` and `psd_check` composite on the **UI thread** (the
  window pauses for as long as the same action does for a person); the UI
  thread never waits on the socket.
* Cancellation (`notifications/cancelled`) is not forwarded to the app; a
  request the app has started runs to the end, and a call that gets no reply
  in 60 s says it may still apply.
* `ui_pointer` coordinates are logical window pixels; a screenshot's pixels are
  physical (`scale_factor` is returned).
* A screenshot needs a surface that can be read back (not every GPU/driver);
  `bridge.status` / the error say when it cannot.
* No progress notifications; `doc_save` is not interruptible.
* Not run in CI: the live-window handlers are exercised by
  `scripts/mcp_bridge_smoke.sh` (needs Xvfb or a display).
