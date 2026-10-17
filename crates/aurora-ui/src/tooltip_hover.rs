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
pub fn hover_point(
    position: Option<(f32, f32)>,
    blockers: HoverBlockers,
) -> Option<(f32, f32)> {
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
            match Tooltip::new(&workspace.tree, owner, scales, text.clone(), TOOLTIP_SHOW_DELAY) {
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
