//! The static description of one command: what it is called, what it does,
//! what it takes, what it may change and when it may run.
//!
//! Everything here is `const`-constructible plain data (`&'static` strings
//! and slices), so the built-in table in [`crate::table`] is a `static`
//! with no allocation and no start-up cost. The owned, serialisable
//! counterparts live in [`crate::describe`].

use serde::{Deserialize, Serialize};

/// The menu or area a command belongs to; used for grouping a listing, not
/// for dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    File,
    Edit,
    Layer,
    Select,
    View,
    Window,
    Tool,
    Colour,
    Paths,
}

impl Category {
    /// Every category, in listing order.
    pub const ALL: [Category; 9] = [
        Category::File,
        Category::Edit,
        Category::Layer,
        Category::Select,
        Category::View,
        Category::Window,
        Category::Tool,
        Category::Colour,
        Category::Paths,
    ];
}

/// What a command may change: the hint an automation front end turns into
/// its read-only / destructive annotations. It is a hint about the command,
/// not a guarantee about every argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Access {
    /// Changes nothing that outlives the call: it reports, checks or looks.
    Read,
    /// Changes only the application's own state outside the document and
    /// every file: the focused or visible panel, the active tool, the
    /// colours, the zoom, the selection-tool mode, the system clipboard.
    /// Nothing here is an undo step and nothing makes a document unsaved.
    Ui,
    /// Edits the document (it is, or can be, an undo step), writes a file,
    /// or changes the set of open documents.
    Write,
}

/// A condition on the application's state a command needs before it can
/// run. A command lists its requirements in [`CommandSpec::requires`]; it
/// is enabled when every one holds ([`Requirement::check`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Requirement {
    /// There is an undo step to undo.
    CanUndo,
    /// There is an undone step to redo.
    CanRedo,
    /// The active layer may be deleted (it is not the last pixel layer and
    /// is not locked with Lock All).
    DeleteAllowed,
    /// Quick Mask mode is off (selection commands are refused inside it).
    NotQuickMask,
    /// The active layer is clipped to the one below.
    Clipped,
    /// The active layer is not clipped.
    NotClipped,
    /// The active layer could be clipped (it is a layer with one below it
    /// in its group, not a group).
    ClipAllowed,
}

/// The application facts [`Requirement`]s are decided from. The app fills it
/// once per question; the registry has no notion of a document.
#[allow(clippy::struct_excessive_bools)] // independent facts, a flag each
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CommandContext {
    pub can_undo: bool,
    pub can_redo: bool,
    /// Deleting the active layer is allowed.
    pub delete_allowed: bool,
    /// Quick Mask mode is on.
    pub quick_mask: bool,
    /// The active layer is clipped.
    pub clipped: bool,
    /// Clipping the active layer is refused (a group, the bottom layer, no
    /// active layer).
    pub clip_refused: bool,
}

impl Requirement {
    /// Why the command cannot run when this requirement fails: a calm
    /// sentence for a disabled menu item, a tooltip or an automation error.
    #[must_use]
    pub const fn reason(self) -> &'static str {
        match self {
            Self::CanUndo => "There is nothing to undo.",
            Self::CanRedo => "There is nothing to redo.",
            Self::DeleteAllowed => "The active layer cannot be deleted.",
            Self::NotQuickMask => "Not available in Quick Mask mode.",
            Self::Clipped => "The active layer is not clipped.",
            Self::NotClipped => "The active layer is already clipped.",
            Self::ClipAllowed => "The active layer cannot be clipped.",
        }
    }

    /// Whether the requirement holds in `context`.
    #[must_use]
    pub const fn holds(self, context: &CommandContext) -> bool {
        match self {
            Self::CanUndo => context.can_undo,
            Self::CanRedo => context.can_redo,
            Self::DeleteAllowed => context.delete_allowed,
            Self::NotQuickMask => !context.quick_mask,
            Self::Clipped => context.clipped,
            Self::NotClipped => !context.clipped,
            Self::ClipAllowed => !context.clip_refused,
        }
    }

    /// `Ok` when the requirement holds, else its reason.
    ///
    /// # Errors
    /// [`Requirement::reason`] when it does not hold.
    pub const fn check(self, context: &CommandContext) -> Result<(), &'static str> {
        if self.holds(context) {
            Ok(())
        } else {
            Err(self.reason())
        }
    }
}

/// One allowed value of a [`ParamKind::Choice`] parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Choice {
    pub value: &'static str,
    pub docs: &'static str,
}

/// Whether a path parameter names something to read or something to write.
/// The registry only checks that it is a plausible path string; deciding
/// where a caller may read or write is the host's job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathUse {
    Read,
    Write,
}

/// The type of a parameter and the range its values must lie in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ParamKind {
    Bool,
    /// A whole number, `min..=max`.
    Int {
        min: i64,
        max: i64,
    },
    /// A finite number, `min..=max`.
    Number {
        min: f64,
        max: f64,
    },
    /// Non-empty text of at most `max_chars` characters, no control
    /// characters.
    Text {
        max_chars: usize,
    },
    /// A file-system path (text rules, up to [`MAX_PATH_CHARS`]).
    Path {
        mode: PathUse,
    },
    /// One of a fixed set of strings.
    Choice(&'static [Choice]),
}

/// The longest path string a [`ParamKind::Path`] accepts, in characters.
pub const MAX_PATH_CHARS: usize = 4096;

/// The value a parameter takes when the caller omits it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ParamDefault {
    Bool(bool),
    Int(i64),
    Number(f64),
    Text(&'static str),
}

/// One named parameter of a command.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParamSpec {
    pub name: &'static str,
    pub docs: &'static str,
    pub kind: ParamKind,
    /// A required parameter has no default and must be given.
    pub required: bool,
    /// Used when the parameter is optional and omitted.
    pub default: Option<ParamDefault>,
}

impl ParamSpec {
    /// A required parameter.
    #[must_use]
    pub const fn required(name: &'static str, docs: &'static str, kind: ParamKind) -> Self {
        Self {
            name,
            docs,
            kind,
            required: true,
            default: None,
        }
    }

    /// An optional parameter with a default.
    #[must_use]
    pub const fn optional(
        name: &'static str,
        docs: &'static str,
        kind: ParamKind,
        default: ParamDefault,
    ) -> Self {
        Self {
            name,
            docs,
            kind,
            required: false,
            default: Some(default),
        }
    }
}

/// Where the application offers a command besides its menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Presence {
    /// Listed by the command palette from the start, under its title.
    Palette,
    /// Added to the palette by the application when it opens, with a title
    /// the application makes up (`Reset Essentials`); the registry's title
    /// is the generic one.
    Dynamic,
    /// In a menu only.
    MenuOnly,
}

/// One command: a stable id and everything a caller needs to know to offer
/// it, validate a call to it and decide whether it can run.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CommandSpec {
    /// The stable id (`file.open`, `layer.new_curves`, `tool.brush`). Ids are
    /// pinned by a golden list; renaming one is a breaking change.
    pub id: &'static str,
    /// The title the palette and menus show.
    pub title: &'static str,
    /// What the command does, for a person or a model reading the listing.
    pub docs: &'static str,
    pub category: Category,
    pub access: Access,
    /// The typed parameters an automation call may pass. Interactive use
    /// gathers the same values with a prompt or dialog instead.
    pub params: &'static [ParamSpec],
    /// Conditions that must all hold for the command to run.
    pub requires: &'static [Requirement],
    /// Whether and how the palette lists it.
    pub presence: Presence,
}

impl CommandSpec {
    /// A command with no parameters, no requirements, listed in the palette.
    #[must_use]
    pub const fn new(
        id: &'static str,
        title: &'static str,
        docs: &'static str,
        category: Category,
        access: Access,
    ) -> Self {
        Self {
            id,
            title,
            docs,
            category,
            access,
            params: &[],
            requires: &[],
            presence: Presence::Palette,
        }
    }

    /// Sets the parameters.
    #[must_use]
    pub const fn params(mut self, params: &'static [ParamSpec]) -> Self {
        self.params = params;
        self
    }

    /// Sets the requirements.
    #[must_use]
    pub const fn requires(mut self, requires: &'static [Requirement]) -> Self {
        self.requires = requires;
        self
    }

    /// Leaves the command out of the palette (menu-only).
    #[must_use]
    pub const fn menu_only(mut self) -> Self {
        self.presence = Presence::MenuOnly;
        self
    }

    /// Marks the command as added to the palette by the application at run
    /// time, under a title it chooses.
    #[must_use]
    pub const fn dynamic(mut self) -> Self {
        self.presence = Presence::Dynamic;
        self
    }

    /// Checks every requirement in order.
    ///
    /// # Errors
    /// The reason of the first requirement that does not hold.
    pub fn check_enabled(&self, context: &CommandContext) -> Result<(), &'static str> {
        for requirement in self.requires {
            requirement.check(context)?;
        }
        Ok(())
    }
}

/// What the part of a family id after its prefix names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    /// A name chosen by the user (a workspace, a saved selection).
    Name { max_chars: usize },
    /// A numeric id (an open document's).
    Id,
}

/// A family of commands whose ids are `prefix` + a target the application
/// creates at run time: one "Workspace: name" per workspace, one "Document:
/// name" per open document, one "Load Selection: name" per saved selection.
/// The family is described once; its members come and go.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FamilySpec {
    /// The id prefix, including its separator (`workspace.switch/`).
    pub prefix: &'static str,
    /// What the title of a member looks like, for a listing (`Workspace:
    /// <name>`). The real titles come from the application.
    pub title_pattern: &'static str,
    pub docs: &'static str,
    pub category: Category,
    pub access: Access,
    pub requires: &'static [Requirement],
    /// The name the target goes by in documentation (`name`, `id`).
    pub target_name: &'static str,
    pub target: TargetKind,
}

impl FamilySpec {
    /// The target of `id` when it is a member of this family: the text
    /// after the prefix, which must not be empty.
    #[must_use]
    pub fn target_of<'a>(&self, id: &'a str) -> Option<&'a str> {
        id.strip_prefix(self.prefix).filter(|rest| !rest.is_empty())
    }

    /// Checks every requirement in order.
    ///
    /// # Errors
    /// The reason of the first requirement that does not hold.
    pub fn check_enabled(&self, context: &CommandContext) -> Result<(), &'static str> {
        for requirement in self.requires {
            requirement.check(context)?;
        }
        Ok(())
    }
}
