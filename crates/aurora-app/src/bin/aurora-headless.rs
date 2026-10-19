//! A timed headless open -> composite -> export run, for measuring
//! Aurora from the outside (`spike/photocraft-compare/`). The logic is
//! `aurora_app::headless`, which runs the app's own open and export code.

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    aurora_app::headless::main_from_args(&args)
}
