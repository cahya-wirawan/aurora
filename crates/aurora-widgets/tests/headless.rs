//! Proof, not just prose: this crate's whole pipeline — tree
//! construction, `taffy` layout, pointer/focus routing, the concrete
//! widget set, IME composition, and `accesskit` accessibility content —
//! runs to completion with no `winit::Window`, no `wgpu::Device`, and no
//! platform accessibility adapter anywhere in the call graph. That is
//! what "headless mode for automated UI tests" (PLAN.md M1.7) actually
//! asks for; this integration test is the permanent, automatically
//! re-checked version of that claim. If a future change to this crate
//! ever makes some part of the widget pipeline require a real window or
//! GPU to exercise, this test — not a doc comment — is what fails.
//!
//! Uses only `aurora_widgets`' public API: the same surface `aurora-ui`
//! will eventually build real panels against, exercised here exactly as
//! an external consumer would use it (an integration test, unlike every
//! other test in this crate, has no access to private internals).

use accesskit::{Action, Role, Toggled};
use aurora_theme::Scales;
use aurora_widgets::widgets::{self, WidgetKind};
use aurora_widgets::{FocusManager, hit_test};
use taffy::{FlexDirection, Style};

fn scales() -> Scales {
    const SCALES_TOML: &str = include_str!("../../../design/tokens/scales.toml");
    match Scales::from_toml_str(SCALES_TOML) {
        Ok(scales) => scales,
        Err(err) => unreachable!("the real, committed scales must parse: {err:?}"),
    }
}

#[test]
// One deliberately linear scenario (build -> layout -> input -> mutate
// -> accessibility) exercising the whole pipeline end to end -- splitting
// it into smaller functions would just chop up one story, not express
// separate ones.
#[allow(clippy::too_many_lines)]
fn a_small_form_builds_lays_out_and_exposes_accessibility_with_no_window_or_gpu() {
    let scales = scales();
    let root_style = Style {
        flex_direction: FlexDirection::Column,
        ..Default::default()
    };
    let (mut tree, root) = widgets::new_tree(root_style);

    let save = match widgets::insert_button(&mut tree, root, &scales, "Save") {
        Ok(id) => id,
        Err(err) => unreachable!("{err:?}"),
    };
    let remember = match widgets::insert_checkbox(&mut tree, root, &scales, "Remember me") {
        Ok(id) => id,
        Err(err) => unreachable!("{err:?}"),
    };
    let volume = match widgets::insert_slider(&mut tree, root, &scales, "Volume", 50.0, 0.0, 100.0)
    {
        Ok(id) => id,
        Err(err) => unreachable!("{err:?}"),
    };
    let name = match widgets::insert_text_field(&mut tree, root, &scales, "Name", "Ada") {
        Ok(id) => id,
        Err(err) => unreachable!("{err:?}"),
    };
    assert_eq!(tree.len(), 5, "root + 4 widgets");

    // Layout: a fixed-size viewport standing in for a window's client
    // area -- no window was created to get it.
    tree.compute_layout(400.0, 300.0);
    let Some(name_bounds) = tree.bounds(name) else {
        unreachable!("just laid out");
    };
    assert_ne!(name_bounds.width, 0, "a real width must have been computed");

    // Input/focus: Tab cycles through every focusable widget in order,
    // then wraps.
    let mut focus = FocusManager::default();
    for expected in [save, remember, volume, name] {
        assert_eq!(focus.focus_next(&mut tree), Some(expected));
    }
    assert_eq!(focus.focus_next(&mut tree), Some(save), "must wrap around");

    // hit_test: a document-space point inside the text field resolves to
    // it, purely from the computed bounds above.
    #[allow(clippy::cast_precision_loss)]
    let point = (name_bounds.x as f64 + 1.0, name_bounds.y as f64 + 1.0);
    assert_eq!(hit_test(&tree, point.0, point.1), Some(name));

    // Mutate every widget through its own public API.
    if let Err(err) = widgets::set_button_pressed(&mut tree, save, true) {
        unreachable!("{err:?}");
    }
    if let Err(err) = widgets::toggle_checkbox(&mut tree, remember) {
        unreachable!("{err:?}");
    }
    if let Err(err) = widgets::set_slider_value(&mut tree, volume, 75.0) {
        unreachable!("{err:?}");
    }
    if let Err(err) = widgets::with_text_field_mut(&mut tree, name, |field| {
        field.move_to_end(false);
        field.insert_str(" Lovelace");
    }) {
        unreachable!("{err:?}");
    }
    // IME: a short composition (as if a CJK IME were mid-composition),
    // then committed -- the exact `winit::event::Ime::Preedit`/`Commit`
    // shape `set_composition`/`commit_composition` mirror.
    if let Err(err) =
        widgets::with_text_field_mut(&mut tree, name, |field| field.set_composition("ni", None))
    {
        unreachable!("{err:?}");
    }
    if let Err(err) =
        widgets::with_text_field_mut(&mut tree, name, |field| field.commit_composition("你"))
    {
        unreachable!("{err:?}");
    }

    // Accessibility: build the same kind of `accesskit::TreeUpdate` a
    // platform adapter would consume, and inspect it directly -- no
    // `accesskit_winit`, no live screen reader, no platform at all.
    let update = tree.accessibility_update(name);
    assert_eq!(update.nodes.len(), 5, "root + 4 widgets");
    assert_eq!(update.focus, name);

    let node_for = |id| {
        update
            .nodes
            .iter()
            .find(|(node_id, _)| *node_id == id)
            .map(|(_, node)| node)
    };

    let Some(save_node) = node_for(save) else {
        unreachable!("save must be in the update");
    };
    assert_eq!(save_node.role(), Role::Button);
    assert!(save_node.supports_action(Action::Click));

    let Some(remember_node) = node_for(remember) else {
        unreachable!("remember must be in the update");
    };
    assert_eq!(remember_node.toggled(), Some(Toggled::True));

    let Some(volume_node) = node_for(volume) else {
        unreachable!("volume must be in the update");
    };
    #[allow(clippy::float_cmp)]
    {
        assert_eq!(volume_node.numeric_value(), Some(75.0));
    }

    let Some(name_node) = node_for(name) else {
        unreachable!("name must be in the update");
    };
    assert_eq!(name_node.value(), Some("Ada Lovelace你"));
    assert_eq!(
        name_node.description(),
        None,
        "composition was committed, so nothing should still be announced as composing"
    );

    match tree.payload(save) {
        Some(WidgetKind::Button(state)) => assert!(state.pressed),
        other => unreachable!("expected Button, got {other:?}"),
    }
}
