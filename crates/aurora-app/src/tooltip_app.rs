//! `App`'s half of the tooltips (0.262.0): feeding the pointer, the clock
//! and the hide events to [`aurora_ui::TooltipHover`] and laying out again
//! when a tooltip appears, moves or goes. The rules (delay, placement,
//! what hides it, one at a time) live in `aurora_ui::tooltip_hover` and
//! are tested there without a window; this file is only the glue that
//! needs `App`'s fields.

use aurora_ui::HoverBlockers;

use crate::App;

impl App {
    /// What owns the pointer right now (a modal, a drag, a divider or panel
    /// drag in progress, a window without focus).
    fn tooltip_blockers(&self) -> HoverBlockers {
        HoverBlockers {
            modal_open: self.dialog.is_some() || self.command_palette.is_some(),
            canvas_drag: self.drag.is_some(),
            rail_resize: self.rail_resize.is_some(),
            panel_drag: self.panel_drag.is_some(),
            window_unfocused: !self.caret.window_focused,
        }
    }

    /// A tooltip appeared, moved or went: lay out again, repaint and tell
    /// assistive technology the tree changed.
    fn tooltip_tree_changed(&mut self) {
        let window_size = self.window.as_ref().map(|window| window.inner_size());
        if let Some(size) = window_size {
            self.apply_resize((size.width, size.height));
        }
        self.push_accessibility();
        self.needs_redraw = true;
    }

    /// Reports the pointer (`None`: it left the window) to the tooltips,
    /// masked while anything else owns it.
    pub(crate) fn update_tooltip_hover(&mut self, position: Option<(f32, f32)>) {
        let point = aurora_ui::hover_point(position, self.tooltip_blockers());
        match self.tooltip_hover.pointer(
            &mut self.workspace,
            &self.scales,
            point,
            std::time::Instant::now(),
        ) {
            Ok(true) => self.tooltip_tree_changed(),
            Ok(false) => {}
            Err(err) => tracing::warn!(?err, "tooltip hover failed"),
        }
    }

    /// A press, a key or a wheel step: the tooltip hides until the pointer
    /// leaves its owner and returns.
    pub(crate) fn dismiss_tooltip(&mut self) {
        match self.tooltip_hover.dismiss(&mut self.workspace) {
            Ok(true) => self.tooltip_tree_changed(),
            Ok(false) => {}
            Err(err) => tracing::warn!(?err, "tooltip dismissal failed"),
        }
    }

    /// Drops any tooltip outright (window blur, a dialog or the palette
    /// opening, a document switch).
    pub(crate) fn clear_tooltip(&mut self) {
        if self.tooltip_hover.clear(&mut self.workspace) {
            self.tooltip_tree_changed();
        }
    }

    /// Once per loop turn: a modal or lost focus clears the tooltip, the
    /// delay is ticked, a shown tooltip is sized and placed against the
    /// current window (which also repositions it after a resize). Returns
    /// the next wake-up, `None` when no tooltip is pending, so an idle
    /// loop still sleeps.
    pub(crate) fn tooltip_step(&mut self, now: std::time::Instant) -> Option<std::time::Instant> {
        if self.tooltip_blockers().any() {
            self.clear_tooltip();
            return None;
        }
        let mut changed = match self.tooltip_hover.tick(&mut self.workspace, now) {
            Ok(changed) => changed,
            Err(err) => {
                tracing::warn!(?err, "tooltip tick failed");
                return None;
            }
        };
        #[allow(clippy::cast_possible_truncation)]
        let scale_factor = self.scale_factor as f32;
        match self.tooltip_hover.place(
            &mut self.workspace,
            self.text_engine.as_mut(),
            &self.scales,
            scale_factor,
        ) {
            Ok(placed) => changed |= placed,
            Err(err) => tracing::warn!(?err, "tooltip placement failed"),
        }
        if changed {
            self.tooltip_tree_changed();
        }
        self.tooltip_hover.next_deadline()
    }
}

#[cfg(test)]
mod tests {
    //! `App` needs a window, so the glue is pinned by single-line source
    //! checks of where it is called from (the rules are tested in
    //! `aurora_ui::tooltip_hover`).

    const SOURCE: &str = include_str!("lib.rs");

    fn count(needle: &str) -> usize {
        SOURCE.lines().filter(|line| line.trim() == needle).count()
    }

    #[test]
    fn a_press_a_key_and_the_wheel_dismiss_the_tooltip() {
        assert_eq!(count("self.dismiss_tooltip();"), 3);
    }

    #[test]
    fn the_pointer_moving_and_leaving_report_to_the_tooltips() {
        // At the end of `update_toolbar_hover`, which the pointer move and
        // `CursorLeft` both call.
        assert_eq!(count("self.update_tooltip_hover(position);"), 1);
        assert_eq!(count("self.update_toolbar_hover(Some(position));"), 1);
        assert_eq!(count("self.update_toolbar_hover(None);"), 1);
    }

    #[test]
    fn losing_window_focus_clears_it_and_the_loop_steps_it_once() {
        assert_eq!(count("self.clear_tooltip();"), 1);
        assert_eq!(count("let tooltip_deadline = self.tooltip_step(now);"), 1);
        assert_eq!(
            count("let deadline = earliest_deadline(deadline, tooltip_deadline);"),
            1
        );
    }
}
