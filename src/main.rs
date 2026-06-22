// Mirror the library's lint posture for the binary crate root (see src/lib.rs).
#![warn(unsafe_code)]
#![warn(clippy::all)]

use std::process::ExitCode;

fn main() -> ExitCode {
    // The whole program is a library; `main` only translates the chosen exit
    // code into a process exit.
    ExitCode::from(ripref::run())
}
