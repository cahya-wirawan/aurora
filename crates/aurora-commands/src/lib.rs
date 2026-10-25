//! The command registry: one description of every editor operation, shared
//! by the command palette, the menus and (later) automation.
//!
//! A **command** has a stable id (`layer.new_curves`), a title, docs, a
//! category, an [`Access`] hint (what it may change), typed
//! [`ParamSpec`] parameters, [`Requirement`]s that decide whether it is
//! enabled (with a reason when it is not) and a flag for whether the
//! palette lists it. A **family** ([`FamilySpec`]) describes commands whose
//! ids the application makes up at run time from a prefix (one "Workspace:
//! name" per workspace).
//!
//! This crate is **pure metadata and validation**: it knows no document, no
//! window and no file system, and depends on nothing but `serde`. Running a
//! command stays in `aurora-app`; the registry says what may be asked of
//! it ([`Registry::validate_args`]), whether it can run now
//! ([`Registry::check_enabled`]) and how to describe it
//! ([`Registry::list`]).
//!
//! The built-in table is [`Registry::builtin`]; `docs/command-registry.md`
//! says how to add a command.

mod args;
mod describe;
mod registry;
mod spec;
mod table;

pub use args::{
    ArgError, ArgValue, Args, Validated, default_value, validate_command, validate_family,
};
pub use describe::{
    ChoiceInfo, CommandInfo, DefaultInfo, FamilyInfo, KindInfo, Listing, ParamInfo,
    RequirementInfo, TargetInfo,
};
pub use registry::{NotEnabled, Registry, RegistryError, Resolved};
pub use spec::{
    Access, Category, Choice, CommandContext, CommandSpec, FamilySpec, MAX_PATH_CHARS,
    ParamDefault, ParamKind, ParamSpec, PathUse, Presence, Requirement, TargetKind,
};
