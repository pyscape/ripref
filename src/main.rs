//! The `rr` binary: a thin shell around the [`ripref`] library, which owns
//! every verb, its output, and its exit code.

// Mirror the library's lint posture for the binary crate root (see
// src/lib.rs).
#![warn(unsafe_code)]
#![warn(clippy::all)]

use std::process::ExitCode;

fn main() -> ExitCode {
    ExitCode::from(ripref::run())
}
