//! Input routing (pointer hit-testing) and focus management/keyboard
//! navigation. PLAN.md M1.7's third deliverable.
//!
//! Deliberately platform-agnostic: this module works in terms of a
//! document-space point and `Tab`/`Shift+Tab` *steps*, not
//! `winit::WindowEvent`s — translating real platform input into these
//! primitives is `aurora-app`'s job (still unstarted), the same seam
//! that keeps this crate's own widget API free of `wgpu`/`winit`
//! assumptions (see ADR 0001's escape-hatch note).

use accesskit::Action;
use aurora_core::Rect;

use crate::error::WidgetError;
use crate::tree::{WidgetId, WidgetTree};

/// Finds the topmost widget under document-space point `(x, y)`, if any.
/// "Topmost" means the deepest descendant whose bounds actually contain
/// the point, checking a node's children in reverse (last child first) —
/// [`WidgetTree`]'s own children are in paint order, first to last, so
/// the last child painted is the one on top when siblings overlap.
/// `None` only when `(x, y)` falls outside the tree's own root bounds
/// (e.g. layout hasn't run yet, so the root is still zero-sized, or the
/// point is genuinely off-window).
///
/// Popover roots ([`crate::PaintLayer::Popover`]) are tried first,
/// topmost first, each gated only by its own bounds — this delegates to
/// the same single traversal [`WidgetTree::hit_test`] uses, with this
/// function's own `f64` containment predicate, so the two can never
/// disagree about layering. See that method for the full rule.
#[must_use]
pub fn hit_test<W>(tree: &WidgetTree<W>, x: f64, y: f64) -> Option<WidgetId> {
    tree.hit_test_by(|bounds| contains(bounds, x, y))
}

fn contains(bounds: Rect, x: f64, y: f64) -> bool {
    #[allow(clippy::cast_precision_loss)]
    let (left, top, right, bottom) = (
        bounds.x as f64,
        bounds.y as f64,
        bounds.right() as f64,
        bounds.bottom() as f64,
    );
    x >= left && x < right && y >= top && y < bottom
}

/// Which widget has keyboard focus, and `Tab`/`Shift+Tab` navigation
/// between focusable widgets. "Focusable" reuses `accesskit`'s own
/// vocabulary (a widget's `Node::supports_action(Action::Focus)`) rather
/// than a second, parallel flag — the same "no second id space"
/// discipline [`WidgetId`] itself already established by literally being
/// `accesskit::NodeId`.
///
/// **Doesn't track tree mutations**: if the currently focused widget is
/// removed from a [`WidgetTree`] via a direct `remove` call, this type
/// has no way to know — it holds no reference into the tree, the same
/// "mixing direct calls with a higher-level manager can leave a stale
/// reference" limitation `aurora_doc::History` already documents for
/// itself. Call [`Self::validate`] after removing widgets if stale focus
/// matters to the caller.
///
/// **Focus modality** (CSS `:focus-visible`): alongside *which* widget is
/// focused, this type tracks whether its focus ring should be *shown*,
/// driven by the [`FocusOrigin`] of the input that moved focus.
/// Keyboard navigation and assistive-technology focus requests show it;
/// a pointer click hides it (a mouse user already knows where they
/// clicked); a programmatic move keeps whatever the last real input
/// decided — the same heuristic browsers use, and what lets every
/// existing [`Self::focus`] caller stay source-compatible. A fresh
/// manager starts *visible* (Chrome's autofocus rule): the failure mode
/// of guessing wrong is a ring a mouse user did not need, never a
/// keyboard or screen-magnifier user losing track of focus.
#[derive(Debug)]
pub struct FocusManager {
    focused: Option<WidgetId>,
    /// Whether the focus ring is shown — see this type's own "focus
    /// modality" paragraph. Meaningful only while `focused` is `Some`;
    /// [`Self::focus_visible`] is the combined predicate.
    visible: bool,
}

/// What moved keyboard focus — the input *modality* [`FocusManager`]
/// uses to decide whether the focus ring is visible (CSS
/// `:focus-visible`). See [`FocusManager`]'s own "focus modality"
/// paragraph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusOrigin {
    /// `Tab`/`Shift+Tab` or any other keyboard navigation — shows the
    /// ring.
    Keyboard,
    /// A pointer press (mouse, pen, touch) — hides the ring.
    Pointer,
    /// An assistive technology's `accesskit::Action::Focus` request —
    /// shows the ring (a screen-magnifier user follows it).
    Accessibility,
    /// Application code moving focus on its own (a dialog focusing its
    /// first button, a rebuilt row being refocused) — keeps the current
    /// modality rather than deciding one.
    Programmatic,
}

impl FocusManager {
    #[must_use]
    pub fn new() -> Self {
        Self {
            focused: None,
            visible: true,
        }
    }

    #[must_use]
    pub fn focused(&self) -> Option<WidgetId> {
        self.focused
    }

    /// Whether a focus ring should currently be painted: something is
    /// focused *and* the last real input modality was keyboard or
    /// accessibility (see this type's own "focus modality" paragraph).
    #[must_use]
    pub fn focus_visible(&self) -> bool {
        self.focused.is_some() && self.visible
    }

    /// Moves focus to `id` programmatically — [`Self::focus_with`] with
    /// [`FocusOrigin::Programmatic`], so the ring's visibility is left as
    /// the last real input decided it.
    ///
    /// # Errors
    ///
    /// Same as [`Self::focus_with`].
    pub fn focus<W>(&mut self, tree: &mut WidgetTree<W>, id: WidgetId) -> Result<(), WidgetError> {
        self.focus_with(tree, id, FocusOrigin::Programmatic)
    }

    /// Moves focus to `id`, recording `origin` as the input modality.
    /// Marks both the previously- and newly-focused widget dirty,
    /// including each one's focus-ring overhang (a focus ring is visual
    /// state, the same as bounds or accessibility content changing), and
    /// moves the focused widget's damage outset
    /// (`FOCUS_RING_MAX_OUTSET`, 5 px) from the old widget to
    /// the new one. Re-focusing the already-focused widget is not a
    /// no-op when it changes the modality: clicking the widget `Tab`
    /// just reached hides its ring, and dirties it so the ring is erased.
    ///
    /// # Errors
    ///
    /// Returns [`WidgetError::UnknownWidget`] if `id` doesn't exist, or
    /// [`WidgetError::NotFocusable`] if it exists but doesn't support the
    /// `accesskit::Action::Focus` action. Nothing changes when this
    /// happens.
    pub fn focus_with<W>(
        &mut self,
        tree: &mut WidgetTree<W>,
        id: WidgetId,
        origin: FocusOrigin,
    ) -> Result<(), WidgetError> {
        let supports_focus = tree
            .accessibility(id)
            .ok_or(WidgetError::UnknownWidget(id))?
            .supports_action(Action::Focus);
        if !supports_focus {
            return Err(WidgetError::NotFocusable(id));
        }
        self.set_focus(tree, Some(id), origin);
        Ok(())
    }

    /// Records an input event's modality without moving focus — e.g. a
    /// key press that is not `Tab` (arrow keys on a focused slider) shows
    /// the ring again after a click hid it, the way `:focus-visible`
    /// does. Dirties the focused widget (ring overhang included) only if
    /// the visibility actually changed. [`FocusOrigin::Programmatic`] is
    /// a no-op.
    pub fn note_input<W>(&mut self, tree: &mut WidgetTree<W>, origin: FocusOrigin) {
        let visible = self.visible_after(origin);
        if visible == self.visible {
            return;
        }
        self.visible = visible;
        if let Some(id) = self.focused {
            // Already gone is fine (see this type's own doc comment).
            let _ = tree.mark_dirty(id);
        }
    }

    /// Clears focus (nothing focused). Leaves the modality alone.
    pub fn blur<W>(&mut self, tree: &mut WidgetTree<W>) {
        self.set_focus(tree, None, FocusOrigin::Programmatic);
    }

    /// Moves focus to the next focusable widget in tree order (pre-order,
    /// depth-first — the same order a screen reader's linear navigation,
    /// or a browser's default no-`tabindex` `Tab` order, would use),
    /// wrapping around after the last one. `None` if the tree has no
    /// focusable widgets at all. Records [`FocusOrigin::Keyboard`].
    pub fn focus_next<W>(&mut self, tree: &mut WidgetTree<W>) -> Option<WidgetId> {
        self.step(tree, true)
    }

    /// Same as [`Self::focus_next`], but backwards (`Shift+Tab`).
    pub fn focus_previous<W>(&mut self, tree: &mut WidgetTree<W>) -> Option<WidgetId> {
        self.step(tree, false)
    }

    /// Combines [`hit_test`] with focus: hit-tests `(x, y)`, then walks
    /// up from the hit widget to the nearest ancestor (inclusive) that's
    /// actually focusable, and focuses it — the same "a click bubbles to
    /// the nearest focusable ancestor" behaviour every mainstream UI
    /// toolkit uses (clicking a button's icon glyph focuses the button,
    /// not nothing). Returns the widget that ended up focused, or `None`
    /// if nothing at `(x, y)`, or any of its ancestors, is focusable.
    /// Records [`FocusOrigin::Pointer`] whenever it focuses something —
    /// including the widget that was already focused, which hides its
    /// ring.
    pub fn focus_at<W>(&mut self, tree: &mut WidgetTree<W>, x: f64, y: f64) -> Option<WidgetId> {
        let hit = hit_test(tree, x, y)?;
        let mut current = Some(hit);
        while let Some(id) = current {
            let is_focusable = tree
                .accessibility(id)
                .is_some_and(|node| node.supports_action(Action::Focus));
            if is_focusable {
                self.set_focus(tree, Some(id), FocusOrigin::Pointer);
                return Some(id);
            }
            current = tree.parent(id);
        }
        None
    }

    /// Clears focus if the currently focused widget no longer exists in
    /// `tree` (see this type's own doc comment). Returns whether focus
    /// was actually cleared.
    ///
    /// This is the half of the stale-focus invariant this type owns.
    /// The other half lives in [`WidgetTree::accessibility_update`],
    /// which falls back to the tree's root if the id it's handed is
    /// gone — that guard is what keeps a stale `self.focused` from ever
    /// reaching `accesskit_consumer` as an invalid `TreeUpdate.focus`
    /// and panicking there, but it doesn't repair `self.focused` itself.
    /// A caller that removes widgets (a tree collapse, a dialog close,
    /// a palette close) should still call `validate` afterward so this
    /// type's own idea of focus doesn't go on pointing at a dead id even
    /// though the next accessibility push would no longer crash on it.
    pub fn validate<W>(&mut self, tree: &WidgetTree<W>) -> bool {
        if let Some(id) = self.focused
            && !tree.contains(id)
        {
            self.focused = None;
            return true;
        }
        false
    }

    /// The ring visibility `origin` leads to — see [`FocusOrigin`].
    fn visible_after(&self, origin: FocusOrigin) -> bool {
        match origin {
            FocusOrigin::Keyboard | FocusOrigin::Accessibility => true,
            FocusOrigin::Pointer => false,
            FocusOrigin::Programmatic => self.visible,
        }
    }

    fn set_focus<W>(
        &mut self,
        tree: &mut WidgetTree<W>,
        new: Option<WidgetId>,
        origin: FocusOrigin,
    ) {
        let visible = self.visible_after(origin);
        if self.focused == new {
            // Same widget: only a modality change has anything to
            // repaint (the ring appearing or disappearing).
            if visible != self.visible {
                self.visible = visible;
                if let Some(id) = new {
                    let _ = tree.mark_dirty(id);
                }
            }
            return;
        }
        if let Some(old) = self.focused {
            // Already gone is fine (see this type's own doc comment) --
            // nothing left to mark dirty. Dirtied *before* its outset is
            // reset, so the damage still covers its ring's overhang.
            let _ = tree.mark_dirty(old);
        }
        // Every outset back to `0` -- `old`'s, and any a dropped or
        // replaced manager left behind without a blur (review F6).
        tree.clear_damage_outsets();
        if let Some(new_id) = new {
            let _ = tree.set_damage_outset(new_id, crate::paint::FOCUS_RING_MAX_OUTSET);
            let _ = tree.mark_dirty(new_id);
        }
        self.focused = new;
        self.visible = visible;
    }

    fn step<W>(&mut self, tree: &mut WidgetTree<W>, forward: bool) -> Option<WidgetId> {
        let order = focus_order(tree);
        if order.is_empty() {
            self.set_focus(tree, None, FocusOrigin::Keyboard);
            return None;
        }
        let len = order.len();
        let next_index = match self
            .focused
            .and_then(|id| order.iter().position(|&o| o == id))
        {
            Some(index) if forward => (index + 1) % len,
            Some(index) => (index + len - 1) % len,
            None if forward => 0,
            None => len - 1,
        };
        let Some(&next) = order.get(next_index) else {
            unreachable!("next_index is always < len by construction");
        };
        self.set_focus(tree, Some(next), FocusOrigin::Keyboard);
        Some(next)
    }
}

impl Default for FocusManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Every focusable widget in `tree`, pre-order (depth-first, parent
/// before children) — [`FocusManager`]'s own `Tab` order.
fn focus_order<W>(tree: &WidgetTree<W>) -> Vec<WidgetId> {
    let mut order = Vec::new();
    collect_focusable(tree, tree.root(), &mut order);
    order
}

fn collect_focusable<W>(tree: &WidgetTree<W>, id: WidgetId, out: &mut Vec<WidgetId>) {
    let is_focusable = tree
        .accessibility(id)
        .is_some_and(|node| node.supports_action(Action::Focus));
    if is_focusable {
        out.push(id);
    }
    if let Some(children) = tree.children(id) {
        for &child in children {
            collect_focusable(tree, child, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{FocusManager, hit_test};
    use crate::WidgetError;
    use crate::tree::WidgetTree;
    use accesskit::{Action, Node, Role};
    use taffy::style_helpers::length;
    use taffy::{Size, Style};

    fn label(text: &str) -> Node {
        let mut node = Node::new(Role::Label);
        node.set_label(text);
        node
    }

    fn focusable(text: &str) -> Node {
        let mut node = Node::new(Role::Button);
        node.set_label(text);
        node.add_action(Action::Focus);
        node
    }

    fn sized(width: f32, height: f32) -> Style {
        Style {
            size: Size {
                width: length(width),
                height: length(height),
            },
            ..Default::default()
        }
    }

    // -- hit_test --

    #[test]
    fn hit_test_finds_the_root_when_nothing_else_matches() {
        let (mut tree, root) = WidgetTree::new(label("root"), sized(100.0, 100.0), "root");
        tree.compute_layout(100.0, 100.0);
        assert_eq!(hit_test(&tree, 50.0, 50.0), Some(root));
    }

    #[test]
    fn hit_test_returns_none_outside_the_root() {
        let (mut tree, _root) = WidgetTree::new(label("root"), sized(100.0, 100.0), "root");
        tree.compute_layout(100.0, 100.0);
        assert_eq!(hit_test(&tree, 500.0, 500.0), None);
    }

    #[test]
    fn hit_test_finds_the_deepest_matching_child() {
        let root_style = Style {
            flex_direction: taffy::FlexDirection::Row,
            ..Default::default()
        };
        let (mut tree, root) = WidgetTree::new(label("root"), root_style, "root");
        let a = match tree.insert(root, sized(40.0, 40.0), label("a"), "a") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let b = match tree.insert(root, sized(40.0, 40.0), label("b"), "b") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        tree.compute_layout(100.0, 100.0);

        assert_eq!(hit_test(&tree, 10.0, 10.0), Some(a));
        assert_eq!(hit_test(&tree, 50.0, 10.0), Some(b));
    }

    #[test]
    fn hit_test_prefers_the_last_child_when_siblings_overlap() {
        // Two children explicitly placed on top of each other via
        // set_bounds (the layout engine's own flexbox math never
        // overlaps siblings, so this exercises the escape hatch on
        // purpose).
        let (mut tree, root) = WidgetTree::new(label("root"), Style::default(), "root");
        let a = match tree.insert(root, Style::default(), label("a"), "a") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let b = match tree.insert(root, Style::default(), label("b"), "b") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let overlap = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 50,
            height: 50,
        };
        if let Err(err) = tree.set_bounds(root, overlap) {
            unreachable!("{err:?}");
        }
        if let Err(err) = tree.set_bounds(a, overlap) {
            unreachable!("{err:?}");
        }
        if let Err(err) = tree.set_bounds(b, overlap) {
            unreachable!("{err:?}");
        }

        assert_eq!(
            hit_test(&tree, 10.0, 10.0),
            Some(b),
            "b was inserted after a, so it paints on top"
        );
    }

    // -- FocusManager --

    #[test]
    fn fresh_focus_manager_has_nothing_focused() {
        let manager = FocusManager::new();
        assert_eq!(manager.focused(), None);
    }

    #[test]
    fn focus_moves_to_a_focusable_widget() {
        let (mut tree, root) = WidgetTree::new(label("root"), Style::default(), "root");
        let a = match tree.insert(root, Style::default(), focusable("a"), "a") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let mut manager = FocusManager::new();
        if let Err(err) = manager.focus(&mut tree, a) {
            unreachable!("{err:?}");
        }
        assert_eq!(manager.focused(), Some(a));
    }

    #[test]
    fn focus_rejects_a_non_focusable_widget() {
        let (mut tree, root) = WidgetTree::new(label("root"), Style::default(), "root");
        let a = match tree.insert(root, Style::default(), label("a"), "a") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let mut manager = FocusManager::new();
        match manager.focus(&mut tree, a) {
            Err(WidgetError::NotFocusable(id)) => assert_eq!(id, a),
            other => unreachable!("expected NotFocusable, got {other:?}"),
        }
        assert_eq!(manager.focused(), None);
    }

    #[test]
    fn focus_rejects_an_unknown_widget() {
        let (mut tree, _root) = WidgetTree::new(label("root"), Style::default(), "root");
        let bogus = accesskit::NodeId(999);
        let mut manager = FocusManager::new();
        match manager.focus(&mut tree, bogus) {
            Err(WidgetError::UnknownWidget(id)) => assert_eq!(id, bogus),
            other => unreachable!("expected UnknownWidget, got {other:?}"),
        }
    }

    #[test]
    fn blur_clears_focus() {
        let (mut tree, root) = WidgetTree::new(label("root"), Style::default(), "root");
        let a = match tree.insert(root, Style::default(), focusable("a"), "a") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let mut manager = FocusManager::new();
        if let Err(err) = manager.focus(&mut tree, a) {
            unreachable!("{err:?}");
        }
        manager.blur(&mut tree);
        assert_eq!(manager.focused(), None);
    }

    #[test]
    fn focus_and_blur_mark_the_affected_widgets_dirty() {
        let (mut tree, root) = WidgetTree::new(label("root"), Style::default(), "root");
        let a = match tree.insert(root, Style::default(), focusable("a"), "a") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        tree.take_damage();
        let mut manager = FocusManager::new();
        if let Err(err) = manager.focus(&mut tree, a) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.is_dirty(a), Some(true));

        tree.take_damage();
        manager.blur(&mut tree);
        assert_eq!(tree.is_dirty(a), Some(true));
    }

    // -- focus modality (focus-visible) and ring damage --

    use super::FocusOrigin;
    use crate::paint::FOCUS_RING_MAX_OUTSET;
    use aurora_core::Rect;

    /// A root holding two focusable 20x20 widgets placed side by side,
    /// with a gap wider than twice the ring outset.
    fn two_placed() -> (WidgetTree<&'static str>, WidgetId, WidgetId) {
        let (mut tree, root) = WidgetTree::new(label("root"), Style::default(), "root");
        let a = match tree.insert(root, Style::default(), focusable("a"), "a") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let b = match tree.insert(root, Style::default(), focusable("b"), "b") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        for (id, x) in [(root, 0), (a, 20), (b, 60)] {
            let size = if id == root { 100 } else { 20 };
            let rect = Rect {
                x,
                y: 20,
                width: size,
                height: size,
            };
            if let Err(err) = tree.set_bounds(id, rect) {
                unreachable!("{err:?}");
            }
        }
        tree.take_damage();
        (tree, a, b)
    }

    use crate::tree::WidgetId;

    fn grown(rect: Rect, by: u32) -> Rect {
        Rect {
            x: rect.x - i64::from(by),
            y: rect.y - i64::from(by),
            width: rect.width + 2 * by,
            height: rect.height + 2 * by,
        }
    }

    fn bounds_of<W>(tree: &WidgetTree<W>, id: WidgetId) -> Rect {
        match tree.bounds(id) {
            Some(rect) => rect,
            None => unreachable!("{id:?} exists"),
        }
    }

    #[test]
    fn a_fresh_manager_starts_with_the_ring_visible_once_something_is_focused() {
        let (mut tree, a, _) = two_placed();
        let mut manager = FocusManager::new();
        assert!(!manager.focus_visible(), "nothing focused yet");
        if let Err(err) = manager.focus(&mut tree, a) {
            unreachable!("{err:?}");
        }
        assert!(manager.focus_visible());
    }

    #[test]
    fn tab_shows_the_ring_and_a_click_hides_it() {
        let (mut tree, a, b) = two_placed();
        let mut manager = FocusManager::new();
        manager.note_input(&mut tree, FocusOrigin::Pointer);
        assert_eq!(manager.focus_next(&mut tree), Some(a));
        assert!(manager.focus_visible(), "Tab is keyboard modality");
        assert_eq!(manager.focus_at(&mut tree, 65.0, 25.0), Some(b));
        assert!(!manager.focus_visible(), "a click is pointer modality");
        assert_eq!(manager.focus_previous(&mut tree), Some(a));
        assert!(manager.focus_visible());
    }

    #[test]
    fn an_accessibility_focus_shows_the_ring_even_after_a_click() {
        let (mut tree, a, b) = two_placed();
        let mut manager = FocusManager::new();
        assert_eq!(manager.focus_at(&mut tree, 25.0, 25.0), Some(a));
        assert!(!manager.focus_visible());
        if let Err(err) = manager.focus_with(&mut tree, b, FocusOrigin::Accessibility) {
            unreachable!("{err:?}");
        }
        assert!(manager.focus_visible());
    }

    #[test]
    fn a_programmatic_focus_keeps_whatever_the_last_real_input_decided() {
        let (mut tree, a, b) = two_placed();
        let mut manager = FocusManager::new();
        assert_eq!(manager.focus_at(&mut tree, 25.0, 25.0), Some(a));
        if let Err(err) = manager.focus(&mut tree, b) {
            unreachable!("{err:?}");
        }
        assert!(
            !manager.focus_visible(),
            "pointer, then programmatic: hidden"
        );

        assert_eq!(manager.focus_next(&mut tree), Some(a));
        if let Err(err) = manager.focus(&mut tree, b) {
            unreachable!("{err:?}");
        }
        assert!(
            manager.focus_visible(),
            "keyboard, then programmatic: shown"
        );
        manager.blur(&mut tree);
        assert!(!manager.focus_visible(), "nothing focused");
        if let Err(err) = manager.focus(&mut tree, a) {
            unreachable!("{err:?}");
        }
        assert!(manager.focus_visible(), "blur leaves the modality alone");
    }

    #[test]
    fn clicking_the_widget_tab_reached_hides_its_ring_and_repaints_it() {
        let (mut tree, a, _) = two_placed();
        let mut manager = FocusManager::new();
        assert_eq!(manager.focus_next(&mut tree), Some(a));
        tree.take_damage();
        assert_eq!(manager.focus_at(&mut tree, 25.0, 25.0), Some(a));
        assert!(!manager.focus_visible());
        assert_eq!(
            tree.take_damage(),
            Some(grown(bounds_of(&tree, a), FOCUS_RING_MAX_OUTSET)),
            "the ring's pixels must be erased, overhang included"
        );
        // And the same click again changes nothing.
        assert_eq!(manager.focus_at(&mut tree, 25.0, 25.0), Some(a));
        assert_eq!(tree.take_damage(), None);
    }

    #[test]
    fn note_input_flips_the_modality_in_place_and_dirties_only_on_a_change() {
        let (mut tree, a, _) = two_placed();
        let mut manager = FocusManager::new();
        assert_eq!(manager.focus_at(&mut tree, 25.0, 25.0), Some(a));
        tree.take_damage();
        manager.note_input(&mut tree, FocusOrigin::Keyboard);
        assert!(manager.focus_visible());
        assert_eq!(
            tree.take_damage(),
            Some(grown(bounds_of(&tree, a), FOCUS_RING_MAX_OUTSET))
        );
        manager.note_input(&mut tree, FocusOrigin::Keyboard);
        manager.note_input(&mut tree, FocusOrigin::Programmatic);
        assert_eq!(tree.take_damage(), None, "no change, no damage");
        assert!(manager.focus_visible());
        manager.note_input(&mut tree, FocusOrigin::Pointer);
        assert!(!manager.focus_visible());
        assert!(tree.take_damage().is_some());
    }

    #[test]
    fn moving_focus_damages_both_rings_and_moves_the_outset() {
        let (mut tree, a, b) = two_placed();
        let mut manager = FocusManager::new();
        if let Err(err) = manager.focus(&mut tree, a) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.damage_outset(a), Some(FOCUS_RING_MAX_OUTSET));
        assert_eq!(tree.damage_outset(b), Some(0));
        tree.take_damage();
        if let Err(err) = manager.focus(&mut tree, b) {
            unreachable!("{err:?}");
        }
        let old_ring = grown(bounds_of(&tree, a), FOCUS_RING_MAX_OUTSET);
        let new_ring = grown(bounds_of(&tree, b), FOCUS_RING_MAX_OUTSET);
        assert_eq!(tree.take_damage(), Some(old_ring.union(&new_ring)));
        assert_eq!(tree.damage_outset(a), Some(0));
        assert_eq!(tree.damage_outset(b), Some(FOCUS_RING_MAX_OUTSET));

        // The widget focus left reports exact bounds again ...
        if let Err(err) = tree.mark_dirty(a) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.take_damage(), Some(bounds_of(&tree, a)));
        // ... and the focused one keeps covering its ring on every repaint.
        if let Err(err) = tree.mark_dirty(b) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.take_damage(), Some(new_ring));
        manager.blur(&mut tree);
        assert_eq!(tree.take_damage(), Some(new_ring));
        assert_eq!(tree.damage_outset(b), Some(0));
    }

    #[test]
    fn focus_next_cycles_through_every_focusable_widget_in_tree_order() {
        let (mut tree, root) = WidgetTree::new(label("root"), Style::default(), "root");
        let a = match tree.insert(root, Style::default(), focusable("a"), "a") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        // Not focusable -- must be skipped.
        if let Err(err) = tree.insert(root, Style::default(), label("skip"), "skip") {
            unreachable!("{err:?}");
        }
        let b = match tree.insert(root, Style::default(), focusable("b"), "b") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };

        let mut manager = FocusManager::new();
        assert_eq!(manager.focus_next(&mut tree), Some(a));
        assert_eq!(manager.focus_next(&mut tree), Some(b));
        assert_eq!(
            manager.focus_next(&mut tree),
            Some(a),
            "must wrap back to the first focusable widget"
        );
    }

    #[test]
    fn focus_previous_cycles_backwards() {
        let (mut tree, root) = WidgetTree::new(label("root"), Style::default(), "root");
        let a = match tree.insert(root, Style::default(), focusable("a"), "a") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let b = match tree.insert(root, Style::default(), focusable("b"), "b") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };

        let mut manager = FocusManager::new();
        assert_eq!(
            manager.focus_previous(&mut tree),
            Some(b),
            "starting with nothing focused, Shift+Tab must land on the last one"
        );
        assert_eq!(manager.focus_previous(&mut tree), Some(a));
        assert_eq!(
            manager.focus_previous(&mut tree),
            Some(b),
            "must wrap back to the last focusable widget"
        );
    }

    #[test]
    fn focus_next_returns_none_when_nothing_is_focusable() {
        let (mut tree, _root) = WidgetTree::new(label("root"), Style::default(), "root");
        let mut manager = FocusManager::new();
        assert_eq!(manager.focus_next(&mut tree), None);
        assert_eq!(manager.focused(), None);
    }

    #[test]
    fn focus_at_focuses_the_widget_hit_by_a_point() {
        let (mut tree, root) = WidgetTree::new(label("root"), Style::default(), "root");
        let a = match tree.insert(root, sized(50.0, 50.0), focusable("a"), "a") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        tree.compute_layout(100.0, 100.0);

        let mut manager = FocusManager::new();
        assert_eq!(manager.focus_at(&mut tree, 10.0, 10.0), Some(a));
        assert_eq!(manager.focused(), Some(a));
    }

    #[test]
    fn focus_at_bubbles_to_the_nearest_focusable_ancestor() {
        let (mut tree, root) = WidgetTree::new(label("root"), Style::default(), "root");
        let button = match tree.insert(root, sized(50.0, 50.0), focusable("button"), "button") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        // A non-focusable glyph inside the button -- clicking it must
        // still focus the button, not do nothing.
        let glyph = match tree.insert(button, sized(10.0, 10.0), label("glyph"), "glyph") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        tree.compute_layout(100.0, 100.0);
        assert_eq!(
            hit_test(&tree, 5.0, 5.0),
            Some(glyph),
            "sanity check: the point really does hit the inner glyph"
        );

        let mut manager = FocusManager::new();
        assert_eq!(manager.focus_at(&mut tree, 5.0, 5.0), Some(button));
    }

    #[test]
    fn focus_at_returns_none_when_nothing_focusable_is_hit() {
        let (mut tree, root) = WidgetTree::new(label("root"), sized(100.0, 100.0), "root");
        tree.compute_layout(100.0, 100.0);
        let mut manager = FocusManager::new();
        assert_eq!(manager.focus_at(&mut tree, 10.0, 10.0), None);
        let _ = root;
    }

    /// Review F6: a manager dropped (or replaced) without blurring
    /// leaves its widget's damage outset behind; the next manager's
    /// first focus change clears it, dirtying the stale ring's area.
    #[test]
    fn a_replaced_manager_leaves_no_stale_damage_outset() {
        let (mut tree, a, b) = two_placed();
        {
            // Dropped at the end of this block, never blurred.
            let mut first = FocusManager::new();
            assert!(
                first
                    .focus_with(&mut tree, a, FocusOrigin::Keyboard)
                    .is_ok()
            );
        }
        assert_eq!(tree.damage_outset(a), Some(FOCUS_RING_MAX_OUTSET));
        let _ = tree.take_damage();
        let mut second = FocusManager::new();
        assert!(
            second
                .focus_with(&mut tree, b, FocusOrigin::Keyboard)
                .is_ok()
        );
        assert_eq!(tree.damage_outset(a), Some(0), "the stale outset is gone");
        let Some(damage) = tree.take_damage() else {
            unreachable!("focusing b damages something");
        };
        // a sits at x = 20; its stale ring reached 4 px further left.
        assert!(
            damage.x <= 16,
            "the stale ring's area is repainted: {damage:?}"
        );
        assert_eq!(tree.damage_outset(b), Some(FOCUS_RING_MAX_OUTSET));
    }

    #[test]
    fn validate_clears_focus_on_a_removed_widget() {
        let (mut tree, root) = WidgetTree::new(label("root"), Style::default(), "root");
        let a = match tree.insert(root, Style::default(), focusable("a"), "a") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let mut manager = FocusManager::new();
        if let Err(err) = manager.focus(&mut tree, a) {
            unreachable!("{err:?}");
        }

        if let Err(err) = tree.remove(a) {
            unreachable!("{err:?}");
        }
        assert!(manager.validate(&tree));
        assert_eq!(manager.focused(), None);
    }

    #[test]
    fn validate_is_a_no_op_when_focus_is_still_valid() {
        let (mut tree, root) = WidgetTree::new(label("root"), Style::default(), "root");
        let a = match tree.insert(root, Style::default(), focusable("a"), "a") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let mut manager = FocusManager::new();
        if let Err(err) = manager.focus(&mut tree, a) {
            unreachable!("{err:?}");
        }
        assert!(!manager.validate(&tree));
        assert_eq!(manager.focused(), Some(a));
    }

    // -- popover layer (0.127.0) --

    /// A focusable owner whose non-focusable popover child hangs wholly
    /// outside it, over a later base sibling.
    fn popover_scene() -> (WidgetTree<&'static str>, [crate::WidgetId; 4]) {
        let column = Style {
            flex_direction: taffy::FlexDirection::Column,
            ..sized(100.0, 100.0)
        };
        let (mut tree, root) = WidgetTree::new(label("root"), column, "root");
        let owner = match tree.insert(root, sized(100.0, 20.0), focusable("owner"), "owner") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let below = match tree.insert(root, sized(100.0, 40.0), focusable("below"), "below") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let list = match tree.insert(owner, Style::default(), label("list"), "list") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        tree.compute_layout(100.0, 100.0);
        let place = aurora_core::Rect {
            x: 10,
            y: 20,
            width: 50,
            height: 90,
        };
        if let Err(err) = tree.set_bounds(list, place) {
            unreachable!("{err:?}");
        }
        if let Err(err) = tree.set_layer(list, crate::PaintLayer::Popover) {
            unreachable!("{err:?}");
        }
        assert_eq!(
            tree.bounds(below),
            Some(aurora_core::Rect {
                x: 0,
                y: 20,
                width: 100,
                height: 40
            }),
            "the base sibling sits under the popover"
        );
        (tree, [root, owner, below, list])
    }

    #[test]
    fn both_hit_testers_agree_everywhere_over_a_popover_scene() {
        let (tree, [root, owner, below, list]) = popover_scene();
        let mut seen = std::collections::HashSet::new();
        // Every half-pixel step: pixel centres *and* integer coordinates,
        // which land exactly on every widget's edges, where the two
        // testers' half-open containment rules must still agree.
        for yi in -8..=217 {
            for xi in -8..=217 {
                let (x, y) = (f64::from(xi) * 0.5, f64::from(yi) * 0.5);
                #[allow(clippy::cast_possible_truncation)]
                let tree_hit = tree.hit_test((x as f32, y as f32));
                assert_eq!(hit_test(&tree, x, y), tree_hit, "at ({x}, {y})");
                seen.insert(tree_hit);
            }
        }
        assert_eq!(hit_test(&tree, 10.0, 20.0), Some(list), "the list's corner");
        assert_eq!(
            hit_test(&tree, 60.0, 30.0),
            Some(below),
            "the list's right edge is half-open"
        );
        assert_eq!(hit_test(&tree, 30.0, 100.0), None, "the window's edge");
        for expected in [None, Some(root), Some(owner), Some(below), Some(list)] {
            assert!(seen.contains(&expected), "{expected:?} never hit");
        }
        assert_eq!(
            hit_test(&tree, 30.5, 50.5),
            Some(list),
            "over the base sibling"
        );
        assert_eq!(hit_test(&tree, 30.5, 99.5), Some(list), "past the owner");
        assert_eq!(hit_test(&tree, 30.5, 100.5), None, "past the window");
    }

    #[test]
    fn focus_at_over_a_popover_bubbles_to_its_owner() {
        let (mut tree, [_, owner, below, _]) = popover_scene();
        let mut manager = FocusManager::new();
        assert_eq!(manager.focus_at(&mut tree, 30.0, 50.0), Some(owner));
        assert_eq!(manager.focus_at(&mut tree, 80.0, 50.0), Some(below));
        // Tab order stays structural: the popover changes nothing there.
        assert_eq!(manager.focus_next(&mut tree), Some(owner));
    }
}
