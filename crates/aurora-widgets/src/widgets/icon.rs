//! An icon (0.194.0, round L1 of the Photoshop-style Layers panel): one
//! picture from [`crate::icons`]'s set, drawn as vector geometry in an
//! `icon.*` colour. Generic — it knows nothing about layers; `aurora-ui`
//! decides which icon means what.
//!
//! **Shape.** A square, [`icon_side`] logical px a side (a spacing token:
//! [`IconSize::Small`] is `spacing.md`, 16 px, [`IconSize::Large`] is
//! `spacing.lg`, 24 px — the set is drawn to read at 16), that does not
//! shrink. It paints the icon's geometry ([`crate::icons::icon_paths`])
//! through the same `Paint` (mesh + colour) path every other widget uses:
//! `aurora_vector::fill` for the filled parts and
//! `aurora_vector::stroke_rounded` — round caps and joins, the one
//! addition 0.194.0 made to `aurora-vector`, since [`aurora_vector::stroke`]'s
//! butt caps and mitres suit rectangles and not line icons — for the
//! strokes. No new GPU pipeline, no glyphs.
//!
//! **Colour** is the [`IconTone`], an `icon.*` token chosen by state:
//! `icon.default`, `icon.secondary`, `icon.disabled`, and `icon.on_accent`
//! for an icon drawn on an `accent.*` fill. `design/check_contrast.py`
//! gates `icon.default` and `icon.secondary` at 3:1 (WCAG 1.4.11, a
//! graphical object) on every surface an icon sits on, in every built-in
//! theme; `icon.disabled` is deliberately not gated (disabled content).
//!
//! **Accessibility** has two modes, chosen explicitly ([`IconAccess`]):
//!
//! - [`IconAccess::Decorative`] — the icon only repeats what text or a
//!   named parent already says (the eye inside a "Visible" checkbox, the
//!   icon that is the only content of a button, whose *button* carries the
//!   name): the node is `hidden`, so assistive technology never meets it.
//! - [`IconAccess::Named`] — the icon stands alone and means something:
//!   a `Role::Image` with that name as its label.
//!
//! Either way it exposes no actions: never a tab stop, never a click
//! target an assistive technology is offered. An icon that must be
//! clicked is a button (or a row) that contains one.
//!
//! **Clipping.** A widget's mesh is not scissored (`paint`'s doc comment):
//! an icon that an ancestor clips only partly is **not drawn**, rather
//! than painted over its neighbour. A scrolled row's icon therefore
//! appears once it is fully in view.

use accesskit::{Node, Role};
use aurora_theme::Scales;
use taffy::style_helpers::length;
use taffy::{Size, Style};

use super::WidgetKind;
use crate::error::WidgetError;
use crate::icons::IconKind;
use crate::tree::{WidgetId, WidgetTree};

/// Which `icon.*` token colours an icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum IconTone {
    /// `icon.default`.
    #[default]
    Default,
    /// `icon.secondary`: de-emphasised (an inactive tool, a secondary
    /// action).
    Secondary,
    /// `icon.disabled`; also announced as disabled when the icon is
    /// named.
    Disabled,
    /// `icon.on_accent`: on an `accent.*` fill (a selected row).
    OnAccent,
}

impl IconTone {
    /// Every tone, in the order the gallery shows them.
    pub const ALL: [IconTone; 4] = [
        IconTone::Default,
        IconTone::Secondary,
        IconTone::Disabled,
        IconTone::OnAccent,
    ];

    /// The tone's name, for labels.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            IconTone::Default => "Default",
            IconTone::Secondary => "Secondary",
            IconTone::Disabled => "Disabled",
            IconTone::OnAccent => "On Accent",
        }
    }
}

/// How an icon presents itself to assistive technology.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub enum IconAccess {
    /// Purely decorative: the node is hidden.
    #[default]
    Decorative,
    /// The icon carries meaning on its own: a `Role::Image` with this
    /// accessible name.
    Named(String),
    /// The icon is itself a small button (0.195.0, the Layers panel's eye):
    /// a `Role::Button` with this accessible name, `toggled` as accesskit's
    /// toggle state when it has one, and **`Click` only** — never `Focus`,
    /// so it is not a tab stop (a row of a long list owns the one stop;
    /// the icon is reached by a pointer, by an assistive technology's
    /// `Click`, or by the owner's own key). Clicking does **not** flip
    /// `toggled`: like [`crate::widgets::ButtonState`], the owner decides
    /// what a click means and sets the state back
    /// ([`set_icon_access`]). It paints no outline or fill of its own: the
    /// picture is the whole control.
    Button {
        label: String,
        toggled: Option<bool>,
    },
}

/// An icon's size class: a spacing token, a square.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum IconSize {
    /// `spacing.md` (16 px by default): the size the set is drawn for.
    #[default]
    Small,
    /// `spacing.lg` (24 px by default).
    Large,
}

/// The side of an icon of `size`, in logical px, resolved from the token
/// scales (`spacing.md` / `spacing.lg`, so density scales it like any
/// other spacing).
#[must_use]
pub fn icon_side(scales: &Scales, size: IconSize) -> f32 {
    match size {
        IconSize::Small => super::spacing(scales.spacing.md),
        IconSize::Large => super::spacing(scales.spacing.lg),
    }
}

/// An icon's state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconState {
    pub kind: IconKind,
    pub tone: IconTone,
    pub access: IconAccess,
}

fn node(state: &IconState) -> Node {
    let mut node = Node::new(Role::Image);
    match &state.access {
        IconAccess::Decorative => node.set_hidden(),
        IconAccess::Named(name) => {
            node.set_label(name.clone());
            if state.tone == IconTone::Disabled {
                node.set_disabled();
            }
        }
        IconAccess::Button { label, toggled } => {
            node = Node::new(Role::Button);
            node.set_label(label.clone());
            if let Some(on) = toggled {
                node.set_toggled(if *on {
                    accesskit::Toggled::True
                } else {
                    accesskit::Toggled::False
                });
            }
            node.add_action(accesskit::Action::Click);
        }
    }
    node
}

fn style(side: f32) -> Style {
    Style {
        size: Size {
            width: length(side),
            height: length(side),
        },
        flex_shrink: 0.0,
        ..Default::default()
    }
}

/// Adds an icon `kind`, `size` big, in `tone`, with `access`, as the last
/// child of `parent`.
///
/// # Errors
///
/// [`WidgetError::UnknownWidget`] if `parent` doesn't exist.
pub fn insert_icon(
    tree: &mut WidgetTree<WidgetKind>,
    parent: WidgetId,
    scales: &Scales,
    kind: IconKind,
    size: IconSize,
    tone: IconTone,
    access: IconAccess,
) -> Result<WidgetId, WidgetError> {
    let state = IconState { kind, tone, access };
    tree.insert(
        parent,
        style(icon_side(scales, size)),
        node(&state),
        WidgetKind::Icon(state),
    )
}

/// `id`'s icon state.
///
/// # Errors
///
/// [`WidgetError::UnknownWidget`] / [`WidgetError::WrongWidgetKind`].
pub fn icon_state(tree: &WidgetTree<WidgetKind>, id: WidgetId) -> Result<&IconState, WidgetError> {
    match tree.payload(id) {
        None => Err(WidgetError::UnknownWidget(id)),
        Some(WidgetKind::Icon(state)) => Ok(state),
        Some(_) => Err(WidgetError::WrongWidgetKind(id)),
    }
}

fn with_icon_mut(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    f: impl FnOnce(&mut IconState),
) -> Result<(), WidgetError> {
    let rebuilt = {
        let kind = tree.payload_mut(id).ok_or(WidgetError::UnknownWidget(id))?;
        let WidgetKind::Icon(state) = kind else {
            return Err(WidgetError::WrongWidgetKind(id));
        };
        f(state);
        node(state)
    };
    tree.set_accessibility(id, rebuilt)?;
    tree.mark_dirty(id)
}

/// Shows a different icon. Returns whether anything changed (an
/// unchanged icon touches nothing).
///
/// # Errors
///
/// [`WidgetError::UnknownWidget`] / [`WidgetError::WrongWidgetKind`].
pub fn set_icon_kind(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    kind: IconKind,
) -> Result<bool, WidgetError> {
    if icon_state(tree, id)?.kind == kind {
        return Ok(false);
    }
    with_icon_mut(tree, id, |state| state.kind = kind)?;
    Ok(true)
}

/// Changes the colour role. Returns whether anything changed.
///
/// # Errors
///
/// [`WidgetError::UnknownWidget`] / [`WidgetError::WrongWidgetKind`].
pub fn set_icon_tone(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    tone: IconTone,
) -> Result<bool, WidgetError> {
    if icon_state(tree, id)?.tone == tone {
        return Ok(false);
    }
    with_icon_mut(tree, id, |state| state.tone = tone)?;
    Ok(true)
}

/// Switches between decorative and named. Returns whether anything
/// changed.
///
/// # Errors
///
/// [`WidgetError::UnknownWidget`] / [`WidgetError::WrongWidgetKind`].
pub fn set_icon_access(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    access: IconAccess,
) -> Result<bool, WidgetError> {
    if icon_state(tree, id)?.access == access {
        return Ok(false);
    }
    with_icon_mut(tree, id, |state| state.access = access)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::{
        IconAccess, IconSize, IconTone, icon_side, icon_state, insert_icon, set_icon_access,
        set_icon_kind, set_icon_tone,
    };
    use crate::WidgetError;
    use crate::icons::IconKind;
    use crate::paint_widget;
    use crate::widgets::{WidgetKind, insert_button, insert_container, new_tree, test_scales};
    use accesskit::{Action, Role};
    use aurora_theme::{Palette, Theme, ThemeSet};
    use taffy::Style;

    fn dark_theme() -> Theme {
        let Ok(palette) =
            Palette::from_toml_str(include_str!("../../../../design/tokens/palette.toml"))
        else {
            unreachable!()
        };
        let mut themes = ThemeSet::new();
        if themes
            .register(include_str!("../../../../design/themes/dark.toml"))
            .is_err()
        {
            unreachable!()
        }
        match themes.resolve("Dark", &palette) {
            Ok(theme) => theme,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn laid_out(
        kind: IconKind,
        tone: IconTone,
    ) -> (crate::WidgetTree<WidgetKind>, crate::WidgetId) {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let id = match insert_icon(
            &mut tree,
            root,
            &scales,
            kind,
            IconSize::Small,
            tone,
            IconAccess::Decorative,
        ) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        tree.compute_layout(100.0, 100.0);
        (tree, id)
    }

    fn paints(
        tree: &crate::WidgetTree<WidgetKind>,
        id: crate::WidgetId,
        scale: f32,
    ) -> Vec<crate::Paint> {
        match paint_widget(tree, id, &dark_theme(), &test_scales(), scale) {
            Ok(paints) => paints,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    // Exact: a token colour is copied through, not computed.
    #[allow(clippy::float_cmp)]
    #[test]
    fn every_icon_paints_something_in_the_colour_its_tone_names() {
        let theme = dark_theme();
        let token = |tone: IconTone| match tone {
            IconTone::Default => theme.icon.default,
            IconTone::Secondary => theme.icon.secondary,
            IconTone::Disabled => theme.icon.disabled,
            IconTone::OnAccent => theme.icon.on_accent,
        };
        let mut seen = Vec::new();
        for tone in IconTone::ALL {
            let [r, g, b] = token(tone).to_srgb_f32();
            for kind in IconKind::ALL {
                let (tree, id) = laid_out(kind, tone);
                let paints = paints(&tree, id, 1.0);
                assert!(!paints.is_empty(), "{kind:?} paints nothing");
                for (mesh, color) in &paints {
                    assert!(!mesh.indices.is_empty(), "{kind:?}: an empty mesh");
                    assert_eq!(*color, [r, g, b, 1.0], "{kind:?} in {tone:?}");
                }
            }
            seen.push([r, g, b]);
        }
        for (i, a) in seen.iter().enumerate() {
            for b in seen.iter().skip(i + 1) {
                assert_ne!(a, b, "two tones share a colour in Dark");
            }
        }
    }

    #[test]
    fn paint_places_the_icon_in_its_layout_box_and_a_higher_scale_tessellates_curves_finer() {
        let (mut tree, root) = new_tree(Style {
            padding: taffy::Rect {
                left: taffy::style_helpers::length(10.0_f32),
                top: taffy::style_helpers::length(20.0_f32),
                right: taffy::style_helpers::length(0.0_f32),
                bottom: taffy::style_helpers::length(0.0_f32),
            },
            ..Style::default()
        });
        let scales = test_scales();
        let id = match insert_icon(
            &mut tree,
            root,
            &scales,
            IconKind::Eye,
            IconSize::Small,
            IconTone::Default,
            IconAccess::Decorative,
        ) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        tree.compute_layout(100.0, 100.0);
        let at1 = paints(&tree, id, 1.0);
        let at2 = paints(&tree, id, 2.0);
        let extent = |paints: &[crate::Paint]| {
            let (mut lo, mut hi) = ((f32::MAX, f32::MAX), (f32::MIN, f32::MIN));
            for (mesh, _) in paints {
                for v in &mesh.vertices {
                    lo = (lo.0.min(v.x), lo.1.min(v.y));
                    hi = (hi.0.max(v.x), hi.1.max(v.y));
                }
            }
            (lo, hi)
        };
        let (lo, hi) = extent(&at1);
        assert!(lo.0 >= 10.0 && lo.1 >= 20.0 && hi.0 <= 26.0 + 1e-3 && hi.1 <= 36.0 + 1e-3);
        assert!(hi.0 - lo.0 > 8.0, "the lens spans most of the box");
        // Meshes stay in logical px at any scale; only the tessellation
        // tolerance follows it.
        let (lo2, hi2) = extent(&at2);
        assert!((lo2.0 - lo.0).abs() < 0.2 && (hi2.0 - hi.0).abs() < 0.2);
        let vertices =
            |paints: &[crate::Paint]| paints.iter().map(|(m, _)| m.vertices.len()).sum::<usize>();
        assert!(
            vertices(&at2) > vertices(&at1),
            "scale 2 must tessellate the same curves into more segments ({} vs {})",
            vertices(&at2),
            vertices(&at1)
        );
    }

    #[test]
    fn an_icon_an_ancestor_clips_is_not_drawn_rather_than_spilling() {
        let scales = test_scales();
        let (mut tree, root) = new_tree(Style {
            size: taffy::Size {
                width: taffy::style_helpers::length(100.0_f32),
                height: taffy::style_helpers::length(100.0_f32),
            },
            ..Style::default()
        });
        let clip = match insert_container(
            &mut tree,
            root,
            Style {
                overflow: taffy::Point {
                    x: taffy::Overflow::Hidden,
                    y: taffy::Overflow::Hidden,
                },
                size: taffy::Size {
                    width: taffy::style_helpers::length(40.0_f32),
                    height: taffy::style_helpers::length(10.0_f32),
                },
                ..Style::default()
            },
        ) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let id = match insert_icon(
            &mut tree,
            clip,
            &scales,
            IconKind::Lock,
            IconSize::Small,
            IconTone::Default,
            IconAccess::Decorative,
        ) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        tree.compute_layout(100.0, 100.0);
        let drawn = paints(&tree, id, 1.0);
        assert!(!drawn.is_empty(), "the visible 10 px of the icon is drawn");
        let Some(clip_bounds) = tree.bounds(clip) else {
            unreachable!("laid out");
        };
        let limit = (clip_bounds.y + i64::from(clip_bounds.height)) as f32;
        let mut full_height = 0.0_f32;
        for (mesh, _) in &drawn {
            for vertex in &mesh.vertices {
                assert!(
                    vertex.y <= limit + 1e-3,
                    "no vertex spills below the clip: {} > {limit}",
                    vertex.y
                );
                full_height = full_height.max(vertex.y);
            }
        }
        assert!(
            full_height > limit - 1.5,
            "the cut reaches the clip edge: {full_height} vs {limit}"
        );
    }

    /// 0.195.0: a button icon is a `Role::Button` with `Click` only (never
    /// a tab stop), and `handle_action` reports its click as `Activated`.
    #[test]
    fn a_button_icon_is_a_click_only_toggle_button() {
        let (mut tree, id) = one(IconAccess::Button {
            label: "Hide Post 1".into(),
            toggled: Some(true),
        });
        let Some(node) = tree.accessibility(id) else {
            unreachable!("exists");
        };
        assert_eq!(node.role(), Role::Button);
        assert_eq!(node.label(), Some("Hide Post 1"));
        assert_eq!(node.toggled(), Some(accesskit::Toggled::True));
        assert!(node.supports_action(Action::Click));
        assert!(!node.supports_action(Action::Focus), "not a tab stop");
        let mut focus = crate::FocusManager::new();
        let outcome = crate::handle_action(
            &mut tree,
            &mut focus,
            &accesskit::ActionRequest {
                action: Action::Click,
                target_tree: crate::ACCESSIBILITY_TREE_ID,
                target_node: id,
                data: None,
            },
        );
        assert!(matches!(outcome, Ok(crate::ActionOutcome::Activated(clicked)) if clicked == id));
        // Clicking never flips the state itself.
        assert_eq!(
            tree.accessibility(id).and_then(accesskit::Node::toggled),
            Some(accesskit::Toggled::True)
        );
    }

    fn one(access: IconAccess) -> (crate::WidgetTree<WidgetKind>, crate::WidgetId) {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let id = match insert_icon(
            &mut tree,
            root,
            &scales,
            IconKind::Eye,
            IconSize::Small,
            IconTone::Default,
            access,
        ) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        (tree, id)
    }

    #[test]
    fn a_decorative_icon_is_a_hidden_image_with_no_name_and_no_actions() {
        let (tree, id) = one(IconAccess::Decorative);
        let Some(node) = tree.accessibility(id) else {
            unreachable!("just inserted");
        };
        assert_eq!(node.role(), Role::Image);
        assert!(node.is_hidden(), "a decorative icon must be hidden from AT");
        assert_eq!(node.label(), None);
        assert!(!node.supports_action(Action::Focus));
        assert!(!node.supports_action(Action::Click));
    }

    #[test]
    fn a_named_icon_is_a_visible_image_with_its_name_and_no_actions() {
        let (tree, id) = one(IconAccess::Named("Visible".into()));
        let Some(node) = tree.accessibility(id) else {
            unreachable!("just inserted");
        };
        assert_eq!(node.role(), Role::Image);
        assert!(!node.is_hidden());
        assert_eq!(node.label(), Some("Visible"));
        assert!(!node.is_disabled());
        assert!(!node.supports_action(Action::Focus));
        assert!(!node.supports_action(Action::Click));
    }

    #[test]
    fn switching_access_rebuilds_the_node_and_is_a_no_op_when_unchanged() {
        let (mut tree, id) = one(IconAccess::Decorative);
        assert!(matches!(
            set_icon_access(&mut tree, id, IconAccess::Decorative),
            Ok(false)
        ));
        assert!(matches!(
            set_icon_access(&mut tree, id, IconAccess::Named("Hidden".into())),
            Ok(true)
        ));
        assert_eq!(
            tree.accessibility(id).and_then(accesskit::Node::label),
            Some("Hidden")
        );
        assert!(
            !tree
                .accessibility(id)
                .is_some_and(accesskit::Node::is_hidden)
        );
        assert!(matches!(
            set_icon_access(&mut tree, id, IconAccess::Decorative),
            Ok(true)
        ));
        assert!(
            tree.accessibility(id)
                .is_some_and(accesskit::Node::is_hidden)
        );
    }

    #[test]
    fn a_named_disabled_icon_is_announced_as_disabled_and_a_decorative_one_is_not_named() {
        let (mut tree, id) = one(IconAccess::Named("Lock".into()));
        assert!(matches!(
            set_icon_tone(&mut tree, id, IconTone::Disabled),
            Ok(true)
        ));
        assert!(
            tree.accessibility(id)
                .is_some_and(accesskit::Node::is_disabled)
        );
        assert!(matches!(
            set_icon_tone(&mut tree, id, IconTone::Disabled),
            Ok(false)
        ));
        let (mut tree, id) = one(IconAccess::Decorative);
        assert!(matches!(
            set_icon_tone(&mut tree, id, IconTone::Secondary),
            Ok(true)
        ));
        assert!(
            tree.accessibility(id)
                .is_some_and(accesskit::Node::is_hidden)
        );
        assert_eq!(
            tree.accessibility(id).and_then(accesskit::Node::label),
            None
        );
    }

    #[test]
    fn an_icon_is_a_square_of_the_token_side_and_does_not_shrink() {
        let scales = test_scales();
        assert!((icon_side(&scales, IconSize::Small) - 16.0).abs() < 1e-6);
        assert!((icon_side(&scales, IconSize::Large) - 24.0).abs() < 1e-6);
        let (mut tree, root) = new_tree(Style {
            size: taffy::Size {
                width: taffy::style_helpers::length(8.0_f32),
                height: taffy::style_helpers::length(8.0_f32),
            },
            ..Style::default()
        });
        let id = match insert_icon(
            &mut tree,
            root,
            &scales,
            IconKind::Lock,
            IconSize::Large,
            IconTone::Default,
            IconAccess::Decorative,
        ) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        tree.compute_layout(8.0, 8.0);
        let Some(b) = tree.bounds(id) else {
            unreachable!("laid out");
        };
        assert_eq!((b.width, b.height), (24, 24), "an icon never shrinks");
    }

    #[test]
    fn setters_update_state_mark_dirty_and_reject_other_widgets() {
        let (mut tree, id) = one(IconAccess::Decorative);
        tree.compute_layout(100.0, 100.0);
        let _ = tree.take_damage();
        assert!(matches!(
            set_icon_kind(&mut tree, id, IconKind::Eye),
            Ok(false)
        ));
        assert!(tree.take_damage().is_none());
        assert!(matches!(
            set_icon_kind(&mut tree, id, IconKind::EyeOff),
            Ok(true)
        ));
        assert_eq!(
            icon_state(&tree, id).map(|s| s.kind).ok(),
            Some(IconKind::EyeOff)
        );
        assert!(tree.take_damage().is_some(), "a changed icon repaints");

        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let button = match insert_button(&mut tree, root, &scales, "b") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        assert!(matches!(
            set_icon_kind(&mut tree, button, IconKind::Eye),
            Err(WidgetError::WrongWidgetKind(got)) if got == button
        ));
        assert!(tree.remove(button).is_ok());
        assert!(matches!(
            set_icon_tone(&mut tree, button, IconTone::Default),
            Err(WidgetError::UnknownWidget(got)) if got == button
        ));
    }
}
