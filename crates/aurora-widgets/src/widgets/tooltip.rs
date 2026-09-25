//! A tooltip: a short, non-interactive text hint attached to one owner
//! widget, shown after a hover or keyboard-focus delay and dismissed by
//! `Escape`, a press, or losing hover and focus.
//!
//! **Shape: a caller-owned controller, not a tree payload.** Unlike
//! `dropdown.rs` and `tab_bar.rs`, whose state lives in the owning
//! widget's own [`WidgetKind`] payload, a tooltip's state lives in a
//! [`Tooltip`] value the caller holds next to the owner's id. The
//! tooltip is not a widget of its own until it is shown: it attaches to
//! an *existing* widget (a button, a slider, a tab) whose own module
//! owns that widget's payload and node, and this module must not write
//! either (see "The owner's node is never written" below). So the
//! controller holds the timing state — a pure state machine, unit-tested
//! with no tree at all — and the tree holds exactly one thing: a
//! [`WidgetKind::Tooltip`] child of the owner, present **if and only
//! if** the tooltip is [`TooltipPhase::Shown`].
//!
//! Every mutator takes the tree, computes the next state with a pure
//! transition, reconciles the tree to match, and commits the new state
//! **only if every tree operation succeeded** — a call that returns an
//! error leaves the controller exactly as it was, with one deliberate
//! exception (an owner removed from outside; see "Robustness" below).
//! **That guarantee is structural and untested**: every tree operation a
//! call makes is on an id it has just checked is alive under a live
//! owner, and no such operation can fail today, so no test can make one
//! fail — a mutation committing the new state *before* the tree
//! operations survives every test, and is disclosed rather than hidden.
//!
//! # No clock
//!
//! Nothing here reads a clock. Every time-dependent call takes the
//! caller's `now: Instant`, and elapsed time is always
//! `now.saturating_duration_since(since)` — never `now - since` or
//! `duration_since`, whose std docs say they *currently* saturate on an
//! earlier `now` but that "future versions may reintroduce the panic"
//! (a caller's event timestamps are not guaranteed monotonic across
//! sources). On today's toolchain the three are therefore
//! indistinguishable, so no test can pin this choice — it is a
//! portability guard, not a tested behaviour. A caller drives the show delay
//! by calling [`Tooltip::tick`] at or after [`Tooltip::next_deadline`];
//! tests fabricate `Instant`s rather than sleeping.
//!
//! **The show delay is a caller-supplied [`Duration`], not a token.**
//! `design/tokens/scales.toml`'s `motion.duration.*` are *animation*
//! durations that collapse to `0` under OS reduced motion
//! (`scales.toml:83-88`); a tooltip's hover-intent delay must not
//! collapse with them, or reduced-motion users would get a tooltip on
//! every pointer pass. Whether a dedicated "tooltip show delay" token
//! should exist is a design-owner question (PRD FR-027 *Ownership*),
//! raised rather than invented here.
//!
//! # Transitions
//!
//! `engaged` is `owner_hovered || owner_focused || (tooltip_hovered &&
//! phase == Shown)`. A "rise" is a `false -> true` edge of the owner's
//! hover or focus flag, as last reported to this controller.
//!
//! | event | `Idle` | `Pending { since }` | `Shown` | `Dismissed` |
//! |---|---|---|---|---|
//! | hover: owner rises | `Pending { now }` | unchanged (keeps `since`) | `Shown` | `Pending { now }` |
//! | hover: no rise, not engaged after | `Idle` | `Idle` | `Idle` (node removed) | `Idle` |
//! | hover: otherwise | `Idle` | unchanged | `Shown` | `Dismissed` |
//! | focus rises | `Pending { now }` | unchanged | `Shown` | `Pending { now }` |
//! | focus falls, not engaged | `Idle` | `Idle` | `Idle` (node removed) | `Idle` |
//! | focus falls, still engaged | — | unchanged | `Shown` | `Dismissed` |
//! | [`tick`], elapsed `>=` delay | `Idle` | `Shown` (node inserted) | `Shown` | `Dismissed` |
//! | [`tick`], elapsed `<` delay | `Idle` | unchanged | `Shown` | `Dismissed` |
//! | [`tick`], `now < since` | `Idle` | `Pending { now }`, then as above | `Shown` | `Dismissed` |
//! | [`dismiss`] (`Escape`) | `Idle`, `false` | `Dismissed`, **`false`** | `Dismissed` (node removed), `true` | `Dismissed`, `false` |
//! | [`owner_pressed`] | `Idle` | `Dismissed` | `Dismissed` (node removed) | `Dismissed` |
//! | [`set_text`] | text | text | text, node relabelled | text |
//!
//! The pointer-over-tooltip flag is stored only while `Shown` and forced
//! `false` otherwise, so it can keep a *shown* tooltip open (WCAG 2.2
//! SC 1.4.13 "hoverable": the pointer can move from the owner onto the
//! tooltip without it vanishing) but can never open or hold open one
//! that is not shown. **`Dismissed` is sticky**: it stays dismissed
//! under continued hover or focus, only a fresh rise re-arms it, and
//! losing all engagement returns it to `Idle` (WCAG 1.4.13
//! "dismissible").
//!
//! **Hover is level-triggered**: [`Tooltip::set_hover`] takes both
//! flags at once — "is the pointer over the owner, is it over the
//! tooltip" — rather than separate enter/leave events. With separate
//! events, moving the pointer across the owner/tooltip seam delivers an
//! owner-leave before the tooltip-enter, and the tooltip would close in
//! between. Calling it again with the same flags is a no-op, so a caller
//! can report on every pointer move.
//!
//! **The caller reports tooltip hover.** Since 0.127.0 the shown
//! tooltip is a [`PaintLayer::Popover`](crate::PaintLayer) root, so
//! [`crate::hit_test`] (and `WidgetTree::hit_test`) *does* reach it even
//! though it lies outside its owner's bounds. This module still observes
//! no pointer itself: a caller derives the `tooltip` flag from a hit
//! test, e.g. `hit.is_some_and(|h| tree.popover_root_of(h) ==
//! tooltip.node())`, and reports it through [`Tooltip::set_hover`].
//!
//! # The accessibility vocabulary, checked against the pinned sources
//!
//! Checked against `accesskit` 0.24.1 and `accesskit_consumer` 0.38.0,
//! `accesskit_windows` 0.34.0, `accesskit_macos` 0.26.3,
//! `accesskit_atspi_common` 0.19.1, as pinned in `Cargo.lock`.
//!
//! - **The shown tooltip is a `Role::Tooltip` child of its owner**,
//!   labelled with its text, with **no actions** (never focusable, never
//!   clickable) and nothing else set. It maps to UIA's `ToolTip` control
//!   type (`accesskit_windows` `src/node.rs:206`, localized type
//!   "tooltip" at `:392`), `NSAccessibilityGroupRole` with subrole
//!   `AXUserInterfaceTooltip` (`accesskit_macos` `src/node.rs:173`,
//!   `:282`) and AT-SPI `ToolTip` (`accesskit_atspi_common`
//!   `src/node.rs:279`). `accesskit_consumer` reports it read-only by
//!   default (`Node::should_have_read_only_state_by_default`,
//!   `src/node.rs:864-882`).
//! - **The tooltip cannot pollute its owner's name.** A `Button` or
//!   `CheckBox` with no label of its own takes its name from its
//!   descendants (`Node::labelled_by`, `src/node.rs:701-718`), but only
//!   through `descendant_label_filter` (`src/node.rs:693-699`), which
//!   includes `Label`/`Image` children and excludes every other role's
//!   whole subtree — a `Tooltip` included. A test pins both halves: a
//!   labelled button keeps its label while the tooltip is shown, and an
//!   unlabelled one does **not** gain the tooltip's text as its name.
//!   **The flip side, disclosed**: an icon-only button therefore gets
//!   *no* accessible name from its tooltip. It needs a real label of its
//!   own (through its own module's API), as it would without a tooltip.
//! - **`described_by` is deliberately not set** — on the owner or
//!   anywhere. No pinned adapter reads it: a search of
//!   `accesskit_windows` 0.34.0, `accesskit_macos` 0.26.3,
//!   `accesskit_atspi_common` 0.19.1 and `accesskit_consumer` 0.38.0
//!   finds no read of `described_by` at all, so setting it would be a
//!   claim with no effect.
//! - **The owner's node is never written.** `description` *is* read by
//!   every adapter (UIA `FullDescription`, `accesskit_windows`
//!   `src/node.rs:1310`; `accesskit_macos` `src/node.rs:315`, `:519`;
//!   `accesskit_atspi_common` `src/node.rs:46`, `:77`, `:580`;
//!   `accesskit_consumer` `Node::description`, `src/node.rs:779`), but
//!   every owner module rebuilds its own node wholesale on its next
//!   mutation (a button's press, a tab's selection), which would silently
//!   clobber a description written from here. A caller that wants the
//!   owner to *describe* itself with the tooltip's text sets it through
//!   the owner's own API, where it survives.
//! - **No "tooltip opened" announcement exists in any pinned adapter**:
//!   Windows only maps the role (`src/node.rs:206`, `:392`), macOS only
//!   the role and subrole (`:173`, `:282`), AT-SPI only the role
//!   (`:279`). A screen-reader user reaches the text by exploring the
//!   owner's children, not by being told it appeared. Disclosed, not
//!   worked around.
//!
//! # Allowed owners
//!
//! [`Tooltip::new`] accepts a [`WidgetKind::Button`],
//! [`WidgetKind::Checkbox`], [`WidgetKind::Slider`],
//! [`WidgetKind::ColorSwatch`], [`WidgetKind::Scrollbar`] or
//! [`WidgetKind::Tab`] owner — leaf widgets no module enumerates the
//! children of — and returns [`WidgetError::WrongWidgetKind`] for every
//! other kind. Rejected on purpose: a `Dropdown` (its open list is an
//! absolutely positioned child in exactly the slot a tooltip would take,
//! `dropdown.rs`'s `list_style`), a `TabBar` (`tab_bar.rs`'s reconcile
//! enumerates the bar's children), a `TreeView`/`TreeItem` and a
//! `CommandPalette` (both walk their children to find rows), the colour
//! picker and the curve editor and their parts (each owns and reconciles
//! its whole subtree), and the
//! containers `Dialog`, `Panel`, `Container` and `DropdownList`, which
//! are not controls a tooltip describes.
//!
//! **A `Tab` owner's tooltip can vanish from outside**: `tab_bar.rs`'s
//! lazy repair rebuilds *every* tab when a caller damages the bar,
//! removing each old tab — and its tooltip child — and every tab id
//! changes. The next call on this controller then finds its owner gone
//! (see "Robustness"); a caller rebuilding its tooltips after a repair
//! creates new controllers for the new tab ids.
//!
//! # Robustness
//!
//! - **Owner removed from outside**: every tree-touching call returns
//!   [`WidgetError::UnknownWidget`] with the owner's id. Because
//!   `WidgetTree::remove` removes a widget's whole subtree, the tooltip
//!   node went with it, so the controller also drops its stale node id
//!   and resets to `Idle` with every flag cleared — the one case where
//!   an erroring call changes the controller, so that
//!   [`Tooltip::node`] never reports a dead id. Ids are never reused, so
//!   the controller stays inert; the caller drops it.
//! - **Tooltip node removed from outside** (owner alive): the next call
//!   that leaves the tooltip `Shown` inserts a fresh node; one that
//!   leaves it hidden simply forgets the dead id. Nothing panics either
//!   way.
//! - **Tooltip node's accessibility overwritten**: the next call that
//!   leaves it `Shown` compares it against what the controller says (the
//!   whole `accesskit::Node`, `PartialEq`) and rewrites it only if it
//!   disagrees — no damage when nothing does. **The repair covers the
//!   accessibility node only**: the node's layout `Style` is not
//!   compared or restored, so a caller that restyles the tooltip node
//!   keeps its style until the next re-show inserts a fresh node.
//! - **Tooltip node's payload overwritten** (no longer
//!   [`WidgetKind::Tooltip`]): the controller no longer recognises it as
//!   its own and leaves it alone — the next call that leaves it `Shown`
//!   inserts a fresh node beside it, and a hide simply forgets it. This
//!   is the price of the next rule.
//! - **Driven against the wrong tree**: ids are per-tree — every
//!   [`WidgetTree`] numbers from the same start — so a controller shown
//!   in one tree and then (a caller bug) handed another finds its
//!   tracked id naming whatever that tree has there. It removes or
//!   repairs a tracked id only if it is a `WidgetKind::Tooltip` child of
//!   the owner; anything else is left alone. Not a full defence: the
//!   other tree's owner id must also exist (else `UnknownWidget`), and a
//!   same-id `Tooltip` child of a same-id owner there — another
//!   controller's — is indistinguishable and would be treated as this
//!   one's.
//! - **Two controllers on one owner** are not detected: each would show
//!   its own child. One tooltip per owner is the caller's contract.
//!   [`Tooltip`] is deliberately not `Clone` for the same reason: a
//!   clone would track the same node, and either copy hiding would
//!   remove the other's.
//! - **The owner allowlist is checked only at [`Tooltip::new`]**. A
//!   caller that later overwrites the owner's payload with a disallowed
//!   kind is not detected; the controller keeps attaching its child to
//!   that id.
//! - **Empty text is the caller's responsibility**, as for `Dropdown`
//!   and `TabBar` labels: `""` is accepted and shows an empty
//!   `Role::Tooltip` node (and an empty box). A test pins that it is
//!   accepted rather than rejected.
//! - **A `now` earlier than `since`** (timestamps from sources that
//!   disagree, or a bogus far-future `now` passed to a hover or focus
//!   report) is self-healed by the next [`Tooltip::tick`], which clamps
//!   `since` to its own `now` first — otherwise, since repeated reports
//!   deliberately keep `since`, the tooltip would never show while
//!   engaged. A merely skewed `since` therefore shows up to that skew
//!   early.
//!
//! # What this does not do, stated rather than implied away
//!
//! - **Layering is creation order, nothing more.** The tooltip is a
//!   popover root (0.127.0): it paints after every base-layer widget and
//!   no clipping ancestor of its owner clips it. Between popovers the
//!   most recently *created* stacks on top, and the tooltip node is
//!   re-inserted on every show, so a tooltip shown over an open dropdown
//!   list paints and hit-tests above it.
//!   The converse holds too, and is not special-cased: a tooltip
//!   *already showing* when a dropdown list or a menu opens sits under
//!   it, since that popover is created after the tooltip (pinned by a
//!   test). A caller that wants the tooltip gone can
//!   [`Tooltip::dismiss`] it whenever it opens another popover. Like
//!   every popover, it also stops painting and hit-testing when its
//!   owner is wholly clipped away.
//! - **No viewport flip or collision handling**: the tooltip always hangs
//!   directly below its owner, left-aligned; the part running past the
//!   window (the tree root's own bounds) is clamped away rather than the
//!   tooltip being moved (`paint::clip_to_clipping_ancestors`).
//! - **No text measurement**: its width is its owner's width
//!   (`percent(1.0)`), its height one `typography.size.xs` line plus
//!   `spacing.xxs` padding above and below — a stand-in until text can be
//!   measured, not a design decision. No glyphs are drawn at all (a
//!   crate-wide gap): the text reaches the accessibility tree only.
//!   **On small or narrow owners this makes the tooltip nearly
//!   unusable**: a `Checkbox` (a 13 × 13 px box under the shipped
//!   scales; a `ColorSwatch` is as small) gets a tooltip 16 px wide —
//!   nothing but its own `spacing.xs` padding either side, which already
//!   exceeds the box (pinned by a test) — far too narrow for any real
//!   text, and a *vertical* `Scrollbar` gets a sliver the scrollbar's
//!   width hanging below its bottom end.
//!   Both are allowed owners anyway, because the fix is text
//!   measurement, not a narrower allowlist.
//! - **No shadow**: the mockup's `box-shadow: var(--elevation-1)`
//!   (`design/gallery/index.html:126-133`) is not rendered — nothing in
//!   `paint.rs` draws shadows. See `paint::paint_tooltip` for the fill,
//!   border, and the `surface.overlay`/"Elevation 1" mismatch raised as a
//!   design-owner question.
//! - **No automatic dismissal on scroll, window blur, or another
//!   tooltip opening**, and no "skip the delay when moving between
//!   tooltipped widgets" warm-up — each is caller policy on top of
//!   [`Tooltip::dismiss`]/[`Tooltip::detach`].
//! - **Damage**: inserting the node dirties its subtree but reaches the
//!   damage region only on the next `compute_layout`, which gives it
//!   bounds (`WidgetTree::insert`'s own contract); removing it marks its
//!   last-known bounds dirty (`WidgetTree::remove`). Both are pinned by
//!   tests.
//!
//! [`tick`]: Tooltip::tick
//! [`dismiss`]: Tooltip::dismiss
//! [`owner_pressed`]: Tooltip::owner_pressed
//! [`set_text`]: Tooltip::set_text

use std::time::{Duration, Instant};

use accesskit::{Node, Role};
use aurora_theme::Scales;
use taffy::style_helpers::{auto, length, percent, zero};
use taffy::{Position, Rect as LayoutRect, Size, Style};

use super::{WidgetKind, spacing, type_size};
use crate::error::WidgetError;
use crate::tree::{PaintLayer, WidgetId, WidgetTree};

/// Where a [`Tooltip`] is in its show/hide cycle — see this module's own
/// doc comment for the transition table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TooltipPhase {
    /// Hidden, with nothing engaging the owner.
    Idle,
    /// The owner was hovered or focused at `since`; the tooltip shows
    /// once the delay has elapsed from there ([`Tooltip::tick`]).
    Pending { since: Instant },
    /// Visible: the tooltip's node exists in the tree.
    Shown,
    /// Hidden by `Escape` or a press while still engaged; stays hidden
    /// until a fresh hover or focus rise, or until engagement is lost.
    Dismissed,
}

/// The pure part of a [`Tooltip`]: its phase and the three engagement
/// flags. Every transition is a function on this alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Model {
    phase: TooltipPhase,
    owner_hovered: bool,
    tooltip_hovered: bool,
    owner_focused: bool,
}

impl Model {
    const IDLE: Self = Self {
        phase: TooltipPhase::Idle,
        owner_hovered: false,
        tooltip_hovered: false,
        owner_focused: false,
    };

    fn shown(self) -> bool {
        self.phase == TooltipPhase::Shown
    }

    /// The phase a hover or focus *rise* moves to: arms the delay from
    /// `Idle` or `Dismissed`, and changes nothing that is already
    /// pending (the delay is not restarted) or shown.
    fn on_rise(phase: TooltipPhase, now: Instant) -> TooltipPhase {
        match phase {
            TooltipPhase::Idle | TooltipPhase::Dismissed => TooltipPhase::Pending { since: now },
            TooltipPhase::Pending { .. } | TooltipPhase::Shown => phase,
        }
    }

    /// Forces the tooltip-hover flag off unless the result is shown.
    fn normalized(mut self) -> Self {
        if !self.shown() {
            self.tooltip_hovered = false;
        }
        self
    }

    fn hover(self, owner: bool, tooltip: bool, now: Instant) -> Self {
        let rise = owner && !self.owner_hovered;
        let engaged = owner || self.owner_focused || (tooltip && self.shown());
        let phase = if rise {
            Self::on_rise(self.phase, now)
        } else if engaged {
            self.phase
        } else {
            TooltipPhase::Idle
        };
        Self {
            phase,
            owner_hovered: owner,
            tooltip_hovered: tooltip,
            owner_focused: self.owner_focused,
        }
        .normalized()
    }

    fn focus(self, focused: bool, now: Instant) -> Self {
        let phase = if focused && !self.owner_focused {
            Self::on_rise(self.phase, now)
        } else if !focused && self.owner_focused {
            let engaged = self.owner_hovered || (self.tooltip_hovered && self.shown());
            if engaged {
                self.phase
            } else {
                TooltipPhase::Idle
            }
        } else {
            self.phase
        };
        Self {
            phase,
            owner_focused: focused,
            ..self
        }
        .normalized()
    }

    /// A `since` later than `now` (a caller's timestamps disagreeing, or
    /// a bogus far-future instant) is first clamped to `now`, so a
    /// pending tooltip can never be stranded: repeated hover and focus
    /// reports deliberately keep `since`, so without the clamp a
    /// far-future one would never show while engaged. The cost is that a
    /// merely skewed `since` shows up to that skew early.
    fn tick(self, now: Instant, delay: Duration) -> Self {
        match self.phase {
            TooltipPhase::Pending { since } => {
                let since = since.min(now);
                let phase = if now.saturating_duration_since(since) >= delay {
                    TooltipPhase::Shown
                } else {
                    TooltipPhase::Pending { since }
                };
                Self { phase, ..self }
            }
            TooltipPhase::Idle | TooltipPhase::Shown | TooltipPhase::Dismissed => self,
        }
    }

    /// `Escape`: returns the next model and whether the key was consumed
    /// — only when a tooltip was *shown*. A pending one is still moved to
    /// `Dismissed` (so it will not pop up later under the same hover or
    /// focus), but nothing was visible, so the key is not consumed and a
    /// caller passes it on (a dialog's own `Escape`, say).
    fn dismiss(self) -> (Self, bool) {
        match self.phase {
            TooltipPhase::Pending { .. } | TooltipPhase::Shown => (
                Self {
                    phase: TooltipPhase::Dismissed,
                    ..self
                }
                .normalized(),
                self.shown(),
            ),
            TooltipPhase::Idle | TooltipPhase::Dismissed => (self, false),
        }
    }

    fn pressed(self) -> Self {
        match self.phase {
            TooltipPhase::Idle => self,
            TooltipPhase::Pending { .. } | TooltipPhase::Shown | TooltipPhase::Dismissed => Self {
                phase: TooltipPhase::Dismissed,
                ..self
            }
            .normalized(),
        }
    }
}

/// The token-derived sizes a shown tooltip's layout needs, resolved once
/// at [`Tooltip::new`] — later mutators take no `&Scales`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct TooltipMetrics {
    pad_v: f32,
    pad_h: f32,
    min_height: f32,
}

/// A tooltip attached to one owner widget — see this module's own doc
/// comment for the transition table, the accessibility shape, and what
/// it deliberately does not do.
///
/// Deliberately not `Clone`: a copy would track the same node id, and
/// either copy hiding would remove the other's node. One controller per
/// owner.
#[derive(Debug, PartialEq)]
pub struct Tooltip {
    owner: WidgetId,
    text: String,
    delay: Duration,
    metrics: TooltipMetrics,
    model: Model,
    node: Option<WidgetId>,
}

/// Whether `kind` may own a tooltip — see "Allowed owners" in this
/// module's doc comment. An exhaustive match, so a new [`WidgetKind`]
/// fails to compile here until someone decides.
fn allowed_owner(kind: &WidgetKind) -> bool {
    match kind {
        WidgetKind::Button(_)
        | WidgetKind::Checkbox(_)
        | WidgetKind::Slider(_)
        | WidgetKind::ColorSwatch(_)
        | WidgetKind::Scrollbar(_)
        | WidgetKind::Tab(_) => true,
        WidgetKind::Container
        | WidgetKind::TextField(_)
        | WidgetKind::CommandPalette(_)
        | WidgetKind::ListRow(_)
        | WidgetKind::TreeItem(_)
        | WidgetKind::Panel
        | WidgetKind::Dialog
        | WidgetKind::Dropdown(_)
        | WidgetKind::DropdownList
        | WidgetKind::TabBar(_)
        | WidgetKind::Tooltip
        | WidgetKind::Menu(_)
        | WidgetKind::MenuSeparator
        | WidgetKind::ColorPicker(_)
        | WidgetKind::ColorPickerPart(_)
        | WidgetKind::CurveEditor(_)
        | WidgetKind::CurveEditorPoint(_) => false,
    }
}

fn tooltip_node(text: &str) -> Node {
    let mut node = Node::new(Role::Tooltip);
    node.set_label(text.to_owned());
    node
}

/// Hangs directly below the owner (`top: 100%`, `left: 0`), as wide as
/// the owner, at least one `typography.size.xs` line plus `spacing.xxs`
/// above and below tall, padded `spacing.xxs` vertically and
/// `spacing.xs` horizontally — the mockup's `.tooltip` padding.
/// `Position::Absolute`, so it takes no part in the owner's own layout:
/// the owner's size is unchanged by showing it.
fn tooltip_style(metrics: TooltipMetrics) -> Style {
    Style {
        position: Position::Absolute,
        inset: LayoutRect {
            left: zero(),
            right: auto(),
            top: percent(1.0_f32),
            bottom: auto(),
        },
        size: Size {
            width: percent(1.0_f32),
            height: auto(),
        },
        min_size: Size {
            width: auto(),
            height: length(metrics.min_height),
        },
        padding: LayoutRect {
            left: length(metrics.pad_h),
            right: length(metrics.pad_h),
            top: length(metrics.pad_v),
            bottom: length(metrics.pad_v),
        },
        ..Default::default()
    }
}

impl Tooltip {
    /// A new, `Idle` tooltip for `owner`, showing `text` after `delay`.
    /// Adds nothing to the tree until it is shown.
    ///
    /// # Errors
    ///
    /// Returns [`WidgetError::UnknownWidget`] if `owner` doesn't exist,
    /// or [`WidgetError::WrongWidgetKind`] if it is not one of the
    /// allowed owner kinds (see this module's doc comment).
    pub fn new(
        tree: &WidgetTree<WidgetKind>,
        owner: WidgetId,
        scales: &Scales,
        text: impl Into<String>,
        delay: Duration,
    ) -> Result<Self, WidgetError> {
        let kind = tree
            .payload(owner)
            .ok_or(WidgetError::UnknownWidget(owner))?;
        if !allowed_owner(kind) {
            return Err(WidgetError::WrongWidgetKind(owner));
        }
        let pad_v = spacing(scales.spacing.xxs);
        Ok(Self {
            owner,
            text: text.into(),
            delay,
            metrics: TooltipMetrics {
                pad_v,
                pad_h: spacing(scales.spacing.xs),
                min_height: type_size(scales.typography.size.xs) + pad_v * 2.0,
            },
            model: Model::IDLE,
            node: None,
        })
    }

    #[must_use]
    pub fn owner(&self) -> WidgetId {
        self.owner
    }

    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    #[must_use]
    pub fn delay(&self) -> Duration {
        self.delay
    }

    #[must_use]
    pub fn phase(&self) -> TooltipPhase {
        self.model.phase
    }

    /// The shown tooltip's own widget id — `Some` exactly while
    /// [`TooltipPhase::Shown`]. It is a popover root, so
    /// [`crate::hit_test`] reaches it: a caller wanting the hoverable
    /// behaviour checks whether a hit's
    /// [`WidgetTree::popover_root_of`] is this id and reports the result
    /// through [`Self::set_hover`]'s `tooltip`.
    #[must_use]
    pub fn node(&self) -> Option<WidgetId> {
        self.node
    }

    /// When a pending tooltip is due to show — `since + delay` while
    /// [`TooltipPhase::Pending`], `None` otherwise, and `None` if that
    /// sum overflows `Instant` (such a tooltip never shows). A caller
    /// schedules its next [`Self::tick`] for this instant.
    #[must_use]
    pub fn next_deadline(&self) -> Option<Instant> {
        match self.model.phase {
            TooltipPhase::Pending { since } => since.checked_add(self.delay),
            TooltipPhase::Idle | TooltipPhase::Shown | TooltipPhase::Dismissed => None,
        }
    }

    /// Reports, level-triggered, whether the pointer is over the owner
    /// and whether it is over the shown tooltip. Idempotent.
    ///
    /// # Errors
    ///
    /// [`WidgetError::UnknownWidget`] if the owner no longer exists (see
    /// "Robustness" in this module's doc comment).
    pub fn set_hover(
        &mut self,
        tree: &mut WidgetTree<WidgetKind>,
        owner: bool,
        tooltip: bool,
        now: Instant,
    ) -> Result<(), WidgetError> {
        let next = self.model.hover(owner, tooltip, now);
        self.commit(tree, next)
    }

    /// Reports whether the owner has keyboard focus. Idempotent.
    ///
    /// # Errors
    ///
    /// As [`Self::set_hover`].
    pub fn set_owner_focused(
        &mut self,
        tree: &mut WidgetTree<WidgetKind>,
        focused: bool,
        now: Instant,
    ) -> Result<(), WidgetError> {
        let next = self.model.focus(focused, now);
        self.commit(tree, next)
    }

    /// Advances time: a pending tooltip whose delay has elapsed at `now`
    /// (`elapsed >= delay`, so exactly at the deadline shows) is shown.
    ///
    /// # Errors
    ///
    /// As [`Self::set_hover`].
    pub fn tick(
        &mut self,
        tree: &mut WidgetTree<WidgetKind>,
        now: Instant,
    ) -> Result<(), WidgetError> {
        let next = self.model.tick(now, self.delay);
        self.commit(tree, next)
    }

    /// `Escape`: hides a pending or shown tooltip until a fresh hover or
    /// focus rise. Returns whether the key was consumed — `true` only
    /// when a tooltip was actually *shown*, so a caller lets an
    /// unconsumed `Escape` through to whatever else wants it (closing a
    /// dialog, cancelling a drag). A merely pending tooltip is still
    /// cancelled, but returns `false`: nothing was visible, and a
    /// keyboard user who tabs to a button inside a dialog and presses
    /// `Escape` within the delay must reach the dialog.
    ///
    /// # Errors
    ///
    /// As [`Self::set_hover`].
    pub fn dismiss(&mut self, tree: &mut WidgetTree<WidgetKind>) -> Result<bool, WidgetError> {
        let (next, consumed) = self.model.dismiss();
        self.commit(tree, next)?;
        Ok(consumed)
    }

    /// The owner was pressed (clicked, activated): hides a pending or
    /// shown tooltip, the same sticky `Dismissed` as [`Self::dismiss`].
    ///
    /// # Errors
    ///
    /// As [`Self::set_hover`].
    pub fn owner_pressed(&mut self, tree: &mut WidgetTree<WidgetKind>) -> Result<(), WidgetError> {
        let next = self.model.pressed();
        self.commit(tree, next)
    }

    /// Replaces the tooltip's text. While shown, the node is relabelled —
    /// compared whole, so identical text touches nothing and costs no
    /// damage.
    ///
    /// # Errors
    ///
    /// As [`Self::set_hover`]. If a tree operation on a live owner fails
    /// (unreachable today, see "Robustness"), the text is left unchanged.
    /// If the owner was removed from outside, the controller resets as
    /// every other call's does and the *new* text is kept — there is no
    /// tree left to disagree with it.
    pub fn set_text(
        &mut self,
        tree: &mut WidgetTree<WidgetKind>,
        text: impl Into<String>,
    ) -> Result<(), WidgetError> {
        let previous = std::mem::replace(&mut self.text, text.into());
        let result = self.commit(tree, self.model);
        if result.is_err() && tree.contains(self.owner) {
            // Only a failed tree operation on a live owner reaches here
            // (unreachable today); keep the old text so the controller
            // and tree still agree. An owner removed from outside has
            // already reset the controller, and the new text is kept.
            self.text = previous;
        }
        result
    }

    /// Sets the show delay. Takes effect from the next [`Self::tick`]
    /// (a pending tooltip keeps its `since`, so a shorter delay can show
    /// it on that tick).
    pub fn set_delay(&mut self, delay: Duration) {
        self.delay = delay;
    }

    /// Removes the tooltip's node (if shown) and returns to `Idle` with
    /// every flag cleared — the next hover or focus report is a fresh
    /// rise. Call before dropping a controller whose owner stays alive.
    ///
    /// # Errors
    ///
    /// As [`Self::set_hover`].
    pub fn detach(&mut self, tree: &mut WidgetTree<WidgetKind>) -> Result<(), WidgetError> {
        self.commit(tree, Model::IDLE)
    }

    /// Makes the tree match `next` and then commits it — or, if any tree
    /// operation fails, commits nothing (except the owner-gone reset
    /// described in this module's doc comment).
    fn commit(
        &mut self,
        tree: &mut WidgetTree<WidgetKind>,
        next: Model,
    ) -> Result<(), WidgetError> {
        if !tree.contains(self.owner) {
            self.model = Model::IDLE;
            self.node = None;
            return Err(WidgetError::UnknownWidget(self.owner));
        }
        let node = if next.shown() {
            Some(self.ensure_node(tree)?)
        } else {
            if let Some(id) = self.node
                && self.is_own_node(tree, id)
            {
                tree.remove(id)?;
            }
            None
        };
        self.model = next;
        self.node = node;
        Ok(())
    }

    /// Whether `id` is, as far as `tree` can tell, this controller's own
    /// node: a live [`WidgetKind::Tooltip`] child of the owner. Checked
    /// before touching a tracked id at all, because ids are per-tree
    /// (every [`WidgetTree`] numbers from the same start), so a
    /// controller driven against a *different* tree than it was shown in
    /// finds its tracked id naming an unrelated widget there — which it
    /// must neither remove nor rewrite.
    fn is_own_node(&self, tree: &WidgetTree<WidgetKind>, id: WidgetId) -> bool {
        tree.parent(id) == Some(self.owner) && matches!(tree.payload(id), Some(WidgetKind::Tooltip))
    }

    /// The live tooltip node for a shown tooltip: the tracked one if it
    /// is still [its own](Self::is_own_node) (relabelling it if its
    /// accessibility node was overwritten, whole-node compare), or a
    /// freshly inserted one — in which case a tracked id that is alive
    /// but no longer a `Tooltip` child of the owner is left untouched.
    /// Does not touch `self.node` — [`Self::commit`] does, once every
    /// tree operation has succeeded.
    fn ensure_node(&self, tree: &mut WidgetTree<WidgetKind>) -> Result<WidgetId, WidgetError> {
        let expected = tooltip_node(&self.text);
        if let Some(id) = self.node
            && self.is_own_node(tree, id)
        {
            if tree.accessibility(id) != Some(&expected) {
                tree.set_accessibility(id, expected)?;
                tree.mark_dirty(id)?;
            }
            // Idempotent: a no-op (and no damage) when already set, and
            // repairs a flag a caller reset behind this module's back.
            tree.set_layer(id, PaintLayer::Popover)?;
            return Ok(id);
        }
        let id = tree.insert(
            self.owner,
            tooltip_style(self.metrics),
            expected,
            WidgetKind::Tooltip,
        )?;
        // A shown tooltip floats above every base-layer widget (this
        // module's own doc comment). Unreachable failure (the id was just
        // inserted and is never the root), but nothing is left behind.
        if let Err(err) = tree.set_layer(id, PaintLayer::Popover) {
            let _ = tree.remove(id);
            return Err(err);
        }
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use super::{Model, Tooltip, TooltipPhase};
    use crate::WidgetError;
    use crate::tree::{PaintLayer, WidgetId, WidgetTree};
    use crate::widgets::{
        CommandEntry, DialogAction, MenuItem, ScrollbarRange, WidgetKind, insert_button,
        insert_checkbox, insert_color_picker, insert_color_swatch, insert_command_palette,
        insert_curve_editor, insert_dialog, insert_dropdown, insert_scrollbar, insert_slider,
        insert_tab_bar, insert_text_field, insert_tree_item, insert_tree_view, new_tree, open_menu,
        set_dropdown_open, spacing, tab_bar_state, test_scales, type_size,
    };
    use accesskit::{Action, Role};
    use aurora_theme::Scales;
    use std::time::{Duration, Instant};
    use taffy::style_helpers::length;
    use taffy::{FlexDirection, Size, Style};

    const DELAY: Duration = Duration::from_millis(500);

    /// One scripted step of the whole-lifecycle test.
    type Step = Box<dyn Fn(&mut WidgetTree<WidgetKind>, &mut Tooltip)>;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn model(phase: TooltipPhase, owner: bool, tooltip: bool, focused: bool) -> Model {
        Model {
            phase,
            owner_hovered: owner,
            tooltip_hovered: tooltip,
            owner_focused: focused,
        }
    }

    // ---- The pure transition table, every cell --------------------------

    #[test]
    fn an_owner_hover_rise_arms_from_idle_and_dismissed_and_keeps_pending_and_shown() {
        let t0 = Instant::now();
        let now = t0 + ms(40);
        let pending = TooltipPhase::Pending { since: t0 };
        let table = [
            (TooltipPhase::Idle, TooltipPhase::Pending { since: now }),
            (pending, pending),
            (TooltipPhase::Shown, TooltipPhase::Shown),
            (
                TooltipPhase::Dismissed,
                TooltipPhase::Pending { since: now },
            ),
        ];
        for (from, to) in table {
            let next = model(from, false, false, false).hover(true, false, now);
            assert_eq!(next.phase, to, "{from:?}");
            assert!(next.owner_hovered);
        }
    }

    #[test]
    fn losing_all_engagement_returns_every_phase_to_idle() {
        let t0 = Instant::now();
        for from in [
            TooltipPhase::Idle,
            TooltipPhase::Pending { since: t0 },
            TooltipPhase::Shown,
            TooltipPhase::Dismissed,
        ] {
            let next = model(from, true, false, false).hover(false, false, t0 + ms(1));
            assert_eq!(next, Model::IDLE, "{from:?}");
        }
    }

    #[test]
    fn continued_engagement_without_a_rise_changes_no_phase() {
        let t0 = Instant::now();
        let later = t0 + ms(300);
        for from in [
            TooltipPhase::Idle,
            TooltipPhase::Pending { since: t0 },
            TooltipPhase::Shown,
            TooltipPhase::Dismissed,
        ] {
            // Still hovered: no rise.
            let next = model(from, true, false, false).hover(true, false, later);
            assert_eq!(next.phase, from, "hovered {from:?}");
            // Not hovered, but focused: engaged, no rise.
            let next = model(from, false, false, true).hover(false, false, later);
            assert_eq!(next.phase, from, "focused {from:?}");
        }
    }

    #[test]
    fn repeated_hover_reports_do_not_restart_the_delay() {
        let t0 = Instant::now();
        let mut m = Model::IDLE.hover(true, false, t0);
        for n in 1..10 {
            m = m.hover(true, false, t0 + ms(n * 40));
        }
        assert_eq!(m.phase, TooltipPhase::Pending { since: t0 });
    }

    #[test]
    fn a_dismissed_tooltip_stays_dismissed_under_continued_hover_and_focus() {
        let t0 = Instant::now();
        let m = model(TooltipPhase::Dismissed, true, false, true);
        assert_eq!(m.hover(true, false, t0).phase, TooltipPhase::Dismissed);
        assert_eq!(m.focus(true, t0).phase, TooltipPhase::Dismissed);
        assert_eq!(m.tick(t0 + DELAY * 4, DELAY).phase, TooltipPhase::Dismissed);
        // A fresh rise re-arms: leave and re-enter.
        let m = model(TooltipPhase::Dismissed, true, false, false)
            .hover(false, false, t0)
            .hover(true, false, t0 + ms(5));
        assert_eq!(m.phase, TooltipPhase::Pending { since: t0 + ms(5) });
    }

    #[test]
    fn the_tooltip_hover_flag_holds_only_a_shown_tooltip_open() {
        let t0 = Instant::now();
        // Shown: moving from the owner onto the tooltip keeps it shown.
        let m = model(TooltipPhase::Shown, true, false, false).hover(false, true, t0);
        assert_eq!(m.phase, TooltipPhase::Shown);
        assert!(m.tooltip_hovered);
        // Leaving the tooltip too hides it.
        assert_eq!(m.hover(false, false, t0), Model::IDLE);
        // Pending: the tooltip flag counts for nothing and is not stored.
        let pending = TooltipPhase::Pending { since: t0 };
        let m = model(pending, true, false, false).hover(false, true, t0);
        assert_eq!(m, Model::IDLE);
        // Dismissed: likewise.
        let m = model(TooltipPhase::Dismissed, true, false, false).hover(false, true, t0);
        assert_eq!(m, Model::IDLE);
        // Idle: a tooltip flag alone neither arms nor is stored.
        let m = Model::IDLE.hover(false, true, t0);
        assert_eq!(m, Model::IDLE);
    }

    #[test]
    fn a_focus_rise_arms_from_idle_and_dismissed_and_keeps_pending_and_shown() {
        let t0 = Instant::now();
        let now = t0 + ms(7);
        let pending = TooltipPhase::Pending { since: t0 };
        let table = [
            (TooltipPhase::Idle, TooltipPhase::Pending { since: now }),
            (pending, pending),
            (TooltipPhase::Shown, TooltipPhase::Shown),
            (
                TooltipPhase::Dismissed,
                TooltipPhase::Pending { since: now },
            ),
        ];
        for (from, to) in table {
            let next = model(from, false, false, false).focus(true, now);
            assert_eq!(next.phase, to, "{from:?}");
            assert!(next.owner_focused);
        }
    }

    #[test]
    fn a_focus_fall_hides_unless_the_owner_or_shown_tooltip_is_still_hovered() {
        let t0 = Instant::now();
        let pending = TooltipPhase::Pending { since: t0 };
        for from in [
            TooltipPhase::Idle,
            pending,
            TooltipPhase::Shown,
            TooltipPhase::Dismissed,
        ] {
            assert_eq!(
                model(from, false, false, true).focus(false, t0),
                Model::IDLE,
                "{from:?}"
            );
        }
        // Still hovered: unchanged.
        for from in [pending, TooltipPhase::Shown, TooltipPhase::Dismissed] {
            let next = model(from, true, false, true).focus(false, t0);
            assert_eq!(next.phase, from, "{from:?}");
            assert!(!next.owner_focused);
        }
        // A shown tooltip held by the tooltip flag alone stays shown.
        let next = model(TooltipPhase::Shown, false, true, true).focus(false, t0);
        assert_eq!(next.phase, TooltipPhase::Shown);
    }

    #[test]
    fn repeated_focus_reports_change_nothing() {
        let t0 = Instant::now();
        let m = model(TooltipPhase::Pending { since: t0 }, false, false, true);
        assert_eq!(m.focus(true, t0 + ms(100)), m);
        assert_eq!(Model::IDLE.focus(false, t0), Model::IDLE);
    }

    #[test]
    fn tick_shows_exactly_at_the_deadline_and_not_a_nanosecond_before() {
        let t0 = Instant::now();
        let pending = model(TooltipPhase::Pending { since: t0 }, true, false, false);
        let before = pending.tick(t0 + Duration::from_nanos(499_999_999), DELAY);
        assert_eq!(before, pending);
        assert_eq!(pending.tick(t0 + DELAY, DELAY).phase, TooltipPhase::Shown);
        assert_eq!(
            pending.tick(t0 + DELAY * 3, DELAY).phase,
            TooltipPhase::Shown
        );
    }

    #[test]
    fn tick_with_an_earlier_now_clamps_since_to_now_rather_than_panicking() {
        let t0 = Instant::now();
        let since = t0 + ms(1_000);
        let pending = model(TooltipPhase::Pending { since }, true, false, false);
        let clamped = pending.tick(t0 + ms(999), DELAY);
        assert_eq!(
            clamped.phase,
            TooltipPhase::Pending {
                since: t0 + ms(999)
            }
        );
        // The delay now runs from the clamped `since`.
        assert_eq!(
            clamped.tick(t0 + ms(999) + DELAY, DELAY).phase,
            TooltipPhase::Shown
        );
        // With a zero delay, the clamped zero elapsed already meets it.
        assert_eq!(
            pending.tick(t0 + ms(999), Duration::ZERO).phase,
            TooltipPhase::Shown
        );
    }

    /// Red-team RT-6: a bogus far-future `now` on the hover that armed
    /// the delay. Repeated hovers keep `since` by design, so without
    /// `tick`'s clamp the tooltip would stay `Pending` for an hour.
    #[test]
    fn a_far_future_since_self_heals_on_the_next_tick() {
        let (mut tree, _button, mut tooltip) = fixture();
        let t0 = Instant::now();
        ok(tooltip.set_hover(&mut tree, true, false, t0 + ms(3_600_000)));
        for n in 1..5 {
            ok(tooltip.set_hover(&mut tree, true, false, t0 + ms(n * 40)));
        }
        ok(tooltip.tick(&mut tree, t0 + ms(200)));
        assert_eq!(tooltip.next_deadline(), Some(t0 + ms(200) + DELAY));
        ok(tooltip.tick(&mut tree, t0 + ms(200) + DELAY));
        assert_eq!(tooltip.phase(), TooltipPhase::Shown);
        assert_node_iff_shown(&tree, &tooltip);
    }

    #[test]
    fn tick_changes_nothing_outside_pending() {
        let t0 = Instant::now();
        for from in [
            TooltipPhase::Idle,
            TooltipPhase::Shown,
            TooltipPhase::Dismissed,
        ] {
            let m = model(from, true, false, false);
            assert_eq!(m.tick(t0 + DELAY * 10, DELAY), m, "{from:?}");
        }
    }

    #[test]
    fn dismiss_cancels_a_pending_tooltip_but_consumes_escape_only_when_shown() {
        let t0 = Instant::now();
        let table = [
            (TooltipPhase::Idle, TooltipPhase::Idle, false),
            (
                TooltipPhase::Pending { since: t0 },
                TooltipPhase::Dismissed,
                false,
            ),
            (TooltipPhase::Shown, TooltipPhase::Dismissed, true),
            (TooltipPhase::Dismissed, TooltipPhase::Dismissed, false),
        ];
        for (from, to, consumed) in table {
            let (next, used) = model(from, true, true, false).normalized().dismiss();
            assert_eq!((next.phase, used), (to, consumed), "{from:?}");
            assert!(!next.tooltip_hovered, "{from:?}: forced off when not shown");
        }
    }

    #[test]
    fn a_press_dismisses_pending_and_shown_and_leaves_idle_alone() {
        let t0 = Instant::now();
        let table = [
            (TooltipPhase::Idle, TooltipPhase::Idle),
            (TooltipPhase::Pending { since: t0 }, TooltipPhase::Dismissed),
            (TooltipPhase::Shown, TooltipPhase::Dismissed),
            (TooltipPhase::Dismissed, TooltipPhase::Dismissed),
        ];
        for (from, to) in table {
            assert_eq!(
                model(from, true, false, false).pressed().phase,
                to,
                "{from:?}"
            );
        }
    }

    /// Critic TT-1: a keyboard user tabs to a button inside a dialog and
    /// presses `Escape` before the delay elapses. Nothing is visible, so
    /// the key must pass through to the dialog — but the pending tooltip
    /// is still cancelled and must not pop up later under the same focus.
    #[test]
    fn escape_while_only_pending_passes_through_and_still_cancels() {
        let (mut tree, button, mut tooltip) = fixture();
        let t0 = Instant::now();
        ok(tooltip.set_owner_focused(&mut tree, true, t0));
        assert!(
            !ok(tooltip.dismiss(&mut tree)),
            "nothing shown: not consumed"
        );
        assert_eq!(tooltip.phase(), TooltipPhase::Dismissed);
        ok(tooltip.tick(&mut tree, t0 + DELAY * 2));
        assert_eq!(tooltip.phase(), TooltipPhase::Dismissed);
        assert!(tooltip_children(&tree, button).is_empty());
        // Once shown, `Escape` is consumed.
        ok(tooltip.set_owner_focused(&mut tree, false, t0 + ms(1_100)));
        show(&mut tree, &mut tooltip, t0 + ms(1_200));
        assert!(ok(tooltip.dismiss(&mut tree)), "shown: consumed");
    }

    // ---- Tree wrappers ---------------------------------------------------

    fn sized_root() -> Style {
        Style {
            flex_direction: FlexDirection::Column,
            size: Size {
                width: length(200.0_f32),
                height: length(120.0_f32),
            },
            ..Default::default()
        }
    }

    fn ok<T>(result: Result<T, WidgetError>) -> T {
        match result {
            Ok(value) => value,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    /// A tree with one "Apply" button and an `Idle` tooltip on it.
    fn fixture() -> (WidgetTree<WidgetKind>, WidgetId, Tooltip) {
        let (mut tree, root) = new_tree(sized_root());
        let button = ok(insert_button(&mut tree, root, &test_scales(), "Apply"));
        let tooltip = ok(Tooltip::new(
            &tree,
            button,
            &test_scales(),
            "Apply the change",
            DELAY,
        ));
        (tree, button, tooltip)
    }

    /// Hover the owner at `t0` and tick past the delay: shown.
    fn show(tree: &mut WidgetTree<WidgetKind>, tooltip: &mut Tooltip, t0: Instant) -> WidgetId {
        ok(tooltip.set_hover(tree, true, false, t0));
        ok(tooltip.tick(tree, t0 + DELAY));
        assert_eq!(tooltip.phase(), TooltipPhase::Shown);
        match tooltip.node() {
            Some(id) => id,
            None => unreachable!("shown"),
        }
    }

    fn tooltip_children(tree: &WidgetTree<WidgetKind>, owner: WidgetId) -> Vec<WidgetId> {
        tree.children(owner)
            .unwrap_or_default()
            .iter()
            .copied()
            .filter(|&c| matches!(tree.payload(c), Some(WidgetKind::Tooltip)))
            .collect()
    }

    /// The invariant: a node exists under the owner iff the phase is
    /// `Shown`, and `node()` names exactly it.
    fn assert_node_iff_shown(tree: &WidgetTree<WidgetKind>, tooltip: &Tooltip) {
        let children = tooltip_children(tree, tooltip.owner());
        if tooltip.phase() == TooltipPhase::Shown {
            let Some(id) = tooltip.node() else {
                unreachable!("shown without a node");
            };
            assert_eq!(children, vec![id]);
            assert_eq!(tree.parent(id), Some(tooltip.owner()));
        } else {
            assert_eq!(tooltip.node(), None);
            assert!(children.is_empty(), "{:?}: {children:?}", tooltip.phase());
        }
    }

    #[test]
    fn a_new_tooltip_is_idle_and_adds_nothing_to_the_tree() {
        let (tree, button, tooltip) = fixture();
        assert_eq!(tooltip.phase(), TooltipPhase::Idle);
        assert_eq!(tooltip.owner(), button);
        assert_eq!(tooltip.text(), "Apply the change");
        assert_eq!(tooltip.delay(), DELAY);
        assert_eq!(tooltip.next_deadline(), None);
        assert_eq!(tree.children(button).map(<[WidgetId]>::len), Some(0));
        assert_node_iff_shown(&tree, &tooltip);
    }

    /// Every [`WidgetKind`] variant's index, by an exhaustive match — so
    /// a new variant fails to compile here until the allowlist test below
    /// covers it.
    fn kind_index(kind: &WidgetKind) -> usize {
        match kind {
            WidgetKind::Container => 0,
            WidgetKind::Button(_) => 1,
            WidgetKind::Checkbox(_) => 2,
            WidgetKind::TextField(_) => 3,
            WidgetKind::Slider(_) => 4,
            WidgetKind::ColorSwatch(_) => 5,
            WidgetKind::CommandPalette(_) => 6,
            WidgetKind::ListRow(_) => 7,
            WidgetKind::Scrollbar(_) => 8,
            WidgetKind::TreeItem(_) => 9,
            WidgetKind::Panel => 10,
            WidgetKind::Dialog => 11,
            WidgetKind::Dropdown(_) => 12,
            WidgetKind::DropdownList => 13,
            WidgetKind::TabBar(_) => 14,
            WidgetKind::Tab(_) => 15,
            WidgetKind::Tooltip => 16,
            WidgetKind::Menu(_) => 17,
            WidgetKind::MenuSeparator => 18,
            WidgetKind::ColorPicker(_) => 19,
            WidgetKind::ColorPickerPart(_) => 20,
            WidgetKind::CurveEditor(_) => 21,
            WidgetKind::CurveEditorPoint(_) => 22,
        }
    }

    /// The allowlist, restated independently of `allowed_owner` so that a
    /// mutation of one is caught by the other.
    const ALLOWED: [usize; 6] = [1, 2, 4, 5, 8, 15];

    fn all_ids(tree: &WidgetTree<WidgetKind>, id: WidgetId, out: &mut Vec<WidgetId>) {
        out.push(id);
        for &child in tree.children(id).unwrap_or_default() {
            all_ids(tree, child, out);
        }
    }

    /// A real open menu under `root` — one `Menu` and one
    /// `MenuSeparator` (plus a `ListRow` item) for the test below.
    fn open_a_menu(tree: &mut WidgetTree<WidgetKind>, root: WidgetId, scales: &Scales) {
        let items = vec![MenuItem::action("Cut"), MenuItem::separator()];
        ok(open_menu(
            tree,
            root,
            scales,
            "Edit",
            (0.0, 0.0),
            100.0,
            items,
        ));
    }

    /// Builds one real widget of every kind — through each module's own
    /// `insert_*` where one exists, `Panel` raw as `paint.rs`'s own tests
    /// do, a real shown tooltip for `Tooltip`, and a real open menu for
    /// `Menu` and `MenuSeparator`, a real colour picker for
    /// `ColorPicker` and `ColorPickerPart`, and a real curve editor for
    /// `CurveEditor` and `CurveEditorPoint` — then asks
    /// [`Tooltip::new`] about every node in the tree.
    #[test]
    #[allow(clippy::too_many_lines)]
    fn new_rejects_a_missing_owner_and_every_disallowed_kind() {
        let (mut tree, root) = new_tree(sized_root());
        let scales = test_scales();
        let bogus = accesskit::NodeId(999);
        match Tooltip::new(&tree, bogus, &scales, "x", DELAY) {
            Err(WidgetError::UnknownWidget(id)) => assert_eq!(id, bogus),
            other => unreachable!("expected UnknownWidget, got {other:?}"),
        }
        let button = ok(insert_button(&mut tree, root, &scales, "Apply"));
        ok(insert_checkbox(&mut tree, root, &scales, "Lock"));
        ok(insert_text_field(&mut tree, root, &scales, "Name", "x"));
        ok(insert_slider(
            &mut tree, root, &scales, "Opacity", 0.5, 0.0, 1.0,
        ));
        let black = aurora_theme::Color { r: 0, g: 0, b: 0 };
        ok(insert_color_swatch(&mut tree, root, &scales, black));
        ok(insert_command_palette(
            &mut tree,
            root,
            vec![CommandEntry::new("a", "Alpha")],
        ));
        let range = ScrollbarRange {
            min: 0.0,
            max: 100.0,
            page_size: 10.0,
        };
        ok(insert_scrollbar(
            &mut tree,
            root,
            &scales,
            accesskit::Orientation::Vertical,
            None,
            0.0,
            range,
        ));
        let tree_view = ok(insert_tree_view(&mut tree, root, Some("Layers")));
        ok(insert_tree_item(
            &mut tree, tree_view, &scales, "Layer 1", false,
        ));
        ok(tree.insert(
            root,
            Style::default(),
            accesskit::Node::new(Role::Pane),
            WidgetKind::Panel,
        ));
        ok(insert_dialog(
            &mut tree,
            root,
            &scales,
            "Title",
            "Message",
            vec![DialogAction::new("ok", "OK")],
        ));
        let dropdown = ok(insert_dropdown(
            &mut tree,
            root,
            &scales,
            "Blend",
            vec!["Normal".to_owned()],
            Some(0),
        ));
        ok(set_dropdown_open(&mut tree, dropdown, true));
        ok(insert_tab_bar(
            &mut tree,
            root,
            &scales,
            "Panels",
            vec!["A".to_owned(), "B".to_owned()],
            0,
        ));
        open_a_menu(&mut tree, root, &scales);
        let white = aurora_theme::Color {
            r: 255,
            g: 255,
            b: 255,
        };
        ok(insert_color_picker(
            &mut tree, root, &scales, "Colour", white, 64.0,
        ));
        ok(insert_curve_editor(
            &mut tree,
            root,
            &scales,
            "Curve",
            64.0,
            aurora_core::ToneCurve::identity(),
        ));
        let mut shown = ok(Tooltip::new(&tree, button, &scales, "x", DELAY));
        show(&mut tree, &mut shown, Instant::now());

        let mut ids = Vec::new();
        all_ids(&tree, root, &mut ids);
        let mut seen = [false; 23];
        for id in ids {
            let Some(kind) = tree.payload(id) else {
                unreachable!("live");
            };
            let index = kind_index(kind);
            if let Some(slot) = seen.get_mut(index) {
                *slot = true;
            }
            let result = Tooltip::new(&tree, id, &scales, "x", DELAY);
            if ALLOWED.contains(&index) {
                assert!(result.is_ok(), "{kind:?} is an allowed owner: {result:?}");
            } else {
                match result {
                    Err(WidgetError::WrongWidgetKind(got)) => assert_eq!(got, id),
                    other => unreachable!("expected WrongWidgetKind for {kind:?}, got {other:?}"),
                }
            }
        }
        assert_eq!(seen, [true; 23], "every WidgetKind was built and asked");
    }

    #[test]
    fn next_deadline_is_since_plus_delay_while_pending_only() {
        let (mut tree, _button, mut tooltip) = fixture();
        let t0 = Instant::now();
        ok(tooltip.set_hover(&mut tree, true, false, t0));
        assert_eq!(tooltip.next_deadline(), Some(t0 + DELAY));
        tooltip.set_delay(ms(100));
        assert_eq!(tooltip.next_deadline(), Some(t0 + ms(100)));
        ok(tooltip.tick(&mut tree, t0 + ms(100)));
        assert_eq!(tooltip.next_deadline(), None, "shown");
    }

    #[test]
    fn next_deadline_is_none_when_the_sum_overflows() {
        let (mut tree, _button, mut tooltip) = fixture();
        let t0 = Instant::now();
        tooltip.set_delay(Duration::MAX);
        ok(tooltip.set_hover(&mut tree, true, false, t0));
        assert_eq!(tooltip.next_deadline(), None);
        ok(tooltip.tick(&mut tree, t0 + ms(10_000)));
        assert!(matches!(tooltip.phase(), TooltipPhase::Pending { .. }));
    }

    #[test]
    fn a_zero_delay_shows_on_the_first_tick() {
        let (mut tree, _button, mut tooltip) = fixture();
        tooltip.set_delay(Duration::ZERO);
        let t0 = Instant::now();
        ok(tooltip.set_hover(&mut tree, true, false, t0));
        assert!(matches!(tooltip.phase(), TooltipPhase::Pending { .. }));
        ok(tooltip.tick(&mut tree, t0));
        assert_eq!(tooltip.phase(), TooltipPhase::Shown);
        assert_node_iff_shown(&tree, &tooltip);
    }

    #[test]
    fn the_node_exists_exactly_while_shown_across_every_transition() {
        let (mut tree, _button, mut tooltip) = fixture();
        let t0 = Instant::now();
        let steps: [(&str, Step); 10] = [
            (
                "hover",
                Box::new(move |t, tt| ok(tt.set_hover(t, true, false, t0))),
            ),
            (
                "early tick",
                Box::new(move |t, tt| ok(tt.tick(t, t0 + ms(499)))),
            ),
            ("tick", Box::new(move |t, tt| ok(tt.tick(t, t0 + DELAY)))),
            (
                "onto tooltip",
                Box::new(move |t, tt| ok(tt.set_hover(t, false, true, t0 + ms(600)))),
            ),
            ("escape", Box::new(|t, tt| assert!(ok(tt.dismiss(t))))),
            (
                "leave",
                Box::new(move |t, tt| ok(tt.set_hover(t, false, false, t0 + ms(700)))),
            ),
            (
                "focus",
                Box::new(move |t, tt| ok(tt.set_owner_focused(t, true, t0 + ms(800)))),
            ),
            (
                "tick 2",
                Box::new(move |t, tt| ok(tt.tick(t, t0 + ms(1_300)))),
            ),
            ("press", Box::new(|t, tt| ok(tt.owner_pressed(t)))),
            (
                "blur",
                Box::new(move |t, tt| ok(tt.set_owner_focused(t, false, t0 + ms(1_400)))),
            ),
        ];
        let expected = [
            "Pending",
            "Pending",
            "Shown",
            "Shown",
            "Dismissed",
            "Idle",
            "Pending",
            "Shown",
            "Dismissed",
            "Idle",
        ];
        for ((name, step), want) in steps.iter().zip(expected) {
            step(&mut tree, &mut tooltip);
            let got = match tooltip.phase() {
                TooltipPhase::Idle => "Idle",
                TooltipPhase::Pending { .. } => "Pending",
                TooltipPhase::Shown => "Shown",
                TooltipPhase::Dismissed => "Dismissed",
            };
            assert_eq!(got, want, "after {name}");
            assert_node_iff_shown(&tree, &tooltip);
        }
    }

    #[test]
    fn a_reshow_inserts_a_fresh_node_and_the_old_id_is_gone() {
        let (mut tree, _button, mut tooltip) = fixture();
        let t0 = Instant::now();
        let first = show(&mut tree, &mut tooltip, t0);
        ok(tooltip.set_hover(&mut tree, false, false, t0 + ms(600)));
        assert!(!tree.contains(first));
        let second = show(&mut tree, &mut tooltip, t0 + ms(700));
        assert_ne!(first, second);
        assert!(!tree.contains(first));
        assert_node_iff_shown(&tree, &tooltip);
    }

    #[test]
    fn a_shown_tooltip_is_a_labelled_tooltip_role_child_with_no_actions() {
        let (mut tree, button, mut tooltip) = fixture();
        let id = show(&mut tree, &mut tooltip, Instant::now());
        assert_eq!(tree.parent(id), Some(button));
        assert_eq!(tree.payload(id), Some(&WidgetKind::Tooltip));
        let Some(node) = tree.accessibility(id) else {
            unreachable!("live");
        };
        assert_eq!(node.role(), Role::Tooltip);
        assert_eq!(node.label(), Some("Apply the change"));
        for action in [Action::Focus, Action::Click] {
            assert!(!node.supports_action(action), "{action:?}");
        }
        assert!(node.described_by().is_empty());
        assert_eq!(node.description(), None);
    }

    #[test]
    fn showing_and_hiding_never_writes_the_owners_node() {
        let (mut tree, button, mut tooltip) = fixture();
        let before = tree.accessibility(button).cloned();
        let t0 = Instant::now();
        show(&mut tree, &mut tooltip, t0);
        ok(tooltip.set_text(&mut tree, "Something else"));
        assert_eq!(tree.accessibility(button).cloned(), before);
        assert!(ok(tooltip.dismiss(&mut tree)));
        assert_eq!(tree.accessibility(button).cloned(), before);
        let Some(node) = before else {
            unreachable!("live");
        };
        assert!(node.described_by().is_empty());
        assert_eq!(node.description(), None);
    }

    #[test]
    fn set_text_relabels_a_shown_node_and_identical_text_costs_no_damage() {
        let (mut tree, _button, mut tooltip) = fixture();
        let id = show(&mut tree, &mut tooltip, Instant::now());
        tree.compute_layout(200.0, 120.0);
        tree.take_damage();
        ok(tooltip.set_text(&mut tree, "Apply the change"));
        assert_eq!(tree.take_damage(), None);
        assert_eq!(tree.is_dirty(id), Some(false));
        ok(tooltip.set_text(&mut tree, "Apply now"));
        assert_eq!(tooltip.node(), Some(id), "relabelled in place");
        assert_eq!(
            tree.accessibility(id).and_then(accesskit::Node::label),
            Some("Apply now")
        );
        assert_eq!(tree.is_dirty(id), Some(true));
        assert_eq!(tree.take_damage(), tree.bounds(id));
    }

    /// A shown tooltip is a popover root: both hit-testers reach it
    /// outside its owner's bounds, and repeated shows keep the flag.
    #[test]
    fn a_shown_tooltip_is_a_hit_testable_popover() {
        let (mut tree, button, mut tooltip) = fixture();
        let t0 = Instant::now();
        let id = show(&mut tree, &mut tooltip, t0);
        tree.compute_layout(200.0, 120.0);
        assert_eq!(tree.layer(id), Some(PaintLayer::Popover));
        assert_eq!(tree.popover_root_of(id), Some(id));
        assert_eq!(tree.layer(button), Some(PaintLayer::Base));
        let (Some(owner), Some(bounds)) = (tree.bounds(button), tree.bounds(id)) else {
            unreachable!("laid out");
        };
        assert!(bounds.y >= owner.bottom(), "hangs below its owner");
        #[allow(clippy::cast_precision_loss)]
        let centre = (
            bounds.x as f32 + bounds.width as f32 / 2.0,
            bounds.y as f32 + bounds.height as f32 / 2.0,
        );
        let hit = tree.hit_test(centre);
        assert_eq!(hit, Some(id));
        assert_eq!(
            crate::hit_test(&tree, f64::from(centre.0), f64::from(centre.1)),
            Some(id)
        );
        assert!(hit.is_some_and(|h| tree.popover_root_of(h) == tooltip.node()));
        // Re-reporting while shown reuses the node and keeps the flag.
        ok(tooltip.set_hover(&mut tree, true, true, t0 + ms(700)));
        assert_eq!(tooltip.node(), Some(id));
        assert_eq!(tree.layer(id), Some(PaintLayer::Popover));
        // A flag reset behind this module's back is repaired on the next
        // commit that reuses the node.
        ok(tree.set_layer(id, PaintLayer::Base));
        ok(tooltip.set_hover(&mut tree, true, false, t0 + ms(800)));
        assert_eq!(tooltip.node(), Some(id), "reused, not re-inserted");
        assert_eq!(tree.layer(id), Some(PaintLayer::Popover));
    }

    /// A tooltip shown after a dropdown opened stacks above the open
    /// list: it paints after it and wins the hit test where they overlap.
    #[test]
    fn a_tooltip_shown_over_an_open_dropdown_list_stacks_above_it() {
        let (mut tree, root) = new_tree(sized_root());
        let dropdown = ok(insert_dropdown(
            &mut tree,
            root,
            &test_scales(),
            "Blend",
            vec![
                "A".to_owned(),
                "B".to_owned(),
                "C".to_owned(),
                "D".to_owned(),
            ],
            Some(0),
        ));
        let button = ok(insert_button(&mut tree, root, &test_scales(), "Apply"));
        let mut tooltip = ok(Tooltip::new(
            &tree,
            button,
            &test_scales(),
            "Apply the change",
            DELAY,
        ));
        ok(set_dropdown_open(&mut tree, dropdown, true));
        let id = show(&mut tree, &mut tooltip, Instant::now());
        tree.compute_layout(200.0, 120.0);
        let Some(list) = tree.children(dropdown).and_then(|c| c.first().copied()) else {
            unreachable!("open");
        };
        assert_eq!(tree.layer(list), Some(PaintLayer::Popover));
        assert_eq!(tree.popover_roots(), vec![list, id]);
        let (Some(list_bounds), Some(tip)) = (tree.bounds(list), tree.bounds(id)) else {
            unreachable!("laid out");
        };
        let overlap_top = list_bounds.y.max(tip.y);
        let overlap_bottom = list_bounds.bottom().min(tip.bottom());
        assert!(
            overlap_bottom > overlap_top,
            "the fixture must overlap: list {list_bounds:?}, tooltip {tip:?}"
        );
        let order = tree.paint_order();
        assert_eq!(order.last(), Some(&id), "the tooltip is painted last");
        #[allow(clippy::cast_precision_loss)]
        let point = (tip.x as f32 + 2.0, overlap_top as f32 + 1.0);
        assert_eq!(tree.hit_test(point), Some(id));
        assert_eq!(
            crate::hit_test(&tree, f64::from(point.0), f64::from(point.1)),
            Some(id)
        );
    }

    /// The disclosed flip side of creation-order stacking: a tooltip
    /// already showing when a dropdown list opens sits *under* that
    /// list, because the list is created (rebuilt) on open, after the
    /// tooltip. Pinned so a change to the stacking rule is deliberate.
    #[test]
    fn a_dropdown_list_opened_after_a_tooltip_stacks_above_it() {
        let (mut tree, root) = new_tree(sized_root());
        let dropdown = ok(insert_dropdown(
            &mut tree,
            root,
            &test_scales(),
            "Blend",
            vec![
                "A".to_owned(),
                "B".to_owned(),
                "C".to_owned(),
                "D".to_owned(),
            ],
            Some(0),
        ));
        let button = ok(insert_button(&mut tree, root, &test_scales(), "Apply"));
        let mut tooltip = ok(Tooltip::new(
            &tree,
            button,
            &test_scales(),
            "Apply the change",
            DELAY,
        ));
        let id = show(&mut tree, &mut tooltip, Instant::now());
        ok(set_dropdown_open(&mut tree, dropdown, true));
        tree.compute_layout(200.0, 120.0);
        let Some(list) = tree.children(dropdown).and_then(|c| c.first().copied()) else {
            unreachable!("open");
        };
        assert_eq!(tree.popover_roots(), vec![id, list]);
        let (Some(list_bounds), Some(tip)) = (tree.bounds(list), tree.bounds(id)) else {
            unreachable!("laid out");
        };
        let overlap_top = list_bounds.y.max(tip.y);
        let overlap_bottom = list_bounds.bottom().min(tip.bottom());
        assert!(
            overlap_bottom > overlap_top,
            "the fixture must overlap: list {list_bounds:?}, tooltip {tip:?}"
        );
        #[allow(clippy::cast_precision_loss)]
        let point = (tip.x as f32 + 2.0, overlap_top as f32 + 1.0);
        let hit = tree.hit_test(point);
        assert!(
            hit.is_some_and(|hit| tree.popover_root_of(hit) == Some(list)),
            "the later list wins: {hit:?}"
        );
    }

    /// `PaintLayer`'s contract, checked on all three shipped popovers:
    /// every popover root either hides its own overflow or lays every
    /// descendant out inside its own bounds — otherwise an overhang would
    /// paint on top while clicks on it fell through to the base layer.
    #[test]
    fn every_shipped_popover_root_contains_its_descendants() {
        let (mut tree, root) = new_tree(sized_root());
        let dropdown = ok(insert_dropdown(
            &mut tree,
            root,
            &test_scales(),
            "Blend",
            vec!["A".to_owned(), "B".to_owned(), "C".to_owned()],
            Some(0),
        ));
        let button = ok(insert_button(&mut tree, root, &test_scales(), "Apply"));
        let mut tooltip = ok(Tooltip::new(
            &tree,
            button,
            &test_scales(),
            "Apply the change",
            DELAY,
        ));
        ok(set_dropdown_open(&mut tree, dropdown, true));
        let menu = ok(open_menu(
            &mut tree,
            root,
            &test_scales(),
            "Edit",
            (10.0, 10.0),
            80.0,
            vec![
                MenuItem::action("Cut"),
                MenuItem::separator(),
                MenuItem::action("Copy"),
            ],
        ));
        let tip = show(&mut tree, &mut tooltip, Instant::now());
        tree.compute_layout(200.0, 120.0);
        let roots = tree.popover_roots();
        assert_eq!(roots.len(), 3, "list, menu and tooltip: {roots:?}");
        assert!(roots.contains(&menu) && roots.contains(&tip));
        let mut checked = 0;
        for popover in roots {
            let Some(outer) = tree.bounds(popover) else {
                unreachable!("live");
            };
            let mut stack: Vec<WidgetId> = tree.children(popover).unwrap_or(&[]).to_vec();
            while let Some(id) = stack.pop() {
                stack.extend_from_slice(tree.children(id).unwrap_or(&[]));
                let Some(inner) = tree.bounds(id) else {
                    unreachable!("live");
                };
                if inner.width == 0 || inner.height == 0 {
                    continue;
                }
                checked += 1;
                assert!(
                    inner.x >= outer.x
                        && inner.y >= outer.y
                        && inner.x + i64::from(inner.width) <= outer.x + i64::from(outer.width)
                        && inner.bottom() <= outer.bottom(),
                    "{id:?} {inner:?} escapes popover {popover:?} {outer:?}"
                );
            }
        }
        assert!(
            checked >= 6,
            "the list's and menu's rows were checked: {checked}"
        );
    }

    #[test]
    fn set_text_while_hidden_only_changes_the_text() {
        let (mut tree, button, mut tooltip) = fixture();
        ok(tooltip.set_text(&mut tree, "Later"));
        assert_eq!(tooltip.text(), "Later");
        assert!(tooltip_children(&tree, button).is_empty());
        let id = show(&mut tree, &mut tooltip, Instant::now());
        assert_eq!(
            tree.accessibility(id).and_then(accesskit::Node::label),
            Some("Later")
        );
    }

    #[test]
    fn hiding_marks_the_vacated_rect_dirty_and_showing_damages_on_layout() {
        let (mut tree, _button, mut tooltip) = fixture();
        let t0 = Instant::now();
        tree.compute_layout(200.0, 120.0);
        tree.take_damage();
        let id = show(&mut tree, &mut tooltip, t0);
        assert_eq!(tree.take_damage(), None, "no bounds until layout");
        tree.compute_layout(200.0, 120.0);
        let Some(bounds) = tree.bounds(id) else {
            unreachable!("laid out");
        };
        let Some(damage) = tree.take_damage() else {
            unreachable!("layout damages the new node");
        };
        assert_eq!(
            damage.union(&bounds),
            damage,
            "{damage:?} covers {bounds:?}"
        );
        ok(tooltip.set_hover(&mut tree, false, false, t0 + ms(600)));
        assert_eq!(tree.take_damage(), Some(bounds));
    }

    #[test]
    fn repeated_identical_reports_while_shown_produce_no_damage() {
        let (mut tree, _button, mut tooltip) = fixture();
        let t0 = Instant::now();
        let id = show(&mut tree, &mut tooltip, t0);
        tree.compute_layout(200.0, 120.0);
        tree.take_damage();
        ok(tooltip.set_hover(&mut tree, true, false, t0 + ms(900)));
        ok(tooltip.tick(&mut tree, t0 + ms(950)));
        ok(tooltip.set_owner_focused(&mut tree, false, t0 + ms(990)));
        assert_eq!(tooltip.node(), Some(id));
        assert_eq!(tree.take_damage(), None);
    }

    #[test]
    fn an_owner_removed_from_outside_errors_resets_and_never_panics() {
        let (mut tree, button, mut tooltip) = fixture();
        let t0 = Instant::now();
        let id = show(&mut tree, &mut tooltip, t0);
        ok(tree.remove(button));
        assert!(!tree.contains(id), "the subtree went with the owner");
        match tooltip.tick(&mut tree, t0 + ms(900)) {
            Err(WidgetError::UnknownWidget(got)) => assert_eq!(got, button),
            other => unreachable!("expected UnknownWidget, got {other:?}"),
        }
        assert_eq!(tooltip.phase(), TooltipPhase::Idle);
        assert_eq!(tooltip.node(), None);
        for result in [
            tooltip.set_hover(&mut tree, true, false, t0),
            tooltip.set_owner_focused(&mut tree, true, t0),
            tooltip.owner_pressed(&mut tree),
            tooltip.set_text(&mut tree, "x"),
            tooltip.detach(&mut tree),
            tooltip.dismiss(&mut tree).map(|_| ()),
        ] {
            assert!(matches!(result, Err(WidgetError::UnknownWidget(_))));
            assert_eq!(tooltip.phase(), TooltipPhase::Idle);
        }
    }

    #[test]
    fn a_tooltip_node_removed_from_outside_is_reinserted_or_forgotten() {
        let (mut tree, button, mut tooltip) = fixture();
        let t0 = Instant::now();
        let id = show(&mut tree, &mut tooltip, t0);
        ok(tree.remove(id));
        // Still shown: the next call reinserts.
        ok(tooltip.set_hover(&mut tree, true, false, t0 + ms(600)));
        let Some(fresh) = tooltip.node() else {
            unreachable!("shown");
        };
        assert_ne!(fresh, id);
        assert_node_iff_shown(&tree, &tooltip);
        // Removed again, then hidden: the dead id is simply forgotten.
        ok(tree.remove(fresh));
        ok(tooltip.set_hover(&mut tree, false, false, t0 + ms(700)));
        assert_eq!(tooltip.node(), None);
        assert!(tooltip_children(&tree, button).is_empty());
    }

    #[test]
    fn an_overwritten_tooltip_accessibility_node_is_repaired_in_place() {
        let (mut tree, _button, mut tooltip) = fixture();
        let t0 = Instant::now();
        let id = show(&mut tree, &mut tooltip, t0);
        tree.compute_layout(200.0, 120.0);
        tree.take_damage();
        ok(tree.set_accessibility(id, accesskit::Node::new(Role::GenericContainer)));
        ok(tooltip.set_hover(&mut tree, true, false, t0 + ms(600)));
        assert_eq!(tooltip.node(), Some(id));
        assert_eq!(tree.payload(id), Some(&WidgetKind::Tooltip));
        assert_eq!(
            tree.accessibility(id).map(accesskit::Node::role),
            Some(Role::Tooltip)
        );
        assert_eq!(tree.is_dirty(id), Some(true), "a repair damages");
    }

    /// A tracked node whose payload is no longer `Tooltip` is not this
    /// controller's to touch any more: a shown call inserts a fresh node
    /// beside it, and a hide leaves it where it is.
    #[test]
    fn a_tracked_node_with_a_foreign_payload_is_left_alone() {
        let (mut tree, button, mut tooltip) = fixture();
        let t0 = Instant::now();
        let id = show(&mut tree, &mut tooltip, t0);
        if let Some(kind) = tree.payload_mut(id) {
            *kind = WidgetKind::Container;
        }
        ok(tooltip.set_hover(&mut tree, true, false, t0 + ms(600)));
        let Some(fresh) = tooltip.node() else {
            unreachable!("shown");
        };
        assert_ne!(fresh, id);
        assert_eq!(tree.payload(id), Some(&WidgetKind::Container));
        assert_node_iff_shown(&tree, &tooltip);
        ok(tooltip.set_hover(&mut tree, false, false, t0 + ms(700)));
        assert!(tree.contains(id) && !tree.contains(fresh));
        assert_eq!(tree.parent(id), Some(button));
    }

    /// Red-team RT-1: ids are per-tree, so a controller shown in tree A
    /// and then handed tree B finds its tracked id naming an unrelated
    /// widget there — under a same-id owner. Hiding must not remove it.
    #[test]
    fn driven_against_another_tree_a_hide_never_removes_an_unrelated_widget() {
        let (mut tree_a, button_a, mut tooltip) = fixture();
        let t0 = Instant::now();
        let id = show(&mut tree_a, &mut tooltip, t0);
        let (mut tree_b, root_b) = new_tree(sized_root());
        let button_b = ok(insert_button(&mut tree_b, root_b, &test_scales(), "Other"));
        let mut icon = accesskit::Node::new(Role::Image);
        icon.set_label("Icon");
        let foreign = ok(tree_b.insert(button_b, Style::default(), icon, WidgetKind::Container));
        assert_eq!((button_b, foreign), (button_a, id), "same ids, other tree");
        ok(tooltip.set_hover(&mut tree_b, false, false, t0 + ms(600)));
        assert!(tree_b.contains(foreign), "the unrelated widget survives");
        assert_eq!(tree_b.parent(foreign), Some(button_b));
        assert_eq!(tooltip.node(), None);
    }

    /// Red-team M6's hazard: in tree B the tracked id *is* a `Tooltip`
    /// node — someone else's — but not under this controller's owner.
    /// The parent check, not just the payload check, must leave it alone.
    #[test]
    fn driven_against_another_tree_a_tooltip_under_another_parent_is_left_alone() {
        let (mut tree_a, button_a, mut tooltip) = fixture();
        let t0 = Instant::now();
        let id = show(&mut tree_a, &mut tooltip, t0);
        let (mut tree_b, root_b) = new_tree(sized_root());
        let other_owner = ok(insert_button(&mut tree_b, root_b, &test_scales(), "Other"));
        let theirs = ok(tree_b.insert(
            root_b,
            Style::default(),
            accesskit::Node::new(Role::Tooltip),
            WidgetKind::Tooltip,
        ));
        assert_eq!(
            (other_owner, theirs),
            (button_a, id),
            "same ids, other tree"
        );
        assert_ne!(tree_b.parent(theirs), Some(other_owner));
        ok(tooltip.set_hover(&mut tree_b, true, false, t0 + ms(600)));
        assert_eq!(
            tree_b.accessibility(theirs),
            Some(&accesskit::Node::new(Role::Tooltip)),
            "not relabelled"
        );
        assert_ne!(tooltip.node(), Some(theirs));
        ok(tooltip.set_hover(&mut tree_b, false, false, t0 + ms(700)));
        assert!(tree_b.contains(theirs), "not removed");
    }

    /// The same mix-up on the show path: the tracked id names an
    /// unrelated widget under a same-id owner in tree B. It must not be
    /// rewritten into a tooltip; a fresh node goes in beside it.
    #[test]
    fn driven_against_another_tree_a_show_never_rewrites_an_unrelated_widget() {
        let (mut tree_a, button_a, mut tooltip) = fixture();
        let t0 = Instant::now();
        let id = show(&mut tree_a, &mut tooltip, t0);
        let (mut tree_b, root_b) = new_tree(sized_root());
        let button_b = ok(insert_button(&mut tree_b, root_b, &test_scales(), "Other"));
        let mut icon = accesskit::Node::new(Role::Image);
        icon.set_label("Icon");
        let foreign = ok(tree_b.insert(
            button_b,
            Style::default(),
            icon.clone(),
            WidgetKind::Container,
        ));
        assert_eq!((button_b, foreign), (button_a, id), "same ids, other tree");
        ok(tooltip.set_hover(&mut tree_b, true, false, t0 + ms(600)));
        assert_eq!(tree_b.payload(foreign), Some(&WidgetKind::Container));
        assert_eq!(tree_b.accessibility(foreign), Some(&icon));
        assert_ne!(tooltip.node(), Some(foreign));
        assert_node_iff_shown(&tree_b, &tooltip);
    }

    /// Red-team RT-7: empty text is accepted and shows an empty
    /// `Role::Tooltip` — the caller's responsibility, as for `Dropdown`
    /// and `TabBar` labels. Pinned so a change to that is deliberate.
    #[test]
    fn empty_text_is_accepted_and_shows_an_empty_tooltip() {
        let (mut tree, root) = new_tree(sized_root());
        let button = ok(insert_button(&mut tree, root, &test_scales(), "Apply"));
        let mut tooltip = ok(Tooltip::new(&tree, button, &test_scales(), "", DELAY));
        let id = show(&mut tree, &mut tooltip, Instant::now());
        let Some(node) = tree.accessibility(id) else {
            unreachable!("live");
        };
        assert_eq!(node.role(), Role::Tooltip);
        assert_eq!(node.label(), Some(""));
    }

    #[test]
    fn detach_removes_the_node_and_clears_every_flag() {
        let (mut tree, button, mut tooltip) = fixture();
        let t0 = Instant::now();
        show(&mut tree, &mut tooltip, t0);
        ok(tooltip.set_owner_focused(&mut tree, true, t0));
        ok(tooltip.detach(&mut tree));
        assert_eq!(tooltip.phase(), TooltipPhase::Idle);
        assert!(tooltip_children(&tree, button).is_empty());
        // The next report is a fresh rise.
        ok(tooltip.set_hover(&mut tree, true, false, t0 + ms(10)));
        assert_eq!(
            tooltip.phase(),
            TooltipPhase::Pending { since: t0 + ms(10) }
        );
    }

    #[test]
    fn a_tab_owners_tooltip_vanishes_with_a_tab_bar_repair() {
        let (mut tree, root) = new_tree(sized_root());
        let scales = test_scales();
        let bar = ok(insert_tab_bar(
            &mut tree,
            root,
            &scales,
            "Panels",
            vec!["A".to_owned(), "B".to_owned()],
            0,
        ));
        let tabs = ok(tab_bar_state(&tree, bar)).tabs().to_vec();
        let (Some(&first), Some(&second)) = (tabs.first(), tabs.get(1)) else {
            unreachable!("two tabs");
        };
        let mut tooltip = ok(Tooltip::new(&tree, first, &scales, "Layers", DELAY));
        let t0 = Instant::now();
        let id = show(&mut tree, &mut tooltip, t0);
        // Damage the bar from outside: the next call rebuilds every tab.
        ok(tree.remove(second));
        ok(crate::widgets::set_tab_bar_disabled(&mut tree, bar, false));
        assert!(!tree.contains(first) && !tree.contains(id));
        assert!(matches!(
            tooltip.tick(&mut tree, t0 + ms(900)),
            Err(WidgetError::UnknownWidget(_))
        ));
        assert_eq!(tooltip.node(), None);
    }

    // ---- Consumer --------------------------------------------------------

    /// The owner's accessible name, as `accesskit_consumer` computes it.
    fn consumer_name(tree: &WidgetTree<WidgetKind>, owner: WidgetId) -> Option<String> {
        let consumer = accesskit_consumer::Tree::new(tree.accessibility_update(owner), true);
        let root = consumer.state().root();
        let button = root.children().find(|n| n.role() == Role::Button);
        button.and_then(|n| n.label())
    }

    #[test]
    fn accesskit_consumer_reads_the_tooltip_as_documented() {
        let (mut tree, button, mut tooltip) = fixture();
        show(&mut tree, &mut tooltip, Instant::now());
        let consumer = accesskit_consumer::Tree::new(tree.accessibility_update(button), true);
        let root = consumer.state().root();
        let Some(owner) = root.children().find(|n| n.role() == Role::Button) else {
            unreachable!("the button is the root's child");
        };
        let children: Vec<_> = owner.children().collect();
        assert_eq!(children.len(), 1);
        let Some(tip) = children.first() else {
            unreachable!("one child");
        };
        assert_eq!(tip.role(), Role::Tooltip);
        assert_eq!(tip.label().as_deref(), Some("Apply the change"));
        assert!(tip.should_have_read_only_state_by_default());
        let filter = accesskit_consumer::common_filter;
        assert!(!tip.is_focusable(&filter));
        assert!(!tip.is_clickable(&filter));
        assert_eq!(owner.description(), None);
    }

    #[test]
    fn a_labelled_owner_keeps_its_name_while_the_tooltip_is_shown() {
        let (mut tree, button, mut tooltip) = fixture();
        assert_eq!(consumer_name(&tree, button).as_deref(), Some("Apply"));
        show(&mut tree, &mut tooltip, Instant::now());
        assert_eq!(consumer_name(&tree, button).as_deref(), Some("Apply"));
    }

    /// The consumer derives an unlabelled button's name from its
    /// descendants, but `descendant_label_filter` excludes a `Tooltip`
    /// subtree — so the tooltip's text never becomes the owner's name.
    #[test]
    fn an_unlabelled_owner_does_not_take_its_name_from_the_tooltip() {
        let (mut tree, button, mut tooltip) = fixture();
        let mut bare = accesskit::Node::new(Role::Button);
        bare.add_action(Action::Focus);
        bare.add_action(Action::Click);
        ok(tree.set_accessibility(button, bare));
        assert_eq!(consumer_name(&tree, button), None);
        show(&mut tree, &mut tooltip, Instant::now());
        assert_eq!(consumer_name(&tree, button), None);
        // Positive control: the descendant-name path is live — a labelled
        // `Image` child *does* name the button, and the tooltip beside it
        // still adds nothing.
        let mut icon = accesskit::Node::new(Role::Image);
        icon.set_label("Checkmark");
        ok(tree.insert(button, Style::default(), icon, WidgetKind::Container));
        assert_eq!(consumer_name(&tree, button).as_deref(), Some("Checkmark"));
    }

    // ---- Layout ----------------------------------------------------------

    #[test]
    fn the_tooltip_hangs_below_its_owner_as_wide_as_it_without_resizing_it() {
        let (mut tree, button, mut tooltip) = fixture();
        tree.compute_layout(200.0, 120.0);
        let before = tree.bounds(button);
        let id = show(&mut tree, &mut tooltip, Instant::now());
        tree.compute_layout(200.0, 120.0);
        let (Some(owner), Some(tip)) = (tree.bounds(button), tree.bounds(id)) else {
            unreachable!("laid out");
        };
        assert_eq!(Some(owner), before, "the owner's own size is unchanged");
        assert_eq!(tip.y, owner.bottom());
        assert_eq!(tip.x, owner.x);
        assert_eq!(tip.width, owner.width);
        let scales = test_scales();
        let min = type_size(scales.typography.size.xs) + 2.0 * spacing(scales.spacing.xxs);
        assert!(f64::from(tip.height) >= f64::from(min), "{tip:?} vs {min}");
    }

    /// Red-team RT-3 / critic TT-6, pinned rather than fixed: on a small
    /// owner the owner-width stand-in leaves a tooltip only as wide as its
    /// own horizontal padding — the module doc's "nearly unusable".
    #[test]
    fn on_a_checkbox_owner_the_tooltip_is_only_its_own_padding_wide() {
        let (mut tree, root) = new_tree(sized_root());
        let scales = test_scales();
        let checkbox = ok(insert_checkbox(&mut tree, root, &scales, "Lock"));
        let mut tooltip = ok(Tooltip::new(&tree, checkbox, &scales, "Lock layer", DELAY));
        let id = show(&mut tree, &mut tooltip, Instant::now());
        tree.compute_layout(200.0, 120.0);
        let (Some(owner), Some(tip)) = (tree.bounds(checkbox), tree.bounds(id)) else {
            unreachable!("laid out");
        };
        assert_eq!(owner.width, scales.typography.size.md);
        assert_eq!(tip.width, 2 * scales.spacing.xs);
    }
}
