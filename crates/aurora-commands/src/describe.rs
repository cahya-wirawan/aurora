//! The owned, serialisable description of the registry that
//! [`crate::Registry::list`] returns: what a front end shows, or sends to a
//! model, to say which commands exist and how to call them.
//!
//! These types are plain data with derived `Serialize`/`Deserialize`
//! (externally tagged enums, so non-self-describing formats round-trip
//! them too); the static table in [`crate::table`] is converted into them
//! on demand.

use serde::{Deserialize, Serialize};

use crate::spec::{
    Access, Category, CommandSpec, FamilySpec, ParamDefault, ParamKind, ParamSpec, PathUse,
    Presence, Requirement, TargetKind,
};

/// A parameter's default, as listed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DefaultInfo {
    Bool(bool),
    Int(i64),
    Number(f64),
    Text(String),
}

/// One allowed value of a choice parameter, as listed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChoiceInfo {
    pub value: String,
    pub docs: String,
}

/// A parameter's type and range, as listed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KindInfo {
    Bool,
    Int { min: i64, max: i64 },
    Number { min: f64, max: f64 },
    Text { max_chars: usize },
    Path { mode: PathUse, max_chars: usize },
    Choice { choices: Vec<ChoiceInfo> },
}

/// A parameter, as listed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParamInfo {
    pub name: String,
    pub docs: String,
    pub kind: KindInfo,
    pub required: bool,
    pub default: Option<DefaultInfo>,
}

/// A requirement with its reason, as listed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequirementInfo {
    pub requirement: Requirement,
    pub reason: String,
}

/// A command, as listed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandInfo {
    pub id: String,
    pub title: String,
    pub docs: String,
    pub category: Category,
    pub access: Access,
    pub params: Vec<ParamInfo>,
    pub requires: Vec<RequirementInfo>,
    pub presence: Presence,
}

/// What a family's target is, as listed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetInfo {
    Name { max_chars: usize },
    Id,
}

/// A command family, as listed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FamilyInfo {
    pub prefix: String,
    pub title_pattern: String,
    pub docs: String,
    pub category: Category,
    pub access: Access,
    pub requires: Vec<RequirementInfo>,
    pub target_name: String,
    pub target: TargetInfo,
}

/// The whole registry, as listed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Listing {
    pub commands: Vec<CommandInfo>,
    pub families: Vec<FamilyInfo>,
}

fn requirement_infos(requires: &[Requirement]) -> Vec<RequirementInfo> {
    requires
        .iter()
        .map(|&requirement| RequirementInfo {
            requirement,
            reason: requirement.reason().to_owned(),
        })
        .collect()
}

impl From<ParamDefault> for DefaultInfo {
    fn from(default: ParamDefault) -> Self {
        match default {
            ParamDefault::Bool(value) => Self::Bool(value),
            ParamDefault::Int(value) => Self::Int(value),
            ParamDefault::Number(value) => Self::Number(value),
            ParamDefault::Text(value) => Self::Text(value.to_owned()),
        }
    }
}

impl From<&ParamKind> for KindInfo {
    fn from(kind: &ParamKind) -> Self {
        match *kind {
            ParamKind::Bool => Self::Bool,
            ParamKind::Int { min, max } => Self::Int { min, max },
            ParamKind::Number { min, max } => Self::Number { min, max },
            ParamKind::Text { max_chars } => Self::Text { max_chars },
            ParamKind::Path { mode } => Self::Path {
                mode,
                max_chars: crate::spec::MAX_PATH_CHARS,
            },
            ParamKind::Choice(choices) => Self::Choice {
                choices: choices
                    .iter()
                    .map(|choice| ChoiceInfo {
                        value: choice.value.to_owned(),
                        docs: choice.docs.to_owned(),
                    })
                    .collect(),
            },
        }
    }
}

impl From<&ParamSpec> for ParamInfo {
    fn from(param: &ParamSpec) -> Self {
        Self {
            name: param.name.to_owned(),
            docs: param.docs.to_owned(),
            kind: KindInfo::from(&param.kind),
            required: param.required,
            default: param.default.map(DefaultInfo::from),
        }
    }
}

impl From<&CommandSpec> for CommandInfo {
    fn from(spec: &CommandSpec) -> Self {
        Self {
            id: spec.id.to_owned(),
            title: spec.title.to_owned(),
            docs: spec.docs.to_owned(),
            category: spec.category,
            access: spec.access,
            params: spec.params.iter().map(ParamInfo::from).collect(),
            requires: requirement_infos(spec.requires),
            presence: spec.presence,
        }
    }
}

impl From<&FamilySpec> for FamilyInfo {
    fn from(spec: &FamilySpec) -> Self {
        Self {
            prefix: spec.prefix.to_owned(),
            title_pattern: spec.title_pattern.to_owned(),
            docs: spec.docs.to_owned(),
            category: spec.category,
            access: spec.access,
            requires: requirement_infos(spec.requires),
            target_name: spec.target_name.to_owned(),
            target: match spec.target {
                TargetKind::Name { max_chars } => TargetInfo::Name { max_chars },
                TargetKind::Id => TargetInfo::Id,
            },
        }
    }
}
