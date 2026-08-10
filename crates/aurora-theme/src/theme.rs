//! Theme parsing, `extends` inheritance, and resolution into
//! [`Theme`]'s fixed token vocabulary (`design/tokens/vocabulary.md`).

use std::collections::HashMap;

use toml::Value;

use crate::color::Color;
use crate::error::ThemeError;
use crate::palette::Palette;

/// A translucent tint composited over a surface (`state.hover_overlay`/
/// `state.active_overlay` — `vocabulary.md`'s "State" section: "not a
/// fixed color, so it works identically on every `surface.*`").
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Overlay {
    pub color: Color,
    pub alpha: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceTokens {
    pub canvas: Color,
    pub app: Color,
    pub panel: Color,
    pub raised: Color,
    pub overlay: Color,
    pub sunken: Color,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextTokens {
    pub primary: Color,
    pub secondary: Color,
    pub disabled: Color,
    pub on_accent: Color,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IconTokens {
    pub default: Color,
    pub secondary: Color,
    pub disabled: Color,
    pub on_accent: Color,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BorderTokens {
    pub default: Color,
    pub strong: Color,
    pub focus: Color,
    pub control: Color,
    pub control_opacity: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AccentTokens {
    pub primary: Color,
    pub primary_hover: Color,
    pub primary_active: Color,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StateTokens {
    pub error: Color,
    pub warning: Color,
    pub success: Color,
    pub disabled_opacity: f32,
    pub hover_overlay: Overlay,
    pub active_overlay: Overlay,
}

/// A fully resolved theme: every token in `design/tokens/vocabulary.md`,
/// with its `extends` chain (if any) already merged and every palette
/// reference already resolved to a concrete [`Color`]. Nothing downstream
/// of this type ever sees a palette reference string or an unresolved
/// parent — see [`ThemeSet::resolve`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Theme {
    pub is_default: bool,
    pub surface: SurfaceTokens,
    pub text: TextTokens,
    pub icon: IconTokens,
    pub border: BorderTokens,
    pub accent: AccentTokens,
    pub state: StateTokens,
}

/// One registered theme's raw, unresolved data: its own declared parent
/// name (`"none"` for a base theme) and its flattened own fields
/// (dotted key, e.g. `"surface.canvas"` -> the TOML value at that path —
/// only the fields *this* theme file itself sets, not inherited ones).
#[derive(Debug)]
struct RawTheme {
    extends: String,
    is_default: bool,
    fields: HashMap<String, Value>,
}

/// A set of registered themes, resolved by name including `extends`
/// inheritance — the actual consumer-facing entry point.
///
/// Hot reload (PLAN.md M1.6) re-registers a theme's source and calls
/// [`Self::resolve`] again; watching the filesystem for changes is a
/// thin, separate piece layered on top of this (not yet wired — no
/// caller exists yet to drive it), not something this type's own parsing
/// and inheritance logic needs to know about.
#[derive(Debug)]
pub struct ThemeSet {
    themes: HashMap<String, RawTheme>,
}

impl ThemeSet {
    #[must_use]
    pub fn new() -> Self {
        Self {
            themes: HashMap::new(),
        }
    }

    /// Parses and registers a theme from raw TOML source (e.g. the
    /// contents of a `themes/*.toml` file), keyed by its own `name`
    /// field. Registering again under the same name replaces the
    /// previous registration — this is hot reload's own re-register step.
    ///
    /// # Errors
    ///
    /// Returns [`ThemeError::Toml`] if `source` isn't valid TOML, or
    /// [`ThemeError::WrongType`] if the required `name`/`extends`/
    /// `is_default` fields are missing or the wrong type.
    pub fn register(&mut self, source: &str) -> Result<(), ThemeError> {
        let root: Value = toml::from_str(source)?;
        let name = require_str(&root, "name")?.to_owned();
        let extends = require_str(&root, "extends")?.to_owned();
        let is_default = root
            .get("is_default")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        let mut fields = HashMap::new();
        for key in ["surface", "text", "icon", "border", "accent", "state"] {
            if let Some(section) = root.get(key) {
                flatten(key, section, &mut fields);
            }
        }

        self.themes.insert(
            name,
            RawTheme {
                extends,
                is_default,
                fields,
            },
        );
        Ok(())
    }

    /// Resolves `name`'s full `extends` chain (root ancestor first) into
    /// a fully-resolved [`Theme`], with every palette reference resolved
    /// through `palette`.
    ///
    /// # Errors
    ///
    /// Returns [`ThemeError::UnknownParent`] if `name` (or an ancestor
    /// named by `extends`) was never [`Self::register`]ed,
    /// [`ThemeError::CyclicExtends`] if the chain loops back on itself,
    /// or [`ThemeError::MissingToken`]/[`ThemeError::WrongType`]/
    /// [`ThemeError::UnknownPaletteReference`]/[`ThemeError::InvalidColor`]
    /// if a required token is missing or malformed once the whole chain
    /// is merged.
    pub fn resolve(&self, name: &str, palette: &Palette) -> Result<Theme, ThemeError> {
        let chain = self.extends_chain(name)?;

        let mut merged: HashMap<String, Value> = HashMap::new();
        let mut is_default = false;
        // Root ancestor first, so a child's own fields overwrite what it
        // inherited -- last insertion into `merged` wins.
        for theme_name in chain.iter().rev() {
            let Some(raw) = self.themes.get(theme_name) else {
                unreachable!("extends_chain only returns names already confirmed registered");
            };
            merged.extend(raw.fields.clone());
            is_default = raw.is_default;
        }

        Ok(Theme {
            is_default,
            surface: SurfaceTokens {
                canvas: resolve_color(&merged, "surface.canvas", palette)?,
                app: resolve_color(&merged, "surface.app", palette)?,
                panel: resolve_color(&merged, "surface.panel", palette)?,
                raised: resolve_color(&merged, "surface.raised", palette)?,
                overlay: resolve_color(&merged, "surface.overlay", palette)?,
                sunken: resolve_color(&merged, "surface.sunken", palette)?,
            },
            text: TextTokens {
                primary: resolve_color(&merged, "text.primary", palette)?,
                secondary: resolve_color(&merged, "text.secondary", palette)?,
                disabled: resolve_color(&merged, "text.disabled", palette)?,
                on_accent: resolve_color(&merged, "text.on_accent", palette)?,
            },
            icon: IconTokens {
                default: resolve_color(&merged, "icon.default", palette)?,
                secondary: resolve_color(&merged, "icon.secondary", palette)?,
                disabled: resolve_color(&merged, "icon.disabled", palette)?,
                on_accent: resolve_color(&merged, "icon.on_accent", palette)?,
            },
            border: BorderTokens {
                default: resolve_color(&merged, "border.default", palette)?,
                strong: resolve_color(&merged, "border.strong", palette)?,
                focus: resolve_color(&merged, "border.focus", palette)?,
                control: resolve_color(&merged, "border.control", palette)?,
                control_opacity: require_f32(&merged, "border.control_opacity")?,
            },
            accent: AccentTokens {
                primary: resolve_color(&merged, "accent.primary", palette)?,
                primary_hover: resolve_color(&merged, "accent.primary_hover", palette)?,
                primary_active: resolve_color(&merged, "accent.primary_active", palette)?,
            },
            state: StateTokens {
                error: resolve_color(&merged, "state.error", palette)?,
                warning: resolve_color(&merged, "state.warning", palette)?,
                success: resolve_color(&merged, "state.success", palette)?,
                disabled_opacity: require_f32(&merged, "state.disabled_opacity")?,
                hover_overlay: Overlay {
                    color: resolve_color(&merged, "state.hover_overlay.color", palette)?,
                    alpha: require_f32(&merged, "state.hover_overlay.alpha")?,
                },
                active_overlay: Overlay {
                    color: resolve_color(&merged, "state.active_overlay.color", palette)?,
                    alpha: require_f32(&merged, "state.active_overlay.alpha")?,
                },
            },
        })
    }

    /// `name`, then its parent, then its parent's parent, ... up to (but
    /// not including) the `"none"` root sentinel.
    fn extends_chain(&self, name: &str) -> Result<Vec<String>, ThemeError> {
        let mut chain = vec![name.to_owned()];
        let mut current = name.to_owned();
        loop {
            let raw = self
                .themes
                .get(&current)
                .ok_or_else(|| ThemeError::UnknownParent(current.clone()))?;
            if raw.extends == "none" {
                return Ok(chain);
            }
            if chain.contains(&raw.extends) {
                return Err(ThemeError::CyclicExtends(name.to_owned()));
            }
            chain.push(raw.extends.clone());
            current = raw.extends.clone();
        }
    }
}

impl Default for ThemeSet {
    fn default() -> Self {
        Self::new()
    }
}

/// Recursively flattens a nested TOML table into dotted-path keys (e.g.
/// `{ hover_overlay = { color = "..." } }` under `"state"` becomes
/// `"state.hover_overlay.color"`), the same flattening
/// `design/check_contrast.py`'s own path-walking resolver relies on
/// implicitly by splitting a reference on `.` — made explicit and
/// reusable here since inheritance needs to merge at this granularity,
/// not section-at-a-time.
fn flatten(prefix: &str, value: &Value, out: &mut HashMap<String, Value>) {
    if let Value::Table(table) = value {
        for (key, child) in table {
            flatten(&format!("{prefix}.{key}"), child, out);
        }
    } else {
        out.insert(prefix.to_owned(), value.clone());
    }
}

fn require_str<'a>(value: &'a Value, key: &str) -> Result<&'a str, ThemeError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| ThemeError::WrongType {
            reference: key.to_owned(),
            expected: "a string",
        })
}

fn require_f32(fields: &HashMap<String, Value>, key: &str) -> Result<f32, ThemeError> {
    let value = fields
        .get(key)
        .ok_or_else(|| ThemeError::MissingToken(key.to_owned()))?;
    #[allow(clippy::cast_possible_truncation)]
    value
        .as_float()
        .map(|f| f as f32)
        .ok_or_else(|| ThemeError::WrongType {
            reference: key.to_owned(),
            expected: "a number",
        })
}

fn resolve_color(
    fields: &HashMap<String, Value>,
    key: &str,
    palette: &Palette,
) -> Result<Color, ThemeError> {
    let reference = fields
        .get(key)
        .ok_or_else(|| ThemeError::MissingToken(key.to_owned()))?
        .as_str()
        .ok_or_else(|| ThemeError::WrongType {
            reference: key.to_owned(),
            expected: "a palette reference string",
        })?;
    palette.resolve(reference)
}

#[cfg(test)]
mod tests {
    use super::ThemeSet;
    use crate::ThemeError;
    use crate::palette::Palette;

    const PALETTE_TOML: &str = include_str!("../../../design/tokens/palette.toml");
    const DARK_TOML: &str = include_str!("../../../design/themes/dark.toml");

    fn palette() -> Palette {
        match Palette::from_toml_str(PALETTE_TOML) {
            Ok(p) => p,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    #[test]
    fn resolves_the_real_committed_dark_theme() {
        let mut set = ThemeSet::new();
        if let Err(err) = set.register(DARK_TOML) {
            unreachable!("{err:?}");
        }
        let theme = match set.resolve("Dark", &palette()) {
            Ok(t) => t,
            Err(err) => unreachable!("{err:?}"),
        };
        assert!(theme.is_default);
        // A handful of spot checks against the real, owner-approved file
        // -- full contrast coverage is `contrast::tests`' job.
        assert_eq!(theme.surface.canvas.r, 0x14);
        assert!((theme.state.disabled_opacity - 0.4).abs() < 1e-6);
        assert!((theme.state.hover_overlay.alpha - 0.06).abs() < 1e-6);
    }

    #[test]
    fn resolve_rejects_an_unregistered_theme_name() {
        let set = ThemeSet::new();
        match set.resolve("NoSuchTheme", &palette()) {
            Err(ThemeError::UnknownParent(got)) => assert_eq!(got, "NoSuchTheme"),
            other => unreachable!("expected UnknownParent, got {other:?}"),
        }
    }

    #[test]
    fn a_child_theme_inherits_unset_fields_and_overrides_set_ones() {
        let mut set = ThemeSet::new();
        if let Err(err) = set.register(DARK_TOML) {
            unreachable!("{err:?}");
        }
        // A synthetic child -- proving the merge logic works generically,
        // not asserting a real second design (Light/HC/Color-Critical
        // themes don't exist yet; inventing their colours here would be
        // the design owner's call to make, not an engineering detail).
        let child = r#"
            schema_version = 1
            name = "TestChild"
            extends = "Dark"
            is_default = false

            [surface]
            canvas = "neutral.900"
        "#;
        if let Err(err) = set.register(child) {
            unreachable!("{err:?}");
        }

        let dark = match set.resolve("Dark", &palette()) {
            Ok(t) => t,
            Err(err) => unreachable!("{err:?}"),
        };
        let resolved = match set.resolve("TestChild", &palette()) {
            Ok(t) => t,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(
            resolved.surface.canvas.r, 0xf5,
            "the overridden field must use the child's own value"
        );
        assert_eq!(
            resolved.surface.panel, dark.surface.panel,
            "an un-overridden field must be inherited unchanged from the parent"
        );
    }

    #[test]
    fn resolve_rejects_a_cyclic_extends_chain() {
        let mut set = ThemeSet::new();
        let a = r#"
            schema_version = 1
            name = "A"
            extends = "B"
            is_default = false
        "#;
        let b = r#"
            schema_version = 1
            name = "B"
            extends = "A"
            is_default = false
        "#;
        if let Err(err) = set.register(a) {
            unreachable!("{err:?}");
        }
        if let Err(err) = set.register(b) {
            unreachable!("{err:?}");
        }
        match set.resolve("A", &palette()) {
            Err(ThemeError::CyclicExtends(_)) => {}
            other => unreachable!("expected CyclicExtends, got {other:?}"),
        }
    }

    #[test]
    fn resolve_reports_a_missing_required_token() {
        let mut set = ThemeSet::new();
        // Deliberately incomplete: no [text]/[icon]/[border]/[accent]/[state].
        let incomplete = r#"
            schema_version = 1
            name = "Incomplete"
            extends = "none"
            is_default = false

            [surface]
            canvas = "neutral.0"
            app = "neutral.50"
            panel = "neutral.100"
            raised = "neutral.150"
            overlay = "neutral.200"
            sunken = "neutral.0"
        "#;
        if let Err(err) = set.register(incomplete) {
            unreachable!("{err:?}");
        }
        match set.resolve("Incomplete", &palette()) {
            Err(ThemeError::MissingToken(_)) => {}
            other => unreachable!("expected MissingToken, got {other:?}"),
        }
    }
}
