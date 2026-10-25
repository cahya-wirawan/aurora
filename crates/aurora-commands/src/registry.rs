//! The registry: lookup, listing, argument validation and enabled checks
//! over a table of [`CommandSpec`]s and [`FamilySpec`]s.

use std::collections::BTreeSet;

use crate::args::{ArgError, Args, Validated, default_value, validate_command, validate_family};
use crate::describe::{CommandInfo, FamilyInfo, Listing};
use crate::spec::{CommandContext, CommandSpec, FamilySpec, ParamKind, ParamSpec, Presence};

/// What an id names.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Resolved<'a> {
    /// A command in the table.
    Command(&'a CommandSpec),
    /// A member of a family; `target` is the text after the prefix.
    Member {
        family: &'a FamilySpec,
        target: &'a str,
    },
}

impl Resolved<'_> {
    /// The enabled check of whichever it is.
    ///
    /// # Errors
    /// The reason of the first requirement that does not hold.
    pub fn check_enabled(&self, context: &CommandContext) -> Result<(), &'static str> {
        match self {
            Self::Command(spec) => spec.check_enabled(context),
            Self::Member { family, .. } => family.check_enabled(context),
        }
    }
}

/// Why a command could not be checked for availability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum NotEnabled {
    #[error("unknown command")]
    UnknownCommand,
    #[error("{0}")]
    Reason(&'static str),
}

/// A problem with a table itself (found by [`Registry::validate`], which
/// the tests run over the built-in table).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RegistryError {
    #[error("duplicate command id `{0}`")]
    DuplicateId(String),
    #[error("duplicate family prefix `{0}`")]
    DuplicatePrefix(String),
    #[error("`{0}` is not a well-formed id (lower-case words joined by `.`, `_`, `-`)")]
    BadId(String),
    #[error("`{id}` is also reachable as a member of the family `{prefix}`")]
    ShadowedByFamily { id: String, prefix: String },
    #[error("`{command}` has two parameters called `{name}`")]
    DuplicateParam { command: String, name: String },
    #[error("`{command}`.`{name}`: {problem}")]
    BadParam {
        command: String,
        name: String,
        problem: &'static str,
    },
    #[error("`{0}` has no title or no docs")]
    Undocumented(String),
    #[error("`{0}` lists the same requirement twice")]
    DuplicateRequirement(String),
}

/// A table of commands and families.
#[derive(Debug, Clone, Copy)]
pub struct Registry {
    commands: &'static [CommandSpec],
    families: &'static [FamilySpec],
}

impl Registry {
    /// A registry over `commands` and `families`. Nothing is checked here;
    /// [`Self::validate`] does it (the built-in table is validated by this
    /// crate's tests).
    #[must_use]
    pub const fn new(commands: &'static [CommandSpec], families: &'static [FamilySpec]) -> Self {
        Self { commands, families }
    }

    /// The built-in table: every command the application offers.
    #[must_use]
    pub const fn builtin() -> &'static Registry {
        &crate::table::BUILTIN
    }

    /// Every command, in table order (which is the palette's order).
    #[must_use]
    pub const fn commands(&self) -> &'static [CommandSpec] {
        self.commands
    }

    /// Every family.
    #[must_use]
    pub const fn families(&self) -> &'static [FamilySpec] {
        self.families
    }

    /// The commands the palette lists, in order.
    pub fn palette(&self) -> impl Iterator<Item = &'static CommandSpec> {
        self.commands
            .iter()
            .filter(|spec| spec.presence == Presence::Palette)
    }

    /// The command with exactly this id (family members are not commands).
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&'static CommandSpec> {
        self.commands.iter().find(|spec| spec.id == id)
    }

    /// What `id` names: a table command, or a family member.
    #[must_use]
    pub fn resolve<'a>(&self, id: &'a str) -> Option<Resolved<'a>> {
        if let Some(spec) = self.get(id) {
            return Some(Resolved::Command(spec));
        }
        self.families.iter().find_map(|family| {
            family
                .target_of(id)
                .map(|target| Resolved::Member { family, target })
        })
    }

    /// The whole registry as a serialisable description.
    #[must_use]
    pub fn list(&self) -> Listing {
        Listing {
            commands: self.commands.iter().map(CommandInfo::from).collect(),
            families: self.families.iter().map(FamilyInfo::from).collect(),
        }
    }

    /// Validates a call before it runs: the id must exist, every argument
    /// must be a known parameter of the right type and range, required
    /// parameters must be present and defaults are filled in.
    ///
    /// # Errors
    /// [`ArgError`] naming the first problem; see [`validate_command`].
    pub fn validate_args(&self, id: &str, args: &Args) -> Result<Validated, ArgError> {
        match self.resolve(id) {
            None => Err(ArgError::UnknownCommand(id.to_owned())),
            Some(Resolved::Command(spec)) => validate_command(spec, args),
            Some(Resolved::Member { family, .. }) => validate_family(family, id, args),
        }
    }

    /// Whether `id` can run in `context`.
    ///
    /// # Errors
    /// [`NotEnabled::UnknownCommand`] for an id the registry does not know,
    /// else [`NotEnabled::Reason`] with the first failing requirement's
    /// reason.
    pub fn check_enabled(&self, id: &str, context: &CommandContext) -> Result<(), NotEnabled> {
        match self.resolve(id) {
            None => Err(NotEnabled::UnknownCommand),
            Some(resolved) => resolved.check_enabled(context).map_err(NotEnabled::Reason),
        }
    }

    /// Checks the table itself: ids well formed and unique, no id shadowed
    /// by a family prefix, parameters well formed with defaults that pass
    /// their own validation, everything documented.
    ///
    /// # Errors
    /// The first problem found.
    pub fn validate(&self) -> Result<(), RegistryError> {
        let mut seen = BTreeSet::new();
        for spec in self.commands {
            if !well_formed_id(spec.id) {
                return Err(RegistryError::BadId(spec.id.to_owned()));
            }
            if !seen.insert(spec.id) {
                return Err(RegistryError::DuplicateId(spec.id.to_owned()));
            }
            if spec.title.trim().is_empty() || spec.docs.trim().is_empty() {
                return Err(RegistryError::Undocumented(spec.id.to_owned()));
            }
            for family in self.families {
                if family.target_of(spec.id).is_some() {
                    return Err(RegistryError::ShadowedByFamily {
                        id: spec.id.to_owned(),
                        prefix: family.prefix.to_owned(),
                    });
                }
            }
            check_requirements(spec.id, spec.requires)?;
            check_params(spec)?;
        }
        let mut prefixes = BTreeSet::new();
        for family in self.families {
            if !prefixes.insert(family.prefix) {
                return Err(RegistryError::DuplicatePrefix(family.prefix.to_owned()));
            }
            if family.docs.trim().is_empty() || family.title_pattern.trim().is_empty() {
                return Err(RegistryError::Undocumented(family.prefix.to_owned()));
            }
            check_requirements(family.prefix, family.requires)?;
        }
        Ok(())
    }
}

fn check_requirements(
    id: &str,
    requires: &[crate::spec::Requirement],
) -> Result<(), RegistryError> {
    for (index, requirement) in requires.iter().enumerate() {
        if requires
            .iter()
            .take(index)
            .any(|other| other == requirement)
        {
            return Err(RegistryError::DuplicateRequirement(id.to_owned()));
        }
    }
    Ok(())
}

/// Ids are lower-case ASCII words joined by `.`, `_`, `-`, with digits
/// allowed (`crop.ratio.16_9`).
fn well_formed_id(id: &str) -> bool {
    !id.is_empty()
        && !id.starts_with(['.', '_', '-'])
        && !id.ends_with(['.', '_', '-'])
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'))
}

fn check_params(spec: &CommandSpec) -> Result<(), RegistryError> {
    for (index, param) in spec.params.iter().enumerate() {
        if spec.params.iter().take(index).any(|p| p.name == param.name) {
            return Err(RegistryError::DuplicateParam {
                command: spec.id.to_owned(),
                name: param.name.to_owned(),
            });
        }
        let bad = |problem: &'static str| RegistryError::BadParam {
            command: spec.id.to_owned(),
            name: param.name.to_owned(),
            problem,
        };
        if param.name.is_empty() || param.docs.trim().is_empty() {
            return Err(bad("a parameter needs a name and docs"));
        }
        if param.required && param.default.is_some() {
            return Err(bad("a required parameter has no default"));
        }
        if !param.required && param.default.is_none() {
            return Err(bad("an optional parameter needs a default"));
        }
        check_kind(param, &bad)?;
        if let Some(default) = param.default {
            // The default must pass the parameter's own validation.
            let probe = CommandSpec {
                params: std::slice::from_ref(param),
                ..*spec
            };
            let mut args = Args::new();
            args.insert(param.name.to_owned(), default_value(default));
            if validate_command(&probe, &args).is_err() {
                return Err(bad("the default does not pass the parameter's own checks"));
            }
        }
    }
    Ok(())
}

fn check_kind(
    param: &ParamSpec,
    bad: &dyn Fn(&'static str) -> RegistryError,
) -> Result<(), RegistryError> {
    match param.kind {
        ParamKind::Int { min, max } if min > max => Err(bad("min is above max")),
        ParamKind::Number { min, max } if !(min.is_finite() && max.is_finite() && min <= max) => {
            Err(bad("the range is not finite or min is above max"))
        }
        ParamKind::Text { max_chars: 0 } => Err(bad("max_chars is zero")),
        ParamKind::Choice([]) => Err(bad("a choice parameter needs choices")),
        ParamKind::Choice(choices) => {
            for (index, choice) in choices.iter().enumerate() {
                if choice.value.is_empty() || choice.docs.trim().is_empty() {
                    return Err(bad("a choice needs a value and docs"));
                }
                if choices.iter().take(index).any(|c| c.value == choice.value) {
                    return Err(bad("two choices share a value"));
                }
            }
            Ok(())
        }
        _ => Ok(()),
    }
}
