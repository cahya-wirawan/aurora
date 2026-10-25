//! Tests for the command registry: the built-in table's integrity, the
//! pinned id list, argument validation, enabled checks and the listing.

#![allow(
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::items_after_statements,
    clippy::format_push_string
)]

use aurora_commands::{
    Access, ArgError, ArgValue, Args, Category, Choice, CommandContext, CommandSpec, FamilySpec,
    Listing, NotEnabled, ParamDefault, ParamKind, ParamSpec, PathUse, Presence, Registry,
    RegistryError, Requirement, Resolved, TargetKind,
};

fn builtin() -> &'static Registry {
    Registry::builtin()
}

fn args(pairs: &[(&str, ArgValue)]) -> Args {
    pairs
        .iter()
        .map(|(name, value)| ((*name).to_owned(), value.clone()))
        .collect()
}

fn text(value: &str) -> ArgValue {
    ArgValue::Text(value.to_owned())
}

// -- the table --

#[test]
fn the_builtin_table_validates() {
    assert_eq!(builtin().validate(), Ok(()));
}

#[test]
fn ids_are_unique_and_the_table_has_the_expected_size() {
    let mut ids: Vec<&str> = builtin().commands().iter().map(|c| c.id).collect();
    let total = ids.len();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), total, "duplicate ids");
    assert_eq!(total, 150);
    assert_eq!(builtin().palette().count(), 147);
}

/// The id list is pinned: an id, its title and whether the palette lists it
/// are public (palette, menus, shortcuts' hints, later automation). Changing
/// one means editing this golden file on purpose.
#[test]
fn the_ids_titles_and_presence_are_pinned_by_the_golden_file() {
    let mut actual = String::new();
    for spec in builtin().commands() {
        actual.push_str(&format!(
            "{}\t{}\t{:?}\n",
            spec.id, spec.title, spec.presence
        ));
    }
    for family in builtin().families() {
        actual.push_str(&format!(
            "{}*\t{}\t{:?}\n",
            family.prefix, family.title_pattern, family.access
        ));
    }
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/command_ids.tsv");
    if std::env::var_os("AURORA_UPDATE_GOLDEN").is_some() {
        std::fs::write(path, &actual).unwrap_or(());
    }
    // A Windows checkout may convert the file to CRLF; the golden is the
    // same text either way.
    let golden = include_str!("command_ids.tsv").replace("\r\n", "\n");
    assert_eq!(
        actual, golden,
        "the registry changed; see docs/command-registry.md"
    );
}

#[test]
fn every_command_has_docs_and_a_valid_default() {
    for spec in builtin().commands() {
        assert!(spec.docs.len() > 10, "{} needs real docs", spec.id);
        for param in spec.params {
            assert!(param.docs.len() > 10, "{}.{}", spec.id, param.name);
        }
    }
}

#[test]
fn a_family_member_resolves_and_a_bare_prefix_does_not() {
    match builtin().resolve("workspace.switch/Photography") {
        Some(Resolved::Member { family, target }) => {
            assert_eq!(family.prefix, "workspace.switch/");
            assert_eq!(target, "Photography");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(builtin().resolve("workspace.switch/"), None);
    assert!(matches!(
        builtin().resolve("file.open"),
        Some(Resolved::Command(spec)) if spec.id == "file.open"
    ));
    assert_eq!(builtin().resolve("nope"), None);
}

// -- table validation catches mistakes --

static OK_PARAM: ParamSpec = ParamSpec::required(
    "n",
    "A number of things.",
    ParamKind::Int { min: 1, max: 5 },
);

fn registry_of(commands: &'static [CommandSpec]) -> Registry {
    Registry::new(commands, &[])
}

#[test]
fn a_bad_table_is_reported() {
    static DUP: [CommandSpec; 2] = [
        CommandSpec::new("a.b", "A", "Does a thing.", Category::Edit, Access::Read),
        CommandSpec::new("a.b", "B", "Does a thing.", Category::Edit, Access::Read),
    ];
    assert_eq!(
        registry_of(&DUP).validate(),
        Err(RegistryError::DuplicateId("a.b".into()))
    );

    static BAD_ID: [CommandSpec; 1] = [CommandSpec::new(
        "A B",
        "A",
        "Does a thing.",
        Category::Edit,
        Access::Read,
    )];
    assert!(matches!(
        registry_of(&BAD_ID).validate(),
        Err(RegistryError::BadId(_))
    ));

    static NO_DOCS: [CommandSpec; 1] = [CommandSpec::new(
        "a.b",
        "A",
        " ",
        Category::Edit,
        Access::Read,
    )];
    assert!(matches!(
        registry_of(&NO_DOCS).validate(),
        Err(RegistryError::Undocumented(_))
    ));

    static TWICE: [ParamSpec; 2] = [OK_PARAM, OK_PARAM];
    static DUP_PARAM: [CommandSpec; 1] =
        [
            CommandSpec::new("a.b", "A", "Does a thing.", Category::Edit, Access::Read)
                .params(&TWICE),
        ];
    assert!(matches!(
        registry_of(&DUP_PARAM).validate(),
        Err(RegistryError::DuplicateParam { .. })
    ));

    static BAD_DEFAULT: [ParamSpec; 1] = [ParamSpec::optional(
        "n",
        "A number of things.",
        ParamKind::Int { min: 1, max: 5 },
        ParamDefault::Int(9),
    )];
    static BAD_DEFAULT_CMD: [CommandSpec; 1] =
        [
            CommandSpec::new("a.b", "A", "Does a thing.", Category::Edit, Access::Read)
                .params(&BAD_DEFAULT),
        ];
    assert!(matches!(
        registry_of(&BAD_DEFAULT_CMD).validate(),
        Err(RegistryError::BadParam { .. })
    ));

    static EMPTY_CHOICE: [ParamSpec; 1] = [ParamSpec::required(
        "c",
        "Pick one of them.",
        ParamKind::Choice(&[]),
    )];
    static EMPTY_CHOICE_CMD: [CommandSpec; 1] =
        [
            CommandSpec::new("a.b", "A", "Does a thing.", Category::Edit, Access::Read)
                .params(&EMPTY_CHOICE),
        ];
    assert!(matches!(
        registry_of(&EMPTY_CHOICE_CMD).validate(),
        Err(RegistryError::BadParam { .. })
    ));

    static REQ: [CommandSpec; 1] =
        [
            CommandSpec::new("a.b", "A", "Does a thing.", Category::Edit, Access::Read)
                .requires(&[Requirement::CanUndo, Requirement::CanUndo]),
        ];
    assert!(matches!(
        registry_of(&REQ).validate(),
        Err(RegistryError::DuplicateRequirement(_))
    ));

    static FAM: [FamilySpec; 1] = [FamilySpec {
        prefix: "a.",
        title_pattern: "A: <x>",
        docs: "Members.",
        category: Category::Edit,
        access: Access::Ui,
        requires: &[],
        target_name: "x",
        target: TargetKind::Id,
    }];
    static SHADOWED: [CommandSpec; 1] = [CommandSpec::new(
        "a.b",
        "A",
        "Does a thing.",
        Category::Edit,
        Access::Read,
    )];
    assert!(matches!(
        Registry::new(&SHADOWED, &FAM).validate(),
        Err(RegistryError::ShadowedByFamily { .. })
    ));
}

// -- argument validation --

#[test]
fn a_call_with_no_parameters_takes_no_arguments() {
    let ok = builtin().validate_args("layer.new", &Args::new());
    assert!(ok.is_ok());
    assert_eq!(
        builtin().validate_args("layer.new", &args(&[("x", ArgValue::Int(1))])),
        Err(ArgError::UnknownArg {
            command: "layer.new".into(),
            name: "x".into()
        })
    );
}

#[test]
fn an_unknown_command_is_refused() {
    assert_eq!(
        builtin().validate_args("bogus.thing", &Args::new()),
        Err(ArgError::UnknownCommand("bogus.thing".into()))
    );
}

#[test]
fn required_arguments_must_be_given() {
    assert_eq!(
        builtin().validate_args("file.open", &Args::new()),
        Err(ArgError::MissingArg {
            command: "file.open".into(),
            name: "path".into()
        })
    );
    let ok = builtin().validate_args("file.open", &args(&[("path", text("/tmp/a.psd"))]));
    assert_eq!(
        ok.map(|v| v.text("path").map(str::to_owned)),
        Ok(Some("/tmp/a.psd".into()))
    );
}

#[test]
fn types_are_checked() {
    for (id, name, value, got) in [
        ("file.open", "path", ArgValue::Int(3), "an integer"),
        ("select.expand", "amount", text("5"), "text"),
        ("select.expand", "amount", ArgValue::Number(5.0), "a number"),
        (
            "select.feather",
            "radius",
            ArgValue::Bool(true),
            "a boolean",
        ),
        ("select.save", "name", ArgValue::Int(1), "an integer"),
    ] {
        match builtin().validate_args(id, &args(&[(name, value)])) {
            Err(ArgError::WrongType {
                name: n, got: g, ..
            }) => {
                assert_eq!(n, name);
                assert_eq!(g, got);
            }
            other => panic!("{id}: {other:?}"),
        }
    }
}

#[test]
fn ranges_are_checked_inclusively() {
    for amount in [1, 500] {
        assert!(
            builtin()
                .validate_args("select.expand", &args(&[("amount", ArgValue::Int(amount))]))
                .is_ok()
        );
    }
    for amount in [0, 501, -3, i64::MAX] {
        assert!(matches!(
            builtin().validate_args("select.expand", &args(&[("amount", ArgValue::Int(amount))])),
            Err(ArgError::OutOfRange { .. })
        ));
    }
    assert!(
        builtin()
            .validate_args("select.smooth", &args(&[("amount", ArgValue::Int(100))]))
            .is_ok()
    );
    assert!(
        builtin()
            .validate_args("select.smooth", &args(&[("amount", ArgValue::Int(101))]))
            .is_err()
    );
}

#[test]
fn numbers_accept_integers_and_reject_non_finite_and_out_of_range() {
    let ok = builtin().validate_args("select.feather", &args(&[("radius", ArgValue::Int(10))]));
    assert_eq!(ok.map(|v| v.number("radius")), Ok(Some(10.0)));
    assert!(
        builtin()
            .validate_args(
                "select.feather",
                &args(&[("radius", ArgValue::Number(0.5))])
            )
            .is_err()
    );
    assert!(
        builtin()
            .validate_args(
                "select.feather",
                &args(&[("radius", ArgValue::Number(250.0))])
            )
            .is_ok()
    );
    assert!(
        builtin()
            .validate_args(
                "select.feather",
                &args(&[("radius", ArgValue::Number(250.5))])
            )
            .is_err()
    );
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert_eq!(
            builtin().validate_args(
                "select.feather",
                &args(&[("radius", ArgValue::Number(bad))])
            ),
            Err(ArgError::NotFinite {
                name: "radius".into()
            })
        );
    }
}

#[test]
fn optional_arguments_get_their_default() {
    let v = builtin().validate_args("select.colour_range", &Args::new());
    assert_eq!(v.map(|v| v.int("fuzziness")), Ok(Some(40)));
    let v = builtin().validate_args(
        "select.colour_range",
        &args(&[("fuzziness", ArgValue::Int(200))]),
    );
    assert_eq!(v.map(|v| v.int("fuzziness")), Ok(Some(200)));
    assert!(
        builtin()
            .validate_args(
                "select.colour_range",
                &args(&[("fuzziness", ArgValue::Int(201))])
            )
            .is_err()
    );
}

#[test]
fn text_rules_empty_long_and_control() {
    let id = "select.save";
    assert_eq!(
        builtin().validate_args(id, &args(&[("name", text(""))])),
        Err(ArgError::Empty {
            name: "name".into()
        })
    );
    assert!(
        builtin()
            .validate_args(id, &args(&[("name", text(&"x".repeat(64)))]))
            .is_ok()
    );
    assert!(matches!(
        builtin().validate_args(id, &args(&[("name", text(&"x".repeat(65)))])),
        Err(ArgError::TooLong {
            len: 65,
            max: 64,
            ..
        })
    ));
    // Characters, not bytes.
    assert!(
        builtin()
            .validate_args(id, &args(&[("name", text(&"é".repeat(64)))]))
            .is_ok()
    );
    assert!(matches!(
        builtin().validate_args(id, &args(&[("name", text("a\nb"))])),
        Err(ArgError::ControlCharacter { .. })
    ));
    assert!(matches!(
        builtin().validate_args("file.open", &args(&[("path", text("a\0b"))])),
        Err(ArgError::ControlCharacter { .. })
    ));
}

#[test]
fn unknown_arguments_are_rejected_before_anything_else() {
    // `path` is also missing; the unknown name is reported first.
    assert_eq!(
        builtin().validate_args("file.open", &args(&[("pth", text("x"))])),
        Err(ArgError::UnknownArg {
            command: "file.open".into(),
            name: "pth".into()
        })
    );
}

#[test]
fn family_members_validate_their_target_and_take_no_arguments() {
    assert!(
        builtin()
            .validate_args("workspace.switch/Mine", &Args::new())
            .is_ok()
    );
    assert!(
        builtin()
            .validate_args("document.switch.12", &Args::new())
            .is_ok()
    );
    assert!(matches!(
        builtin().validate_args("document.switch.abc", &Args::new()),
        Err(ArgError::BadTarget { .. })
    ));
    for bad in [
        "document.switch.+5",
        "document.switch.-1",
        "document.switch.1_0",
        "document.switch. 7",
    ] {
        assert!(
            matches!(
                builtin().validate_args(bad, &Args::new()),
                Err(ArgError::BadTarget { .. })
            ),
            "{bad}"
        );
    }
    let v = builtin().validate_args("select.load:sky", &Args::new());
    assert_eq!(v.map(|v| v.target), Ok(Some("sky".into())));
    assert!(matches!(
        builtin().validate_args("select.load:sky", &args(&[("x", ArgValue::Int(1))])),
        Err(ArgError::UnknownArg { .. })
    ));
    assert!(matches!(
        builtin().validate_args("workspace.switch/a\nb", &Args::new()),
        Err(ArgError::BadTarget { .. })
    ));
}

#[test]
fn a_choice_and_a_bool_parameter_validate() {
    static CHOICES: [Choice; 2] = [
        Choice {
            value: "a",
            docs: "First.",
        },
        Choice {
            value: "b",
            docs: "Second.",
        },
    ];
    static PARAMS: [ParamSpec; 2] = [
        ParamSpec::required("which", "Which one to use.", ParamKind::Choice(&CHOICES)),
        ParamSpec::optional(
            "flag",
            "Turns it on.",
            ParamKind::Bool,
            ParamDefault::Bool(true),
        ),
    ];
    static CMDS: [CommandSpec; 1] =
        [
            CommandSpec::new("x.y", "X", "Does a thing.", Category::Edit, Access::Read)
                .params(&PARAMS),
        ];
    let registry = Registry::new(&CMDS, &[]);
    assert_eq!(registry.validate(), Ok(()));
    let ok = registry.validate_args("x.y", &args(&[("which", text("b"))]));
    assert_eq!(ok.as_ref().map(|v| v.bool("flag")), Ok(Some(true)));
    assert!(matches!(
        registry.validate_args("x.y", &args(&[("which", text("c"))])),
        Err(ArgError::NotAChoice { .. })
    ));
    assert!(matches!(
        registry.validate_args(
            "x.y",
            &args(&[("which", text("a")), ("flag", ArgValue::Int(1))])
        ),
        Err(ArgError::WrongType { .. })
    ));
}

// -- enabled checks --

fn context() -> CommandContext {
    CommandContext {
        can_undo: true,
        can_redo: true,
        delete_allowed: true,
        ..CommandContext::default()
    }
}

#[test]
fn enabled_checks_give_a_reason() {
    let mut c = context();
    assert_eq!(builtin().check_enabled("edit.undo", &c), Ok(()));
    c.can_undo = false;
    assert_eq!(
        builtin().check_enabled("edit.undo", &c),
        Err(NotEnabled::Reason("There is nothing to undo."))
    );
    c = context();
    c.delete_allowed = false;
    assert!(builtin().check_enabled("layer.delete", &c).is_err());
    c = context();
    c.quick_mask = true;
    for id in [
        "select.all",
        "edit.paste",
        "select.feather",
        "select.load:x",
        "paths.fill",
    ] {
        assert_eq!(
            builtin().check_enabled(id, &c),
            Err(NotEnabled::Reason("Not available in Quick Mask mode.")),
            "{id}"
        );
    }
    assert_eq!(builtin().check_enabled("select.quick_mask", &c), Ok(()));
    assert_eq!(builtin().check_enabled("tool.brush", &c), Ok(()));
    assert_eq!(
        builtin().check_enabled("nope", &c),
        Err(NotEnabled::UnknownCommand)
    );
}

#[test]
fn clipping_requirements() {
    let mut c = context();
    assert_eq!(builtin().check_enabled("layer.clip_create", &c), Ok(()));
    assert!(builtin().check_enabled("layer.clip_release", &c).is_err());
    c.clipped = true;
    assert!(builtin().check_enabled("layer.clip_create", &c).is_err());
    assert_eq!(builtin().check_enabled("layer.clip_release", &c), Ok(()));
    c = context();
    c.clip_refused = true;
    assert!(builtin().check_enabled("layer.clip_create", &c).is_err());
}

#[test]
fn access_hints_are_honest_for_a_few_known_commands() {
    let access = |id: &str| builtin().get(id).map(|s| s.access);
    assert_eq!(access("file.check_psd"), Some(Access::Read));
    assert_eq!(access("tool.brush"), Some(Access::Ui));
    assert_eq!(access("layer.new"), Some(Access::Write));
    assert_eq!(access("file.save"), Some(Access::Write));
    assert_eq!(
        builtin().get("workspace.save_as").map(|s| s.presence),
        Some(Presence::Dynamic)
    );
    assert_eq!(
        builtin().get("app.quit").map(|s| s.presence),
        Some(Presence::MenuOnly)
    );
    assert_eq!(
        builtin().get("file.open").map(|s| s.params[0].kind),
        Some(ParamKind::Path {
            mode: PathUse::Read
        })
    );
}

// -- the listing --

#[test]
fn the_listing_describes_every_command_and_round_trips() {
    let listing = builtin().list();
    assert_eq!(listing.commands.len(), builtin().commands().len());
    assert_eq!(listing.families.len(), builtin().families().len());
    let open = listing.commands.iter().find(|c| c.id == "file.open");
    assert_eq!(open.map(|c| c.params.len()), Some(1));
    let undo = listing.commands.iter().find(|c| c.id == "edit.undo");
    assert_eq!(
        undo.map(|c| c.requires[0].reason.clone()),
        Some("There is nothing to undo.".into())
    );
    // Serialised and read back through a binary format: nothing is lost.
    let bytes = postcard::to_stdvec(&listing);
    assert!(bytes.is_ok());
    let back: Result<Listing, _> = postcard::from_bytes(&bytes.unwrap_or_default());
    assert_eq!(back.as_ref(), Ok(&listing));
}

#[test]
fn arg_values_serialise_bare_and_deserialise_by_type() {
    use serde::Deserialize;
    use serde::de::IntoDeserializer;
    let de = |v: ArgValue| match v {
        ArgValue::Bool(b) => {
            ArgValue::deserialize(IntoDeserializer::<serde::de::value::Error>::into_deserializer(b))
        }
        ArgValue::Int(i) => {
            ArgValue::deserialize(IntoDeserializer::<serde::de::value::Error>::into_deserializer(i))
        }
        ArgValue::Number(n) => {
            ArgValue::deserialize(IntoDeserializer::<serde::de::value::Error>::into_deserializer(n))
        }
        ArgValue::Text(t) => {
            ArgValue::deserialize(IntoDeserializer::<serde::de::value::Error>::into_deserializer(t))
        }
    };
    for v in [
        ArgValue::Bool(true),
        ArgValue::Int(-4),
        ArgValue::Number(0.5),
        text("hi"),
    ] {
        assert_eq!(de(v.clone()), Ok(v));
    }
    // An unsigned integer past i64 keeps its magnitude as a number.
    let big = ArgValue::deserialize(
        IntoDeserializer::<serde::de::value::Error>::into_deserializer(u64::MAX),
    );
    assert!(matches!(big, Ok(ArgValue::Number(n)) if n > 1.0e19));
}
