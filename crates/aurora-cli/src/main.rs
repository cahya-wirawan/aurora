//! Headless binary for batch processing and automation.
//!
//! `aurora-cli mcp` serves the Model Context Protocol over stdio (task 159,
//! `docs/mcp.md`). **stdout is the protocol**: nothing else is ever
//! printed to it; logs and usage go to stderr.

use std::process::ExitCode;

use aurora_cli::host::AppHost;
use aurora_cli::mcp_args;

fn init_logging() {
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .try_init();
}

fn run_mcp(args: &[String]) -> ExitCode {
    let options = match mcp_args::parse(args) {
        Ok(options) => options,
        Err(usage) => {
            eprintln!("{usage}");
            return ExitCode::from(2);
        }
    };
    let roots = match aurora_mcp::Roots::open(&options.read_roots, &options.write_roots) {
        Ok(roots) => roots,
        Err(error) => {
            eprintln!("aurora-cli mcp: {error}");
            return ExitCode::from(2);
        }
    };
    let config = aurora_mcp::Config {
        roots,
        limits: options.limits(),
        scratch_parent: options.scratch_dir.clone(),
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("aurora-cli mcp: couldn't start the runtime: {error}");
            return ExitCode::FAILURE;
        }
    };
    let out = match aurora_cli::stdio_guard::protect_stdout() {
        Ok(out) => out,
        Err(error) => {
            eprintln!("aurora-cli mcp: couldn't set up the protocol stream: {error}");
            return ExitCode::FAILURE;
        }
    };
    // Test hook for the stdout-hygiene test: print to "stdout" the way a
    // stray `println!` in a dependency would. After the guard it goes to
    // stderr, never into the protocol.
    if std::env::var_os("AURORA_MCP_SELFTEST_STRAY_STDOUT").is_some() {
        println!("stray output that must not reach the protocol");
    }
    let host = AppHost::with_limits(&config.limits);
    match runtime.block_on(aurora_mcp::serve_io(
        Box::new(host),
        config,
        tokio::io::stdin(),
        out,
    )) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("aurora-cli mcp: {error}");
            ExitCode::FAILURE
        }
    }
}

fn main() -> ExitCode {
    init_logging();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("mcp") => run_mcp(args.get(1..).unwrap_or_default()),
        Some("-h" | "--help") | None => {
            eprintln!("usage: aurora-cli mcp [options]  (see `aurora-cli mcp --help`)");
            ExitCode::from(2)
        }
        Some(other) => {
            eprintln!("aurora-cli: unknown command {other}");
            ExitCode::from(2)
        }
    }
}
