//! The window half of the bridge handlers (task 160): `ui.inspect`,
//! `ui.screenshot`, `ui.pointer`, `ui.key`, `ui.menu` and `ui.dialog`.
//!
//! Every input is injected **above the OS**, into the same handlers a real
//! event reaches (`handle_pointer_*`, `handle_key_pressed`, the dialog and
//! menu paths), so nothing can bypass a dialog, the palette or the
//! unsaved-changes review: those gates are inside the handlers. The
//! pure parts (key names, coordinates, downscaling) are unit tested.

use aurora_automation::bridge::protocol::CallError;
use base64::Engine as _;
use serde_json::{Value, json};

use crate::bridge_app::{CapturedFrame, Job, Pending};
use crate::bridge_doc::bad;
use crate::{
    App, DialogEffect, DialogPurpose, DialogResult, Key, Modifiers, NamedKey, activate_command,
    dialog_effect, menu_model, run_dialog_action,
};

const MAX_INSPECT_NODES: usize = 1500;
const MAX_TEXT_CHARS: usize = 200;
const MAX_TYPED_CHARS: usize = 256;
const MAX_DRAG_STEPS: u64 = 64;
const MAX_SCREENSHOT_SIDE: u32 = 4096;
const DEFAULT_SCREENSHOT_SIDE: u32 = 1600;

fn clip(text: &str) -> String {
    if text.chars().count() > MAX_TEXT_CHARS {
        text.chars().take(MAX_TEXT_CHARS).collect::<String>() + "\u{2026}"
    } else {
        text.to_owned()
    }
}

/// A key name as the bridge accepts it: a named key (case-insensitive) or a
/// single character.
pub(crate) fn parse_key(name: &str) -> Option<Key> {
    let lower = name.to_ascii_lowercase();
    let named = match lower.as_str() {
        "enter" | "return" => NamedKey::Enter,
        "escape" | "esc" => NamedKey::Escape,
        "tab" => NamedKey::Tab,
        "backspace" => NamedKey::Backspace,
        "delete" | "del" => NamedKey::Delete,
        "space" => NamedKey::Space,
        "up" | "arrowup" => NamedKey::ArrowUp,
        "down" | "arrowdown" => NamedKey::ArrowDown,
        "left" | "arrowleft" => NamedKey::ArrowLeft,
        "right" | "arrowright" => NamedKey::ArrowRight,
        "home" => NamedKey::Home,
        "end" => NamedKey::End,
        "pageup" => NamedKey::PageUp,
        "pagedown" => NamedKey::PageDown,
        "f1" => NamedKey::F1,
        "f2" => NamedKey::F2,
        "f3" => NamedKey::F3,
        "f4" => NamedKey::F4,
        "f5" => NamedKey::F5,
        "f6" => NamedKey::F6,
        "f7" => NamedKey::F7,
        "f8" => NamedKey::F8,
        "f9" => NamedKey::F9,
        "f10" => NamedKey::F10,
        "f11" => NamedKey::F11,
        "f12" => NamedKey::F12,
        _ => {
            let mut chars = name.chars();
            return match (chars.next(), chars.next()) {
                (Some(c), None) if !c.is_control() => Some(Key::Character(c.to_ascii_lowercase())),
                _ => None,
            };
        }
    };
    Some(Key::Named(named))
}

/// Whether a controller may choose `action` in a dialog opened for
/// `purpose` (ADR 0015's table). The rule: a controller can acknowledge a
/// report, and can CANCEL a question, but never answer one that discards
/// work, quits, replaces a saved preset or writes a lossy file. Those
/// answers stay a human's.
///
/// # Errors
/// A sentence saying what the controller may do instead.
pub(crate) fn dialog_choice_allowed(purpose: &DialogPurpose, action: &str) -> Result<(), String> {
    use crate::document_close::CloseChoice;
    let refuse = |what: &str| {
        Err(format!(
            "a controller may not choose `{action}` here ({what}); it can only cancel, and a person \
             answers the rest"
        ))
    };
    match dialog_effect(&DialogResult {
        purpose: purpose.clone(),
        action: action.to_owned(),
    }) {
        // OK / Continue / Dismiss: an acknowledgement changes nothing.
        // Acknowledgements change nothing; the first-run welcome only closes
        // or opens the palette; Cancel on a close, quit or lossy-save
        // question leaves everything as it is.
        DialogEffect::Acknowledged
        | DialogEffect::Welcome(_)
        | DialogEffect::Close(_, CloseChoice::Cancel)
        | DialogEffect::Quit(_, CloseChoice::Cancel)
        | DialogEffect::PsdSave(crate::psd_save::PsdSaveChoice::Cancel) => Ok(()),
        DialogEffect::Close(..) | DialogEffect::Quit(..) => {
            refuse("it saves or discards the document's unsaved changes")
        }
        DialogEffect::PsdSave(_) => refuse("it writes a file that loses or changes content"),
        DialogEffect::Workspace(command) => {
            if matches!(
                command,
                crate::workspace_presets::WorkspaceCommand::ReturnToNamePrompt(_)
            ) {
                Ok(())
            } else {
                refuse("it replaces a saved workspace")
            }
        }
        DialogEffect::Unknown => refuse("it is not one of this dialog's actions"),
    }
}

/// The checks `ui.dialog choose` makes before anything is chosen: the
/// dialog has the action, and a controller may answer it.
pub(crate) fn validate_choice(
    purpose: &DialogPurpose,
    known: bool,
    id: &str,
) -> Result<(), CallError> {
    if !known {
        return Err(bad(format!("the dialog has no action `{id}`")));
    }
    dialog_choice_allowed(purpose, id).map_err(|message| CallError::new("refused", message))
}

/// The actions of the open dialog a controller may choose.
fn controller_choices(dialog: &crate::OpenDialog) -> Vec<String> {
    dialog
        .actions
        .iter()
        .map(|(id, _)| id.clone())
        .filter(|id| dialog_choice_allowed(&dialog.purpose, id).is_ok())
        .collect()
}

/// Box-averages RGBA8 to fit `max_side` (never enlarging).
pub(crate) fn downscale_rgba(frame: &CapturedFrame, max_side: u32) -> (u32, u32, Vec<u8>) {
    let (w, h) = (frame.width, frame.height);
    let longest = w.max(h);
    if longest <= max_side || longest == 0 {
        return (w, h, frame.rgba.clone());
    }
    let scale = f64::from(max_side) / f64::from(longest);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let (tw, th) = (
        ((f64::from(w) * scale).round() as u32).max(1),
        ((f64::from(h) * scale).round() as u32).max(1),
    );
    let mut out = Vec::with_capacity(tw as usize * th as usize * 4);
    for ty in 0..th {
        let y0 = (u64::from(ty) * u64::from(h) / u64::from(th)) as u32;
        let y1 = (((u64::from(ty) + 1) * u64::from(h) / u64::from(th)) as u32).clamp(y0 + 1, h);
        for tx in 0..tw {
            let x0 = (u64::from(tx) * u64::from(w) / u64::from(tw)) as u32;
            let x1 = (((u64::from(tx) + 1) * u64::from(w) / u64::from(tw)) as u32).clamp(x0 + 1, w);
            let mut sum = [0_u64; 3];
            let mut count = 0_u64;
            for y in y0..y1 {
                for x in x0..x1 {
                    let i = (y as usize * w as usize + x as usize) * 4;
                    if let Some(px) = frame.rgba.get(i..i + 3) {
                        for (s, v) in sum.iter_mut().zip(px) {
                            *s += u64::from(*v);
                        }
                        count += 1;
                    }
                }
            }
            let count = count.max(1);
            for s in sum {
                out.push(u8::try_from(s / count).unwrap_or(255));
            }
            out.push(255);
        }
    }
    (tw, th, out)
}

/// A captured frame as the reply: a PNG, base64, with the sizes.
pub(crate) fn encode_screenshot(
    frame: &CapturedFrame,
    max_side: u32,
    scale_factor: f64,
) -> Result<Value, CallError> {
    let (w, h, rgba) = downscale_rgba(frame, max_side);
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, w, h);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|e| CallError::new("failed", format!("png: {e}")))?;
        writer
            .write_image_data(&rgba)
            .map_err(|e| CallError::new("failed", format!("png: {e}")))?;
    }
    Ok(json!({
        "png_base64": base64::engine::general_purpose::STANDARD.encode(&bytes),
        "width": w,
        "height": h,
        "window_physical_width": frame.width,
        "window_physical_height": frame.height,
        "scale_factor": scale_factor,
    }))
}

/// Reads `ctrl`/`shift`/`alt`/`meta` from a request.
fn modifiers_of(job: &Job) -> Modifiers {
    let flag = |name: &str| job.param(name).and_then(Value::as_bool).unwrap_or(false);
    Modifiers {
        control: flag("ctrl"),
        shift: flag("shift"),
        alt: flag("alt"),
        meta: flag("meta"),
    }
}

fn number(job: &Job, name: &str) -> Result<Option<f64>, CallError> {
    match job.param(name) {
        None => Ok(None),
        Some(v) => v
            .as_f64()
            .filter(|n| n.is_finite() && n.abs() < 1.0e6)
            .map(Some)
            .ok_or_else(|| bad(format!("`{name}` must be a finite number"))),
    }
}

impl App {
    /// Describes the widget tree: roles, names, states and logical bounds.
    #[allow(clippy::unnecessary_wraps)]
    pub(crate) fn bridge_ui_inspect(&mut self, job: &Job) -> Result<Value, CallError> {
        let include_hidden = job
            .param("include_hidden")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let tree = &self.workspace.tree;
        let mut nodes = Vec::new();
        let mut truncated = false;
        let mut stack = vec![tree.root()];
        while let Some(id) = stack.pop() {
            if nodes.len() >= MAX_INSPECT_NODES {
                truncated = true;
                break;
            }
            let Some(node) = tree.accessibility(id) else {
                continue;
            };
            if node.is_hidden() && !include_hidden {
                continue;
            }
            let children: Vec<u64> = tree
                .children(id)
                .map(|c| c.iter().map(|c| c.0).collect())
                .unwrap_or_default();
            let bounds = tree
                .bounds(id)
                .map(|r| json!({"x": r.x, "y": r.y, "width": r.width, "height": r.height}));
            nodes.push(json!({
                "id": id.0,
                "role": format!("{:?}", node.role()),
                "label": node.label().map(clip),
                "description": node.description().map(clip),
                "value": node.value().map(clip),
                "disabled": node.is_disabled(),
                "toggled": node.toggled().map(|t| format!("{t:?}")),
                "bounds": bounds,
                "children": children,
            }));
            if let Some(kids) = tree.children(id) {
                stack.extend(kids.iter().rev().copied());
            }
        }
        let (width, height) = self
            .window
            .as_ref()
            .map(|w| {
                let s = w.inner_size();
                (s.width, s.height)
            })
            .unwrap_or_default();
        let focused = self.focus.focused().map(|f| f.0);
        Ok(json!({
            "window": {
                "physical_width": width,
                "physical_height": height,
                "scale_factor": self.scale_factor,
                "logical_width": f64::from(width) / self.scale_factor.max(0.1),
                "logical_height": f64::from(height) / self.scale_factor.max(0.1),
            },
            "tool": format!("{:?}", self.tool),
            "focus": focused,
            "modal": self.bridge_modal(),
            "palette_open": self.command_palette.is_some(),
            "menu_open": self.menu_bar.open.as_ref().map(|o| o.index),
            "nodes": nodes,
            "truncated": truncated,
        }))
    }

    /// Queues a screenshot for the next drawn frame.
    pub(crate) fn bridge_ui_screenshot(
        &mut self,
        job: Job,
    ) -> Option<(Job, Result<Value, CallError>)> {
        let side = match job.param("max_side") {
            None => Ok(DEFAULT_SCREENSHOT_SIDE),
            Some(v) => v
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .filter(|n| (16..=MAX_SCREENSHOT_SIDE).contains(n))
                .ok_or_else(|| bad(format!("`max_side` must be 16 to {MAX_SCREENSHOT_SIDE}"))),
        };
        let side = match side {
            Ok(side) => side,
            Err(e) => return Some((job, Err(e))),
        };
        if self.window.is_none() || self.surface.is_none() {
            return Some((
                job,
                Err(CallError::new("unavailable", "the window is not up yet")),
            ));
        }
        let Some(bridge) = self.bridge.as_mut() else {
            return Some((job, Err(CallError::new("unavailable", "the bridge is off"))));
        };
        if !bridge.capture_supported {
            return Some((
                job,
                Err(CallError::new(
                    "unavailable",
                    "this display cannot be captured (the surface does not allow reading frames back)",
                )),
            ));
        }
        bridge.captured = None;
        bridge.pending.push(Pending::Screenshot {
            job,
            max_side: side,
            since: std::time::Instant::now(),
        });
        self.needs_redraw = true;
        if let Some(window) = self.window.as_ref() {
            window.request_redraw();
        }
        None
    }

    /// Injects pointer events at logical window coordinates (or at the
    /// centre of the widget `id`).
    pub(crate) fn bridge_ui_pointer(&mut self, job: &Job) -> Result<Value, CallError> {
        use winit::event::{MouseButton, MouseScrollDelta};
        let action = job
            .param("action")
            .and_then(Value::as_str)
            .ok_or_else(|| bad("`action` is required: move, click, double_click, drag or scroll"))?
            .to_owned();
        let button = match job.param("button").and_then(Value::as_str) {
            None | Some("left") => MouseButton::Left,
            Some("right") => MouseButton::Right,
            Some("middle") => MouseButton::Middle,
            Some(other) => return Err(bad(format!("unknown button `{other}`"))),
        };
        let start = self.bridge_point(job, "x", "y")?;
        let end = match (number(job, "to_x")?, number(job, "to_y")?) {
            (Some(x), Some(y)) => Some((x, y)),
            (None, None) => None,
            _ => return Err(bad("`to_x` and `to_y` go together")),
        };
        let scale = self.scale_factor;
        let physical = |p: (f64, f64)| (p.0 * scale, p.1 * scale);
        let saved = self.modifiers;
        self.modifiers = modifiers_of(job);
        let result = match action.as_str() {
            "move" => {
                self.handle_pointer_moved(physical(start));
                Ok(())
            }
            "click" | "double_click" => {
                self.handle_pointer_moved(physical(start));
                let times = if action == "click" { 1 } else { 2 };
                for _ in 0..times {
                    // A click that opened a dialog (a tab's close button)
                    // must not be followed by a second press that lands on
                    // the dialog.
                    if self.bridge_modal().is_some() {
                        break;
                    }
                    self.handle_pointer_pressed(button);
                    self.handle_pointer_released(button);
                }
                Ok(())
            }
            "drag" => match end {
                None => Err(bad("`drag` needs `to_x` and `to_y`")),
                Some(end) => {
                    let steps = job
                        .param("steps")
                        .and_then(Value::as_u64)
                        .unwrap_or(8)
                        .clamp(1, MAX_DRAG_STEPS);
                    self.handle_pointer_moved(physical(start));
                    self.handle_pointer_pressed(button);
                    for i in 1..=steps {
                        if self.bridge_modal().is_some() {
                            break;
                        }
                        #[allow(clippy::cast_precision_loss)]
                        let t = i as f64 / steps as f64;
                        let p = (
                            start.0 + (end.0 - start.0) * t,
                            start.1 + (end.1 - start.1) * t,
                        );
                        self.handle_pointer_moved(physical(p));
                    }
                    self.handle_pointer_released(button);
                    Ok(())
                }
            },
            "scroll" => {
                let dx = number(job, "dx")?.unwrap_or(0.0).clamp(-50.0, 50.0);
                let dy = number(job, "dy")?.unwrap_or(0.0).clamp(-50.0, 50.0);
                self.handle_pointer_moved(physical(start));
                #[allow(clippy::cast_possible_truncation)]
                self.handle_mouse_wheel(MouseScrollDelta::LineDelta(dx as f32, dy as f32));
                Ok(())
            }
            other => Err(bad(format!("unknown pointer action `{other}`"))),
        };
        self.modifiers = saved;
        result?;
        self.push_accessibility();
        self.needs_redraw = true;
        let at = end.unwrap_or(start);
        let hit = self
            .workspace
            .tree
            .hit_test(({ at.0 } as f32, { at.1 } as f32))
            .map(|id| {
                let node = self.workspace.tree.accessibility(id);
                json!({
                    "id": id.0,
                    "role": node.map(|n| format!("{:?}", n.role())),
                    "label": node.and_then(|n| n.label()).map(clip),
                })
            });
        Ok(
            json!({"ok": true, "hit": hit, "tool": format!("{:?}", self.tool), "modal": self.bridge_modal(), "dialog_note": self.bridge_modal_note()}),
        )
    }

    /// The target point of a pointer request: `x`,`y` or a widget `id`.
    fn bridge_point(&self, job: &Job, x: &str, y: &str) -> Result<(f64, f64), CallError> {
        if let Some(id) = job.param("id") {
            let id = id
                .as_u64()
                .ok_or_else(|| bad("`id` must be a widget id from ui_inspect"))?;
            let rect = self
                .workspace
                .tree
                .bounds(accesskit::NodeId(id))
                .ok_or_else(|| bad(format!("no widget {id}")))?;
            #[allow(clippy::cast_precision_loss)]
            return Ok((
                rect.x as f64 + f64::from(rect.width) / 2.0,
                rect.y as f64 + f64::from(rect.height) / 2.0,
            ));
        }
        match (number(job, x)?, number(job, y)?) {
            (Some(px), Some(py)) => Ok((px, py)),
            _ => Err(bad(
                "give `x` and `y` (logical window pixels) or a widget `id`",
            )),
        }
    }

    /// Presses a key chord and/or types text through the real key handler.
    pub(crate) fn bridge_ui_key(&mut self, job: &Job) -> Result<Value, CallError> {
        let key = job.param("key").and_then(Value::as_str);
        let text = job.param("text").and_then(Value::as_str);
        if key.is_none() && text.is_none() {
            return Err(bad("give `key` (e.g. \"Enter\", \"z\") and/or `text`"));
        }
        let parsed = match key {
            Some(name) => {
                Some(parse_key(name).ok_or_else(|| bad(format!("unknown key `{name}`")))?)
            }
            None => None,
        };
        if let Some(text) = text
            && (text.chars().count() > MAX_TYPED_CHARS || text.chars().any(char::is_control))
        {
            return Err(bad(format!(
                "`text` is at most {MAX_TYPED_CHARS} printable characters"
            )));
        }
        let saved = self.modifiers;
        self.modifiers = modifiers_of(job);
        if let Some(key) = parsed {
            let typed = match key {
                Key::Character(c) if !self.modifiers.control && !self.modifiers.meta => {
                    Some(c.to_string())
                }
                _ => None,
            };
            self.handle_key_pressed(Some(key), typed.as_deref());
        }
        self.modifiers = Modifiers::default();
        if let Some(text) = text {
            for ch in text.chars() {
                // Typing never continues into a dialog the key opened.
                if self.bridge_modal().is_some() {
                    break;
                }
                let mut buffer = [0_u8; 4];
                let s: &str = ch.encode_utf8(&mut buffer);
                self.handle_key_pressed(Some(Key::Character(ch.to_ascii_lowercase())), Some(s));
            }
        }
        self.modifiers = saved;
        self.needs_redraw = true;
        Ok(
            json!({"ok": true, "modal": self.bridge_modal(), "palette_open": self.command_palette.is_some(), "dialog_note": self.bridge_modal_note()}),
        )
    }

    /// Lists the menus, or runs an enabled menu item by its command id.
    pub(crate) fn bridge_ui_menu(&mut self, job: &Job) -> Result<Value, CallError> {
        let context = self.menu_context();
        let menus = menu_model::build_menus(&context, &self.shortcuts);
        match job.param("action").and_then(Value::as_str) {
            Some("list") | None => {
                let listing: Vec<Value> = menus
                    .iter()
                    .map(|menu| {
                        let items: Vec<Value> = menu
                            .entries
                            .iter()
                            .filter_map(|entry| match entry {
                                menu_model::Entry::Item { command, label, hint, enabled } => Some(
                                    json!({"command": command, "label": label, "hint": hint, "enabled": enabled}),
                                ),
                                menu_model::Entry::Separator => None,
                            })
                            .collect();
                        json!({"menu": menu.title, "items": items})
                    })
                    .collect();
                Ok(json!({"menus": listing}))
            }
            Some("activate") => {
                let id = job
                    .param("command")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        bad("`command` (a command id from the menu listing) is required")
                    })?
                    .to_owned();
                let entry = menus
                    .iter()
                    .flat_map(|m| m.entries.iter())
                    .find_map(|entry| match entry {
                        menu_model::Entry::Item {
                            command, enabled, ..
                        } if *command == id => Some(*enabled),
                        _ => None,
                    })
                    .ok_or_else(|| bad(format!("`{id}` is not a menu item")))?;
                if !entry {
                    return Err(CallError::new(
                        "disabled",
                        format!("`{id}` is disabled in the menu"),
                    ));
                }
                if let Some(reason) = aurora_commands::controller_refusal(&id) {
                    return Err(CallError::new(
                        "refused",
                        format!("`{id}` is not available over the bridge: {reason}"),
                    ));
                }
                if id == crate::COMMAND_FILE_OPEN || id == crate::COMMAND_FILE_SAVE {
                    return Err(CallError::new(
                        "refused",
                        format!(
                            "`{id}` is not available over the bridge (use doc_open / doc_save for files)"
                        ),
                    ));
                }
                let picked = activate_command(
                    &mut self.workspace,
                    &mut self.focus,
                    &id,
                    &mut RefuseDialogs,
                );
                self.run_activated(picked);
                self.push_accessibility();
                self.relayout_after_gallery();
                self.needs_redraw = true;
                Ok(
                    json!({"ok": true, "command": id, "modal": self.bridge_modal(), "dialog_note": self.bridge_modal_note()}),
                )
            }
            Some(other) => Err(bad(format!("unknown menu action `{other}`"))),
        }
    }

    /// Reports the open dialog or chooses one of its actions by id.
    pub(crate) fn bridge_ui_dialog(&mut self, job: &Job) -> Result<Value, CallError> {
        let describe = |app: &Self| -> Value {
            match app.dialog.as_ref() {
                None => json!({"open": false}),
                Some(dialog) => {
                    let tree = &app.workspace.tree;
                    let text = |id| tree.accessibility(id);
                    let actions: Vec<Value> = dialog
                        .actions
                        .iter()
                        .map(|(id, button)| {
                            json!({
                                "id": id,
                                "label": text(*button).and_then(|n| n.label()).map(clip),
                                "default": dialog.default_action == Some(*button),
                                "cancel": dialog.cancel_action == Some(*button),
                            })
                        })
                        .collect();
                    json!({
                        "open": true,
                        "title": text(dialog.root).and_then(|n| n.label()).map(clip),
                        "message": text(dialog.message).and_then(|n| n.value().or_else(|| n.label())).map(clip),
                        "purpose": format!("{:?}", dialog.purpose),
                        "actions": actions,
                        "controller_may_choose": controller_choices(dialog),
                        "note": "A controller can acknowledge reports and cancel questions; it cannot answer one that discards unsaved changes, quits, replaces a workspace or saves a lossy file. A person answers those.",
                    })
                }
            }
        };
        match job.param("action").and_then(Value::as_str) {
            Some("get") | None => Ok(describe(self)),
            Some("choose") => {
                let id = job
                    .param("id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| bad("`id` (an action id from the dialog) is required"))?
                    .to_owned();
                if self.dialog.is_none() {
                    return Err(CallError::new("failed", "no dialog is open"));
                }
                if let Some(dialog) = self.dialog.as_ref() {
                    validate_choice(&dialog.purpose, dialog.action_id_known(&id), &id)?;
                }
                let before = describe(self);
                let result = run_dialog_action(
                    &mut self.workspace,
                    &mut self.focus,
                    &mut self.dialog,
                    Some(id.clone()),
                );
                if let Some(result) = result {
                    self.run_dialog_result(&result);
                }
                self.push_accessibility();
                self.relayout_after_gallery();
                self.needs_redraw = true;
                Ok(json!({"chosen": id, "dialog": before, "now": describe(self)}))
            }
            Some(other) => Err(bad(format!("unknown dialog action `{other}`"))),
        }
    }
}

struct RefuseDialogs;

impl crate::FileDialogAccess for RefuseDialogs {
    fn pick_file(&mut self) -> Option<std::path::PathBuf> {
        None
    }

    fn save_file(&mut self) -> Option<std::path::PathBuf> {
        None
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn key_names_parse() {
        assert_eq!(parse_key("Enter"), Some(Key::Named(NamedKey::Enter)));
        assert_eq!(parse_key("ESC"), Some(Key::Named(NamedKey::Escape)));
        assert_eq!(parse_key("Z"), Some(Key::Character('z')));
        assert_eq!(parse_key("f12"), Some(Key::Named(NamedKey::F12)));
        assert_eq!(parse_key("ab"), None);
        assert_eq!(parse_key(""), None);
        assert_eq!(parse_key("\n"), None);
    }

    #[test]
    fn downscaling_fits_and_never_enlarges() {
        let frame = CapturedFrame {
            width: 400,
            height: 200,
            rgba: vec![200; 400 * 200 * 4],
        };
        let (w, h, px) = downscale_rgba(&frame, 100);
        assert_eq!((w, h), (100, 50));
        assert_eq!(px.len(), 100 * 50 * 4);
        assert!(px.chunks(4).all(|p| p == [200, 200, 200, 255]));
        let (w, h, _) = downscale_rgba(&frame, 1000);
        assert_eq!((w, h), (400, 200));
    }

    #[test]
    fn a_screenshot_reply_is_a_decodable_png() {
        let frame = CapturedFrame {
            width: 4,
            height: 2,
            rgba: vec![9; 4 * 2 * 4],
        };
        let v = encode_screenshot(&frame, 100, 2.0).unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(v["png_base64"].as_str().unwrap())
            .unwrap();
        assert!(bytes.starts_with(b"\x89PNG"));
        assert_eq!(v["scale_factor"], 2.0);
    }
}
