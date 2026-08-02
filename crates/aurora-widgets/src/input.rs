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
#[must_use]
pub fn hit_test<W>(tree: &WidgetTree<W>, x: f64, y: f64) -> Option<WidgetId> {
    hit_test_node(tree, tree.root(), x, y)
}

fn hit_test_node<W>(tree: &WidgetTree<W>, id: WidgetId, x: f64, y: f64) -> Option<WidgetId> {
    let bounds = tree.bounds(id)?;
    if !contains(bounds, x, y) {
        return None;
    }
    let children = tree.children(id)?;
    for &child in children.iter().rev() {
        if let Some(hit) = hit_test_node(tree, child, x, y) {
            return Some(hit);
        }
    }
    Some(id)
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
#[derive(Debug)]
pub struct FocusManager {
    focused: Option<WidgetId>,
}

impl FocusManager {
    #[must_use]
    pub fn new() -> Self {
        Self { focused: None }
    }

    #[must_use]
    pub fn focused(&self) -> Option<WidgetId> {
        self.focused
    }

    /// Moves focus to `id`. Marks both the previously- and newly-focused
    /// widget dirty (a focus ring is visual state, the same as bounds or
    /// accessibility content changing).
    ///
    /// # Errors
    ///
    /// Returns [`WidgetError::UnknownWidget`] if `id` doesn't exist, or
    /// [`WidgetError::NotFocusable`] if it exists but doesn't support the
    /// `accesskit::Action::Focus` action. Nothing changes when this
    /// happens.
    pub fn focus<W>(&mut self, tree: &mut WidgetTree<W>, id: WidgetId) -> Result<(), WidgetError> {
        let supports_focus = tree
            .accessibility(id)
            .ok_or(WidgetError::UnknownWidget(id))?
            .supports_action(Action::Focus);
        if !supports_focus {
            return Err(WidgetError::NotFocusable(id));
        }
        self.set_focus(tree, Some(id));
        Ok(())
    }

    /// Clears focus (nothing focused).
    pub fn blur<W>(&mut self, tree: &mut WidgetTree<W>) {
        self.set_focus(tree, None);
    }

    /// Moves focus to the next focusable widget in tree order (pre-order,
    /// depth-first — the same order a screen reader's linear navigation,
    /// or a browser's default no-`tabindex` `Tab` order, would use),
    /// wrapping around after the last one. `None` if the tree has no
    /// focusable widgets at all.
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
    pub fn focus_at<W>(&mut self, tree: &mut WidgetTree<W>, x: f64, y: f64) -> Option<WidgetId> {
        let hit = hit_test(tree, x, y)?;
        let mut current = Some(hit);
        while let Some(id) = current {
            let is_focusable = tree
                .accessibility(id)
                .is_some_and(|node| node.supports_action(Action::Focus));
            if is_focusable {
                self.set_focus(tree, Some(id));
                return Some(id);
            }
            current = tree.parent(id);
        }
        None
    }

    /// Clears focus if the currently focused widget no longer exists in
    /// `tree` (see this type's own doc comment). Returns whether focus
    /// was actually cleared.
    pub fn validate<W>(&mut self, tree: &WidgetTree<W>) -> bool {
        if let Some(id) = self.focused
            && !tree.contains(id)
        {
            self.focused = None;
            return true;
        }
        false
    }

    fn set_focus<W>(&mut self, tree: &mut WidgetTree<W>, new: Option<WidgetId>) {
        if self.focused == new {
            return;
        }
        if let Some(old) = self.focused {
            // Already gone is fine (see this type's own doc comment) --
            // nothing left to mark dirty.
            let _ = tree.mark_dirty(old);
        }
        if let Some(new_id) = new {
            let _ = tree.mark_dirty(new_id);
        }
        self.focused = new;
    }

    fn step<W>(&mut self, tree: &mut WidgetTree<W>, forward: bool) -> Option<WidgetId> {
        let order = focus_order(tree);
        if order.is_empty() {
            self.set_focus(tree, None);
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
        self.set_focus(tree, Some(next));
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
}
