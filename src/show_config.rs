//! `delta tui --show-config`: every setting the TUI takes from git config's `[delta]` section,
//! what it does, and its value in the repository it is run in.

use crate::{Res, config::Config};
use std::path::Path;

/// A setting, as `--show-config` shows it.
#[derive(Debug, PartialEq)]
pub(crate) struct Shown {
    /// Its name in git config's `[delta]` section.
    pub name: String,
    /// What it does.
    pub description: String,
    /// Its value, as git config gives it: an item for each of a list's items, and none for a
    /// setting with no value.
    pub values: Vec<String>,
    /// Its default, as `values` gives its value.
    pub default: Vec<String>,
    /// Whether the user set it, in git config or in gitu's config file.
    pub set: bool,
}

/// Every setting the TUI takes from git config, with its value in the repository at `dir` with
/// `config`.
pub(crate) fn settings(_config: &Config, _dir: &Path) -> Res<Vec<Shown>> {
    Ok(vec![])
}

/// `settings` as git config: each setting under a comment saying what it does.
pub(crate) fn text(_settings: &[Shown]) -> String {
    String::new()
}

/// `settings` as JSON.
pub(crate) fn json(_settings: &[Shown]) -> serde_json::Value {
    serde_json::Value::Null
}
