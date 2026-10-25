# 0015. The MCP bridge: driving the running app over an authenticated loopback socket

**Status:** Accepted (Cahya approved starting task 160 and confirmed the bridge is **off unless explicitly enabled**, 2026-10-25)
**Date:** 2026-10-25
**Deciders:** Cahya Wirawan; implementation by Claude
**Related:** ADR 0014 (the headless MCP server), PLAN.md Task 160, `docs/mcp.md`

## Context

ADR 0014 gave Aurora a headless MCP server. It cannot show a person's real
window, so a session on the Mac cannot look at what Aurora actually draws or
operate the real menus, dialogs and canvas. Task 160 lets the **same tools**
drive the **running desktop app**, and adds tools to look at and operate the
window.

That is a new kind of authority. A process that can drive the window can do
what the user can do in it: edit and save documents, answer dialogs. The
threat model matters more than the feature.

## Threat model

| Who | What we assume | What we do |
|---|---|---|
| **A process running as the same user** | Can already read the app's memory, its files and the discovery file. | Not defended against, by design: the token is a capability for "whoever can run as this user". The docs say so. |
| **Another user on the same machine** | Can connect to any loopback port; cannot read the user's files. | A 256-bit token is required as the first line; the discovery file is `0600` in a `0700` per-user directory that must be owned by the user; the proxy refuses a discovery file that is not ours or is readable by others. |
| **A web page in the user's browser** (including DNS rebinding) | Can open a TCP connection to `127.0.0.1:<port>` and send **HTTP**. Cannot read files, cannot send arbitrary bytes. | The socket is **not HTTP**: the first line must be exactly the handshake JSON with the token. Anything HTTP-shaped is dropped without an answer and without counting against the lockout. `Origin`/`Host` are therefore irrelevant: nothing parses them, and a page cannot guess a 256-bit token. |
| **A prompt-injected model** driving the tools | The realistic attacker for the *tools*. | Roots and `..` rules (ADR 0014) enforced in the proxy **and again in the app**; the registry validates arguments in both; quitting is not a tool; a modal dialog (an unsaved-changes question included) blocks everything except reads and the dialog tool; a visible, un-hideable indication; the user can disconnect. |
| **A network peer** | Must not reach the app. | The listener binds `127.0.0.1` only, refuses any other configured address, drops non-loopback peers. |

## Decision

1. **Off unless enabled.** `aurora-app --mcp-bridge` (or `AURORA_MCP_BRIDGE=1`).
   Without it nothing listens and no file is written (the option parsing is
   tested; the live path was not run). File access needs
   `--mcp-bridge-read-root DIR` (`--mcp-bridge-root` is the same) and, for
   saving, a separate `--mcp-bridge-write-root DIR` (repeatable); a read root
   never grants writing, and none means no file tools.
2. **Transport.** Newline-delimited JSON over TCP on `127.0.0.1`, an
   ephemeral port. No HTTP, no TLS (loopback; the token authenticates).
   First line from the client: exactly `{"hello":"aurora-bridge","version":1,"token":"<64 hex>"}`
   (unknown fields refused, at most 512 bytes, within 3 s *in total*, not per
   read). Token from the OS RNG (`getrandom`), compared in constant time.
   After five failed handshakes new connections are refused for a growing
   time (1 s doubling to 60 s); **only a well-formed handshake carrying a
   wrong token counts** (binary junk, a TLS hello, invalid UTF-8, JSON of
   another shape and HTTP are refused or dropped and never counted). **One** controller; a second authenticated
   connection is refused; at most four handshakes in progress. Requests are
   at most 1 MiB, rate-limited (burst 32, 20/s) and served one at a time per
   connection; a controller silent for 5 minutes is dropped (the proxy
   reconnects).
3. **Discovery.** `aurora-bridge-<pid>.json` (`port`, `token`, `pid`,
   `version`) and a lock file `aurora-bridge-<pid>.lock` held for the life of
   the app, in `$XDG_RUNTIME_DIR/aurora` (Linux), `<temp>/aurora-bridge-<uid>`
   (Linux fallback and macOS, where `$TMPDIR` is per-user) or
   `%LOCALAPPDATA%\Aurora\bridge` (Windows). Created with `create_new` and
   mode `0600` regardless of umask; the directory `0700`, owned by the user,
   never a symbolic link. Removed on a clean exit. A crash leaves them; the
   next app or proxy sweeps **only** files of exactly that name shape that
   parse as ours, name the same pid, are owned by the user, are regular files
   and whose lock can be taken (the scratch directory's rule, ADR 0014). The
   lock protocol: a discovery file is created and removed only while its
   lock is held, and a lock file is unlinked only by whoever holds the lock
   on the named file, so a sweeper can never delete a newer app's files; a
   publisher re-checks that its locked file is still the named one.
   `AURORA_BRIDGE_DIR` overrides the directory (the same checks apply).
4. **The same tools.** `aurora-cli mcp --bridge [--discovery FILE]` serves
   the ADR 0014 tools through a `BridgeSession`: every operation is forwarded
   to the app. Validation (`validate_args`, `check_enabled`, `-32602`) and the
   honest hints are unchanged. Commands run on the app's UI thread through
   `activate_command` / `run_activated`, the path the palette and menus use.
   New tools: `ui_inspect`, `ui_screenshot`, `ui_pointer`, `ui_key`,
   `ui_menu`, `ui_dialog`. Inputs are injected above the OS, into the same
   handlers real input reaches, so the modal and unsaved-changes gates inside
   those handlers apply.
5. **Policy in the app** (not only in the proxy): a modal dialog or quit
   review refuses everything but reads and `ui_dialog`; an open command
   palette refuses commands and file tools; `app.quit`,
   `automation.disconnect`, `file.open`/`file.save` (use `doc_open`/`doc_save`)
   and every command with a file-path parameter are never run; native file
   dialogs are refused (they would block the UI thread); `doc_open` copies the
   confined file to private scratch and header-checks the copy; `doc_save`
   writes a new file with `create_new` and never overwrites.
6. **A controller never answers a discard or quit question.** `ui.dialog`
   `choose` goes through `dialog_choice_allowed`: a controller may acknowledge
   a report and cancel a question, nothing else. Audit of every
   `DialogPurpose` (a test asserts each row and the `match` over the enum is
   exhaustive, so a new purpose does not compile until classified):

   | Dialog | Actions | Controller may choose |
   |---|---|---|
   | CrashRecovery | Continue | Continue |
   | OpenedWithChanges | OK | OK |
   | OpenFailed / SaveFailed / ExportRefused / SkippedTiles / MoveRefused | Dismiss | Dismiss |
   | ExportedNotSaved | Dismiss | Dismiss |
   | PsdCheck | OK | OK |
   | Welcome | Get Started, Open Command Palette | both (they only close it or open the palette) |
   | **CloseDocument** | Save, Don't Save, Cancel | **Cancel only** |
   | **QuitReview** | Save, Don't Save, Cancel | **Cancel only** |
   | **PsdSaveWithChanges** | Save Anyway, Cancel | **Cancel only** (Save Anyway writes a lossy file) |
   | **ReplaceWorkspace** | Replace, Cancel | **Cancel only** (Replace overwrites a saved preset) |

   `file.close` and Ctrl+W are not blocked (the normal review is the point);
   the dialog they open can only be cancelled by the controller, and the tool
   reply says so (`dialog_note`). A click, drag or typed text that opens a
   dialog stops the rest of that call, so a second click cannot land on it.
   Save is refused too: Aurora's Save is Save As and would raise a native
   dialog, which the bridge never does.
7. **Visible and revocable.** While a controller is connected the window title
   ends with "[automation connected]" and the status bar ends with
   "Automation connected" (also after a notice). The palette command
   **Disconnect Automation** (`automation.disconnect`) closes the controller
   and turns the bridge off for good (the token stops working, the discovery
   file is removed); quitting does the same.
8. **Screenshots** read Aurora's own swapchain frame back (the surface is
   configured with `COPY_SRC` only when the bridge is on). They are not an OS
   screen capture, so no other window can appear in one.

## Dependencies

* **No async runtime or MCP SDK in the app.** The listener, framing and
  client are std-only in a new crate `aurora-automation`; `rmcp` and `tokio`
  stay in `aurora-mcp` and `aurora-cli`.
* `aurora-automation` also now holds the path-confinement code (`roots`,
  `scratch`, `header`) moved out of `aurora-mcp` (which re-exports it), so
  the app confines paths with the **same** code the proxy uses.
* `getrandom` 0.3 (already in `Cargo.lock` through `tempfile`) for the token;
  `rustix` gains its `process` feature (uid, umask in tests), `png` and
  `base64` (already in the lock) join `aurora-app` for the screenshot and
  preview replies. `cargo deny` passes.

## Alternatives considered

* **A token on the command line.** Visible to every local process in `ps`.
  The file is mode `0600`.
* **Unix domain sockets / named pipes.** Better isolation on Unix, but three
  different transports to build and test, and the Mac and Windows are the
  point. Loopback TCP with a token and the strict first line is one code path.
* **HTTP with Origin/Host checks (PhotoCraft's neighbours do this).** Needs a
  parser exposed to browsers. A raw socket whose first line must be a token
  has nothing for a page to say.
* **Screenshots through the OS.** Would capture other windows (a privacy
  leak) and needs per-platform permission prompts.
* **Letting the bridge answer unsaved-changes dialogs freely.** It can only
  through `ui_dialog`, visibly, and only for a dialog the user (or a command)
  opened; quitting is not exposed.

## Consequences

**Gained:** a session can see and operate the real window on any platform the
app runs on; the file rules are the ones already tested; nothing is exposed
unless the user asks.

**Costs / honest limits:**
* Preview, export and PSD check composite on the **UI thread** (bounded by
  `--mcp-bridge-max-mpx`, default 50 Mpx): the window can pause for as long as
  the same action takes for a person. The UI thread never *waits* on the
  socket.
* A failed-handshake lockout is also a denial of service against the real
  controller (capped at 60 s).
* Any process running as the same user can use the bridge. A multi-user
  machine is protected by the permission checks, not by the token alone.
* Windows ACLs on the discovery directory are not verified by code (the
  per-user profile directory is relied on); Windows and macOS are untested.
* The token is generated once per app run and is **not rotated per
  controller**: a controller that has read the discovery file can reconnect
  until the app exits or the user runs Disconnect Automation. Anything that
  could read the file once can read it again.
* **Windows ACLs on the discovery directory are not verified by code**; the
  per-user profile directory's own ACL is relied on. Windows and macOS are
  untested.
* The live-window handlers (`ui_*`, screenshots, the UI-thread marshalling,
  the dialog gate's wiring in `ui.dialog`, the "stop after a dialog opens"
  guards) have **not been run at all** (no display server was available when
  this was written) and are covered by a manual smoke script, not by
  automated tests. The pure parts are tested.
