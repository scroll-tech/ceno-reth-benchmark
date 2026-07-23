//! Compile-time diagnostic trims for ranking guest cost centers.
//!
//! These switches are intentionally unsound and are only for controlled profiling runs.
//! Build with, for example:
//! `CENO_RETH_TRIM=skip_state_root_update cargo ceno build --release`.

pub(crate) fn enabled(name: &str) -> bool {
    option_env!("CENO_RETH_TRIM")
        .map(|value| value.split(',').any(|item| item.trim() == name))
        .unwrap_or(false)
}
