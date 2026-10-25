//! Opt-in debug output controlled by the `LSPYX_DEBUG` environment variable.

use std::env;
use std::process::Stdio;

/// Return whether `LSPYX_DEBUG` is set.
pub(crate) fn enabled() -> bool {
    env::var_os("LSPYX_DEBUG").is_some()
}

/// Print `message` to stderr when debug output is enabled.
pub(crate) fn log(message: &str) {
    if enabled() {
        eprintln!("{message}");
    }
}

/// Return the stderr target for child processes: inherited when debugging, discarded otherwise.
pub(crate) fn child_stderr() -> Stdio {
    if enabled() {
        Stdio::inherit()
    } else {
        Stdio::null()
    }
}
