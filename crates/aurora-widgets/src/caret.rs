//! The text caret's blink (0.139.0).
//!
//! A caret is drawn by whichever widget [`crate::paint_widget_ops_frame`]
//! is handed as its caret owner: a focused, enabled text field, or the
//! command palette's query strip while focus is anywhere inside the
//! palette ([`crate::text_runs`]). Blinking it needs no paint API of its
//! own — the frame walker passes `None` as that owner during a hidden
//! half-period — so this module is only the clock: [`CaretBlink`] says
//! whether the caret is in a visible half-period at an instant, and when
//! the next flip is due, so an event loop can sleep until exactly then.
//!
//! **What restarts the blink.** Every change a user would expect to show
//! the caret solid again — a typed or deleted character, a caret move, a
//! selection change, an IME preedit, a pointer click, an accessibility
//! action, focus moving to another field — changes the
//! [`CaretSignature`] [`caret_signature`] reads off the tree, and
//! [`CaretBlink::observe`] restarts the clock on any change. That covers
//! every input path without a reset call at each one, which is the point:
//! a path that forgets its call would leave a caret that can vanish right
//! after the user acts on it.
//!
//! **What never blinks.** No caret owner (nothing focused that draws a
//! caret, or a disabled field) and reduced motion both mean "always
//! visible" and "no next flip" — so an idle window with no focused field,
//! or one whose user asked the OS to minimize motion, never wakes the
//! event loop for the caret at all.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::time::{Duration, Instant};

use crate::tree::{WidgetId, WidgetTree};
use crate::widgets::WidgetKind;

/// How long the caret stays in each visible or hidden half-period —
/// 530 ms, the long-standing Windows `GetCaretBlinkTime` default and the
/// common desktop convention. **Not a token**: `design/tokens/scales.toml`
/// has no motion value for a repeating cadence (its `motion.duration`
/// values are one-shot transition lengths, and zeroing them under reduced
/// motion is the wrong semantics for a blink, which reduced motion stops
/// outright instead). Same status as [`crate::CARET_WIDTH`]: flagged to
/// the design owner (Cahya, PRD FR-027 *Ownership*) rather than invented
/// as a token here. Nor is it the user's OS preference yet — reading the
/// platform's own blink rate (and its "don't blink" setting) is
/// platform-specific follow-on work.
pub const CARET_BLINK_INTERVAL: Duration = Duration::from_millis(530);

/// Everything about a caret whose change restarts the blink: its owner
/// (the focused widget), cursor, selection anchor, and a hash of the
/// text it sits in (content plus any IME composition, for a text field;
/// the query, for the command palette). Read by [`caret_signature`];
/// compared, never inspected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaretSignature {
    owner: WidgetId,
    cursor: usize,
    anchor: Option<usize>,
    text: u64,
}

/// The [`CaretSignature`] of the caret `focused` would draw this frame,
/// or `None` when it would draw none: nothing focused, a disabled text
/// field, or a focused widget that is neither a text field nor inside a
/// command palette — the same rule [`crate::text_runs`] applies to decide
/// whether a caret is drawn at all.
#[must_use]
pub fn caret_signature(
    tree: &WidgetTree<WidgetKind>,
    focused: Option<WidgetId>,
) -> Option<CaretSignature> {
    let focused = focused?;
    if let Some(WidgetKind::TextField(state)) = tree.payload(focused) {
        if state.disabled {
            return None;
        }
        let mut hasher = DefaultHasher::new();
        state.content.hash(&mut hasher);
        if let Some(composition) = state.composition.as_ref() {
            composition.text.hash(&mut hasher);
            composition.target_range.hash(&mut hasher);
        }
        return Some(CaretSignature {
            owner: focused,
            cursor: state.cursor,
            anchor: state.selection_anchor,
            text: hasher.finish(),
        });
    }
    // The palette's query strip draws its caret while focus is the
    // palette itself or any of its rows (`container_text`).
    let mut current = Some(focused);
    while let Some(id) = current {
        if let Some(WidgetKind::CommandPalette(state)) = tree.payload(id) {
            let mut hasher = DefaultHasher::new();
            state.query().hash(&mut hasher);
            return Some(CaretSignature {
                owner: focused,
                cursor: state.query().len(),
                anchor: None,
                text: hasher.finish(),
            });
        }
        current = tree.parent(id);
    }
    None
}

/// The caret's blink clock: an epoch (when the current signature was
/// first observed) and that signature. Visible for the first
/// [`CARET_BLINK_INTERVAL`] after the epoch, hidden for the next, and so
/// on. See the module docs for what restarts it and what never blinks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaretBlink {
    epoch: Instant,
    signature: Option<CaretSignature>,
}

impl CaretBlink {
    /// A clock with no caret owner, epoch `now`.
    #[must_use]
    pub fn new(now: Instant) -> Self {
        Self {
            epoch: now,
            signature: None,
        }
    }

    /// Records the caret this frame would draw. A signature different
    /// from the last one observed — any edit, caret or selection move,
    /// composition change, or focus change, including gaining or losing
    /// an owner — restarts the clock at `now`, so the caret shows solid
    /// immediately after any change. The same signature leaves it alone.
    pub fn observe(&mut self, signature: Option<CaretSignature>, now: Instant) {
        if signature != self.signature {
            self.signature = signature;
            self.epoch = now;
        }
    }

    /// Whether the caret is drawn at `now`: in an even half-period since
    /// the epoch. Always `true` with no owner (there is nothing to hide,
    /// and nothing to wake for) and under `reduced_motion` (a steady
    /// caret). An instant before the epoch counts as the first, visible
    /// half-period.
    #[must_use]
    pub fn visible(&self, now: Instant, reduced_motion: bool) -> bool {
        if reduced_motion || self.signature.is_none() {
            return true;
        }
        let interval = CARET_BLINK_INTERVAL.as_nanos();
        let elapsed = now.saturating_duration_since(self.epoch).as_nanos();
        (elapsed / interval).is_multiple_of(2)
    }

    /// When [`Self::visible`] next changes, **strictly after** `now` — so
    /// an event loop waiting until it can never spin — or `None` when it
    /// never will (no owner, or `reduced_motion`), so the loop may block.
    /// `None` too in the unreachable case of an instant that overflows.
    #[must_use]
    pub fn next_toggle(&self, now: Instant, reduced_motion: bool) -> Option<Instant> {
        if reduced_motion || self.signature.is_none() {
            return None;
        }
        if now < self.epoch {
            return self.epoch.checked_add(CARET_BLINK_INTERVAL);
        }
        let interval = CARET_BLINK_INTERVAL.as_nanos();
        let elapsed = now.duration_since(self.epoch).as_nanos();
        // In `1..=interval`: never zero, so the flip is in the future.
        let remaining = interval - elapsed % interval;
        let remaining = Duration::from_nanos(u64::try_from(remaining).ok()?);
        now.checked_add(remaining)
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{CARET_BLINK_INTERVAL, CaretBlink, CaretSignature, caret_signature};
    use crate::tree::{WidgetId, WidgetTree};
    use crate::widgets::{
        CommandEntry, WidgetKind, command_palette_state, insert_button, insert_command_palette,
        insert_text_field, new_tree, set_command_palette_query, set_text_field_disabled,
        test_scales, with_text_field_mut,
    };

    fn ok<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
        match result {
            Ok(value) => value,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn sig(owner: WidgetId, cursor: usize) -> CaretSignature {
        CaretSignature {
            owner,
            cursor,
            anchor: None,
            text: 0,
        }
    }

    fn field_tree() -> (WidgetTree<WidgetKind>, WidgetId, WidgetId) {
        let (mut tree, root) = new_tree(taffy::Style::default());
        let scales = test_scales();
        let field = ok(insert_text_field(&mut tree, root, &scales, "Name", "abc"));
        let button = ok(insert_button(&mut tree, root, &scales, "OK"));
        (tree, field, button)
    }

    const I: Duration = CARET_BLINK_INTERVAL;

    #[test]
    fn a_caret_is_visible_then_hidden_in_alternating_half_periods() {
        let (_, field, _) = field_tree();
        let t0 = Instant::now();
        let mut blink = CaretBlink::new(t0);
        blink.observe(Some(sig(field, 0)), t0);
        let ms = Duration::from_millis;
        assert!(blink.visible(t0, false));
        assert!(blink.visible(t0 + I.saturating_sub(ms(1)), false));
        assert!(!blink.visible(t0 + I, false), "the flip is at the interval");
        assert!(!blink.visible(t0 + (I * 2).saturating_sub(ms(1)), false));
        assert!(blink.visible(t0 + I * 2, false));
        assert!(!blink.visible(t0 + I * 3, false));
        // Before the epoch counts as the first, visible half-period.
        assert!(blink.visible(t0.checked_sub(ms(10)).unwrap_or(t0), false));
    }

    #[test]
    fn next_toggle_is_the_next_half_period_boundary_strictly_after_now() {
        let (_, field, _) = field_tree();
        let t0 = Instant::now();
        let mut blink = CaretBlink::new(t0);
        blink.observe(Some(sig(field, 0)), t0);
        let ms = Duration::from_millis;
        assert_eq!(blink.next_toggle(t0, false), Some(t0 + I));
        assert_eq!(blink.next_toggle(t0 + ms(100), false), Some(t0 + I));
        // Exactly on a boundary: the *next* one, never `now` itself.
        assert_eq!(blink.next_toggle(t0 + I, false), Some(t0 + I * 2));
        assert_eq!(blink.next_toggle(t0 + I * 5, false), Some(t0 + I * 6));
        // Strictly future at every sampled instant, and the visibility
        // really does change there and not a nanosecond before.
        for step in 0..40u32 {
            let now = t0 + ms(u64::from(step) * 37);
            let Some(next) = blink.next_toggle(now, false) else {
                unreachable!("an owned caret always has a next flip");
            };
            assert!(next > now, "{step}");
            assert!(next - now <= I, "{step}");
            let Some(before) = next.checked_sub(Duration::from_nanos(1)) else {
                unreachable!("a flip after `now` has an instant before it");
            };
            assert_eq!(blink.visible(before, false), blink.visible(now, false));
            assert_ne!(blink.visible(next, false), blink.visible(now, false));
        }
        // Before the epoch: the first flip.
        let early = t0.checked_sub(ms(10)).unwrap_or(t0);
        assert_eq!(blink.next_toggle(early, false), Some(t0 + I));
    }

    #[test]
    fn a_new_signature_restarts_the_clock_visible_and_the_same_one_does_not() {
        let (_, field, button) = field_tree();
        let t0 = Instant::now();
        let mut blink = CaretBlink::new(t0);
        blink.observe(Some(sig(field, 0)), t0);
        let hidden = t0 + I + Duration::from_millis(100);
        assert!(!blink.visible(hidden, false));
        // Re-observing the same caret changes nothing.
        blink.observe(Some(sig(field, 0)), hidden);
        assert!(!blink.visible(hidden, false));
        // A caret move (a typed character, a click) shows it at once and
        // restarts the full visible half-period from there.
        blink.observe(Some(sig(field, 1)), hidden);
        assert!(blink.visible(hidden, false));
        assert_eq!(blink.next_toggle(hidden, false), Some(hidden + I));
        // So does focus moving to another owner.
        let later = hidden + I + Duration::from_millis(1);
        assert!(!blink.visible(later, false));
        blink.observe(Some(sig(button, 1)), later);
        assert!(blink.visible(later, false));
    }

    #[test]
    fn no_owner_or_reduced_motion_is_always_visible_and_never_wakes() {
        let (_, field, _) = field_tree();
        let t0 = Instant::now();
        let mut blink = CaretBlink::new(t0);
        for k in 0..6u32 {
            assert!(blink.visible(t0 + I * k, false));
            assert_eq!(blink.next_toggle(t0 + I * k, false), None);
        }
        blink.observe(Some(sig(field, 0)), t0);
        for k in 0..6u32 {
            let now = t0 + I * k;
            assert!(blink.visible(now, true), "reduced motion: {k}");
            assert_eq!(blink.next_toggle(now, true), None, "{k}");
        }
        // Losing the owner stops it again.
        blink.observe(None, t0 + I);
        assert!(blink.visible(t0 + I, false));
        assert_eq!(blink.next_toggle(t0 + I, false), None);
    }

    #[test]
    fn a_focused_enabled_field_has_a_signature_that_follows_every_edit() {
        let (mut tree, field, button) = field_tree();
        assert_eq!(caret_signature(&tree, None), None);
        assert_eq!(caret_signature(&tree, Some(button)), None, "no caret");
        let Some(first) = caret_signature(&tree, Some(field)) else {
            unreachable!("a focused field draws a caret");
        };
        assert_eq!(caret_signature(&tree, Some(field)), Some(first), "stable");
        let mut seen = vec![first];
        let mut edit = |tree: &mut WidgetTree<WidgetKind>,
                        f: &dyn Fn(&mut crate::widgets::TextFieldState)| {
            ok(with_text_field_mut(tree, field, f));
            let Some(now) = caret_signature(tree, Some(field)) else {
                unreachable!("still focused and enabled");
            };
            assert!(!seen.contains(&now), "{now:?} repeats a signature");
            seen.push(now);
        };
        // Content change at the same cursor (a delete-forward, say).
        edit(&mut tree, &|s| {
            s.content = "abd".to_owned();
        });
        edit(&mut tree, &|s| s.cursor = 1);
        edit(&mut tree, &|s| s.selection_anchor = Some(3));
        edit(&mut tree, &|s| s.set_composition("か".to_owned(), None));
        edit(&mut tree, &|s| {
            s.set_composition("か".to_owned(), Some((0, 3)));
        });
        ok(set_text_field_disabled(&mut tree, field, true));
        assert_eq!(caret_signature(&tree, Some(field)), None, "disabled");
    }

    #[test]
    fn the_palette_signature_holds_anywhere_inside_it_and_follows_the_query() {
        let (mut tree, root) = new_tree(taffy::Style::default());
        let palette = ok(insert_command_palette(
            &mut tree,
            root,
            vec![
                CommandEntry::new("a", "Undo"),
                CommandEntry::new("b", "Redo"),
            ],
        ));
        let rows = ok(command_palette_state(&tree, palette)).rows().to_vec();
        let Some(&row) = rows.first() else {
            unreachable!("two entries, two rows");
        };
        let Some(on_palette) = caret_signature(&tree, Some(palette)) else {
            unreachable!("the palette draws its query caret");
        };
        assert!(caret_signature(&tree, Some(row)).is_some(), "a row too");
        assert_eq!(caret_signature(&tree, Some(root)), None, "outside it");
        ok(set_command_palette_query(&mut tree, palette, "u"));
        assert_ne!(caret_signature(&tree, Some(palette)), Some(on_palette));
    }
}
