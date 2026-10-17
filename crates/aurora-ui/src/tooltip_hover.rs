//! Tooltips in the live workspace (0.262.0): which widget the pointer is
//! over, when its tooltip shows, where it goes and when it goes away.
//!
//! **One tooltip at a time.** [`TooltipHover`] holds at most one toolkit
//! [`Tooltip`] controller, for the widget under the pointer; moving to
//! another tooltipped widget drops the first (its node removed) and arms a
//! fresh one, so two tooltips never exist together and the delay restarts
//! on every new owner. The delay is [`TOOLTIP_SHOW_DELAY`], a constant:
//! the scales' `motion.duration.*` are animation durations that collapse
//! to zero under reduced motion, and a hover-intent delay must not (the
//! toolkit's `tooltip.rs` explains), and no tooltip-delay token exists.
//!
//! **Pure and clock-free.** Every call takes `now`; nothing reads a clock,
//! a window or a GPU. The text comes from [`crate::tooltip_text_for`], the
//! placement from `aurora_widgets::widgets::place_tooltip` through
//! [`Tooltip::place`], the size from the text measured at the tooltip's
//! own `type.size.xs` line, capped at `5 x spacing.xxxl`.
//!
//! **What hides it** (the app calls these; each is a method here so the
//! rules are tested without a window):
//!
//! - the pointer leaves the owner ([`TooltipHover::pointer`] with another
//!   target, or `None`: it left the window, or [`hover_point`] masked it
//!   because a modal, a drag, a rail resize or a panel drag owns the
//!   pointer, the window lost focus, or another popover is open);
//! - a press, a key, a wheel step ([`TooltipHover::dismiss`]): the owner's
//!   tooltip stays hidden until the pointer leaves it and comes back
//!   (WCAG 2.2 1.4.13 "dismissible");
//! - a dialog or the palette opening, a document switch
//!   ([`TooltipHover::clear`]).
//!
//! **Hoverable, not blocking.** A pointer that moves onto the shown tooltip
//! keeps it open (it is a popover above the canvas, so the pointer can
//! land on it); a press dismisses it before the press is routed, so it can
//! never swallow a click.
//!
//! **Accessibility.** The shown node is hidden from assistive technology
//! (`Tooltip::with_assistive_hidden`): the owner already carries the
//! words as its accessible description, and a screen reader must not hear
//! them twice. The tooltip never writes the owner's node.
//!
//! **Not covered:** the menu bar's text buttons (their label is their
//! tooltip) and the document tabs' close buttons (not a tooltip owner
//! kind: a `Container` with a button role).

use std::time::{Duration, Instant};

use aurora_text::TextEngine;
use aurora_theme::Scales;
use aurora_widgets::widgets::{Tooltip, TooltipSide, WidgetKind};
use aurora_widgets::{WidgetError, WidgetId, WidgetTree};

use crate::toolbar_tooltips::tooltip_text_for;
use crate::workspace::Workspace;

/// How long the pointer must rest on a widget before its tooltip shows.
/// A constant, not a token (see the module documentation); 500 ms is the
/// common desktop hover-intent delay.
pub const TOOLTIP_SHOW_DELAY: Duration = Duration::from_millis(500);

/// Everything that takes the pointer away from the tooltips.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
// One flag per independent owner of the pointer; an enum would only
// rename them.
#[allow(clippy::struct_excessive_bools)]
pub struct HoverBlockers {
    /// A dialog or the command palette is open.
    pub modal_open: bool,
    /// A canvas drag is live.
    pub canvas_drag: bool,
    /// The dock rail's divider is being dragged.
    pub rail_resize: bool,
    /// A panel is being dragged to redock.
    pub panel_drag: bool,
    /// The window does not have keyboard focus.
    pub window_unfocused: bool,
}

impl HoverBlockers {
    /// Whether anything owns the pointer.
    #[must_use]
    pub fn any(self) -> bool {
        self.modal_open
            || self.canvas_drag
            || self.rail_resize
            || self.panel_drag
            || self.window_unfocused
    }
}

/// Where the tooltips should see the pointer: `None` (as if it had left)
/// while any blocker owns it.
#[must_use]
pub fn hover_point(position: Option<(f32, f32)>, blockers: HoverBlockers) -> Option<(f32, f32)> {
    if blockers.any() { None } else { position }
}

/// The side a widget's tooltip prefers: to the right of the Tools panel's
/// buttons (a vertical strip against the window's left edge, where a
/// tooltip below would cover the next tool), below everything else (the
/// placement flips above at the window's bottom edge: the Layers toolbar
/// sits on the rail's bottom).
#[must_use]
pub fn preferred_side(workspace: &Workspace, id: WidgetId) -> TooltipSide {
    let colours = workspace.tools.colours;
    if workspace.tools.tool_for(id).is_some() || id == colours.swap || id == colours.defaults {
        TooltipSide::Right
    } else {
        TooltipSide::Below
    }
}

/// The live tooltip, if any (see the module documentation).
#[derive(Debug, Default, PartialEq)]
pub struct TooltipHover {
    current: Option<Tooltip>,
}

impl TooltipHover {
    /// A tracker showing nothing.
    #[must_use]
    pub fn new() -> Self {
        Self { current: None }
    }

    /// The widget whose tooltip is pending, shown or dismissed.
    #[must_use]
    pub fn owner(&self) -> Option<WidgetId> {
        self.current.as_ref().map(Tooltip::owner)
    }

    /// The shown tooltip's node, if it is shown.
    #[must_use]
    pub fn shown_node(&self) -> Option<WidgetId> {
        self.current.as_ref().and_then(Tooltip::node)
    }

    /// The text the current tooltip would show.
    #[must_use]
    pub fn text(&self) -> Option<&str> {
        self.current.as_ref().map(Tooltip::text)
    }

    /// When the pending tooltip is due, `None` when nothing is pending —
    /// the event loop sleeps until the next real event then.
    #[must_use]
    pub fn next_deadline(&self) -> Option<Instant> {
        self.current.as_ref().and_then(Tooltip::next_deadline)
    }

    /// Drops the current tooltip, removing its node. Returns whether a
    /// shown node went away (the caller lays out again).
    fn drop_current(&mut self, workspace: &mut Workspace) -> bool {
        let Some(mut tooltip) = self.current.take() else {
            return false;
        };
        let was_shown = tooltip.node().is_some();
        if let Err(err) = tooltip.detach(&mut workspace.tree) {
            // The owner vanished (a panel rebuild): its subtree, the node
            // included, went with it.
            tracing::debug!(?err, "tooltip owner gone while detaching");
        }
        was_shown
    }

    /// Reports the pointer (`None`: it left, or something else owns it).
    /// Starts, keeps or ends a tooltip. Returns whether the tree changed
    /// in a way that needs a layout (a node appeared, went or was
    /// relabelled).
    ///
    /// # Errors
    ///
    /// Whatever the toolkit controller refuses on a live owner.
    pub fn pointer(
        &mut self,
        workspace: &mut Workspace,
        scales: &Scales,
        point: Option<(f32, f32)>,
        now: Instant,
    ) -> Result<bool, WidgetError> {
        let shown_before = self.shown_node();
        let hit = point.and_then(|point| workspace.tree.hit_test(point));
        // Another popover (a menu, a dropdown list, the colour picker)
        // owns the pointer exactly as a modal does.
        let hit = hit.filter(|_| !foreign_popover_open(&workspace.tree, shown_before));
        let on_tip = hit.is_some_and(|hit| {
            shown_before.is_some() && workspace.tree.popover_root_of(hit) == shown_before
        });
        let target: Option<(WidgetId, String)> = if on_tip {
            self.current
                .as_ref()
                .map(|tooltip| (tooltip.owner(), tooltip.text().to_owned()))
        } else {
            hit.and_then(|hit| tooltip_text_for(workspace, hit).map(|text| (hit, text)))
        };
        let Some((owner, text)) = target else {
            let changed = self.drop_current(workspace);
            return Ok(changed);
        };
        let mut changed = false;
        if self.owner() != Some(owner) {
            changed |= self.drop_current(workspace);
            match Tooltip::new(
                &workspace.tree,
                owner,
                scales,
                text.clone(),
                TOOLTIP_SHOW_DELAY,
            ) {
                Ok(tooltip) => self.current = Some(tooltip.with_assistive_hidden(true)),
                Err(err) => {
                    // Not an allowed owner kind: no tooltip for it.
                    tracing::debug!(?err, "no tooltip for this widget");
                    return Ok(changed);
                }
            }
        }
        let Some(tooltip) = self.current.as_mut() else {
            return Ok(changed);
        };
        if tooltip.text() != text {
            tooltip.set_text(&mut workspace.tree, text)?;
            changed |= tooltip.node().is_some();
        }
        tooltip.set_hover(&mut workspace.tree, !on_tip, on_tip, now)?;
        changed |= self.shown_node() != shown_before;
        Ok(changed)
    }

    /// Advances the delay to `now`. Returns whether the node appeared.
    ///
    /// # Errors
    ///
    /// Whatever [`Tooltip::tick`] refuses (the owner gone).
    pub fn tick(&mut self, workspace: &mut Workspace, now: Instant) -> Result<bool, WidgetError> {
        let Some(tooltip) = self.current.as_mut() else {
            return Ok(false);
        };
        let before = tooltip.node();
        if let Err(err) = tooltip.tick(&mut workspace.tree, now) {
            self.current = None;
            return Err(err);
        }
        Ok(tooltip.node() != before)
    }

    /// A press, a key or a wheel step: hides a pending or shown tooltip
    /// until the pointer leaves its owner and returns. Returns whether a
    /// shown node went away.
    ///
    /// # Errors
    ///
    /// Whatever the toolkit controller refuses on a live owner.
    pub fn dismiss(&mut self, workspace: &mut Workspace) -> Result<bool, WidgetError> {
        let Some(tooltip) = self.current.as_mut() else {
            return Ok(false);
        };
        let before = tooltip.node();
        if let Err(err) = tooltip.owner_pressed(&mut workspace.tree) {
            self.current = None;
            return Err(err);
        }
        Ok(before.is_some() && tooltip.node().is_none())
    }

    /// Drops any tooltip outright (a dialog opened, the document changed,
    /// the window lost focus). Returns whether a shown node went away.
    pub fn clear(&mut self, workspace: &mut Workspace) -> bool {
        self.drop_current(workspace)
    }

    /// Sizes and places a shown tooltip against the window (the root's
    /// bounds): call after every layout. Returns whether the placement
    /// changed, i.e. a layout is needed; the second call after that layout
    /// finds nothing to change.
    ///
    /// # Errors
    ///
    /// Whatever [`Tooltip::place`] refuses (the owner gone).
    pub fn place(
        &mut self,
        workspace: &mut Workspace,
        engine: Option<&mut TextEngine>,
        scales: &Scales,
        scale_factor: f32,
    ) -> Result<bool, WidgetError> {
        let Some(tooltip) = self.current.as_mut() else {
            return Ok(false);
        };
        if tooltip.node().is_none() {
            return Ok(false);
        }
        let Some(window) = workspace.tree.bounds(workspace.root) else {
            return Ok(false);
        };
        let size = tooltip.measured_size(engine, scales, scale_factor);
        #[allow(clippy::cast_precision_loss)]
        let window = (window.width as f32, window.height as f32);
        let side = preferred_side(workspace, tooltip.owner());
        tooltip.place(&mut workspace.tree, size, window, side)
    }
}

/// Whether `tree` holds any popover root other than `own`: a menu, a
/// dropdown list or the colour picker is open.
#[must_use]
pub fn foreign_popover_open(tree: &WidgetTree<WidgetKind>, own: Option<WidgetId>) -> bool {
    tree.popover_roots()
        .into_iter()
        .any(|root| Some(root) != own)
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use std::time::{Duration, Instant};

    use aurora_core::Rect;
    use aurora_text::TextEngine;
    use aurora_theme::Scales;
    use aurora_widgets::widgets::{TooltipSide, WidgetKind};
    use aurora_widgets::{PaintLayer, TextMeasure, WidgetId, compute_text_layout};

    use super::{
        HoverBlockers, TOOLTIP_SHOW_DELAY, TooltipHover, foreign_popover_open, hover_point,
        preferred_side,
    };
    use crate::tool::Tool;
    use crate::workspace::{Workspace, build_workspace};

    fn ok<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
        match result {
            Ok(value) => value,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn scales() -> Scales {
        const SCALES_TOML: &str = include_str!("../../../design/tokens/scales.toml");
        ok(Scales::from_toml_str(SCALES_TOML))
    }

    const WINDOW: (f32, f32) = (1280.0, 800.0);

    struct Rig {
        ws: Workspace,
        scales: Scales,
        engine: TextEngine,
        hover: TooltipHover,
    }

    impl Rig {
        fn new() -> Self {
            let scales = scales();
            let mut rig = Self {
                ws: build_workspace(&scales),
                engine: ok(TextEngine::new()),
                scales,
                hover: TooltipHover::new(),
            };
            rig.layout(WINDOW);
            rig
        }

        fn layout(&mut self, window: (f32, f32)) {
            compute_text_layout(
                &mut self.ws.tree,
                window.0,
                window.1,
                Some(TextMeasure {
                    engine: &mut self.engine,
                    scales: &self.scales,
                    scale_factor: 1.0,
                }),
            );
        }

        /// Lays out, places the tooltip and lays out again until stable,
        /// as the app does after every change.
        fn settle(&mut self, window: (f32, f32)) {
            for _ in 0..4 {
                self.layout(window);
                let moved =
                    ok(self
                        .hover
                        .place(&mut self.ws, Some(&mut self.engine), &self.scales, 1.0));
                if !moved {
                    return;
                }
            }
            unreachable!("placement did not settle");
        }

        fn centre(&self, id: WidgetId) -> (f32, f32) {
            let Some(b) = self.ws.tree.bounds(id) else {
                unreachable!("laid out");
            };
            #[allow(clippy::cast_precision_loss)]
            (
                b.x as f32 + b.width as f32 / 2.0,
                b.y as f32 + b.height as f32 / 2.0,
            )
        }

        fn tool_button(&self, tool: Tool) -> WidgetId {
            let Some((_, id)) = self.ws.tools.buttons.iter().find(|(t, _)| *t == tool) else {
                unreachable!("tool button");
            };
            *id
        }

        fn point_at(&mut self, id: WidgetId, now: Instant) -> bool {
            let at = self.centre(id);
            ok(self
                .hover
                .pointer(&mut self.ws, &self.scales, Some(at), now))
        }

        fn popover_count(&self) -> usize {
            self.ws.tree.popover_roots().len()
        }
    }

    fn rect_of(rig: &Rig, id: WidgetId) -> Rect {
        let Some(b) = rig.ws.tree.bounds(id) else {
            unreachable!("laid out");
        };
        b
    }

    // ---- delay state machine ----------------------------------------

    #[test]
    fn a_tooltip_shows_exactly_at_the_delay_and_not_before() {
        let mut rig = Rig::new();
        let brush = rig.tool_button(Tool::Brush);
        let t0 = Instant::now();
        assert!(!rig.point_at(brush, t0), "arming changes nothing visible");
        assert_eq!(rig.hover.owner(), Some(brush));
        assert_eq!(rig.hover.shown_node(), None);
        assert_eq!(rig.hover.next_deadline(), Some(t0 + TOOLTIP_SHOW_DELAY));
        assert!(!ok(rig
            .hover
            .tick(&mut rig.ws, t0 + Duration::from_millis(499))));
        assert_eq!(rig.hover.shown_node(), None);
        assert!(ok(rig.hover.tick(&mut rig.ws, t0 + TOOLTIP_SHOW_DELAY)));
        assert!(rig.hover.shown_node().is_some());
        assert_eq!(rig.hover.text(), Some("Brush (B)"));
        // Shown: no deadline, so the loop sleeps.
        assert_eq!(rig.hover.next_deadline(), None);
    }

    #[test]
    fn moving_within_the_owner_keeps_the_original_arming_time() {
        let mut rig = Rig::new();
        let brush = rig.tool_button(Tool::Brush);
        let t0 = Instant::now();
        rig.point_at(brush, t0);
        rig.point_at(brush, t0 + Duration::from_millis(300));
        assert_eq!(rig.hover.next_deadline(), Some(t0 + TOOLTIP_SHOW_DELAY));
    }

    #[test]
    fn nothing_hovered_means_no_tooltip_and_no_wakeup() {
        let mut rig = Rig::new();
        let t0 = Instant::now();
        assert!(!ok(rig.hover.pointer(&mut rig.ws, &rig.scales, None, t0)));
        assert_eq!(rig.hover.owner(), None);
        assert_eq!(rig.hover.next_deadline(), None);
        assert!(!ok(rig
            .hover
            .tick(&mut rig.ws, t0 + Duration::from_secs(9))));
        // A point over a widget with no tooltip arms nothing either.
        let at = rig.centre(rig.ws.canvas_area);
        assert!(!ok(rig.hover.pointer(
            &mut rig.ws,
            &rig.scales,
            Some(at),
            t0
        )));
        assert_eq!(rig.hover.owner(), None);
        assert_eq!(rig.hover.next_deadline(), None);
    }

    #[test]
    fn leaving_the_owner_removes_the_tooltip_and_the_deadline() {
        let mut rig = Rig::new();
        let brush = rig.tool_button(Tool::Brush);
        let t0 = Instant::now();
        rig.point_at(brush, t0);
        ok(rig.hover.tick(&mut rig.ws, t0 + TOOLTIP_SHOW_DELAY));
        assert_eq!(rig.popover_count(), 1);
        let changed =
            ok(rig
                .hover
                .pointer(&mut rig.ws, &rig.scales, None, t0 + Duration::from_secs(1)));
        assert!(changed, "a node went away");
        assert_eq!(rig.popover_count(), 0);
        assert_eq!(rig.hover.owner(), None);
        // And a pending one is cancelled by leaving too.
        rig.point_at(brush, t0 + Duration::from_secs(2));
        assert!(rig.hover.next_deadline().is_some());
        ok(rig
            .hover
            .pointer(&mut rig.ws, &rig.scales, None, t0 + Duration::from_secs(2)));
        assert_eq!(rig.hover.next_deadline(), None);
    }

    #[test]
    fn one_tooltip_at_a_time_and_the_delay_restarts_on_a_new_owner() {
        let mut rig = Rig::new();
        let (brush, eraser) = (rig.tool_button(Tool::Brush), rig.tool_button(Tool::Eraser));
        let t0 = Instant::now();
        rig.point_at(brush, t0);
        ok(rig.hover.tick(&mut rig.ws, t0 + TOOLTIP_SHOW_DELAY));
        assert_eq!(rig.popover_count(), 1);
        let t1 = t0 + Duration::from_millis(900);
        assert!(rig.point_at(eraser, t1), "the first node went away");
        assert_eq!(rig.popover_count(), 0, "the new one is only pending");
        assert_eq!(rig.hover.owner(), Some(eraser));
        assert_eq!(rig.hover.next_deadline(), Some(t1 + TOOLTIP_SHOW_DELAY));
        ok(rig.hover.tick(&mut rig.ws, t1 + TOOLTIP_SHOW_DELAY));
        assert_eq!(rig.popover_count(), 1);
        assert_eq!(rig.hover.text(), Some("Eraser (E)"));
    }

    // ---- hide conditions --------------------------------------------

    #[test]
    fn a_dismissal_hides_it_until_the_pointer_leaves_and_returns() {
        let mut rig = Rig::new();
        let brush = rig.tool_button(Tool::Brush);
        let t0 = Instant::now();
        rig.point_at(brush, t0);
        ok(rig.hover.tick(&mut rig.ws, t0 + TOOLTIP_SHOW_DELAY));
        assert!(ok(rig.hover.dismiss(&mut rig.ws)), "a shown node went away");
        assert_eq!(rig.popover_count(), 0);
        // Still over the owner: stays hidden however long it rests.
        let t1 = t0 + Duration::from_secs(2);
        rig.point_at(brush, t1);
        assert_eq!(rig.hover.next_deadline(), None);
        assert!(!ok(rig
            .hover
            .tick(&mut rig.ws, t1 + Duration::from_secs(9))));
        assert_eq!(rig.hover.shown_node(), None);
        // Leave and come back: armed afresh.
        ok(rig.hover.pointer(&mut rig.ws, &rig.scales, None, t1));
        let t2 = t1 + Duration::from_secs(1);
        rig.point_at(brush, t2);
        assert_eq!(rig.hover.next_deadline(), Some(t2 + TOOLTIP_SHOW_DELAY));
    }

    #[test]
    fn a_dismissal_cancels_a_pending_tooltip_without_reporting_a_change() {
        let mut rig = Rig::new();
        let brush = rig.tool_button(Tool::Brush);
        let t0 = Instant::now();
        rig.point_at(brush, t0);
        assert!(!ok(rig.hover.dismiss(&mut rig.ws)), "nothing was shown");
        assert_eq!(rig.hover.next_deadline(), None);
        assert!(!ok(rig
            .hover
            .tick(&mut rig.ws, t0 + Duration::from_secs(9))));
        // With nothing at all, a dismissal is a no-op.
        let mut idle = TooltipHover::new();
        assert!(!ok(idle.dismiss(&mut rig.ws)));
    }

    #[test]
    fn clear_drops_even_a_dismissed_tooltip_and_reports_a_shown_node() {
        let mut rig = Rig::new();
        let brush = rig.tool_button(Tool::Brush);
        let t0 = Instant::now();
        rig.point_at(brush, t0);
        ok(rig.hover.tick(&mut rig.ws, t0 + TOOLTIP_SHOW_DELAY));
        assert!(rig.hover.clear(&mut rig.ws));
        assert_eq!(rig.popover_count(), 0);
        assert_eq!(rig.hover.owner(), None);
        assert!(!rig.hover.clear(&mut rig.ws), "nothing left to clear");
        // After a clear the same owner arms afresh (a dialog closed).
        rig.point_at(brush, t0 + Duration::from_secs(5));
        assert!(rig.hover.next_deadline().is_some());
    }

    #[test]
    fn blockers_mask_the_pointer() {
        let p = Some((3.0, 4.0));
        assert_eq!(hover_point(p, HoverBlockers::default()), p);
        assert_eq!(hover_point(None, HoverBlockers::default()), None);
        for blockers in [
            HoverBlockers {
                modal_open: true,
                ..HoverBlockers::default()
            },
            HoverBlockers {
                canvas_drag: true,
                ..HoverBlockers::default()
            },
            HoverBlockers {
                rail_resize: true,
                ..HoverBlockers::default()
            },
            HoverBlockers {
                panel_drag: true,
                ..HoverBlockers::default()
            },
            HoverBlockers {
                window_unfocused: true,
                ..HoverBlockers::default()
            },
        ] {
            assert!(blockers.any());
            assert_eq!(hover_point(p, blockers), None, "{blockers:?}");
        }
    }

    #[test]
    fn another_open_popover_takes_the_pointer_from_the_tooltips() {
        let mut rig = Rig::new();
        let brush = rig.tool_button(Tool::Brush);
        let t0 = Instant::now();
        rig.point_at(brush, t0);
        ok(rig.hover.tick(&mut rig.ws, t0 + TOOLTIP_SHOW_DELAY));
        assert!(!foreign_popover_open(&rig.ws.tree, rig.hover.shown_node()));
        // A menu opens elsewhere: any popover root counts.
        let list = ok(rig.ws.tree.insert(
            rig.ws.root,
            taffy::Style::default(),
            accesskit::Node::new(accesskit::Role::Menu),
            WidgetKind::Container,
        ));
        ok(rig.ws.tree.set_layer(list, PaintLayer::Popover));
        assert!(foreign_popover_open(&rig.ws.tree, rig.hover.shown_node()));
        let changed = rig.point_at(brush, t0 + Duration::from_secs(1));
        assert!(changed, "the shown tooltip went away");
        assert_eq!(rig.hover.owner(), None);
    }

    // ---- text ---------------------------------------------------------

    #[test]
    fn every_tool_button_arms_a_tooltip_with_its_name_and_shortcut() {
        let mut rig = Rig::new();
        let t0 = Instant::now();
        for tool in Tool::ALL {
            let button = rig.tool_button(tool);
            ok(rig.hover.pointer(&mut rig.ws, &rig.scales, None, t0));
            rig.point_at(button, t0);
            assert_eq!(
                rig.hover.text(),
                Some(format!("{} ({})", tool.label(), tool.shortcut_text()).as_str()),
                "{tool:?}"
            );
            assert_eq!(rig.hover.owner(), Some(button), "{tool:?}");
        }
    }

    #[test]
    fn the_other_buttons_have_tooltips_too() {
        let mut rig = Rig::new();
        let t0 = Instant::now();
        let colours = rig.ws.tools.colours;
        for (id, text) in [
            (colours.swap, "Swap Colours (X)"),
            (colours.defaults, "Default Colours (D)"),
        ] {
            rig.point_at(id, t0);
            assert_eq!(rig.hover.text(), Some(text));
        }
        for (_, id) in rig.ws.layer_toolbar.buttons {
            rig.point_at(id, t0);
            assert!(rig.hover.text().is_some_and(|t| !t.is_empty()));
        }
    }

    // ---- placement ----------------------------------------------------

    fn show(rig: &mut Rig, id: WidgetId, window: (f32, f32)) -> WidgetId {
        let t0 = Instant::now();
        rig.point_at(id, t0);
        ok(rig.hover.tick(&mut rig.ws, t0 + TOOLTIP_SHOW_DELAY));
        rig.settle(window);
        let Some(node) = rig.hover.shown_node() else {
            unreachable!("shown");
        };
        node
    }

    fn inside(rect: Rect, window: (f32, f32)) -> bool {
        #[allow(clippy::cast_precision_loss)]
        let (x, y, r, b) = (
            rect.x as f32,
            rect.y as f32,
            (rect.x + i64::from(rect.width)) as f32,
            (rect.y + i64::from(rect.height)) as f32,
        );
        x >= 0.0 && y >= 0.0 && r <= window.0 && b <= window.1
    }

    #[test]
    fn a_tool_tooltip_is_content_wide_and_sits_right_of_its_button() {
        let mut rig = Rig::new();
        let marquee = rig.tool_button(Tool::EllipticalMarquee);
        let node = show(&mut rig, marquee, WINDOW);
        let (owner, tip) = (rect_of(&rig, marquee), rect_of(&rig, node));
        assert!(
            tip.width > owner.width,
            "wider than the 32 px button: {tip:?}"
        );
        assert!(
            f64::from(tip.width) <= 5.0 * 64.0,
            "capped at five xxxl steps: {tip:?}"
        );
        assert_eq!(preferred_side(&rig.ws, marquee), TooltipSide::Right);
        assert_eq!(tip.x, owner.x + i64::from(owner.width) + 4, "gap xxs");
        assert_eq!(tip.y, owner.y);
        assert!(inside(tip, WINDOW));
    }

    #[test]
    fn the_tooltip_width_follows_the_measured_text() {
        let mut rig = Rig::new();
        let (a, b) = (
            rig.tool_button(Tool::Move),
            rig.tool_button(Tool::EllipticalMarquee),
        );
        let short = show(&mut rig, a, WINDOW);
        let short_w = rect_of(&rig, short).width;
        rig.hover.clear(&mut rig.ws);
        let long = show(&mut rig, b, WINDOW);
        let long_w = rect_of(&rig, long).width;
        assert!(long_w > short_w, "{long_w} vs {short_w}");
    }

    #[test]
    fn the_layers_toolbar_tooltip_flips_above_at_the_window_bottom() {
        let mut rig = Rig::new();
        // A window the rail fills: the Layers toolbar sits at the bottom
        // of the rail, near the window's lower edge.
        let Some((_, button)) = rig.ws.layer_toolbar.buttons.first().copied() else {
            unreachable!("buttons");
        };
        let node = show(&mut rig, button, WINDOW);
        let (owner, tip) = (rect_of(&rig, button), rect_of(&rig, node));
        assert!(inside(tip, WINDOW), "{tip:?}");
        let below = owner.y + i64::from(owner.height) + 4 + i64::from(tip.height);
        if below > 800 {
            assert!(tip.y < owner.y, "flipped above: {tip:?} vs {owner:?}");
        } else {
            assert_eq!(tip.y, owner.y + i64::from(owner.height) + 4);
        }
    }

    #[test]
    fn a_small_window_keeps_every_tooltip_inside_and_a_resize_replaces_it() {
        let mut rig = Rig::new();
        let marquee = rig.tool_button(Tool::EllipticalMarquee);
        let node = show(&mut rig, marquee, WINDOW);
        let wide = rect_of(&rig, node);
        // Shrink the window: the tooltip is repositioned inside it.
        let small = (200.0, 300.0);
        rig.settle(small);
        let Some(node) = rig.hover.shown_node() else {
            unreachable!("still shown");
        };
        let tip = rect_of(&rig, node);
        assert!(inside(tip, small), "{tip:?} in {small:?}");
        assert_eq!(
            tip.width, wide.width,
            "the box does not depend on the window"
        );
        // Growing it back moves it back beside the button.
        rig.settle(WINDOW);
        let Some(node) = rig.hover.shown_node() else {
            unreachable!("still shown");
        };
        assert_eq!(rect_of(&rig, node), wide);
    }

    #[test]
    fn placement_settles_in_one_extra_pass() {
        let mut rig = Rig::new();
        let brush = rig.tool_button(Tool::Brush);
        let t0 = Instant::now();
        rig.point_at(brush, t0);
        ok(rig.hover.tick(&mut rig.ws, t0 + TOOLTIP_SHOW_DELAY));
        rig.layout(WINDOW);
        let first = ok(rig
            .hover
            .place(&mut rig.ws, Some(&mut rig.engine), &rig.scales, 1.0));
        assert!(first, "a fresh node needs its placement");
        rig.layout(WINDOW);
        let second = ok(rig
            .hover
            .place(&mut rig.ws, Some(&mut rig.engine), &rig.scales, 1.0));
        assert!(!second, "and then nothing changes");
    }

    #[test]
    fn the_shown_tooltip_is_hidden_from_assistive_technology_and_the_owner_untouched() {
        let mut rig = Rig::new();
        let brush = rig.tool_button(Tool::Brush);
        let before = rig.ws.tree.accessibility(brush).cloned();
        let node = show(&mut rig, brush, WINDOW);
        let Some(access) = rig.ws.tree.accessibility(node) else {
            unreachable!("node");
        };
        assert!(access.is_hidden(), "the owner's description says it");
        assert_eq!(access.label(), Some("Brush (B)"));
        assert_eq!(rig.ws.tree.accessibility(brush).cloned(), before);
    }
}
