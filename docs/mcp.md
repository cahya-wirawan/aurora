# The Aurora MCP server

`aurora-cli mcp` serves the [Model Context Protocol](https://modelcontextprotocol.io)
over standard input and output, so a model or a script can open a document,
inspect it, run editor commands, look at it, check a PSD save and write files,
**without a window or a GPU**. It is headless only: driving the running desktop
app (screenshots, pointer, menus) is task 160. The design and the reasons for
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
  before the file is copied or decoded (by content, not by extension: a PSB
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
  `0600`, whatever the umask. It is locked for the life of the process and
  removed on exit; a `kill -9` leaves it, and the next server started in the
  same parent directory removes such leftovers (a directory is removed only
  if its lock can be taken, so a running server's is never touched).
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

## What task 160 adds

Bridge mode: the same tools drive the running desktop app over a loopback
socket (256-bit token in an owner-only file), plus `ui_inspect`,
`ui_screenshot`, pointer and menu/dialog control, so a session can look at the
real window (including on the Mac), and the commands that need a window become
runnable there.
