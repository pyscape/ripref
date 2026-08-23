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
#![warn(unreachable_pub)]
#![deny(missing_docs)]

pub(crate) mod atomic;
pub(crate) mod cli;
pub(crate) mod commands;
pub mod config;
pub mod indexer;
pub(crate) mod languages;
pub mod marker;
pub(crate) mod output;
pub mod refidx;
pub(crate) mod scan;
pub(crate) mod verify;

use cli::{ParseOutcome, Special, Subcommand};

/// Exit codes, one model across the verbs: `[[rr:AD-4]]` fixes them and
/// `[[rr:Shared options]]` spells each one out for a user.
pub(crate) mod exit {
    pub(crate) const OK: u8 = 0;
    pub(crate) const ADVERSE: u8 = 1;
    pub(crate) const USAGE: u8 = 2;
    pub(crate) const STALE: u8 = 3;
}

/// Every diagnostic prints here; a failure also flips a flag, so one
/// unreadable file neither aborts the run nor passes unreported. Mirrors
/// ripgrep's `message` beside its `err_message`.
pub(crate) mod messages {
    use std::io::Write;
    use std::sync::atomic::{AtomicBool, Ordering};

    static ERRORED: AtomicBool = AtomicBool::new(false);

    /// stdout is held across the write, so a diagnostic never lands inside a
    /// line of output; the write error is dropped because a closed stderr
    /// must not abort the run, which is what `eprintln!` would do.
    fn print(msg: impl std::fmt::Display) {
        let mut stderr = std::io::stderr().lock();
        let stdout = std::io::stdout();
        let _held = stdout.lock();
        let _ = writeln!(stderr, "rr: {msg}");
    }

    pub(crate) fn error(msg: impl std::fmt::Display) {
        ERRORED.store(true, Ordering::Relaxed);
        print(msg);
    }

    pub(crate) fn errored() -> bool {
        ERRORED.load(Ordering::Relaxed)
    }

    pub(crate) fn warn(msg: impl std::fmt::Display) {
        print(msg);
    }
}

/// `[[rr:AD-4#Decision outcome]]`
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
            messages::error(err);
            messages::warn("Try 'rr --help' for more information.");
            return exit::USAGE;
        }
    };

    let result = match args.command {
        Subcommand::Index => commands::run_index(&args),
        Subcommand::Read => commands::run_read(&args),
        Subcommand::At => commands::run_at(&args),
        Subcommand::Search => commands::run_search(&args),
        Subcommand::Verify => verify::run_verify(&args),
    };

    match result {
        // [[rr:AD-4#Decision outcome]]
        // A stale index is its own answer; every other code yields to a
        // failure the run already reported.
        Ok(exit::STALE) => exit::STALE,
        Ok(_) if messages::errored() => exit::USAGE,
        Ok(code) => code,
        Err(err) => {
            messages::error(err);
            exit::USAGE
        }
    }
}
