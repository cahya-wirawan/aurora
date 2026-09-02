//! Real content for the Layers panel: a real
//! `aurora_widgets::widgets::tree_view` — a `Role::Tree` holding one
//! `Role::TreeItem` row per layer in an `aurora_doc::LayerTree`, nested
//! to match group structure. PLAN.md M1.8's "Layers, history,
//! tool-options panels" bullet, first slice — Layers only; History and
//! tool-options panels are separate, still-open work.
//!
//! **A real tree, not a hand-built list.** Until 0.77.0 this module
//! wrote its own `Role::List`/`Role::ListItem` nodes and its own row
//! style. It now uses the widget, which is what carries the things a
//! hand-built list structurally could not: a 0-based `level` per row
//! (derived from the row's real parent, never computed here), a real
//! `selected` state that reaches the row's *payload* — so
//! `aurora_widgets::paint_widget` actually draws the highlight, not just
//! announces it — and an `expanded` state with exactly one of
//! `Action::Expand`/`Action::Collapse` on every group row.
//!
//! **Nothing routes those expand/collapse requests yet.** The actions
//! are declared and the state is announced, but no code in `aurora-app`
//! turns an incoming `accesskit::ActionRequest` for `Action::Expand` or
//! `Action::Collapse` on a layer row into a
//! `set_tree_item_expanded` call — that is separate, still-open work,
//! and until it lands a screen-reader user can be told a group is
//! collapsible without being able to collapse it.
//!
//! **The `VoiceOver` navigation bug PLAN.md records for this panel is
//! neither fixed nor claimed fixed here.** No display server exists in
//! this workspace's sandbox, so this change was not tested against a
//! real screen reader either way; whether a real `Role::Tree` helps it
//! is a question for a human on real hardware.
//!
//! **`WidgetError::TreeTooDeep` is propagated but unreachable from a
//! real document.** `aurora_widgets::widgets::MAX_TREE_DEPTH` is `255`
//! and 0-based; `aurora_doc::MAX_LAYER_TREE_DEPTH` is `256` and 1-based
//! (a root layer sits at depth `1`), so the deepest layer a `LayerTree`
//! will accept becomes exactly a depth-`255` row — the cap is reachable
//! and not exceedable, with no margin in either direction. That
//! exact-fit is now a real, compiled assertion in this module (the
//! `const _: () = assert!(...)` below), not a claim in prose: if either
//! constant ever moves by one, the build fails here rather than a user's
//! file-open failing with `TreeTooDeep`. The maximum-depth case is also
//! a real test now
//! (`the_deepest_legal_document_populates_without_a_depth_error`),
//! built through the ordinary public API — 255 nested groups and a
//! pixel layer at the bottom — with no `test-support` escape hatch
//! needed, contrary to what this comment claimed through 0.77.0.
//!
//! **The row's label is bounded, sanitized, and never blank.** A layer
//! name on the `.aur` path comes from the file (`App::open_aur_file` →
//! `aurora_io::read_aur`), and `aurora_doc::LayerTree` deliberately
//! stores it unmodified, so this panel puts every name through
//! [`aurora_doc::sanitize_display_name`] before it becomes an
//! `accesskit` label — the same bound `History::journal_descriptions`
//! already applies for the History panel next door. Display-only: the
//! document itself is untouched. The `"Untitled Layer"` fallback covers
//! a name that is *absent, empty, or nothing but whitespace* (0.77.1;
//! before that only an absent one), because both `.aur` and PSD permit
//! `""` and `"   "`, and a row labelled with either is silent or
//! indistinguishable to a screen reader.
//!
//! **Almost no pixel rendering.** A *selected* row does now paint a
//! real, one-row-tall highlight: selection reaches the row's payload, so
//! `aurora_widgets::paint_widget` draws it (that is the whole point of
//! moving to the real widget). Everything else is still unpainted — no
//! thumbnail or colour swatch, no visibility checkbox, no disclosure
//! triangle for a group, and no drawn text at all, since a row's label
//! reaches the accessibility node and nothing else
//! (`aurora-vector`-backed glyph rendering doesn't exist yet).
//!
//! **One `Tab` stop per layer row, and that is live in the app now.**
//! Every enabled row declares `Action::Focus`, which is what
//! `aurora_widgets::FocusManager` treats as focusable — so `Tab` through
//! a 200-layer document is 200 stops inside this one panel before
//! reaching Properties, where the conventional pattern is one stop on
//! the tree plus arrow keys between rows. That is `tree_view`'s own
//! disclosed, crate-wide focus-model question (see its module doc
//! comment), not something this call site can settle; it is named here
//! because this panel is the first shipping code that makes it a real
//! user-visible cost, and pinned by this module's own
//! `tab_order_currently_stops_on_every_layer_row` test so the redesign
//! has a before-picture.
//!
//! **The panel's height is bounded, and excess rows are clipped rather
//! than growing it** (0.77.1). `crate::panel`'s own `root_style`/
//! `body_style` give every docked panel a content-independent share of
//! the rail; before that, this panel grew ~21 px per layer and at 43
//! layers in a 1600×900 window had pushed Properties and History to zero
//! height, off the bottom of the window and out of reach of
//! `hit_test`. The honest consequence of the fix: rows past the bottom
//! of the panel are laid out but **not reachable** — nothing clips them
//! for painting either, they are simply painted over by the panels below
//! and refused by `hit_test`, which does not descend into a parent whose
//! own bounds exclude the point. A real scrolling/virtualized list is
//! what would make them reachable, and that is `tree_view`'s own
//! disclosed gap, still open.
//!
//! **Real, clickable size** (PLAN.md M1.9's "active-layer selection"):
//! row layout is `tree_view`'s own — one row height tall, indented one
//! `scales.spacing.md` step per level, with a width floor so a deeply
//! nested row never resolves to a degenerate zero-width box. So
//! `WidgetTree::compute_layout` gives each row a real, non-zero screen
//! rect a pointer can actually land on
//! (`aurora_widgets::WidgetTree::hit_test`), and every enabled row
//! carries `Action::Focus`/`Action::Click` for the same reason — a
//! screen reader needs a real target too, not just a mouse.
//! [`populate_layers_panel`] returns the `WidgetId -> LayerId` map a
//! caller (`aurora-app`) needs to turn a hit or an `ActionRequest` back
//! into "which layer".
//!
//! **One-shot, not reactive**: [`populate_layers_panel`] builds rows
//! once from whatever `LayerTree` state it's given. It does not diff
//! against a previous population or react to later document edits —
//! refreshing after an edit means calling it again, which is safe
//! (0.77.1: it empties the panel body itself first, so a second call
//! replaces the rows rather than silently stacking a second tree beside
//! the first) but rebuilds every row. Real incremental refresh is
//! separate work for whenever a document can actually be edited live in
//! `aurora-app`.

use std::collections::HashMap;

use aurora_doc::{LayerId, LayerKind, LayerTree, sanitize_display_name};
use aurora_theme::Scales;
use aurora_widgets::widgets::{
    WidgetKind, insert_tree_item, insert_tree_view, set_tree_item_description,
};
use aurora_widgets::{WidgetError, WidgetId, WidgetTree};

use crate::panel::{PanelHandle, clear_panel_body};

/// The exact-fit between the two depth caps this module sits between,
/// asserted at compile time rather than in prose. `aurora_doc`'s limit
/// is 1-based (a root layer is at depth `1`) and `aurora_widgets`' is
/// 0-based (a top-level row is level `0`), so the deepest legal layer
/// becomes exactly the deepest legal row and there is no margin in
/// either direction: `MAX_LAYER_TREE_DEPTH` growing by one would make a
/// legal document un-openable with `WidgetError::TreeTooDeep`, and
/// `MAX_TREE_DEPTH` growing by one would leave a row depth no document
/// can reach. Either way the build should fail here, where both
/// constants are in view, rather than a user's file-open failing later.
const _: () = assert!(
    aurora_doc::MAX_LAYER_TREE_DEPTH == aurora_widgets::widgets::MAX_TREE_DEPTH + 1,
    "aurora_doc::MAX_LAYER_TREE_DEPTH (1-based) and \
     aurora_widgets::widgets::MAX_TREE_DEPTH (0-based) must stay exactly one apart"
);

/// Inserts a real tree (`aurora_widgets::widgets::insert_tree_view`)
/// under `panel.body`, then one `Role::TreeItem` row
/// per layer in `layers` inside it — root layers first, top-to-bottom,
/// matching `LayerTree`'s own ordering convention, nested to mirror
/// group structure (a group's own layers become its row's children).
/// Returns every inserted row's own id mapped to the `LayerId` it
/// represents, so a caller can turn a real pointer hit or accessibility
/// action back into "which layer".
///
/// **The tree is its own container under `panel.body`, deliberately —
/// the rows are not `panel.body`'s own children.** `crate::panel::
/// insert_panel` builds the body Row-direction (`crate::panel`'s own
/// `body_style`); tree rows are `width: percent(1.0)` and as
/// `Row`-direction siblings would lay out *side by side* rather than
/// stacked. Setting the body's own style to `Column` instead would not
/// hold either: `crate::panel::set_panel_collapsed` resets it to that
/// same shared `body_style` on every expand, so the first collapse/
/// expand round trip would silently scramble the panel.
/// `insert_tree_view` brings its own `FlexDirection::Column` container,
/// which that reset path cannot reach.
///
/// **The tree container is deliberately unlabelled.** `panel.root` is
/// already a `Role::Region` labelled "Layers"; naming the container
/// inside it "Layers" too made a screen reader announce the name twice
/// on entry. The History panel next door avoids the same double-naming
/// by relabelling the body rather than nesting a second named container.
///
/// **Repopulating is safe**: `panel.body`'s existing children are
/// removed first ([`crate::panel::clear_panel_body`]), so calling this
/// twice replaces the rows instead of stacking a second `Role::Tree`
/// container beside the first with the old, now-meaningless
/// `WidgetId`s still live and hit-testable. Every caller in the
/// workspace already cleared first; this makes the one that forgets a
/// no-op rather than a corrupted panel.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `panel.body` doesn't
/// exist, or [`WidgetError::TreeTooDeep`] if a layer nests deeper than
/// `aurora_widgets::widgets::MAX_TREE_DEPTH` — see this module's own doc
/// comment for why no real `LayerTree` can reach that.
pub fn populate_layers_panel(
    tree: &mut WidgetTree<WidgetKind>,
    panel: PanelHandle,
    scales: &Scales,
    layers: &LayerTree,
) -> Result<HashMap<WidgetId, LayerId>, WidgetError> {
    clear_panel_body(tree, panel.body)?;
    let view = insert_tree_view(tree, panel.body, None)?;
    let mut rows = HashMap::new();
    for &id in layers.roots() {
        insert_layer_row(tree, view, scales, layers, id, &mut rows)?;
    }
    Ok(rows)
}

fn insert_layer_row(
    tree: &mut WidgetTree<WidgetKind>,
    parent: WidgetId,
    scales: &Scales,
    layers: &LayerTree,
    id: LayerId,
    rows: &mut HashMap<WidgetId, LayerId>,
) -> Result<WidgetId, WidgetError> {
    // `LayerTree` stores whatever name the file gave it -- there is no
    // length or content validation on `LayerTree::name`, deliberately,
    // so the document round-trips unmodified. That makes sanitizing the
    // *label* this panel's own job: see `sanitize_display_name`'s own
    // module doc comment, and the History panel next door, which goes
    // through the same bound via `History::journal_descriptions`.
    //
    // The fallback is applied *after* sanitizing, and to whitespace as
    // well as to emptiness: a missing name is not the only way to get a
    // blank row. Both `.aur` and PSD accept a layer literally named ""
    // or "   " -- and so does `sanitize_display_name`'s own output, if
    // the name was nothing but characters it strips. Any of those left
    // the row with an empty or all-space accessible label, which a
    // screen reader announces as nothing at all, and several such rows
    // are then mutually indistinguishable.
    let sanitized = sanitize_display_name(layers.name(id).unwrap_or_default());
    let name: &str = if sanitized.trim().is_empty() {
        "Untitled Layer"
    } else {
        sanitized.as_ref()
    };
    // "Non-empty group", not "is a group". `LayerTree::children` returns
    // `None` for a pixel layer and `Some(&[])` for an *empty* group (its
    // own doc comment says so), and `TreeItem`'s `has_children` is a
    // declaration that there is something to expand. An empty group
    // therefore becomes a leaf row -- correctly: it has nothing under
    // it, and announcing "collapsed" over nothing is exactly the state
    // `tree_view` refuses to produce. Its description still reads
    // "Group", so a screen-reader user can still tell what it is.
    //
    // What `!children.is_empty()` is *not*: load-bearing for the node
    // output of today's eager population. Mutation-tested during the
    // 0.77.0 review round -- hardcoding this argument to `false` passed
    // the entire `aurora-ui` + `aurora-app` suite, because
    // `insert_tree_item` sets an ancestor's own `has_children`/`expanded`
    // the moment a child is inserted under it, and the loop below always
    // inserts a non-empty group's children immediately. Only the
    // opposite mutation (`true`, marking a leaf as a group) is a real,
    // test-caught bug. This expression stays because it is the correct
    // thing for a caller to *declare* -- it matches `LayerKind::Group`'s
    // real semantics, and it is what a lazily-populated (or
    // populated-then-collapsed) group would depend on -- not because it
    // changes any node this function currently produces.
    let children = layers.children(id).unwrap_or_default();
    let row = insert_tree_item(tree, parent, scales, name, !children.is_empty())?;
    // Through the widget's own setter, never onto the node: `tree_view`
    // rebuilds a row's whole node from its `TreeItemState` on every
    // mutation, including the one `insert_tree_item` performs on *this*
    // row the moment the loop below inserts a child under it.
    set_tree_item_description(tree, row, Some(&describe_layer(layers, id)))?;
    rows.insert(row, id);

    for &child in children {
        insert_layer_row(tree, row, scales, layers, child, rows)?;
    }
    Ok(row)
}

/// `"Group"` / `"Normal, 100%"` / `"Multiply, 80%, hidden"` — blend
/// mode and opacity for a pixel layer, just the kind for a group.
/// `LayerTree` stores opacity/blend mode on groups too, but describing
/// a group only by its kind avoids implying group-level compositing
/// already does something — no compositor honours it yet (`aurora-render`
/// still needs a real layer model to call it with, per that crate's own
/// M1.3 notes).
fn describe_layer(layers: &LayerTree, id: LayerId) -> String {
    let hidden = matches!(layers.visible(id), Some(false));
    let suffix = if hidden { ", hidden" } else { "" };
    match layers.kind(id) {
        Some(LayerKind::Group { .. }) => format!("Group{suffix}"),
        Some(LayerKind::Pixel { .. }) => {
            let blend = layers.blend_mode(id).unwrap_or_default();
            let opacity = layers.opacity(id).unwrap_or(1.0);
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let percent = (opacity * 100.0).round() as u32;
            format!("{blend:?}, {percent}%{suffix}")
        }
        None => "Unknown layer".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::populate_layers_panel;
    use crate::panel::{PanelHandle, insert_panel};
    use aurora_core::Rect;
    use aurora_doc::LayerTree;
    use aurora_theme::Scales;
    use aurora_widgets::widgets::{self, WidgetKind};
    use aurora_widgets::{WidgetId, WidgetTree};
    use taffy::Style;
    use taffy::style_helpers::length;

    fn bounds() -> Rect {
        Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 100,
        }
    }

    // The real, committed, owner-approved scales -- the same file
    // `aurora-theme`'s own tests parse, so this exercises real token
    // values, not a synthetic fixture.
    fn test_scales() -> Scales {
        const SCALES_TOML: &str = include_str!("../../../design/tokens/scales.toml");
        match Scales::from_toml_str(SCALES_TOML) {
            Ok(scales) => scales,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    /// The `Role::Tree` container `populate_layers_panel` inserts under
    /// `panel.body` — the rows' real parent, and the extra level every
    /// assertion below has to look through. See
    /// [`super::populate_layers_panel`]'s own doc comment for why the
    /// rows are not `panel.body`'s own children.
    fn tree_root(tree: &WidgetTree<WidgetKind>, panel: PanelHandle) -> WidgetId {
        let Some(children) = tree.children(panel.body) else {
            unreachable!("a populated panel body exists");
        };
        match children {
            [only] => *only,
            other => unreachable!("expected exactly one tree container, got {other:?}"),
        }
    }

    /// Every row under `root`, in insertion order.
    fn rows_of(tree: &WidgetTree<WidgetKind>, root: WidgetId) -> Vec<WidgetId> {
        match tree.children(root) {
            Some(children) => children.to_vec(),
            None => unreachable!("just populated"),
        }
    }

    #[test]
    fn populate_layers_panel_adds_one_row_per_layer_top_to_bottom() {
        let mut layers = LayerTree::new();
        let background = match layers.add_pixel_layer("Background", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let retouch = match layers.add_pixel_layer("Retouch", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = layers.set_blend_mode(retouch, aurora_doc::BlendMode::Multiply) {
            unreachable!("{err:?}");
        }
        if let Err(err) = layers.set_opacity(retouch, 0.8) {
            unreachable!("{err:?}");
        }
        if let Err(err) = layers.set_visible(background, false) {
            unreachable!("{err:?}");
        }

        let (mut tree, root) = widgets::new_tree(Style::default());
        let panel = match insert_panel(&mut tree, root, "Layers") {
            Ok(panel) => panel,
            Err(err) => unreachable!("{err:?}"),
        };
        let scales = test_scales();
        let rows = match populate_layers_panel(&mut tree, panel, &scales, &layers) {
            Ok(rows) => rows,
            Err(err) => unreachable!("{err:?}"),
        };

        let view = tree_root(&tree, panel);
        let Some(view_accessibility) = tree.accessibility(view) else {
            unreachable!("just populated");
        };
        assert_eq!(view_accessibility.role(), accesskit::Role::Tree);
        assert_eq!(
            view_accessibility.label(),
            None,
            "the panel root is already labelled \"Layers\"; labelling the tree inside it too \
             made a screen reader announce the name twice on entry"
        );
        let Some(panel_accessibility) = tree.accessibility(panel.root) else {
            unreachable!("just inserted");
        };
        assert_eq!(
            panel_accessibility.label(),
            Some("Layers"),
            "...so the panel's own label is the one that has to carry the name"
        );

        // `add_pixel_layer` inserts each new layer as the topmost root,
        // so `retouch` (added second) must come before `background` in
        // `roots()` -- and so in row order too.
        let row_ids = rows_of(&tree, view);
        assert_eq!(row_ids.len(), 2);
        let Some(first_row) = row_ids.first() else {
            unreachable!("just asserted len() == 2");
        };
        let Some(second_row) = row_ids.get(1) else {
            unreachable!("just asserted len() == 2");
        };

        let Some(first) = tree.accessibility(*first_row) else {
            unreachable!("just inserted");
        };
        assert_eq!(first.role(), accesskit::Role::TreeItem);
        assert_eq!(first.label(), Some("Retouch"));
        assert_eq!(first.description(), Some("Multiply, 80%"));
        assert_eq!(first.level(), Some(0), "a root layer is a top-level row");
        assert_eq!(
            first.is_selected(),
            Some(false),
            "a freshly populated panel selects nothing"
        );
        assert_eq!(
            first.is_expanded(),
            None,
            "a pixel layer is a leaf, and a leaf must omit `expanded` entirely"
        );
        assert!(first.supports_action(accesskit::Action::Focus));
        assert!(first.supports_action(accesskit::Action::Click));

        let Some(second) = tree.accessibility(*second_row) else {
            unreachable!("just inserted");
        };
        assert_eq!(second.role(), accesskit::Role::TreeItem);
        assert_eq!(second.label(), Some("Background"));
        assert_eq!(second.description(), Some("Normal, 100%, hidden"));
        assert_eq!(second.level(), Some(0));
        assert_eq!(second.is_selected(), Some(false));

        assert_eq!(
            rows.len(),
            2,
            "the returned map must have one entry per row"
        );
        assert_eq!(rows.get(first_row), Some(&retouch));
        assert_eq!(rows.get(second_row), Some(&background));
    }

    #[test]
    fn populate_layers_panel_gives_rows_a_real_nonzero_computed_size() {
        let mut layers = LayerTree::new();
        if let Err(err) = layers.add_pixel_layer("Background", bounds(), None) {
            unreachable!("{err:?}");
        }

        let (mut tree, root) = widgets::new_tree(Style::default());
        let panel = match insert_panel(&mut tree, root, "Layers") {
            Ok(panel) => panel,
            Err(err) => unreachable!("{err:?}"),
        };
        let scales = test_scales();
        if let Err(err) = populate_layers_panel(&mut tree, panel, &scales, &layers) {
            unreachable!("{err:?}");
        }
        tree.compute_layout(1000.0, 800.0);

        let row_ids = rows_of(&tree, tree_root(&tree, panel));
        let Some(&row) = row_ids.first() else {
            unreachable!("just added one layer");
        };
        let Some(row_bounds) = tree.bounds(row) else {
            unreachable!("just laid out");
        };
        assert!(
            row_bounds.width > 0 && row_bounds.height > 0,
            "a row must have a real, clickable size after layout: {row_bounds:?}"
        );
    }

    #[test]
    fn populate_layers_panel_nests_group_children_under_their_own_row() {
        let mut layers = LayerTree::new();
        let group = match layers.add_group("Effects", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let glow = match layers.add_pixel_layer("Glow", bounds(), Some(group)) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };

        let (mut tree, root) = widgets::new_tree(Style::default());
        let panel = match insert_panel(&mut tree, root, "Layers") {
            Ok(panel) => panel,
            Err(err) => unreachable!("{err:?}"),
        };
        let scales = test_scales();
        let rows = match populate_layers_panel(&mut tree, panel, &scales, &layers) {
            Ok(rows) => rows,
            Err(err) => unreachable!("{err:?}"),
        };

        let row_ids = rows_of(&tree, tree_root(&tree, panel));
        assert_eq!(row_ids.len(), 1, "one top-level row for the group");
        let Some(&group_row) = row_ids.first() else {
            unreachable!("just asserted len() == 1");
        };
        let Some(group_accessibility) = tree.accessibility(group_row) else {
            unreachable!("just inserted");
        };
        assert_eq!(group_accessibility.label(), Some("Effects"));
        assert_eq!(group_accessibility.description(), Some("Group"));
        assert_eq!(group_accessibility.level(), Some(0));
        assert_eq!(
            group_accessibility.is_expanded(),
            Some(true),
            "a group with a real row under it announces itself expanded"
        );
        assert!(
            group_accessibility.supports_action(accesskit::Action::Collapse),
            "and offers exactly the action that could change that"
        );
        assert!(!group_accessibility.supports_action(accesskit::Action::Expand));

        let Some(group_children) = tree.children(group_row) else {
            unreachable!("just inserted");
        };
        assert_eq!(group_children.len(), 1);
        let Some(&child_row) = group_children.first() else {
            unreachable!("just asserted len() == 1");
        };
        let Some(child_accessibility) = tree.accessibility(child_row) else {
            unreachable!("just inserted");
        };
        assert_eq!(child_accessibility.label(), Some("Glow"));
        assert_eq!(
            child_accessibility.level(),
            Some(1),
            "a layer inside a group is one level deeper -- derived from the real parent"
        );
        assert_eq!(
            child_accessibility.is_expanded(),
            None,
            "the child is a leaf, so it must carry no expanded state at all"
        );

        assert_eq!(rows.get(&group_row), Some(&group));
        assert_eq!(rows.get(&child_row), Some(&glow));
    }

    /// The sibling of the History panel's own bound. A layer name on
    /// the `.aur` path is whatever the file said, and `LayerTree` keeps
    /// it verbatim -- so the *label* is where it has to be bounded, or a
    /// 500 KB name with an embedded bidi override crosses straight into
    /// an assistive technology.
    #[test]
    fn a_hostile_layer_name_reaches_the_label_bounded_and_sanitized() {
        let hostile = format!(
            "safe{}txet{}{}{}{}",
            '\u{202E}',                                     // bidi override
            '\u{0007}',                                     // BEL
            '\u{2028}',                                     // line separator
            char::from_u32(0xE_0041).unwrap_or('\u{FFFD}'), // Tag 'A'
            "a".repeat(500_000),
        );
        let mut layers = LayerTree::new();
        if let Err(err) = layers.add_pixel_layer(hostile.clone(), bounds(), None) {
            unreachable!("{err:?}");
        }

        let (mut tree, root) = widgets::new_tree(Style::default());
        let panel = match insert_panel(&mut tree, root, "Layers") {
            Ok(panel) => panel,
            Err(err) => unreachable!("{err:?}"),
        };
        let scales = test_scales();
        if let Err(err) = populate_layers_panel(&mut tree, panel, &scales, &layers) {
            unreachable!("{err:?}");
        }

        let row_ids = rows_of(&tree, tree_root(&tree, panel));
        let Some(&row) = row_ids.first() else {
            unreachable!("just added one layer");
        };
        let Some(accessibility) = tree.accessibility(row) else {
            unreachable!("just inserted");
        };
        assert_eq!(accessibility.role(), accesskit::Role::TreeItem);
        let Some(label) = accessibility.label() else {
            unreachable!("every row carries a label");
        };
        assert!(
            label.chars().count() <= 129,
            "{} chars",
            label.chars().count()
        );
        for hostile_char in ['\u{202E}', '\u{0007}', '\u{2028}', '\u{E0041}'] {
            assert!(!label.contains(hostile_char), "{hostile_char:?} survived");
        }
        assert!(label.starts_with("safetxet"), "{label:?}");
        // Display-only: the document still holds every byte the file
        // gave it, so a save round-trips the user's own name unchanged.
        let Some(&id) = layers.roots().first() else {
            unreachable!("just added one layer");
        };
        assert_eq!(layers.name(id), Some(hostile.as_str()));
    }

    /// A blank name is not a missing name, and both `.aur` and PSD
    /// accept one. Before 0.77.1 the `"Untitled Layer"` fallback only
    /// covered `LayerTree::name` returning `None`, so a layer named ""
    /// reached a screen reader with a zero-length label (nothing
    /// announced at all) and one named "     " with five spaces --
    /// several such rows being mutually indistinguishable.
    #[test]
    fn a_blank_or_whitespace_only_name_falls_back_to_untitled_layer() {
        // The third case is a name that is not blank in the file but
        // sanitizes away to nothing -- the fallback has to be applied
        // after sanitizing, not before it, to catch that one.
        for blank in ["", "     ", "\u{00A0}\u{2028}", "\u{0007}\u{0007}"] {
            let mut layers = LayerTree::new();
            if let Err(err) = layers.add_pixel_layer(blank, bounds(), None) {
                unreachable!("{err:?}");
            }

            let (mut tree, root) = widgets::new_tree(Style::default());
            let panel = match insert_panel(&mut tree, root, "Layers") {
                Ok(panel) => panel,
                Err(err) => unreachable!("{err:?}"),
            };
            let scales = test_scales();
            if let Err(err) = populate_layers_panel(&mut tree, panel, &scales, &layers) {
                unreachable!("{err:?}");
            }

            let row_ids = rows_of(&tree, tree_root(&tree, panel));
            let Some(&row) = row_ids.first() else {
                unreachable!("just added one layer");
            };
            let Some(accessibility) = tree.accessibility(row) else {
                unreachable!("just inserted");
            };
            assert_eq!(
                accessibility.label(),
                Some("Untitled Layer"),
                "{blank:?} must not reach a screen reader as a blank label"
            );

            // Display-only, same as the hostile-name case: the document
            // still holds exactly the name the file gave it.
            let Some(&id) = layers.roots().first() else {
                unreachable!("just added one layer");
            };
            assert_eq!(layers.name(id), Some(blank));
        }
    }

    /// Latent before 0.77.1, and unguarded: a second
    /// `populate_layers_panel` on the same panel without an intervening
    /// `clear_panel_body` stacked a *second* `Role::Tree` container
    /// under the body, with the first call's rows still live and
    /// hit-testable beside the second's.
    #[test]
    fn populating_the_same_panel_twice_replaces_the_rows_instead_of_stacking_them() {
        let mut first_document = LayerTree::new();
        for name in ["Background", "Retouch", "Glow"] {
            if let Err(err) = first_document.add_pixel_layer(name, bounds(), None) {
                unreachable!("{err:?}");
            }
        }
        let mut second_document = LayerTree::new();
        if let Err(err) = second_document.add_pixel_layer("Photo", bounds(), None) {
            unreachable!("{err:?}");
        }

        let (mut tree, root) = widgets::new_tree(Style::default());
        let panel = match insert_panel(&mut tree, root, "Layers") {
            Ok(panel) => panel,
            Err(err) => unreachable!("{err:?}"),
        };
        let scales = test_scales();
        let stale = match populate_layers_panel(&mut tree, panel, &scales, &first_document) {
            Ok(rows) => rows,
            Err(err) => unreachable!("{err:?}"),
        };

        let fresh = match populate_layers_panel(&mut tree, panel, &scales, &second_document) {
            Ok(rows) => rows,
            Err(err) => unreachable!("{err:?}"),
        };

        assert_eq!(
            tree.children(panel.body).map(<[_]>::len),
            Some(1),
            "a second population must not leave two tree containers under the body"
        );
        let row_ids = rows_of(&tree, tree_root(&tree, panel));
        assert_eq!(row_ids.len(), 1, "only the second document's own row");
        assert_eq!(fresh.len(), 1);
        for old_row in stale.keys() {
            assert!(
                !tree.contains(*old_row),
                "the first population's rows must be really gone, not just orphaned: {old_row:?}"
            );
        }
    }

    /// `aurora_doc::MAX_LAYER_TREE_DEPTH` (256, 1-based) and
    /// `aurora_widgets::widgets::MAX_TREE_DEPTH` (255, 0-based) fit
    /// exactly, with no margin -- this module's own `const _: () =
    /// assert!(...)` keeps them that way at compile time, and this is
    /// the runtime half: the deepest document `LayerTree` will accept
    /// really does populate, rather than failing with `TreeTooDeep`.
    /// Buildable through the plain public API (255 nested groups and a
    /// pixel layer at the bottom), contrary to what this module's own
    /// doc comment claimed through 0.77.0.
    #[test]
    fn the_deepest_legal_document_populates_without_a_depth_error() {
        let mut layers = LayerTree::new();
        let mut parent = match layers.add_group("Group 0", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        // Depth 1 is the group above; groups 2..=255 go under it, then
        // the pixel layer lands at depth 256 == MAX_LAYER_TREE_DEPTH.
        for i in 1..(aurora_doc::MAX_LAYER_TREE_DEPTH - 1) {
            parent = match layers.add_group(format!("Group {i}"), Some(parent)) {
                Ok(id) => id,
                Err(err) => unreachable!("depth {i} must be legal: {err:?}"),
            };
        }
        let deepest = match layers.add_pixel_layer("Deepest", bounds(), Some(parent)) {
            Ok(id) => id,
            Err(err) => unreachable!("the deepest legal layer must be accepted: {err:?}"),
        };

        let (mut tree, root) = widgets::new_tree(Style::default());
        let panel = match insert_panel(&mut tree, root, "Layers") {
            Ok(panel) => panel,
            Err(err) => unreachable!("{err:?}"),
        };
        let scales = test_scales();
        let rows = match populate_layers_panel(&mut tree, panel, &scales, &layers) {
            Ok(rows) => rows,
            Err(err) => unreachable!("a legal document must never fail to populate: {err:?}"),
        };

        assert_eq!(rows.len(), aurora_doc::MAX_LAYER_TREE_DEPTH);
        let Some((&deepest_row, _)) = rows.iter().find(|entry| *entry.1 == deepest) else {
            unreachable!("the deepest layer got a row");
        };
        let Some(accessibility) = tree.accessibility(deepest_row) else {
            unreachable!("just inserted");
        };
        assert_eq!(
            accessibility.level(),
            Some(aurora_widgets::widgets::MAX_TREE_DEPTH),
            "the deepest legal layer must land on exactly the deepest legal row"
        );
    }

    /// Not an endorsement -- a pinned before-picture. Every enabled tree
    /// row declares `Action::Focus`, which is what `FocusManager` counts
    /// as a tab stop, so `Tab` walks *through* the Layers panel one
    /// layer at a time before it ever reaches Properties. See this
    /// module's own doc comment: the fix is `tree_view`'s crate-wide
    /// focus-model question (one stop per tree plus arrow keys), not a
    /// change this call site can make.
    #[test]
    fn tab_order_currently_stops_on_every_layer_row() {
        let mut layers = LayerTree::new();
        for name in ["Background", "Retouch", "Glow"] {
            if let Err(err) = layers.add_pixel_layer(name, bounds(), None) {
                unreachable!("{err:?}");
            }
        }

        let mut ws = crate::workspace::build_workspace();
        let scales = test_scales();
        let rows = match populate_layers_panel(&mut ws.tree, ws.layers, &scales, &layers) {
            Ok(rows) => rows,
            Err(err) => unreachable!("{err:?}"),
        };
        let row_ids = rows_of(&ws.tree, tree_root(&ws.tree, ws.layers));
        assert_eq!(row_ids.len(), 3);

        let mut focus = aurora_widgets::FocusManager::new();
        let mut visited = Vec::new();
        for _ in 0..6 {
            match focus.focus_next(&mut ws.tree) {
                Some(id) => visited.push(id),
                None => unreachable!("the workspace has focusable widgets"),
            }
        }

        let mut expected = vec![ws.layers.root];
        expected.extend_from_slice(&row_ids);
        expected.push(ws.properties.root);
        expected.push(ws.history.root);
        assert_eq!(
            visited, expected,
            "three layers currently cost three tab stops between Layers and Properties"
        );
        assert_eq!(rows.len(), 3);
    }

    /// The `.unwrap_or` fallback is unchanged by the sanitizing step --
    /// only the sanitization is new.
    #[test]
    fn an_ordinary_name_is_labelled_verbatim() {
        let mut layers = LayerTree::new();
        if let Err(err) = layers.add_pixel_layer("Retouch — skin", bounds(), None) {
            unreachable!("{err:?}");
        }

        let (mut tree, root) = widgets::new_tree(Style::default());
        let panel = match insert_panel(&mut tree, root, "Layers") {
            Ok(panel) => panel,
            Err(err) => unreachable!("{err:?}"),
        };
        let scales = test_scales();
        if let Err(err) = populate_layers_panel(&mut tree, panel, &scales, &layers) {
            unreachable!("{err:?}");
        }

        let row_ids = rows_of(&tree, tree_root(&tree, panel));
        let Some(&row) = row_ids.first() else {
            unreachable!("just added one layer");
        };
        let Some(accessibility) = tree.accessibility(row) else {
            unreachable!("just inserted");
        };
        assert_eq!(accessibility.role(), accesskit::Role::TreeItem);
        assert_eq!(accessibility.label(), Some("Retouch — skin"));
    }

    /// `LayerTree::children` returns `Some(&[])` for an empty group and
    /// `None` for a pixel layer, so "is a group" and "has something to
    /// expand" are different questions. `TreeItem`'s `has_children` is
    /// the second one: an empty group is a leaf row, which is the truth
    /// (there is nothing to expand into), and its description still says
    /// "Group" so the distinction is not lost to a screen reader.
    #[test]
    fn an_empty_group_is_a_leaf_row_with_no_expand_action() {
        let mut layers = LayerTree::new();
        if let Err(err) = layers.add_group("Empty", None) {
            unreachable!("{err:?}");
        }

        let (mut tree, root) = widgets::new_tree(Style::default());
        let panel = match insert_panel(&mut tree, root, "Layers") {
            Ok(panel) => panel,
            Err(err) => unreachable!("{err:?}"),
        };
        let scales = test_scales();
        if let Err(err) = populate_layers_panel(&mut tree, panel, &scales, &layers) {
            unreachable!("{err:?}");
        }

        let row_ids = rows_of(&tree, tree_root(&tree, panel));
        let Some(&row) = row_ids.first() else {
            unreachable!("just added one group");
        };
        let Some(accessibility) = tree.accessibility(row) else {
            unreachable!("just inserted");
        };
        assert_eq!(accessibility.label(), Some("Empty"));
        assert_eq!(
            accessibility.description(),
            Some("Group"),
            "a screen-reader user must still be told this is a group"
        );
        assert_eq!(
            accessibility.is_expanded(),
            None,
            "an empty group has nothing to expand, so it carries no expanded state"
        );
        assert!(!accessibility.supports_action(accesskit::Action::Expand));
        assert!(!accessibility.supports_action(accesskit::Action::Collapse));
    }

    /// The regression test for the layout bug the tree container exists
    /// to avoid. `insert_panel` builds `panel.body` with
    /// `Style::default()` -- `FlexDirection::Row` -- so rows parented
    /// straight to it would lay out *side by side*. They must stack, and
    /// each level must indent further than the one above it.
    #[test]
    fn populate_layers_panel_stacks_rows_vertically_and_indents_each_level() {
        let mut layers = LayerTree::new();
        // `Background` first, so the group -- added after it -- becomes
        // the topmost root and so the *first* row.
        if let Err(err) = layers.add_pixel_layer("Background", bounds(), None) {
            unreachable!("{err:?}");
        }
        let group = match layers.add_group("Effects", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = layers.add_pixel_layer("Glow", bounds(), Some(group)) {
            unreachable!("{err:?}");
        }

        // A *sized* root, not `Style::default()`: the tree's own
        // `percent` sizes resolve against a definite parent size, and
        // `auto` isn't one (the same precondition `tree_view`'s own
        // layout tests record).
        let (mut tree, root) = widgets::new_tree(Style {
            size: taffy::Size {
                width: length(300.0_f32),
                height: length(400.0_f32),
            },
            ..Default::default()
        });
        let panel = match insert_panel(&mut tree, root, "Layers") {
            Ok(panel) => panel,
            Err(err) => unreachable!("{err:?}"),
        };
        let scales = test_scales();
        if let Err(err) = populate_layers_panel(&mut tree, panel, &scales, &layers) {
            unreachable!("{err:?}");
        }
        tree.compute_layout(300.0, 400.0);

        let row_ids = rows_of(&tree, tree_root(&tree, panel));
        assert_eq!(row_ids.len(), 2, "two roots: the group and Background");
        let (Some(&first), Some(&second)) = (row_ids.first(), row_ids.get(1)) else {
            unreachable!("just asserted len() == 2");
        };
        let (Some(first_bounds), Some(second_bounds)) = (tree.bounds(first), tree.bounds(second))
        else {
            unreachable!("just laid out");
        };
        assert_eq!(
            second_bounds.x, first_bounds.x,
            "sibling rows must share a left edge, not sit beside each other"
        );
        assert_eq!(
            second_bounds.y,
            first_bounds.y + i64::from(first_bounds.height),
            "the second root row must stack directly under the first, not next to it"
        );

        // ... and the group's own child is indented past it.
        let Some(&child) = tree.children(first).unwrap_or_default().first() else {
            unreachable!("the group has one layer in it");
        };
        let Some(child_bounds) = tree.bounds(child) else {
            unreachable!("just laid out");
        };
        assert!(
            child_bounds.x > first_bounds.x,
            "a nested row must be indented further than its own group: {child_bounds:?} vs \
             {first_bounds:?}"
        );
    }

    /// `tree_view::style`'s `padding.top` is what gives a group's own
    /// row somewhere to be: without it the first child would occupy the
    /// same band, and `hit_test` (which prefers the deeper node) would
    /// hand a click on the group to that child. Through this panel's own
    /// API, since that is where a real click arrives.
    #[test]
    fn a_group_rows_own_strip_hit_tests_to_the_group_not_its_first_child() {
        let mut layers = LayerTree::new();
        let group = match layers.add_group("Effects", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let glow = match layers.add_pixel_layer("Glow", bounds(), Some(group)) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };

        let (mut tree, root) = widgets::new_tree(Style {
            size: taffy::Size {
                width: length(300.0_f32),
                height: length(400.0_f32),
            },
            ..Default::default()
        });
        let panel = match insert_panel(&mut tree, root, "Layers") {
            Ok(panel) => panel,
            Err(err) => unreachable!("{err:?}"),
        };
        let scales = test_scales();
        let rows = match populate_layers_panel(&mut tree, panel, &scales, &layers) {
            Ok(rows) => rows,
            Err(err) => unreachable!("{err:?}"),
        };
        tree.compute_layout(300.0, 400.0);

        let row_ids = rows_of(&tree, tree_root(&tree, panel));
        let Some(&group_row) = row_ids.first() else {
            unreachable!("one root row for the group");
        };
        let Some(&child_row) = tree.children(group_row).unwrap_or_default().first() else {
            unreachable!("the group has one layer in it");
        };
        let (Some(group_bounds), Some(child_bounds)) =
            (tree.bounds(group_row), tree.bounds(child_row))
        else {
            unreachable!("just laid out");
        };

        // The group's own strip is everything above where its child
        // starts.
        #[allow(clippy::cast_precision_loss)]
        let group_point = (
            (group_bounds.x + i64::from(group_bounds.width) / 2) as f32,
            (group_bounds.y + (child_bounds.y - group_bounds.y) / 2) as f32,
        );
        assert_eq!(
            tree.hit_test(group_point),
            Some(group_row),
            "a click on a group's own row must select the group"
        );
        assert_eq!(rows.get(&group_row), Some(&group));

        #[allow(clippy::cast_precision_loss)]
        let child_point = (
            (child_bounds.x + i64::from(child_bounds.width) / 2) as f32,
            (child_bounds.y + i64::from(child_bounds.height) / 2) as f32,
        );
        assert_eq!(tree.hit_test(child_point), Some(child_row));
        assert_eq!(rows.get(&child_row), Some(&glow));
    }

    /// Cross-crate proof of `tree_view`'s "the description lives in
    /// `TreeItemState`" rule, through this panel's own surface: a row's
    /// node is rebuilt from state on every mutation, so a description
    /// that was written onto the node instead would vanish the first
    /// time `aurora-app` selects a layer.
    #[test]
    fn a_row_keeps_its_description_after_being_selected() {
        let mut layers = LayerTree::new();
        let retouch = match layers.add_pixel_layer("Retouch", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = layers.set_blend_mode(retouch, aurora_doc::BlendMode::Multiply) {
            unreachable!("{err:?}");
        }
        if let Err(err) = layers.set_opacity(retouch, 0.8) {
            unreachable!("{err:?}");
        }

        let (mut tree, root) = widgets::new_tree(Style::default());
        let panel = match insert_panel(&mut tree, root, "Layers") {
            Ok(panel) => panel,
            Err(err) => unreachable!("{err:?}"),
        };
        let scales = test_scales();
        if let Err(err) = populate_layers_panel(&mut tree, panel, &scales, &layers) {
            unreachable!("{err:?}");
        }
        let row_ids = rows_of(&tree, tree_root(&tree, panel));
        let Some(&row) = row_ids.first() else {
            unreachable!("just added one layer");
        };

        if let Err(err) = widgets::set_tree_item_selected(&mut tree, row, true) {
            unreachable!("{err:?}");
        }
        let Some(accessibility) = tree.accessibility(row) else {
            unreachable!("still exists");
        };
        assert_eq!(accessibility.is_selected(), Some(true));
        assert_eq!(
            accessibility.description(),
            Some("Multiply, 80%"),
            "selecting a row must not cost it its description"
        );
        assert_eq!(accessibility.label(), Some("Retouch"));
    }

    /// The regression test for the bug this panel shipped with in
    /// 0.77.0: the Layers panel's own box grew one row taller per layer
    /// until it pushed Properties and History off the bottom of the
    /// window entirely (both at zero height, neither hit-testable, at
    /// 43 layers in a 1600x900 window). Deliberately built through the
    /// *real* `build_workspace` rather than a bare `insert_panel`,
    /// because the bug is only observable in a rail with siblings --
    /// which is exactly why every other test in this module missed it.
    #[test]
    fn a_crowded_layers_panel_never_starves_its_sibling_panels() {
        for count in [1_usize, 40, 100, 400] {
            let mut layers = LayerTree::new();
            for i in 0..count {
                if let Err(err) = layers.add_pixel_layer(format!("Layer {i}"), bounds(), None) {
                    unreachable!("{err:?}");
                }
            }

            let mut ws = crate::workspace::build_workspace();
            let scales = test_scales();
            if let Err(err) = populate_layers_panel(&mut ws.tree, ws.layers, &scales, &layers) {
                unreachable!("{err:?}");
            }
            ws.tree.compute_layout(1600.0, 900.0);

            let (Some(layers_bounds), Some(properties_bounds), Some(history_bounds)) = (
                ws.tree.bounds(ws.layers.root),
                ws.tree.bounds(ws.properties.root),
                ws.tree.bounds(ws.history.root),
            ) else {
                unreachable!("just laid out");
            };

            assert!(
                properties_bounds.height > 0 && history_bounds.height > 0,
                "{count} layers must not collapse the sibling panels: \
                 {properties_bounds:?}, {history_bounds:?}"
            );
            assert_eq!(
                layers_bounds.height, history_bounds.height,
                "the three panels must keep sharing the rail equally at {count} layers"
            );
            assert!(
                history_bounds.y + i64::from(history_bounds.height) <= 900,
                "no panel may be pushed off the bottom of the window: {history_bounds:?}"
            );

            // ...and both siblings are still really reachable by a
            // pointer, not merely non-zero on paper.
            for (name, panel_bounds) in [
                ("properties", properties_bounds),
                ("history", history_bounds),
            ] {
                #[allow(clippy::cast_precision_loss)]
                let point = (
                    (panel_bounds.x + i64::from(panel_bounds.width) / 2) as f32,
                    (panel_bounds.y + i64::from(panel_bounds.height) / 2) as f32,
                );
                let hit = ws.tree.hit_test(point);
                assert!(
                    hit.is_some(),
                    "{name} must stay hit-testable at {count} layers"
                );
            }
        }
    }

    /// The honest other half of the fix above, pinned rather than left
    /// implied: bounding the panel means the rows that no longer fit are
    /// *clipped*, and with no scrolling container anywhere in
    /// `aurora-widgets` yet (`tree_view`'s own disclosed gap), clipped
    /// means **unreachable** -- laid out, but past the panel's own
    /// bounds, where `hit_test` refuses to descend. The rows that do fit
    /// still work, which is why this is an improvement on losing the
    /// Properties and History panels outright rather than a finished
    /// Layers panel.
    #[test]
    fn rows_past_the_bottom_of_a_bounded_panel_are_clipped_and_not_yet_reachable() {
        let mut layers = LayerTree::new();
        for i in 0..100 {
            if let Err(err) = layers.add_pixel_layer(format!("Layer {i}"), bounds(), None) {
                unreachable!("{err:?}");
            }
        }

        let mut ws = crate::workspace::build_workspace();
        let scales = test_scales();
        if let Err(err) = populate_layers_panel(&mut ws.tree, ws.layers, &scales, &layers) {
            unreachable!("{err:?}");
        }
        ws.tree.compute_layout(1600.0, 900.0);

        let row_ids = rows_of(&ws.tree, tree_root(&ws.tree, ws.layers));
        assert_eq!(row_ids.len(), 100, "every layer still gets a real row");
        let reachable = row_ids
            .iter()
            .filter(|&&row| {
                let Some(row_bounds) = ws.tree.bounds(row) else {
                    unreachable!("just laid out");
                };
                #[allow(clippy::cast_precision_loss)]
                let point = (
                    (row_bounds.x + i64::from(row_bounds.width) / 2) as f32,
                    (row_bounds.y + i64::from(row_bounds.height) / 2) as f32,
                );
                ws.tree.hit_test(point) == Some(row)
            })
            .count();
        assert!(
            reachable > 0,
            "the rows that fit in the panel must still be clickable"
        );
        assert!(
            reachable < row_ids.len(),
            "and the rest are clipped -- this is the disclosed gap a real scrolling container \
             would close, not a claim that all 100 rows are usable"
        );
    }

    #[test]
    fn populate_layers_panel_rejects_an_unknown_panel_body() {
        let layers = LayerTree::new();
        let (mut tree, root) = widgets::new_tree(Style::default());
        let panel = match insert_panel(&mut tree, root, "Layers") {
            Ok(panel) => panel,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = tree.remove(panel.body) {
            unreachable!("{err:?}");
        }
        let scales = test_scales();
        match populate_layers_panel(&mut tree, panel, &scales, &layers) {
            Err(aurora_widgets::WidgetError::UnknownWidget(id)) => assert_eq!(id, panel.body),
            other => unreachable!("expected UnknownWidget, got {other:?}"),
        }
    }
}
