//! The accessibility tree.
//!
//! Aurora draws pixels; a screen reader reads *this*. Everything a user relies
//! on — role, label, value, focus — exists only because it is stated here. This
//! is invariant §7.3.9 made concrete: a widget without a node is invisible to
//! assistive technology, no matter how it looks.

use crate::field::TextField;
use accesskit::{Node, NodeId, Role, Tree, TreeId, TreeUpdate};

pub const WINDOW: NodeId = NodeId(0);
pub const LABEL: NodeId = NodeId(1);
pub const FIELD: NodeId = NodeId(2);

pub fn update(field: &TextField, focused: bool) -> TreeUpdate {
    let mut window = Node::new(Role::Window);
    window.set_label("Aurora a11y + IME spike");
    window.set_children(vec![LABEL, FIELD]);

    let mut label = Node::new(Role::Label);
    label.set_label("Layer name");

    let mut input = Node::new(Role::TextInput);
    // The label is a separate node *and* linked, so a screen reader can announce
    // "Layer name, edit text" rather than just "edit text".
    input.set_labelled_by(vec![LABEL]);
    input.set_value(field.text.clone());
    input.add_action(accesskit::Action::Focus);
    input.add_action(accesskit::Action::SetValue);
    if !field.preedit.is_empty() {
        // Composition in progress: announce it, or the user hears nothing while
        // typing CJK.
        input.set_description(format!("composing: {}", field.preedit));
    }

    TreeUpdate {
        nodes: vec![
            (WINDOW, window),
            (LABEL, label),
            (FIELD, input),
        ],
        tree: Some(Tree::new(WINDOW)),
        tree_id: TreeId::ROOT,
        focus: if focused { FIELD } else { WINDOW },
    }
}

/// Print the tree without opening a window, so tree *construction* can be
/// checked in CI. Delivery to a screen reader still needs a human ear.
pub fn dump() {
    let mut field = TextField::default();
    field.insert("Background");
    let plain = update(&field, true);

    field.set_preedit("ni hao".to_string());
    let composing = update(&field, true);

    println!("Accessibility tree — idle:");
    print(&plain);
    println!("\nAccessibility tree — mid-composition:");
    print(&composing);

    println!(
        "\nTree construction works. This does NOT prove a screen reader receives it:\n\
         run without --dump-tree, enable VoiceOver / Narrator / Orca, and listen."
    );
}

fn print(update: &TreeUpdate) {
    for (id, node) in &update.nodes {
        let focus = if *id == update.focus { "  <- focus" } else { "" };
        println!(
            "  {:?} role={:?} label={:?} value={:?} desc={:?}{}",
            id,
            node.role(),
            node.label(),
            node.value(),
            node.description(),
            focus
        );
    }
}
