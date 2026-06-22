//! Shared fixtures for the `rr` integration tests.
//!
//! A [`Dir`] is a throwaway directory the real `rr` binary runs against. It
//! creates its own fixture files and removes itself on drop, so each test below
//! carries only the files it needs and the behavior it asserts — never
//! scratch-dir bookkeeping.

// Each integration-test binary that `mod common;`s this file compiles the whole
// module but may exercise only part of it; without this, the unused part warns
// in that binary. Standard for a shared `tests/common` helper.
#![allow(dead_code)]

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

/// A throwaway working directory the `rr` binary runs against.
///
/// Named after ripgrep's `tests/util.rs` `Dir`, per this crate's "model on
/// ripgrep" ethos. Lives under the system temp dir and is removed when the
/// value drops, so a panicking assertion can't leak it — an explicit
/// `remove_dir_all` at the end of a test never runs once an earlier `assert!`
/// has unwound.
pub struct Dir {
    dir: PathBuf,
}

impl Dir {
    /// A fresh, empty project. `tag` only flavors the directory name so stray
    /// temp dirs are identifiable; uniqueness comes from pid + nanos.
    pub fn new(tag: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("rr-it-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Dir { dir }
    }

    /// Write `contents` to `rel`, creating parent directories as needed.
    pub fn file(&mut self, rel: &str, contents: &str) -> &mut Self {
        self.write(rel, contents);
        self
    }

    /// Like [`file`](Self::file) but takes `&self`, for rewriting a file
    /// mid-test (e.g. to make an existing index go stale).
    pub fn write(&self, rel: &str, contents: &str) {
        let path = self.dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, contents).unwrap();
    }

    /// Run the `rr` binary with `args` in the project root. Returns raw
    /// `Output`; use [`TestCommand`] (via [`Dir::command`]) for fluent
    /// assertions.
    pub fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_rr"))
            .args(args)
            .current_dir(&self.dir)
            .output()
            .expect("failed to spawn rr")
    }

    /// Create a [`TestCommand`] pre-wired to this directory.
    pub fn command(&self) -> TestCommand {
        TestCommand {
            bin: OsString::from(env!("CARGO_BIN_EXE_rr")),
            dir: self.dir.clone(),
            args: Vec::new(),
        }
    }

    /// Path to this directory.
    pub fn path(&self) -> &std::path::Path {
        &self.dir
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

/// A command builder for the `rr` binary, pre-wired with a test dir as cwd.
///
/// Args accumulate via [`arg`](Self::arg)/[`args`](Self::args) and are drained
/// on each terminal call (`run`, `stdout`, `assert_*`), so the same value can
/// drive multiple sequential invocations without cross-contamination.
pub struct TestCommand {
    bin: OsString,
    dir: PathBuf,
    args: Vec<OsString>,
}

impl TestCommand {
    /// Append one argument.
    pub fn arg(&mut self, a: impl Into<OsString>) -> &mut Self {
        self.args.push(a.into());
        self
    }

    /// Append multiple arguments.
    pub fn args<S: Into<OsString>>(&mut self, iter: impl IntoIterator<Item = S>) -> &mut Self {
        self.args.extend(iter.into_iter().map(Into::into));
        self
    }

    /// Drain accumulated args, spawn `rr`, and return the raw `Output`.
    pub fn run(&mut self) -> Output {
        let args = std::mem::take(&mut self.args);
        Command::new(&self.bin)
            .args(&args)
            .current_dir(&self.dir)
            .output()
            .unwrap_or_else(|e| panic!("failed to spawn rr: {e}"))
    }

    /// Run and return stdout as a `String`. Panics if the process exits
    /// non-zero.
    pub fn stdout(&mut self) -> String {
        let out = self.run();
        if !out.status.success() {
            panic!(
                "rr exited {}: stdout={:?} stderr={:?}",
                out.status,
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr),
            );
        }
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// Run and assert the exit code matches `expected`.
    pub fn assert_exit_code(&mut self, expected: i32) {
        let out = self.run();
        let got = out.status.code().expect("process killed by signal");
        assert_eq!(
            expected,
            got,
            "rr exit code: expected {expected}, got {got}\n  stdout: {}\n  stderr: {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        );
    }

    /// Run and assert the process exited non-zero.
    pub fn assert_err(&mut self) {
        let out = self.run();
        if out.status.success() {
            panic!(
                "expected non-zero exit but rr succeeded\n  stdout: {}\n  stderr: {}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr),
            );
        }
    }
}

/// Create a named [`Dir`] and a pre-wired [`TestCommand`]. Called by
/// [`rrtest!`].
pub fn setup(name: &str) -> (Dir, TestCommand) {
    let dir = Dir::new(name);
    let cmd = dir.command();
    (dir, cmd)
}

/// The child's exit code, or a clear panic if it was killed by a signal.
pub fn code(out: &Output) -> i32 {
    out.status.code().expect("process terminated by signal")
}

/// Declare an integration test that receives a fresh [`Dir`] and
/// [`TestCommand`], auto-named from the test function name.
///
/// Mirrors ripgrep's `rgtest!` macro.
///
/// ```ignore
/// rrtest!(my_test, |mut dir: Dir, mut cmd: TestCommand| {
///     dir.file("a.txt", "hello\n");
///     cmd.arg("index").assert_exit_code(0);
/// });
/// ```
#[macro_export]
macro_rules! rrtest {
    ($name:ident, $fun:expr) => {
        #[test]
        fn $name() {
            let (dir, cmd) = crate::common::setup(stringify!($name));
            $fun(dir, cmd);
        }
    };
}
