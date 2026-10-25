//! Call arguments, their validation and the errors validation reports.
//!
//! [`crate::Registry::validate_args`] runs **before** a command runs: unknown names are
//! refused, every value is checked against its parameter's type and range,
//! required parameters must be present, defaults are filled in. A caller
//! gets back a [`Validated`] it can trust to be well-typed, or an
//! [`ArgError`] naming the first problem. Nothing here touches a document
//! or a file; a path is checked as a string only.

use std::collections::BTreeMap;
use std::fmt;

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::spec::{
    CommandSpec, FamilySpec, MAX_PATH_CHARS, ParamDefault, ParamKind, ParamSpec, TargetKind,
};

/// One argument value as a caller states it. Serialises as the bare JSON
/// value (`true`, `5`, `0.5`, `"text"`) and deserialises from any
/// self-describing format by the value's own type, so an integer stays an
/// integer and `5.0` stays a number.
#[derive(Debug, Clone, PartialEq)]
pub enum ArgValue {
    Bool(bool),
    Int(i64),
    Number(f64),
    Text(String),
}

impl ArgValue {
    /// The type's name as errors and documentation use it.
    #[must_use]
    pub const fn type_name(&self) -> &'static str {
        match self {
            Self::Bool(_) => "a boolean",
            Self::Int(_) => "an integer",
            Self::Number(_) => "a number",
            Self::Text(_) => "text",
        }
    }
}

impl Serialize for ArgValue {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Bool(value) => serializer.serialize_bool(*value),
            Self::Int(value) => serializer.serialize_i64(*value),
            Self::Number(value) => serializer.serialize_f64(*value),
            Self::Text(value) => serializer.serialize_str(value),
        }
    }
}

struct ArgValueVisitor;

impl Visitor<'_> for ArgValueVisitor {
    type Value = ArgValue;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a boolean, a number or a string")
    }

    fn visit_bool<E: de::Error>(self, value: bool) -> Result<ArgValue, E> {
        Ok(ArgValue::Bool(value))
    }

    fn visit_i64<E: de::Error>(self, value: i64) -> Result<ArgValue, E> {
        Ok(ArgValue::Int(value))
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> Result<ArgValue, E> {
        match i64::try_from(value) {
            Ok(value) => Ok(ArgValue::Int(value)),
            // Larger than any parameter's range; keep the magnitude so the
            // range check names it.
            #[allow(clippy::cast_precision_loss)]
            Err(_) => Ok(ArgValue::Number(value as f64)),
        }
    }

    fn visit_f64<E: de::Error>(self, value: f64) -> Result<ArgValue, E> {
        Ok(ArgValue::Number(value))
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<ArgValue, E> {
        Ok(ArgValue::Text(value.to_owned()))
    }

    fn visit_string<E: de::Error>(self, value: String) -> Result<ArgValue, E> {
        Ok(ArgValue::Text(value))
    }
}

impl<'de> Deserialize<'de> for ArgValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(ArgValueVisitor)
    }
}

/// A call's arguments by parameter name.
pub type Args = BTreeMap<String, ArgValue>;

/// Why a call's arguments were refused.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ArgError {
    #[error("unknown command `{0}`")]
    UnknownCommand(String),
    #[error("`{command}` has no parameter `{name}`")]
    UnknownArg { command: String, name: String },
    #[error("`{command}` needs the parameter `{name}`")]
    MissingArg { command: String, name: String },
    #[error("`{name}` must be {expected}, not {got}")]
    WrongType {
        name: String,
        expected: &'static str,
        got: &'static str,
    },
    #[error("`{name}` must be between {min} and {max}, not {got}")]
    OutOfRange {
        name: String,
        min: String,
        max: String,
        got: String,
    },
    #[error("`{name}` must be a finite number")]
    NotFinite { name: String },
    #[error("`{name}` must be one of {choices}, not `{got}`")]
    NotAChoice {
        name: String,
        choices: String,
        got: String,
    },
    #[error("`{name}` must not be empty")]
    Empty { name: String },
    #[error("`{name}` is {len} characters; at most {max} are allowed")]
    TooLong {
        name: String,
        len: usize,
        max: usize,
    },
    #[error("`{name}` must not contain control characters")]
    ControlCharacter { name: String },
    #[error("`{command}` needs a {target} after `{prefix}`")]
    MissingTarget {
        command: String,
        prefix: String,
        target: String,
    },
    #[error("`{command}` takes the {target} after `{prefix}` as a whole number, not `{got}`")]
    BadTarget {
        command: String,
        prefix: String,
        target: String,
        got: String,
    },
}

/// A call whose arguments passed validation. Every parameter with a default
/// is present; nothing else is.
#[derive(Debug, Clone, PartialEq)]
pub struct Validated {
    /// The id as given (for a family member, the whole id).
    pub id: String,
    /// For a family member, the text after the prefix.
    pub target: Option<String>,
    pub args: Args,
}

impl Validated {
    /// The value of parameter `name`, if present.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&ArgValue> {
        self.args.get(name)
    }

    #[must_use]
    pub fn bool(&self, name: &str) -> Option<bool> {
        match self.get(name) {
            Some(ArgValue::Bool(value)) => Some(*value),
            _ => None,
        }
    }

    #[must_use]
    pub fn int(&self, name: &str) -> Option<i64> {
        match self.get(name) {
            Some(ArgValue::Int(value)) => Some(*value),
            _ => None,
        }
    }

    /// A number parameter's value (an integer given for it is converted).
    #[must_use]
    pub fn number(&self, name: &str) -> Option<f64> {
        match self.get(name) {
            Some(ArgValue::Number(value)) => Some(*value),
            _ => None,
        }
    }

    /// A text, path or choice parameter's value.
    #[must_use]
    pub fn text(&self, name: &str) -> Option<&str> {
        match self.get(name) {
            Some(ArgValue::Text(value)) => Some(value),
            _ => None,
        }
    }
}

fn has_control(text: &str) -> bool {
    text.chars().any(char::is_control)
}

fn validate_text(name: &str, text: &str, max_chars: usize) -> Result<(), ArgError> {
    if text.is_empty() {
        return Err(ArgError::Empty {
            name: name.to_owned(),
        });
    }
    let len = text.chars().count();
    if len > max_chars {
        return Err(ArgError::TooLong {
            name: name.to_owned(),
            len,
            max: max_chars,
        });
    }
    if has_control(text) {
        return Err(ArgError::ControlCharacter {
            name: name.to_owned(),
        });
    }
    Ok(())
}

/// Checks one value against its parameter, converting an integer given for
/// a number parameter to a number.
fn validate_value(param: &ParamSpec, value: &ArgValue) -> Result<ArgValue, ArgError> {
    let name = param.name;
    let wrong = |expected: &'static str| ArgError::WrongType {
        name: name.to_owned(),
        expected,
        got: value.type_name(),
    };
    match param.kind {
        ParamKind::Bool => match value {
            ArgValue::Bool(_) => Ok(value.clone()),
            _ => Err(wrong("a boolean")),
        },
        ParamKind::Int { min, max } => match value {
            ArgValue::Int(got) if (min..=max).contains(got) => Ok(value.clone()),
            ArgValue::Int(got) => Err(ArgError::OutOfRange {
                name: name.to_owned(),
                min: min.to_string(),
                max: max.to_string(),
                got: got.to_string(),
            }),
            _ => Err(wrong("an integer")),
        },
        ParamKind::Number { min, max } => {
            #[allow(clippy::cast_precision_loss)] // exact for any plausible parameter
            let got = match value {
                ArgValue::Number(got) => *got,
                ArgValue::Int(got) => *got as f64,
                _ => return Err(wrong("a number")),
            };
            if !got.is_finite() {
                return Err(ArgError::NotFinite {
                    name: name.to_owned(),
                });
            }
            if got < min || got > max {
                return Err(ArgError::OutOfRange {
                    name: name.to_owned(),
                    min: min.to_string(),
                    max: max.to_string(),
                    got: got.to_string(),
                });
            }
            Ok(ArgValue::Number(got))
        }
        ParamKind::Text { max_chars } => match value {
            ArgValue::Text(text) => {
                validate_text(name, text, max_chars)?;
                Ok(value.clone())
            }
            _ => Err(wrong("text")),
        },
        ParamKind::Path { .. } => match value {
            ArgValue::Text(text) => {
                validate_text(name, text, MAX_PATH_CHARS)?;
                Ok(value.clone())
            }
            _ => Err(wrong("a path (text)")),
        },
        ParamKind::Choice(choices) => match value {
            ArgValue::Text(text) if choices.iter().any(|choice| choice.value == text) => {
                Ok(value.clone())
            }
            ArgValue::Text(text) => Err(ArgError::NotAChoice {
                name: name.to_owned(),
                choices: choices
                    .iter()
                    .map(|choice| choice.value)
                    .collect::<Vec<_>>()
                    .join(", "),
                got: text.clone(),
            }),
            _ => Err(wrong("one of the listed choices (text)")),
        },
    }
}

/// A default as an argument value.
#[must_use]
pub fn default_value(default: ParamDefault) -> ArgValue {
    match default {
        ParamDefault::Bool(value) => ArgValue::Bool(value),
        ParamDefault::Int(value) => ArgValue::Int(value),
        ParamDefault::Number(value) => ArgValue::Number(value),
        ParamDefault::Text(value) => ArgValue::Text(value.to_owned()),
    }
}

/// Validates a call to the plain command `spec`.
///
/// # Errors
/// The first problem found: an unknown argument name (in name order), then
/// per parameter in declaration order a missing required value, a value of
/// the wrong type, out of range, not finite, not a listed choice, empty,
/// too long or holding control characters.
pub fn validate_command(spec: &CommandSpec, args: &Args) -> Result<Validated, ArgError> {
    for name in args.keys() {
        if !spec.params.iter().any(|param| param.name == name) {
            return Err(ArgError::UnknownArg {
                command: spec.id.to_owned(),
                name: name.clone(),
            });
        }
    }
    let mut checked = Args::new();
    for param in spec.params {
        match args.get(param.name) {
            Some(value) => {
                checked.insert(param.name.to_owned(), validate_value(param, value)?);
            }
            None => match param.default {
                Some(default) if !param.required => {
                    checked.insert(param.name.to_owned(), default_value(default));
                }
                _ => {
                    return Err(ArgError::MissingArg {
                        command: spec.id.to_owned(),
                        name: param.name.to_owned(),
                    });
                }
            },
        }
    }
    Ok(Validated {
        id: spec.id.to_owned(),
        target: None,
        args: checked,
    })
}

/// Validates a call to a member of `family` named by `id`: the target after
/// the prefix is checked by kind, and the member takes no arguments.
///
/// # Errors
/// [`ArgError::MissingTarget`] / [`ArgError::BadTarget`] for the target,
/// [`ArgError::UnknownArg`] for any argument.
pub fn validate_family(family: &FamilySpec, id: &str, args: &Args) -> Result<Validated, ArgError> {
    let Some(target) = family.target_of(id) else {
        return Err(ArgError::MissingTarget {
            command: id.to_owned(),
            prefix: family.prefix.to_owned(),
            target: family.target_name.to_owned(),
        });
    };
    match family.target {
        TargetKind::Name { max_chars } => {
            validate_text(family.target_name, target, max_chars).map_err(|_| {
                ArgError::BadTarget {
                    command: id.to_owned(),
                    prefix: family.prefix.to_owned(),
                    target: family.target_name.to_owned(),
                    got: target.to_owned(),
                }
            })?;
        }
        TargetKind::Id => {
            if !target.bytes().all(|b| b.is_ascii_digit()) || target.parse::<u64>().is_err() {
                return Err(ArgError::BadTarget {
                    command: id.to_owned(),
                    prefix: family.prefix.to_owned(),
                    target: family.target_name.to_owned(),
                    got: target.to_owned(),
                });
            }
        }
    }
    if let Some(name) = args.keys().next() {
        return Err(ArgError::UnknownArg {
            command: id.to_owned(),
            name: name.clone(),
        });
    }
    Ok(Validated {
        id: id.to_owned(),
        target: Some(target.to_owned()),
        args: Args::new(),
    })
}
