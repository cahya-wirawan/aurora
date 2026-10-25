//! `aurora-cli mcp` arguments.

use std::path::PathBuf;

use aurora_mcp::Limits;

/// The usage text.
pub const USAGE: &str = "usage: aurora-cli mcp [--read-root DIR]... [--write-root DIR]... \
[--max-open-mb N] [--scratch-dir DIR]\n\
Serves the Model Context Protocol over stdin/stdout. Without --read-root or --write-root the \
server has no file access at all. Logs go to stderr.";

/// Parsed `mcp` options.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct McpOptions {
    pub read_roots: Vec<PathBuf>,
    pub write_roots: Vec<PathBuf>,
    pub max_open_mb: Option<u64>,
    pub scratch_dir: Option<PathBuf>,
}

impl McpOptions {
    /// The limits these options give.
    #[must_use]
    pub fn limits(&self) -> Limits {
        let mut limits = Limits::default();
        if let Some(mb) = self.max_open_mb {
            limits.max_open_bytes = mb.saturating_mul(1 << 20);
        }
        limits
    }
}

/// Parses the arguments after `mcp`.
///
/// # Errors
/// A usage message for an unknown flag or a missing or bad value.
pub fn parse(args: &[String]) -> Result<McpOptions, String> {
    let mut options = McpOptions::default();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut value = || {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{arg} needs a value\n{USAGE}"))
        };
        match arg.as_str() {
            "--read-root" => options.read_roots.push(PathBuf::from(value()?)),
            "--write-root" => options.write_roots.push(PathBuf::from(value()?)),
            "--scratch-dir" => options.scratch_dir = Some(PathBuf::from(value()?)),
            "--max-open-mb" => {
                options.max_open_mb = Some(
                    value()?
                        .parse::<u64>()
                        .ok()
                        .filter(|n| (1..=65_536).contains(n))
                        .ok_or_else(|| {
                            format!("--max-open-mb needs a whole number 1..65536\n{USAGE}")
                        })?,
                );
            }
            "-h" | "--help" => return Err(USAGE.to_owned()),
            other => return Err(format!("unknown argument {other}\n{USAGE}")),
        }
    }
    Ok(options)
}
