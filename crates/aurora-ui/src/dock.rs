//! The dock arrangement (0.166.0, workspace round 5): which rail panels
//! share a slot as tabs, in what order, and which tab each group shows —
//! as plain data, apart from the widget tree it is applied to
//! ([`crate::redock::apply_dock_arrangement`]).
//!
//! **Why a model.** Drag-to-redock, the keyboard move commands, a saved
//! layout and the repair of a damaged one are all "turn one arrangement
//! into another"; doing that on data keeps every rule in one place, makes
//! it testable without a tree, and makes "every panel exactly once" a
//! property of one type ([`DockArrangement::repaired_placed`]) rather than
//! of every caller.
//!
//! **Slots.** The rail is a column of [`DockSlot`]s, top to bottom. A slot
//! holding one panel is an ordinary docked panel (its own title row); a
//! slot holding two or more is a tab group ([`crate::panel_group`]). A
//! slot is never empty: a move that takes the last panel out of a slot
//! removes the slot ([`DockArrangement::moved`]).
//!
//! **Floating slots (0.167.0).** A slot can instead float over the canvas
//! area ([`DockPlacement::Floating`], a [`FloatSlot`]): a lone panel or a
//! whole tab group, at a position in logical px relative to the canvas
//! area's top-left. Floating slots are kept in **stacking order**, bottom
//! to top ([`DockArrangement::floating`]); the last one is drawn on top.
//! A floating slot is never empty either. The model clamps only what it
//! can know — a non-finite position is unrecoverable and docks the slot,
//! a negative one is pulled to `0` — and leaves the canvas-size clamp to
//! the tree ([`crate::workspace::sync_floating_frames`]), which knows the
//! canvas area's size.

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

/// Where a slot lives (0.167.0: the rail, or floating over the canvas
/// area at `x`, `y` logical px from its top-left).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub enum DockPlacement {
    #[default]
    Rail,
    Floating {
        x: f32,
        y: f32,
    },
}

/// One slot: its panels in tab order and the selected tab (always in
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

/// A floating slot (0.167.0): a slot and its position, logical px from the
/// canvas area's top-left — always finite and non-negative.
#[derive(Debug, Clone, PartialEq)]
pub struct FloatSlot {
    pub slot: DockSlot,
    pub x: f32,
    pub y: f32,
}

/// Which slot: the `n`th of the rail, top to bottom, or the `n`th floating
/// slot, bottom to top.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotRef {
    Rail(usize),
    Floating(usize),
}

/// Where a dragged (or commanded) panel goes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DropTarget {
    /// Into a gap of the rail, as a slot of its own, before slot `n`
    /// (`n == slots.len()` is after the last slot).
    Gap(usize),
    /// Into rail slot `n` as its last tab, selected.
    Join(usize),
    /// Into floating slot `n` as its last tab, selected (0.167.0).
    JoinFloating(usize),
    /// Floating on top of every other floating slot, its top-left at `x`,
    /// `y` logical px from the canvas area's (0.167.0).
    Float { x: f32, y: f32 },
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

/// The whole arrangement: the rail's slots, top to bottom, and the
/// floating slots, bottom to top. Every value built through this type's
/// own constructors holds every [`DockPanel`] exactly once, in non-empty
/// slots with in-range selections and finite, non-negative float
/// positions.
#[derive(Debug, Clone, PartialEq)]
pub struct DockArrangement {
    slots: Vec<DockSlot>,
    floating: Vec<FloatSlot>,
}

impl Default for DockArrangement {
    /// Layers on its own, then Properties + History as tabs, Properties
    /// selected (0.164.0's grouping); nothing floating.
    fn default() -> Self {
        Self {
            slots: vec![
                DockSlot::single(DockPanel::Layers),
                DockSlot {
                    panels: vec![DockPanel::Properties, DockPanel::History],
                    selected: crate::workspace::PANEL_GROUP_TAB_DEFAULT,
                },
            ],
            floating: Vec::new(),
        }
    }
}

/// A float position as the model keeps it: `None` (dock it) when either
/// coordinate is not finite, else each pulled up to `0`.
fn repaired_position(x: f32, y: f32) -> Option<(f32, f32)> {
    (x.is_finite() && y.is_finite()).then_some((x.max(0.0), y.max(0.0)))
}

impl DockArrangement {
    /// The rail's slots, top to bottom.
    #[must_use]
    pub fn slots(&self) -> &[DockSlot] {
        &self.slots
    }

    /// The floating slots, bottom to top (0.167.0).
    #[must_use]
    pub fn floating(&self) -> &[FloatSlot] {
        &self.floating
    }

    /// The *rail* slot holding `panel`, and its tab index there (`None`
    /// for a floating panel — see [`Self::locate`]).
    #[must_use]
    pub fn position_of(&self, panel: DockPanel) -> Option<(usize, usize)> {
        match self.locate(panel)? {
            (SlotRef::Rail(slot), tab) => Some((slot, tab)),
            (SlotRef::Floating(_), _) => None,
        }
    }

    /// The slot holding `panel`, rail or floating, and its tab index.
    #[must_use]
    pub fn locate(&self, panel: DockPanel) -> Option<(SlotRef, usize)> {
        let find = |slot: &DockSlot| slot.panels.iter().position(|&p| p == panel);
        self.slots
            .iter()
            .enumerate()
            .find_map(|(index, slot)| find(slot).map(|tab| (SlotRef::Rail(index), tab)))
            .or_else(|| {
                self.floating.iter().enumerate().find_map(|(index, float)| {
                    find(&float.slot).map(|tab| (SlotRef::Floating(index), tab))
                })
            })
    }

    /// Whether `panel` floats.
    #[must_use]
    pub fn is_floating(&self, panel: DockPanel) -> bool {
        matches!(self.locate(panel), Some((SlotRef::Floating(_), _)))
    }

    fn slot(&self, at: SlotRef) -> Option<&DockSlot> {
        match at {
            SlotRef::Rail(index) => self.slots.get(index),
            SlotRef::Floating(index) => self.floating.get(index).map(|float| &float.slot),
        }
    }

    fn slot_mut(&mut self, at: SlotRef) -> Option<&mut DockSlot> {
        match at {
            SlotRef::Rail(index) => self.slots.get_mut(index),
            SlotRef::Floating(index) => self.floating.get_mut(index).map(|float| &mut float.slot),
        }
    }

    /// [`Self::repaired_placed`] for rail-only slots (the 0.166.0 form).
    #[must_use]
    pub fn repaired(raw: Vec<(Vec<Option<DockPanel>>, usize)>) -> (Self, bool) {
        Self::repaired_placed(
            raw.into_iter()
                .map(|(panels, selected)| (DockPlacement::Rail, panels, selected))
                .collect(),
        )
    }

    /// Builds a valid arrangement from untrusted slots — a decoded saved
    /// layout, or a caller's own list — repairing rather than refusing:
    /// unknown panels (`None`) are dropped, a panel named twice keeps its
    /// first place, empty slots are dropped, an out-of-range selection
    /// selects the first tab (a selection naming a dropped entry falls to
    /// the first kept one), and every panel left out is appended to the
    /// rail as a slot of its own, in [`DockPanel::ALL`] order. Rail slots
    /// keep their relative order; floating ones too, as their stacking
    /// order (first is bottom). A floating slot whose position is not
    /// finite is unrecoverable and is docked at the rail's end; a negative
    /// coordinate is pulled up to `0` (0.167.0). With no known panel at
    /// all the result is [`Self::default`]. Returns the arrangement and
    /// whether anything had to be repaired.
    #[must_use]
    pub fn repaired_placed(
        raw: Vec<(DockPlacement, Vec<Option<DockPanel>>, usize)>,
    ) -> (Self, bool) {
        let mut repaired = false;
        let mut seen: Vec<DockPanel> = Vec::new();
        let mut slots = Vec::new();
        let mut floating = Vec::new();
        let mut docked_late = Vec::new();
        for (placement, panels, selected) in raw {
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
            let slot = DockSlot {
                panels: kept,
                selected,
            };
            match placement {
                DockPlacement::Rail => slots.push(slot),
                DockPlacement::Floating { x, y } => {
                    if let Some((fx, fy)) = repaired_position(x, y) {
                        #[allow(clippy::float_cmp)]
                        if fx != x || fy != y {
                            repaired = true;
                        }
                        floating.push(FloatSlot { slot, x: fx, y: fy });
                    } else {
                        repaired = true;
                        docked_late.push(slot);
                    }
                }
            }
        }
        slots.extend(docked_late);
        if slots.is_empty() && floating.is_empty() {
            return (Self::default(), true);
        }
        for panel in DockPanel::ALL {
            if !seen.contains(&panel) {
                repaired = true;
                slots.push(DockSlot::single(panel));
            }
        }
        (Self { slots, floating }, repaired)
    }

    /// Selects `panel`'s tab in its slot (a no-op for one it lacks).
    pub fn select(&mut self, panel: DockPanel) {
        if let Some((at, tab)) = self.locate(panel)
            && let Some(entry) = self.slot_mut(at)
        {
            entry.selected = tab;
        }
    }

    /// Whether `target` names an existing place (and a finite position).
    fn target_exists(&self, target: DropTarget) -> bool {
        match target {
            DropTarget::Gap(gap) => gap <= self.slots.len(),
            DropTarget::Join(into) => into < self.slots.len(),
            DropTarget::JoinFloating(into) => into < self.floating.len(),
            DropTarget::Float { x, y } => x.is_finite() && y.is_finite(),
        }
    }

    /// The arrangement without the slot at `from`.
    fn without_slot(&self, from: SlotRef) -> Self {
        let mut rest = self.clone();
        match from {
            SlotRef::Rail(index) if index < rest.slots.len() => {
                rest.slots.remove(index);
            }
            SlotRef::Floating(index) if index < rest.floating.len() => {
                rest.floating.remove(index);
            }
            _ => {}
        }
        rest
    }

    /// `target` re-addressed after the slot at `from` was removed.
    fn shifted(target: DropTarget, from: SlotRef) -> DropTarget {
        match (target, from) {
            (DropTarget::Gap(gap), SlotRef::Rail(index)) if gap > index => DropTarget::Gap(gap - 1),
            (DropTarget::Join(into), SlotRef::Rail(index)) if into > index => {
                DropTarget::Join(into - 1)
            }
            (DropTarget::JoinFloating(into), SlotRef::Floating(index)) if into > index => {
                DropTarget::JoinFloating(into - 1)
            }
            (target, _) => target,
        }
    }

    /// Puts `slot` at `target` (already re-addressed): a gap inserts it as
    /// a rail slot, a join appends its panels to that slot with the
    /// slot's own selected panel selected, a float puts it on top.
    fn place(&mut self, slot: DockSlot, target: DropTarget) -> Option<()> {
        match target {
            DropTarget::Gap(gap) => {
                let gap = gap.min(self.slots.len());
                self.slots.insert(gap, slot);
            }
            DropTarget::Join(into) => self.join(SlotRef::Rail(into), slot)?,
            DropTarget::JoinFloating(into) => self.join(SlotRef::Floating(into), slot)?,
            DropTarget::Float { x, y } => {
                let (x, y) = repaired_position(x, y)?;
                self.floating.push(FloatSlot { slot, x, y });
            }
        }
        Some(())
    }

    /// Appends `slot`'s panels to the slot at `at`, `slot`'s own selected
    /// panel selected there.
    fn join(&mut self, at: SlotRef, slot: DockSlot) -> Option<()> {
        let shown = slot.selected_panel();
        let dest = self.slot_mut(at)?;
        dest.panels.extend(slot.panels);
        dest.selected = shown
            .and_then(|panel| dest.panels.iter().position(|&p| p == panel))
            .unwrap_or(dest.panels.len().saturating_sub(1));
        Some(())
    }

    /// The arrangement after moving `panel` to `target`, or `None` when
    /// the move would change nothing or names no slot (a cancel): a lone
    /// panel dropped into the gap just above or below itself, joining its
    /// own slot, or a lone floating panel floated back to where it already
    /// is, on top. A slot the panel leaves empty is removed; a group that
    /// loses its selected tab selects the tab now in that place (or the
    /// new last one); the moved panel is selected where it lands. A
    /// [`DropTarget::Float`] makes it a floating slot of its own, on top
    /// (0.167.0) — out of its group if it had one.
    #[must_use]
    pub fn moved(&self, panel: DockPanel, target: DropTarget) -> Option<Self> {
        let (from, tab) = self.locate(panel)?;
        if !self.target_exists(target) {
            return None;
        }
        let alone = self.slot(from).is_some_and(|slot| slot.panels.len() == 1);
        if alone {
            // A lone panel moves as its whole slot.
            return self.moved_slot(panel, target);
        }
        if matches!(
            (target, from),
            (DropTarget::Join(into), SlotRef::Rail(index)) if into == index
        ) || matches!(
            (target, from),
            (DropTarget::JoinFloating(into), SlotRef::Floating(index)) if into == index
        ) {
            return None;
        }
        let mut next = self.clone();
        let source = next.slot_mut(from)?;
        source.panels.remove(tab);
        if tab < source.selected {
            source.selected -= 1;
        }
        source.selected = source.selected.min(source.panels.len().saturating_sub(1));
        next.place(DockSlot::single(panel), target)?;
        Some(next)
    }

    /// The arrangement after moving the **whole slot** holding `member` to
    /// `target` (0.167.0: a floating slot's move or re-dock, the same
    /// drag). A gap docks the slot as it is (its tabs and selection); a
    /// join adds its panels to that slot, its selected panel selected; a
    /// float puts it on top at that position. `None` when nothing would
    /// change: into the gap next to itself, joining itself, or a float to
    /// its own place while already on top.
    #[must_use]
    pub fn moved_slot(&self, member: DockPanel, target: DropTarget) -> Option<Self> {
        let (from, _) = self.locate(member)?;
        if !self.target_exists(target) {
            return None;
        }
        let unchanged = match (target, from) {
            (DropTarget::Gap(gap), SlotRef::Rail(index)) => gap == index || gap == index + 1,
            (DropTarget::Join(into), SlotRef::Rail(index))
            | (DropTarget::JoinFloating(into), SlotRef::Floating(index)) => into == index,
            (DropTarget::Float { x, y }, SlotRef::Floating(index)) => {
                let on_top = index + 1 == self.floating.len();
                #[allow(clippy::float_cmp)]
                let same = self
                    .floating
                    .get(index)
                    .is_some_and(|float| repaired_position(x, y) == Some((float.x, float.y)));
                on_top && same
            }
            _ => false,
        };
        if unchanged {
            return None;
        }
        let slot = self.slot(from)?.clone();
        let mut next = self.without_slot(from);
        next.place(slot, Self::shifted(target, from))?;
        Some(next)
    }

    /// The arrangement with floating slot `index` raised to the top of the
    /// stacking order (0.167.0), or `None` when it already is (or there is
    /// no such slot).
    #[must_use]
    pub fn raised(&self, index: usize) -> Option<Self> {
        if index.saturating_add(1) >= self.floating.len() {
            return None;
        }
        let mut next = self.clone();
        let float = next.floating.remove(index);
        next.floating.push(float);
        Some(next)
    }

    /// The target a keyboard move of `panel` means (see [`PanelMove`]),
    /// or `None` at an end (a lone top panel moved up, a lone bottom panel
    /// moved down, a next group with only one slot) and for a floating
    /// panel (0.167.0: the move commands rearrange the rail only; "Dock
    /// PANEL Panel" brings a floating one back).
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
    use super::{
        DockArrangement, DockPanel, DockPlacement, DockSlot, DropTarget, PanelMove, SlotRef,
    };
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

    fn floats(arrangement: &DockArrangement) -> Vec<(Vec<DockPanel>, usize, f32, f32)> {
        arrangement
            .floating()
            .iter()
            .map(|float| {
                (
                    float.slot.panels.clone(),
                    float.slot.selected,
                    float.x,
                    float.y,
                )
            })
            .collect()
    }

    fn every_panel_once_anywhere(arrangement: &DockArrangement) -> bool {
        let slots: Vec<&DockSlot> = arrangement
            .slots()
            .iter()
            .chain(arrangement.floating().iter().map(|float| &float.slot))
            .collect();
        let mut all: Vec<_> = slots
            .iter()
            .flat_map(|slot| slot.panels.iter().copied())
            .collect();
        all.sort();
        slots
            .iter()
            .all(|slot| !slot.panels.is_empty() && slot.selected < slot.panels.len())
            && arrangement.floating().iter().all(|float| {
                float.x.is_finite() && float.y.is_finite() && float.x >= 0.0 && float.y >= 0.0
            })
            && all == DockPanel::ALL.to_vec()
    }

    /// 0.167.0, AC-1/AC-2 at the model level: floating a docked tab pulls
    /// it out of its group onto the top of the stack; a lone panel floats
    /// whole; joining a floating slot, moving a whole floating group,
    /// re-docking it, raising and the no-ops.
    #[test]
    fn floating_moves_join_split_raise_and_redock() {
        let default = DockArrangement::default();
        let Some(one) = default.moved(History, DropTarget::Float { x: 10.0, y: 20.0 }) else {
            unreachable!("History floats");
        };
        assert_eq!(shape(&one), vec![(vec![Layers], 0), (vec![Properties], 0)]);
        assert_eq!(floats(&one), vec![(vec![History], 0, 10.0, 20.0)]);
        assert_eq!(one.locate(History), Some((SlotRef::Floating(0), 0)));
        assert_eq!(one.position_of(History), None, "position_of is the rail's");
        assert!(one.is_floating(History));
        // Layers floats whole; it is on top.
        let Some(two) = one.moved(Layers, DropTarget::Float { x: 50.0, y: 60.0 }) else {
            unreachable!("Layers floats");
        };
        assert_eq!(floats(&two).len(), 2);
        assert_eq!(floats(&two).last().map(|f| f.0.clone()), Some(vec![Layers]));
        // Properties joins the floating History (index 0).
        let Some(joined) = two.moved(Properties, DropTarget::JoinFloating(0)) else {
            unreachable!("joins");
        };
        assert!(joined.slots().is_empty(), "the rail emptied");
        assert_eq!(
            floats(&joined).first().map(|f| (f.0.clone(), f.1)),
            Some((vec![History, Properties], 1))
        );
        // The whole group moves (and comes to the top), as one.
        let Some(moved) = joined.moved_slot(History, DropTarget::Float { x: 5.0, y: 6.0 }) else {
            unreachable!("moves");
        };
        assert_eq!(
            floats(&moved).last().cloned(),
            Some((vec![History, Properties], 1, 5.0, 6.0))
        );
        // A whole-group re-dock keeps its tabs and selection.
        let Some(docked) = moved.moved_slot(Properties, DropTarget::Gap(0)) else {
            unreachable!("docks");
        };
        assert_eq!(shape(&docked), vec![(vec![History, Properties], 1)]);
        assert_eq!(floats(&docked), vec![(vec![Layers], 0, 50.0, 60.0)]);
        // Raising.
        assert_eq!(two.raised(1), None, "already on top");
        let raised = two.raised(0);
        assert_eq!(
            raised
                .as_ref()
                .and_then(|r| floats(r).last().map(|f| f.0.clone())),
            Some(vec![History])
        );
        // No-ops: a float to its own place on top, joining itself,
        // non-finite targets, a missing floating slot.
        assert_eq!(
            two.moved(Layers, DropTarget::Float { x: 50.0, y: 60.0 }),
            None
        );
        assert!(
            two.moved(History, DropTarget::Float { x: 10.0, y: 20.0 })
                .is_some(),
            "not on top: raises"
        );
        assert_eq!(
            joined.moved_slot(History, DropTarget::JoinFloating(0)),
            None
        );
        assert_eq!(joined.moved(History, DropTarget::JoinFloating(0)), None);
        assert_eq!(
            default.moved(
                Layers,
                DropTarget::Float {
                    x: f32::NAN,
                    y: 0.0
                }
            ),
            None
        );
        assert_eq!(default.moved(Layers, DropTarget::JoinFloating(0)), None);
        // The move commands leave floating panels alone.
        assert_eq!(one.move_target(History, PanelMove::Up), None);
        // A negative float position is pulled to 0.
        let pulled = default.moved(Layers, DropTarget::Float { x: -5.0, y: -9.0 });
        assert_eq!(
            pulled.as_ref().map(floats),
            Some(vec![(vec![Layers], 0, 0.0, 0.0)])
        );
        for each in [one, two, joined, moved, docked] {
            assert!(every_panel_once_anywhere(&each), "{each:?}");
        }
    }

    /// Every arrangement reachable with floating targets too keeps every
    /// panel exactly once, with no empty slot anywhere.
    #[test]
    fn no_floating_move_ever_leaves_an_empty_slot() {
        let mut frontier = vec![DockArrangement::default()];
        let mut seen: Vec<DockArrangement> = Vec::new();
        while let Some(next) = frontier.pop() {
            if seen.contains(&next) || seen.len() > 400 {
                continue;
            }
            assert!(every_panel_once_anywhere(&next), "{next:?}");
            let mut targets = vec![DropTarget::Float { x: 1.0, y: 2.0 }];
            targets.extend((0..=next.slots().len()).map(DropTarget::Gap));
            targets.extend((0..next.slots().len()).map(DropTarget::Join));
            targets.extend((0..next.floating().len()).map(DropTarget::JoinFloating));
            for panel in DockPanel::ALL {
                for &target in &targets {
                    frontier.extend(next.moved(panel, target));
                    frontier.extend(next.moved_slot(panel, target));
                }
            }
            seen.push(next);
        }
        assert!(seen.len() > 13, "floats add arrangements: {}", seen.len());
    }

    /// AC-6 at the model level: a damaged floating slot repairs — a
    /// non-finite position docks it at the rail's end, a negative one is
    /// pulled to 0, a duplicate across rail and float keeps its first
    /// place, and a float-only arrangement is kept as it is.
    #[test]
    fn a_damaged_float_repairs_by_clamping_or_docking() {
        let (repaired, changed) = DockArrangement::repaired_placed(vec![
            (
                DockPlacement::Floating {
                    x: f32::INFINITY,
                    y: 3.0,
                },
                vec![Some(Layers)],
                0,
            ),
            (
                DockPlacement::Floating { x: -4.0, y: 7.0 },
                vec![Some(History), Some(Layers)],
                1,
            ),
            (DockPlacement::Rail, vec![Some(Properties)], 0),
        ]);
        assert!(changed);
        assert_eq!(
            shape(&repaired),
            vec![(vec![Properties], 0), (vec![Layers], 0)]
        );
        assert_eq!(floats(&repaired), vec![(vec![History], 0, 0.0, 7.0)]);
        assert!(every_panel_once_anywhere(&repaired));
        let all_floating = vec![(
            DockPlacement::Floating { x: 1.0, y: 2.0 },
            DockPanel::ALL.into_iter().map(Some).collect(),
            2,
        )];
        let (kept, changed) = DockArrangement::repaired_placed(all_floating);
        assert!(!changed, "everything floating is valid");
        assert!(kept.slots().is_empty());
        assert_eq!(floats(&kept), vec![(DockPanel::ALL.to_vec(), 2, 1.0, 2.0)]);
        let (nan, changed) = DockArrangement::repaired_placed(vec![(
            DockPlacement::Floating {
                x: f32::NAN,
                y: f32::NAN,
            },
            vec![Some(Layers)],
            0,
        )]);
        assert!(changed);
        assert!(nan.floating().is_empty());
        assert!(every_panel_once_anywhere(&nan));
    }
}
