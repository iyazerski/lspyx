//! Opt-in debug output controlled by the `LSPYX_DEBUG` environment variable.

use std::env;

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
