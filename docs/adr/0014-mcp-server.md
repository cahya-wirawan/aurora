# 0014. A headless MCP server: `rmcp`, `tokio` for its I/O only, `cap-std` for file access

**Status:** Accepted
**Date:** 2026-10-25
**Deciders:** Cahya Wirawan (approved headless-first and the three dependencies on 2026-10-25); implementation by Claude
**Related:** PLAN.md Tasks 158-160, ADR 0012 (background compositing), `docs/mcp.md`, `docs/command-registry.md`

## Context

Aurora should be drivable by a model or a script: open a document, inspect
it, run editor commands, look at a rendering, save, and check what a PSD
save would lose. The Model Context Protocol (MCP) is the interface models
already speak. Task 158 gave every editor operation an id, typed
parameters and an enabled check (`aurora-commands`); this task puts a
server in front of it. It must run **without a window or a GPU** (task 159;
task 160 later drives the running app over a socket).

Constraints that make it hard:

* An MCP server hands a stranger's arguments to code that reads and writes
  files. A prompt-injected model is the realistic attacker. Paths are the
  attack surface: `..`, symbolic links, absolute paths, overwriting a file
  the user cares about, a 50 GB "image".
* stdout is the protocol. One stray `println!` corrupts the session.
* Aurora's lints deny `unwrap`, `expect`, `panic` and indexing: a hostile
  argument must be an error, never a crash.
* CLAUDE.md invariant 4: the UI thread never blocks on rendering; the
  workspace keeps async runtimes away from the render loop.
* The document code (open, composite, PSD report, export) lives in
  `aurora-app`, much of it `pub(crate)`.

## Decision

1. **Three new dependencies**, pinned in the workspace `Cargo.toml` and
   `Cargo.lock`, `cargo deny check all` clean:
   * `rmcp` (the official Rust MCP SDK, features `server`, `macros`,
     `transport-io`): protocol framing, initialisation, tool and resource
     plumbing, progress and cancellation. `Cargo.toml` asks for `3.5`; the
     lock resolves 3.7.0.
   * `tokio`: `rmcp`'s runtime. It is used for the stdio stream, the
     message pump and the hand-off to blocking threads, **in `aurora-mcp`
     and `aurora-cli` only**. `aurora-app`, the render loop, the tile
     store and every document crate stay free of it (`cargo tree -i tokio` lists only those two
     and `rmcp`; `cargo tree -p aurora-app -i tokio` finds nothing).
     Document work runs on `spawn_blocking` threads, one operation at a
     time, so a long export never occupies an async worker.
   * `cap-std`: capability-based file access. Every path is opened through a
     `Dir` handle of a granted root, so confinement is the `openat` itself,
     not a check made before it.
2. **A new crate `aurora-mcp`** (layering: depends on `aurora-commands`
   only) holds the protocol side: tools, resources, roots, the hardened
   line transport and a `Session` trait the document host implements.
   It knows no document.
3. **The document host lives in `aurora-app`** as `aurora_app::mcp_session`
   (`HeadlessSession`), next to `aurora_app::headless`, because it must call
   the application's own open, composite, export and PSD-report code, which
   is private to that crate. It uses the CPU export composite; no `App`,
   window, event loop or GPU context is created. It speaks plain Rust and
   `serde_json`, not MCP types, so the app does not depend on `rmcp` or
   `tokio`.
4. **`aurora-cli mcp`** is the binary. **`aurora-cli` now links the whole
   `aurora-app` crate** (winit, wgpu, accesskit, the widget toolkit and every
   document crate), although `mcp` never creates a window or a GPU context.
   That is deliberate and the price of the design: the code that opens a
   file, composites it and writes PSD/`.aur` is private to `aurora-app`, and
   moving it down into a library crate is a refactor of that crate's `lib.rs`
   as large as the feature. Until then a headless binary that does real
   document work has to depend on the crate that holds it. It depends on `aurora-mcp` and on
   `aurora-app` (a `scripts/layering.json` edit: `aurora-cli` gains
   `aurora-mcp` and `aurora-app`) and adapts one to the other with a
   newtype (`AppHost`), because the trait and the session type are both
   foreign to the CLI.
5. **Security model.**
   * No filesystem authority unless the user passes `--read-root DIR` /
     `--write-root DIR` (repeatable). The only ambient-authority call is
     `Roots::open`, for exactly those directories.
   * `..` is refused on the text; absolute paths must lie inside a root;
     symbolic links that lead out of a root are refused by `cap-std` at
     open time; control characters and over-long paths are refused.
   * The server **copies** a confined read file once into its private
     (`0700`) scratch directory and gives the host that copy, so the host
     never re-resolves a caller path (no check/use gap) and a file cannot
     change under the decoder.
   * A write is produced by the host into a private scratch path, then
     copied into the write root with `create_new` (`O_EXCL`): an existing
     file, a symlink to one and a dangling link are refused atomically;
     nothing is ever opened for writing in place; a failed or cancelled
     copy removes the file this call created. The target is also checked
     before the expensive export starts.
   * Caps: request line 1 MiB (an oversized line ends the session and is
     never forwarded, not even a valid prefix), reply 4 MiB, preview side
     2048 and PNG 6 MiB, open size 2 GiB (`--max-open-mb`), batch 32 steps,
     inspect 2000 layers, 200 Mpx composites.
   * Arguments are strict: unknown tool argument, wrong type, unknown command
     id, unknown command argument, out-of-range value are JSON-RPC `-32602`
     before anything runs; a whole batch is validated before its first step.
   * The session lock is never taken on an async worker (only on
     `spawn_blocking` threads), a command's enabled check and run share one
     lock hold, and a batch of host commands holds it across its steps.
   * stdout is the protocol: on Unix it is moved to a private duplicate of
     fd 1 and fd 1 is redirected to stderr (`rustix`'s safe `dup`/`dup2`,
     already in the lock file through `cap-std`, no `unsafe`).
   * Scratch lives in one `0700` directory locked for the process's life and
     swept after a crash; the document's tile store is inside it.
   * stdout carries only JSON-RPC lines; a non-JSON line gets `-32700` and
     the next line is still served; logs go to stderr.
   * Honest hints on every tool (all four annotations), documented in
     `docs/mcp.md`; hints are not permissions.

A fourth dependency was added during review: `rustix` (features `std`,
`fs`, `stdio`; Unix only), for `O_NONBLOCK`, `statvfs` and the stdout
redirect without `unsafe`. It was already in `Cargo.lock` (a `cap-std`
dependency) and `cargo deny` passes.

## Alternatives considered

* **Hand-written JSON-RPC, no `rmcp`.** Fewer dependencies, but the
  protocol moves (the SDK tracks the 2026-07-28 spec: progress tokens,
  cancellation, result/caching fields, schema generation). Re-implementing
  it is more code to get wrong than the SDK is.
* **`std::fs` with `canonicalize` and a prefix check.** Simple, and wrong:
  the check and the open are separate steps, so a symlink swapped between
  them escapes; `canonicalize` also needs the file to exist. `cap-std`
  makes the open itself the check.
* **Hand the host the confined path (or an open handle) instead of a
  copy.** Avoids a copy of up to 2 GiB, but the decoders take paths and
  re-resolve them; the copy keeps the host free of caller paths entirely.
  Cost: one extra read of the file and scratch space equal to it.
* **The server inside `aurora-app` (a third binary).** No layering edit, but
  it puts `rmcp`, `tokio` and `cap-std` into the GUI application's
  dependency graph and binary. Keeping the protocol in its own crate keeps
  them out of the app; the cost is the CLI depending on the app.
* **Move the document code down into a library crate so `aurora-cli` need
  not depend on `aurora-app`.** The right long-term shape, and a very large
  refactor (open, composite, PSD save, undo order and selection code are
  woven into `aurora-app`'s `lib.rs`). Not done here; see Reconsider.
* **Atomic `command_batch` (roll back on failure).** The document model has
  per-step undo, not transactions across commands; faking atomicity would
  claim more than it gives. The batch is documented as not atomic, with the
  recovery recipe (`edit.undo` per applied edit).

## Consequences

**Gained:** a model can open, inspect, edit (a subset), render, check and
save documents without a window; the file-access rules are small, local
and tested with symlinks, `..`, overwrites and cancellation; `tokio` is
confined to two crates; the protocol side is testable with a mock host.

**Cost:** `aurora-cli` links the whole application crate (winit, wgpu, the
widget toolkit) although `mcp` never creates a window or GPU context; the
binary is large and its startup is the app's library load. A copy of every
opened file goes through scratch. Requests pipelined without waiting for
replies are served concurrently by `rmcp` and may run out of order (a
client that sends `doc_open` and `command_run` back to back, without
waiting, can see the second run first); calls on the session itself are
serialised. One document per session. Linux is the only platform the
tests ran on: `cap-std` is cross-platform, but Windows drive-prefixed
absolute paths and macOS were not exercised. The headless command set is
small (14 of about 150 registry commands); the rest report "not available
headless".

**Not verified:** a red-team reproduced several faults against the first
version, which this revision fixes with tests; the red team's own live
path-escape battery and the PSD/`.aur`/zip bomb batteries were not run by
anyone but the builder's tests (docs/mcp.md lists what is covered).

**Follow-on work:** task 160 (bridge mode to the running app, `ui_*`
tools); more headless commands as their effects get document-level
implementations (adjustment layers, paths, crop, saved selections, colour
output profile); multiple documents; moving the document code down so the
CLI no longer depends on the app; a cancellable composite pool (today
cancellation is checked between tiles of the single-threaded export
composite).

## Reconsider if...

* `rmcp` stops tracking the spec, or a licence/advisory in its tree fails
  `cargo deny`.
* The CLI's dependence on the app crate becomes a real cost (binary size,
  link time, a platform where the app's libraries cannot load without a
  display): then move the document code down.
* A second consumer (a plugin host, task 160's bridge) needs the same
  confinement: promote `Roots` to a lower crate.
* Someone needs `tokio` near the render loop: this ADR says no; write a new
  one.
