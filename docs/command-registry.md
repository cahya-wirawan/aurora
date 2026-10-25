# The command registry

One description of every editor operation, shared by the command palette,
the menus and (tasks 159 and 160) automation. It lives in the `aurora-commands`
crate: pure metadata and validation, depending only on `serde` and
`thiserror`, so `aurora-cli` can use it without the app.

## What a command is

`CommandSpec` (`crates/aurora-commands/src/spec.rs`):

| Field | Meaning |
|---|---|
| `id` | Stable, lower-case (`layer.new_curves`, `tool.brush`). Pinned by `tests/command_ids.tsv`. |
| `title` | The palette title; menus use it too unless a menu spells it differently. |
| `docs` | What it does, for a person or a model reading the listing. |
| `category` | Grouping only (File, Edit, Layer, Select, View, Window, Tool, Colour, Paths). |
| `access` | `Read` (changes nothing), `Ui` (app state only: tool, colours, zoom, panels, clipboard), `Write` (edits the document, writes a file, changes the open documents). A hint, not a guarantee. |
| `params` | Typed parameters: name, docs, kind (`Bool`, `Int{min,max}`, `Number{min,max}`, `Text{max_chars}`, `Path{mode}`, `Choice`), required or optional with a default. |
| `requires` | Conditions for being enabled (`CanUndo`, `NotQuickMask`, ...). Each has a reason sentence. |
| `presence` | `Palette` (listed from the start), `Dynamic` (the app adds it to the palette when it opens, under its own title), `MenuOnly`. |

Commands whose ids the app makes up at run time (one per workspace, open
document or saved selection) are described once as a `FamilySpec` with an id
prefix (`workspace.switch/`, `document.switch.`, `select.load:`, ...).

## Using it

```rust
let registry = aurora_commands::Registry::builtin();
let listing = registry.list();                         // serde description
let call = registry.validate_args("select.expand", &args)?; // Validated or ArgError
registry.check_enabled("edit.undo", &context)?;        // Err(NotEnabled::Reason(..))
```

`validate_args` runs before anything executes: unknown names are refused
(first), then per parameter a missing required value, a wrong type (an
integer for a number parameter is accepted and converted, nothing else is
coerced), out-of-range or non-finite numbers, bad choices, empty or over-long
text, control characters. Defaults are filled in. A `Path` parameter is
checked as text only; where a caller may read or write is the host's job
(task 159).

## What stays in `aurora-app`

Running a command. `activate_command` resolves an id to an
`ActivatedCommand` (or does a panel command inline) and `App::run_activated`
performs it; the registry executes nothing. Parameters describe the
**automation** form of a command; the interactive form gathers the same
values with a dialog or prompt. `command_registry::activated_from_args` is the
first bridge from a validated typed call to the same `ActivatedCommand` the
UI produces, for: Open, Save As, Set Output Profile, Select > Modify (Feather,
Expand, Contract, Smooth, Border), Select Colour Range, Save Selection and
Save Workspace As (the app's own name rules apply after the registry's).
Every other command is run by id.

The palette (`palette_commands`) is the `Palette` entries of the table in
table order. The menu model takes labels and the enabled rules from the
registry (`menu_model::enabled` asks `check_enabled`).

## Adding a command

1. Add a `CommandSpec` to `COMMANDS` in `crates/aurora-commands/src/table.rs`
   **at the place it should appear in the palette**: id, title, docs, category,
   access, parameters (if automation can pass any), requirements.
2. Resolve it in `activate_command` (`crates/aurora-app/src/lib.rs`) to an
   `ActivatedCommand`, and run that in `App::run_activated`. If it has
   parameters, add its arm to `command_registry::typed_form`.
3. If a menu lists it, add `Cmd(id)` to `menu_model::static_rows` (and the
   macOS `build_menu` / `NATIVE_MENU_COMMANDS`, by hand).
4. Run `AURORA_UPDATE_GOLDEN=1 cargo test -p aurora-commands` to rewrite
   `tests/command_ids.tsv`, and read the diff: **an id, once released, is
   never renamed or reused.**
5. The app tests (`command_registry_tests.rs`) fail if the palette, a menu
   or the dispatcher disagrees with the registry, if a title differs from the
   app-side constant, or if the enabled rules change.

## Not covered (gaps)

* Shortcuts: the shortcut registry maps keys to `AppCommand`, and
  `menu_model::hint_command` maps ids to those; the registry does not own
  default bindings yet.
* The macOS native menu (`build_menu`) still spells its labels and ids itself
  (cfg-gated, not compiled on Linux); `NATIVE_MENU_COMMANDS` is checked
  against the registry by a test.
* Prompt rows (`select.save.apply`, `paths.name.apply`, `workspace.name.save`)
  are plumbing inside open prompts, not commands.
* Tool operations (strokes, drags) and panel widgets are not commands.
* Typed forms exist for the commands listed above; `paths.rename`,
  `paths.save_work` and the others that act on UI state have no parameters
  and no typed form yet. Parameters are described for tools by one id per
  tool (`tool.brush`), not one `tool.select` with a choice.
