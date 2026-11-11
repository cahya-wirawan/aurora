//! A flat chrome strip (task 172): a container that fills its box with one
//! surface token. See [`WidgetKind::Strip`].

use accesskit::Node;
use taffy::Style;

use super::WidgetKind;
use crate::error::WidgetError;
use crate::tree::{WidgetId, WidgetTree};

/// Which surface token a [`WidgetKind::Strip`] (or a tab bar) fills with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StripSurface {
    /// `surface.chrome`: the window's chrome strips (tools column, options
    /// bar, status bar, document tab strip).
    Chrome,
    /// `surface.header`: a panel's title strip and a panel group's tab row.
    Header,
    /// `surface.panel`: a panel's own colour, used by the title tab of a
    /// lone panel and by the selected tab of a panel group (task 173), so
    /// the tab joins the body below it.
    Panel,
}

/// Inserts a [`WidgetKind::Strip`] under `parent` with the given layout
/// style and accessibility node.
///
/// # Errors
///
/// [`WidgetError::UnknownWidget`] if `parent` does not exist.
pub fn insert_strip(
    tree: &mut WidgetTree<WidgetKind>,
    parent: WidgetId,
    style: Style,
    node: Node,
    surface: StripSurface,
) -> Result<WidgetId, WidgetError> {
    tree.insert(parent, style, node, WidgetKind::Strip(surface))
}
