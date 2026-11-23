use std::env;

pub(crate) fn enabled() -> bool {
    env::var_os("LSPYX_DEBUG").is_some()
}

pub(crate) fn log(message: &str) {
    if enabled() {
        eprintln!("{message}");
    }
}
