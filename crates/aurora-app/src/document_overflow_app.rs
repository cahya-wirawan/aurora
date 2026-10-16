//! `App`'s half of the document strip's overflow menu (0.260.0): opening
//! the popup with a measured width, turning a routed outcome into a
//! switch, and the per-turn housekeeping. The transitions are
//! [`crate::document_overflow`], tested headlessly; this file is only the
//! glue that needs `App`'s fields.

use aurora_widgets::PointerOutcome;

use crate::App;
use crate::document_overflow::OverflowAction;

impl App {
    /// A dialog or the command palette is open.
    fn document_overflow_modal(&self) -> bool {
        self.dialog.is_some() || self.command_palette.is_some()
    }

    /// Opens the overflow menu (closing the other popovers and menus).
    fn open_document_overflow(&mut self) {
        if self.document_overflow_modal() {
            return;
        }
        self.close_colour_popover_now(false);
        self.menu_bar
            .close(&mut self.workspace, &mut self.focus, false);
        let controls = self.layer_controls.controls;
        crate::close_open_popovers(
            &mut self.workspace.tree,
            &mut self.focus,
            self.gallery.as_mut(),
            controls.as_ref(),
            &mut self.scroll_follow,
        );
        let (labels, _) = crate::document_tabs::tab_labels(&self.shelf, &self.doc);
        let scale_factor = self.scale_factor;
        let scales = &self.scales;
        let mut engine = self.text_engine.as_mut();
        #[allow(clippy::cast_precision_loss)]
        let estimate = |text: &str| {
            text.chars().count() as f32 * scales.typography.size.md as f32 * 0.6
        };
        let widest = labels
            .iter()
            .map(|label| {
                #[allow(clippy::cast_possible_truncation)]
                let measured = engine.as_mut().map(|engine| {
                    engine
                        .shape(
                            label,
                            &aurora_widgets::label_style(scales),
                            scale_factor as f32,
                        )
                        .width
                });
                measured
                    .filter(|width| width.is_finite())
                    .unwrap_or_else(|| estimate(label))
            })
            .fold(0.0_f32, f32::max);
        let modal = self.document_overflow_modal();
        let opened = self.doc_overflow.open(
            &mut self.workspace,
            &mut self.focus,
            &self.scales,
            &self.shelf,
            &self.doc,
            widest,
            modal,
        );
        if opened {
            self.relayout_after_gallery();
        }
    }

    /// What a routed outcome on the overflow button or popup means.
    /// `true` when it was the overflow menu's (so the menu bar's own
    /// handling is skipped).
    pub(crate) fn apply_document_overflow_outcome(&mut self, outcome: &PointerOutcome) -> bool {
        let owned = match outcome {
            PointerOutcome::Action(aurora_widgets::ActionOutcome::Activated(id)) => {
                *id == self.workspace.document_overflow
            }
            PointerOutcome::Action(aurora_widgets::ActionOutcome::MenuActivated {
                menu, ..
            })
            | PointerOutcome::MenuCancelled { menu } => self
                .doc_overflow
                .open
                .as_ref()
                .is_some_and(|open| open.menu == *menu),
            _ => false,
        };
        if !owned {
            return false;
        }
        match self
            .doc_overflow
            .action_for(&mut self.workspace, &mut self.focus, outcome)
        {
            OverflowAction::None => {}
            OverflowAction::Toggle => {
                if self
                    .doc_overflow
                    .close(&mut self.workspace, &mut self.focus, true)
                {
                    self.relayout_after_gallery();
                } else {
                    self.open_document_overflow();
                }
                return true;
            }
            OverflowAction::Switch(target) => {
                self.activate_document(target.get());
                return true;
            }
        }
        self.relayout_after_gallery();
        true
    }

    /// Per event-loop turn: forgets a popup that went away, and closes an
    /// open menu under a modal or when the open documents changed (its rows
    /// would name documents that may be gone).
    pub(crate) fn sync_document_overflow_now(&mut self) {
        let modal = self.document_overflow_modal();
        if self.doc_overflow.sync(
            &mut self.workspace,
            &mut self.focus,
            &self.shelf,
            &self.doc,
            modal,
        ) {
            self.relayout_after_gallery();
        }
    }
}
