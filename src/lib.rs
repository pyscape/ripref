/*!
ripref (`rr`) — cite code and prose by stable *anchors* instead of fragile line
numbers.

This is the **walking skeleton**: the thinnest end-to-end path through the real
architecture (writer → on-disk index → mmap reader → freshness check), carrying
only the `path` anchor kind. A file path is the degenerate anchor — it is its
own stable identity — so this version proves the format, the binary search and
the `stat`-only freshness signal without any extractor intelligence. Every other
anchor kind (symbols, scenarios, records, …) layers additively on top of the
machinery exercised here.
*/

// Lint posture. `rr` mmaps its index, so `unsafe` is expected — but every use
// must be conspicuous: a `// SAFETY:` note plus a local `#[allow(unsafe_code)]`.
// `cargo lint` (-D warnings) is the enforcing gate.
#![warn(unsafe_code)]
#![warn(clippy::all)]

pub mod cli;
pub mod commands;
pub mod extractors;
pub mod indexer;
pub mod languages;
pub mod refidx;

use cli::{ParseOutcome, Special, Subcommand};

/// Exit codes, consistent across commands.
pub mod exit {
    /// Success.
    pub const OK: u8 = 0;
    /// Findings: nothing found, or an ambiguous match.
    pub const FINDINGS: u8 = 1;
    /// Usage error (bad flags, bad anchor syntax).
    pub const USAGE: u8 = 2;
    /// The index is stale — rebuild with `rr index`, or fall back to ripgrep.
    pub const STALE: u8 = 3;
}

/// Parse argv, dispatch to the chosen command, and return the process exit code.
pub fn run() -> u8 {
    let argv: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    let args = match cli::parse(&argv) {
        Ok(ParseOutcome::Special(Special::Help)) => {
            print!("{}", cli::help_text());
            return exit::OK;
        }
        Ok(ParseOutcome::Special(Special::Version)) => {
            println!("rr {}", env!("CARGO_PKG_VERSION"));
            return exit::OK;
        }
        Ok(ParseOutcome::Run(args)) => args,
        Err(err) => {
            eprintln!("rr: {err}");
            eprintln!("Try 'rr --help' for more information.");
            return exit::USAGE;
        }
    };

    let result = match args.command {
        Subcommand::Index => commands::run_index(&args),
        Subcommand::Read => commands::run_read(&args),
    };

    match result {
        Ok(code) => code,
        Err(err) => {
            eprintln!("rr: {err}");
            exit::USAGE
        }
    }
}
