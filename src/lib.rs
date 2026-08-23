/*!
ripref (`rr`): reference code and prose by stable *anchors* instead of
fragile line numbers.

One writer and many readers over one index. `index` maps each anchor to its
definition locations and records where prose writes paths; `read` and `at`
convert between markers and locations; `search` lists the markers a project
writes; `verify` judges them. The records under doc/ad fix the design: the
domain model `[[rr:AD-1]]`, the marker grammar `[[rr:AD-2]]`, the verbs
`[[rr:AD-3]]`, the output contract `[[rr:AD-4]]`, and path mentions
`[[rr:AD-5]]`.
*/

// Lint posture. `rr` mmaps its index, so `unsafe` is expected, but every use
// must be conspicuous: a `// SAFETY:` note plus a local `#[allow(unsafe_code)]`.
// `cargo lint` (-D warnings) is the enforcing gate.
#![warn(unsafe_code)]
#![warn(clippy::all)]

pub mod atomic;
pub mod cli;
pub mod commands;
pub mod config;
pub mod indexer;
pub mod languages;
pub mod marker;
pub mod refidx;
pub mod scan;

use cli::{ParseOutcome, Special, Subcommand};

/// Exit codes, one model across the verbs: `[[rr:AD-4]]` fixes them and
/// `[[rr:Shared options]]` spells each one out for a user.
pub mod exit {
    pub const OK: u8 = 0;
    pub const ADVERSE: u8 = 1;
    pub const USAGE: u8 = 2;
    pub const STALE: u8 = 3;
}

/// A non-fatal failure prints here and flips a flag `run` reads once the
/// walk is over, so one unreadable file neither aborts the run nor passes
/// unreported. Mirrors ripgrep's `messages::set_errored`.
pub mod messages {
    use std::sync::atomic::{AtomicBool, Ordering};

    static ERRORED: AtomicBool = AtomicBool::new(false);

    pub fn error(msg: impl std::fmt::Display) {
        ERRORED.store(true, Ordering::Relaxed);
        eprintln!("rr: {msg}");
    }

    pub fn errored() -> bool {
        ERRORED.load(Ordering::Relaxed)
    }
}

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
        Subcommand::At => commands::run_at(&args),
        Subcommand::Search => commands::run_search(&args),
        Subcommand::Verify => commands::run_verify(&args),
    };

    match result {
        // A stale index is its own answer and keeps saying so; anything else
        // yields to a failure the run already reported.
        Ok(exit::STALE) => exit::STALE,
        Ok(_) if messages::errored() => exit::USAGE,
        Ok(code) => code,
        Err(err) => {
            eprintln!("rr: {err}");
            exit::USAGE
        }
    }
}
