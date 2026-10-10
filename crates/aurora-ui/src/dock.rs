//! The dock arrangement (0.166.0, workspace round 5): which rail panels
//! share a slot as tabs, in what order, and which tab each group shows —
//! as plain data, apart from the widget tree it is applied to
//! ([`crate::redock::apply_dock_arrangement`]).
//!
//! **Why a model.** Drag-to-redock, the keyboard move commands, a saved
//! layout and the repair of a damaged one are all "turn one arrangement
//! into another"; doing that on data keeps every rule in one place, makes
//! it testable without a tree, and makes "every panel exactly once" a
//! property of one type ([`DockArrangement::repaired`]) rather than of
//! every caller.
//!
//! **Slots.** The rail is a column of [`DockSlot`]s, top to bottom. A slot
//! holding one panel is an ordinary docked panel (its own title row); a
//! slot holding two or more is a tab group ([`crate::panel_group`]). A
//! slot is never empty: a move that takes the last panel out of a slot
//! removes the slot ([`DockArrangement::moved`]).
//!
//! **Placement.** Every slot is in the right rail today. 0.167.0's
//! floating panels add a second [`DockPlacement`] variant rather than a
//! new type, so the persisted format (`aurora-app`'s `SavedDockSlot`
//! carries the placement as an enum) gains a variant without a new
//! layout version.

/// A rail panel's stable identity — the one name a saved layout, a
/// command and the tree all agree on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DockPanel {
    Layers,
    Properties,
    History,
}

impl DockPanel {
    /// Every rail panel, in the default top-to-bottom order.
    pub const ALL: [Self; 3] = [Self::Layers, Self::Properties, Self::History];

    /// The panel's title (its tab label and accessible name).
    #[must_use]
    pub fn title(self) -> &'static str {
        match self {
            Self::Layers => "Layers",
            Self::Properties => "Properties",
            Self::History => "History",
        }
    }

    /// The panel's persisted key — stable across releases, independent of
    /// the (translatable, some day) title.
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::Layers => "layers",
            Self::Properties => "properties",
            Self::History => "history",
        }
    }

    /// The panel a persisted key names, if any.
    #[must_use]
    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|panel| panel.key() == key)
    }
}

/// Where a slot lives. Only the right rail exists in 0.166.0; see this
/// module's doc comment.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DockPlacement {
    #[default]
    Rail,
}

/// One rail slot: its panels in tab order and the selected tab (always in
/// range; `0` for a single panel).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockSlot {
    pub panels: Vec<DockPanel>,
    pub selected: usize,
}

impl DockSlot {
    /// A one-panel slot.
    #[must_use]
    pub fn single(panel: DockPanel) -> Self {
        Self {
            panels: vec![panel],
            selected: 0,
        }
    }

    /// The selected panel.
    #[must_use]
    pub fn selected_panel(&self) -> Option<DockPanel> {
        self.panels.get(self.selected).copied()
    }
}

/// Where a dragged (or commanded) panel goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropTarget {
    /// Into a gap of the rail, as a slot of its own, before slot `n`
    /// (`n == slots.len()` is after the last slot).
    Gap(usize),
    /// Into slot `n` as its last tab, selected.
    Join(usize),
}

/// A keyboard move (0.166.0's command path).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelMove {
    /// Out of its group into the gap above it, or — alone — above the
    /// slot before it.
    Up,
    /// Out of its group into the gap below it, or — alone — below the
    /// slot after it.
    Down,
    /// Into the next slot as a tab (wrapping to the first).
    NextGroup,
}

/// The whole rail arrangement: its slots, top to bottom. Every value
/// built through this type's own constructors holds every
/// [`DockPanel`] exactly once, in non-empty slots with in-range
/// selections.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockArrangement {
    slots: Vec<DockSlot>,
}

impl Default for DockArrangement {
    /// Layers on its own, then Properties + History as tabs, Properties
    /// selected (0.164.0's grouping).
    fn default() -> Self {
        Self {
            slots: vec![
                DockSlot::single(DockPanel::Layers),
                DockSlot {
                    panels: vec![DockPanel::Properties, DockPanel::History],
                    selected: crate::workspace::PANEL_GROUP_TAB_DEFAULT,
                },
            ],
        }
    }
}

impl DockArrangement {
    /// The slots, top to bottom.
    #[must_use]
    pub fn slots(&self) -> &[DockSlot] {
        &self.slots
    }

    /// The slot holding `panel`, and its tab index there.
    #[must_use]
    pub fn position_of(&self, panel: DockPanel) -> Option<(usize, usize)> {
        self.slots.iter().enumerate().find_map(|(slot, entry)| {
            entry
                .panels
                .iter()
                .position(|&p| p == panel)
                .map(|tab| (slot, tab))
        })
    }

    /// Builds a valid arrangement from untrusted slots — a decoded saved
    /// layout, or a caller's own list — repairing rather than refusing:
    /// unknown panels (`None`) are dropped, a panel named twice keeps its
    /// first place, empty slots are dropped, an out-of-range selection
    /// selects the first tab (a selection naming a dropped entry falls to
    /// the first kept one), and every panel left out is appended as a slot
    /// of its own, in [`DockPanel::ALL`] order. With no known panel at all
    /// the result is [`Self::default`]. Returns the arrangement and
    /// whether anything had to be repaired.
    #[must_use]
    pub fn repaired(raw: Vec<(Vec<Option<DockPanel>>, usize)>) -> (Self, bool) {
        let mut repaired = false;
        let mut seen: Vec<DockPanel> = Vec::new();
        let mut slots = Vec::new();
        for (panels, selected) in raw {
            let selected_panel = panels.get(selected).copied().flatten();
            if selected >= panels.len() {
                repaired = true;
            }
            let mut kept = Vec::with_capacity(panels.len());
            for panel in panels {
                match panel {
                    Some(panel) if !seen.contains(&panel) => {
                        seen.push(panel);
                        kept.push(panel);
                    }
                    _ => repaired = true,
                }
            }
            if kept.is_empty() {
                repaired = true;
                continue;
            }
            let selected = selected_panel
                .and_then(|panel| kept.iter().position(|&p| p == panel))
                .unwrap_or(0);
            slots.push(DockSlot {
                panels: kept,
                selected,
            });
        }
        if slots.is_empty() {
            return (Self::default(), true);
        }
        for panel in DockPanel::ALL {
            if !seen.contains(&panel) {
                repaired = true;
                slots.push(DockSlot::single(panel));
            }
        }
        (Self { slots }, repaired)
    }

    /// Selects `panel`'s tab in its slot (a no-op for one it lacks).
    pub fn select(&mut self, panel: DockPanel) {
        if let Some((slot, tab)) = self.position_of(panel)
            && let Some(entry) = self.slots.get_mut(slot)
        {
            entry.selected = tab;
        }
    }

    /// The arrangement after moving `panel` to `target`, or `None` when
    /// the move would change nothing or names no slot (a cancel): a lone
    /// panel dropped into the gap just above or below itself, or joining
    /// its own slot. A slot the panel leaves empty is removed; a group
    /// that loses its selected tab selects the tab now in that place (or
    /// the new last one); the moved panel is selected where it lands.
    #[must_use]
    pub fn moved(&self, panel: DockPanel, target: DropTarget) -> Option<Self> {
        let (from, tab) = self.position_of(panel)?;
        let alone = self
            .slots
            .get(from)
            .is_some_and(|slot| slot.panels.len() == 1);
        match target {
            DropTarget::Gap(gap) => {
                if gap > self.slots.len() || (alone && (gap == from || gap == from + 1)) {
                    return None;
                }
            }
            DropTarget::Join(into) => {
                if into >= self.slots.len() || into == from {
                    return None;
                }
            }
        }
        let mut slots = self.slots.clone();
        let removed_slot = {
            let source = slots.get_mut(from)?;
            source.panels.remove(tab);
            if tab < source.selected {
                source.selected -= 1;
            }
            source.selected = source.selected.min(source.panels.len().saturating_sub(1));
            source.panels.is_empty()
        };
        if removed_slot {
            slots.remove(from);
        }
        let shift = |index: usize| {
            if removed_slot && index > from {
                index - 1
            } else {
                index
            }
        };
        match target {
            DropTarget::Gap(gap) => slots.insert(shift(gap), DockSlot::single(panel)),
            DropTarget::Join(into) => {
                let dest = slots.get_mut(shift(into))?;
                dest.panels.push(panel);
                dest.selected = dest.panels.len() - 1;
            }
        }
        Some(Self { slots })
    }

    /// The target a keyboard move of `panel` means (see [`PanelMove`]),
    /// or `None` at an end (a lone top panel moved up, a lone bottom panel
    /// moved down, a next group with only one slot).
    #[must_use]
    pub fn move_target(&self, panel: DockPanel, direction: PanelMove) -> Option<DropTarget> {
        let (slot, _) = self.position_of(panel)?;
        let alone = self
            .slots
            .get(slot)
            .is_some_and(|entry| entry.panels.len() == 1);
        match direction {
            PanelMove::Up if alone => slot.checked_sub(1).map(DropTarget::Gap),
            PanelMove::Up => Some(DropTarget::Gap(slot)),
            PanelMove::Down if alone => {
                (slot + 2 <= self.slots.len()).then_some(DropTarget::Gap(slot + 2))
            }
            PanelMove::Down => Some(DropTarget::Gap(slot + 1)),
            PanelMove::NextGroup => {
                if self.slots.len() < 2 {
                    return None;
                }
                Some(DropTarget::Join((slot + 1) % self.slots.len()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{DockArrangement, DockPanel, DockSlot, DropTarget, PanelMove};
    use DockPanel::{History, Layers, Properties};

    fn arrangement(slots: &[(&[DockPanel], usize)]) -> DockArrangement {
        let raw = slots
            .iter()
            .map(|(panels, selected)| (panels.iter().copied().map(Some).collect(), *selected))
            .collect();
        let (built, repaired) = DockArrangement::repaired(raw);
        assert!(!repaired, "{slots:?} is already valid");
        built
    }

    fn shape(arrangement: &DockArrangement) -> Vec<(Vec<DockPanel>, usize)> {
        arrangement
            .slots()
            .iter()
            .map(|slot| (slot.panels.clone(), slot.selected))
            .collect()
    }

    fn every_panel_once(arrangement: &DockArrangement) -> bool {
        let mut all: Vec<_> = arrangement
            .slots()
            .iter()
            .flat_map(|slot| slot.panels.iter().copied())
            .collect();
        all.sort();
        arrangement
            .slots()
            .iter()
            .all(|slot| !slot.panels.is_empty() && slot.selected < slot.panels.len())
            && all == DockPanel::ALL.to_vec()
    }

    #[test]
    fn the_default_is_layers_then_properties_and_history_as_tabs() {
        let default = DockArrangement::default();
        assert_eq!(
            shape(&default),
            vec![(vec![Layers], 0), (vec![Properties, History], 0)]
        );
        assert!(every_panel_once(&default));
        for panel in DockPanel::ALL {
            assert_eq!(DockPanel::from_key(panel.key()), Some(panel));
        }
        assert_eq!(DockPanel::from_key("brushes"), None);
    }

    /// AC-1/AC-2 at the model level: a gap reorders, a join adds a
    /// selected tab, a tab dragged into a gap becomes its own slot, the
    /// last tab leaving removes its slot, and no-op drops are `None`.
    #[test]
    fn moves_reorder_join_split_and_remove_empty_slots() {
        let default = DockArrangement::default();
        // Reorder: Layers below the group.
        let reordered = default.moved(Layers, DropTarget::Gap(2));
        assert_eq!(
            reordered.as_ref().map(shape),
            Some(vec![(vec![Properties, History], 0), (vec![Layers], 0)])
        );
        // Join: Layers into the group, selected; its own slot is gone.
        let joined = default.moved(Layers, DropTarget::Join(1));
        assert_eq!(
            joined.as_ref().map(shape),
            Some(vec![(vec![Properties, History, Layers], 2)])
        );
        // Split: History out of the group into the top gap.
        let split = default.moved(History, DropTarget::Gap(0));
        assert_eq!(
            split.as_ref().map(shape),
            Some(vec![
                (vec![History], 0),
                (vec![Layers], 0),
                (vec![Properties], 0)
            ])
        );
        // The selected tab leaving: the tab now in its place is selected.
        let mut history_shown = default.clone();
        history_shown.select(History);
        assert_eq!(
            history_shown
                .moved(History, DropTarget::Gap(0))
                .as_ref()
                .map(shape),
            Some(vec![
                (vec![History], 0),
                (vec![Layers], 0),
                (vec![Properties], 0)
            ])
        );
        // No-ops: a lone panel just above or below itself, a join of its
        // own slot, a target past the end.
        assert_eq!(default.moved(Layers, DropTarget::Gap(0)), None);
        assert_eq!(default.moved(Layers, DropTarget::Gap(1)), None);
        assert_eq!(default.moved(Properties, DropTarget::Join(1)), None);
        assert_eq!(default.moved(Layers, DropTarget::Gap(3)), None);
        assert_eq!(default.moved(Layers, DropTarget::Join(2)), None);
        // A grouped tab into the gap right next to its own group is a
        // real move (it leaves the group).
        assert!(default.moved(Properties, DropTarget::Gap(1)).is_some());
        for moved in [reordered, joined, split].into_iter().flatten() {
            assert!(every_panel_once(&moved), "{moved:?}");
        }
    }

    /// The last tab of a group leaving (a two-tab group losing one of
    /// them) leaves a one-panel slot, and a lone panel joining elsewhere
    /// removes its slot — no empty slot survives any move.
    #[test]
    fn no_move_ever_leaves_an_empty_slot() {
        let grouped = arrangement(&[(&[Layers, Properties, History], 1)]);
        let mut frontier = vec![grouped];
        let mut seen = Vec::new();
        while let Some(next) = frontier.pop() {
            if seen.contains(&next) {
                continue;
            }
            assert!(every_panel_once(&next), "{next:?}");
            for panel in DockPanel::ALL {
                for gap in 0..=next.slots().len() {
                    frontier.extend(next.moved(panel, DropTarget::Gap(gap)));
                }
                for into in 0..next.slots().len() {
                    frontier.extend(next.moved(panel, DropTarget::Join(into)));
                }
            }
            seen.push(next);
        }
        // Every reachable arrangement was checked; there are 13 of them
        // for three panels (orderings of slots of ordered tabs).
        assert!(seen.len() >= 13, "{}", seen.len());
    }

    /// AC-6 at the model level: unknown, duplicate and missing panels,
    /// empty slots and out-of-range selections are repaired so every
    /// panel appears exactly once.
    #[test]
    fn a_damaged_arrangement_repairs_to_every_panel_exactly_once() {
        let (repaired, changed) = DockArrangement::repaired(vec![
            (vec![None, Some(History)], 0),
            (vec![Some(History), Some(Layers)], 9),
            (vec![], 0),
        ]);
        assert!(changed);
        assert_eq!(
            shape(&repaired),
            vec![(vec![History], 0), (vec![Layers], 0), (vec![Properties], 0)]
        );
        assert!(every_panel_once(&repaired));
        let (from_nothing, changed) = DockArrangement::repaired(vec![(vec![None], 0)]);
        assert!(changed);
        assert_eq!(from_nothing, DockArrangement::default());
        let (from_empty, _) = DockArrangement::repaired(Vec::new());
        assert_eq!(from_empty, DockArrangement::default());
        // A selection that named a dropped duplicate falls to the first
        // kept tab; one naming a kept panel keeps it.
        let (kept, _) = DockArrangement::repaired(vec![
            (vec![Some(Layers)], 0),
            (vec![Some(Properties), Some(Layers), Some(History)], 2),
        ]);
        assert_eq!(
            shape(&kept),
            vec![(vec![Layers], 0), (vec![Properties, History], 1)]
        );
    }

    /// AC-5 at the model level: the keyboard moves.
    #[test]
    fn keyboard_moves_map_to_targets_and_stop_at_the_ends() {
        let default = DockArrangement::default();
        assert_eq!(default.move_target(Layers, PanelMove::Up), None);
        assert_eq!(
            default.move_target(Layers, PanelMove::Down),
            Some(DropTarget::Gap(2))
        );
        assert_eq!(
            default.move_target(Properties, PanelMove::Up),
            Some(DropTarget::Gap(1))
        );
        assert_eq!(
            default.move_target(History, PanelMove::Down),
            Some(DropTarget::Gap(2))
        );
        assert_eq!(
            default.move_target(History, PanelMove::NextGroup),
            Some(DropTarget::Join(0))
        );
        let one_slot = arrangement(&[(&[Layers, Properties, History], 0)]);
        assert_eq!(one_slot.move_target(Layers, PanelMove::NextGroup), None);
        let bottom = arrangement(&[(&[Properties, History], 0), (&[Layers], 0)]);
        assert_eq!(bottom.move_target(Layers, PanelMove::Down), None);
        let _ = DockSlot::single(Layers);
    }
}
