//! `delta tui --migrate-old-config`: the TUI's settings in git config's old `[delta-tui]`
//! sections, printed as they go in `[delta]` (see [`crate::config_git`]). The command is
//! temporary, for the release that moved the settings, and will be removed.

use crate::{Res, error::Error};
use std::process::Command;

/// An entry in an old `[delta-tui]` section: its full name and its value, `None` for a key with
/// no `=`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Entry {
    pub name: String,
    pub value: Option<String>,
}

/// Print the settings in git config's `[delta-tui]` sections as a `[delta]` section. With none,
/// say so on stderr.
pub(crate) fn print() -> Res<()> {
    let entries = entries(&|| Command::new("git"))?;
    if entries.is_empty() {
        eprintln!("There are no [delta-tui] sections in git config.");
        return Ok(());
    }
    print!("{}", migrated(&entries));
    Ok(())
}

/// The entries of the `[delta-tui]` sections in git config, in the order git reads them.
pub(crate) fn entries(git: &dyn Fn() -> Command) -> Res<Vec<Entry>> {
    let output = git()
        .args(["config", "--null", "--get-regexp", "^delta-tui\\."])
        .output()
        .map_err(|e| Error::EditConfig(e.to_string()))?;
    // Finding nothing is status 1.
    if !output.status.success() && output.status.code() != Some(1) {
        return Err(Error::EditConfig(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }
    let output = String::from_utf8_lossy(&output.stdout);
    // Each entry is its name and, after a newline, its value.
    Ok(output
        .split('\0')
        .filter(|entry| !entry.is_empty())
        .map(|entry| match entry.split_once('\n') {
            Some((name, value)) => Entry {
                name: name.to_owned(),
                value: Some(value.to_owned()),
            },
            None => Entry {
                name: entry.to_owned(),
                value: None,
            },
        })
        .collect())
}

/// `entries` as a `[delta]` section, whichever files they are in.
pub(crate) fn migrated(entries: &[Entry]) -> String {
    let mut out = String::from("[delta]\n");
    for entry in entries {
        out.push_str("    ");
        out.push_str(&line(&entry.name, entry.value.as_deref()));
        out.push('\n');
    }
    out
}

/// The line in `[delta]` for the old entry `name` with `value`.
fn line(name: &str, value: Option<&str>) -> String {
    // A key with no `=` is true.
    let value = value.unwrap_or("true");
    let key = name.strip_prefix("delta-tui.").unwrap_or(name);
    let (subsection, variable) = match key.rsplit_once('.') {
        Some((subsection, variable)) => (Some(dashed(subsection)), variable),
        None => (None, key),
    };
    let named = |setting: &str| format!("{setting} = {}", quoted(&format!("{variable} {value}")));
    match subsection.as_deref() {
        None => format!("{variable} = {}", quoted(value)),
        Some("general-commands") => named("command"),
        Some("general-views") => named("view"),
        Some("general-diff-renderer-feature-keys") => named("feature-key"),
        Some(subsection) if subsection.starts_with("bindings-") => {
            let menu = &key["bindings.".len()..key.len() - variable.len() - 1];
            format!(
                "bind = {}",
                quoted(&format!("{}.{variable} {value}", dashed(menu)))
            )
        }
        Some("style") => style(&format!("{variable}-style"), name, value),
        Some(subsection) if subsection.starts_with("style-") => style(
            &format!("{}-{variable}-style", &subsection["style-".len()..]),
            name,
            value,
        ),
        Some(subsection) if subsection.starts_with("general-") => {
            format!(
                "{}-{variable} = {}",
                &subsection["general-".len()..],
                quoted(value)
            )
        }
        Some(_) => format!("# {name} is not a setting of the TUI's, so it is left out."),
    }
}

/// The line for a style, but for `hunk-header-style`, which is delta's own option.
fn style(setting: &str, name: &str, value: &str) -> String {
    if setting == "hunk-header-style" {
        return format!(
            "# {name} = {value}: hunk-header-style is delta's own option, so this goes in \
             gitu's config file instead, as [style] hunk_header."
        );
    }
    format!("{setting} = {}", quoted(value))
}

fn dashed(name: &str) -> String {
    name.replace(['.', '_'], "-")
}

/// `value` as git config reads it back: in quotes, with `"` and `\` escaped, when it has what
/// git would otherwise take as a comment, a quote or an escape, or space at either end.
fn quoted(value: &str) -> String {
    let needs_quotes = value.contains(['"', '\\', '#', ';', '\n', '\t'])
        || value.starts_with(' ')
        || value.ends_with(' ');
    if !needs_quotes {
        return value.to_owned();
    }
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\t', "\\t");
    format!("\"{escaped}\"")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::configured;
    use crate::style::{Color, Modifier};
    use std::fs;
    use temp_dir::TempDir;

    fn entry(name: &str, value: &str) -> Entry {
        Entry {
            name: name.to_owned(),
            value: Some(value.to_owned()),
        }
    }

    /// The lines of `[delta]` that `entries` migrate to.
    fn lines(entries: &[Entry]) -> Vec<String> {
        migrated(entries)
            .lines()
            .filter_map(|line| line.strip_prefix("    "))
            .map(String::from)
            .collect()
    }

    #[test]
    fn a_general_setting_keeps_its_name() {
        assert_eq!(
            lines(&[entry("delta-tui.start-folded", "true")]),
            ["start-folded = true"]
        );
    }

    #[test]
    fn a_nested_setting_is_named_by_its_path() {
        assert_eq!(
            lines(&[
                entry("delta-tui.general.diff-renderer.theme", "my-theme"),
                entry("delta-tui.general.confirm-quit.enabled", "true"),
            ]),
            [
                "diff-renderer-theme = my-theme",
                "confirm-quit-enabled = true"
            ]
        );
    }

    #[test]
    fn commands_views_and_feature_keys_become_named_items() {
        assert_eq!(
            lines(&[
                entry(
                    "delta-tui.general.commands.open-pr",
                    "gh pr view --web {commit}"
                ),
                entry("delta-tui.general.views.diff-since", "git diff {commit}..."),
                entry(
                    "delta-tui.general.diff-renderer.feature-keys.side-by-side",
                    "S"
                ),
            ]),
            [
                "command = open-pr gh pr view --web {commit}",
                "view = diff-since git diff {commit}...",
                "feature-key = side-by-side S",
            ]
        );
    }

    #[test]
    fn bindings_become_bind_items() {
        assert_eq!(
            lines(&[
                entry("delta-tui.bindings.root.quit", "q"),
                entry("delta-tui.bindings.rebase-todo.rebase-todo-drop", "ctrl+k"),
            ]),
            [
                "bind = root.quit q",
                "bind = rebase-todo.rebase-todo-drop ctrl+k",
            ]
        );
    }

    #[test]
    fn styles_are_named_with_style() {
        assert_eq!(
            lines(&[
                entry("delta-tui.style.cursor", "blue"),
                entry("delta-tui.style.rebase-todo.drop", "bold red"),
            ]),
            ["cursor-style = blue", "rebase-todo-drop-style = bold red"]
        );
    }

    /// gitu's hunk header style has no name in `[delta]`, since delta has that one.
    #[test]
    fn the_hunk_header_style_is_said_to_go_in_the_config_file() {
        let lines = lines(&[entry("delta-tui.style.hunk-header", "blue")]);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].starts_with("# "), "{lines:?}");
        assert!(lines[0].contains("hunk_header"), "{lines:?}");
    }

    #[test]
    fn a_value_git_would_read_otherwise_is_quoted() {
        assert_eq!(
            lines(&[entry("delta-tui.general.commands.x", "echo a#b \"c\"")]),
            [r#"command = "x echo a#b \"c\"""#]
        );
    }

    /// The output is the config alone: one `[delta]` section.
    #[test]
    fn the_output_is_one_delta_section() {
        assert_eq!(
            migrated(&[
                entry("delta-tui.start-folded", "true"),
                entry("delta-tui.stash-list-limit", "0"),
            ]),
            "[delta]\n    start-folded = true\n    stash-list-limit = 0\n"
        );
    }

    /// An old config read from git config and migrated is read as it was meant.
    #[test]
    fn a_migrated_config_is_read_as_the_old_one_was() {
        let dir = TempDir::new().unwrap();
        let global = dir.path().join("global.gitconfig");
        fs::write(
            &global,
            "[delta-tui]\n\
             \tfile-header = stat\n\
             \tlog-args = --stat\n\
             \tlog-args = --pretty=commit-format\n\
             \tstart-folded\n\
             [delta-tui \"general.diff-renderer\"]\n\
             \tbuiltin-themes = diff-so-fancy\n\
             [delta-tui \"general.views\"]\n\
             \tdiff = git diff {commit}...\n\
             [delta-tui \"general.commands\"]\n\
             \tsend = redis-cli XADD delta '*' message \"{input}\"\n\
             [delta-tui \"general.diff-renderer.feature-keys\"]\n\
             \tsend = @\n\
             \tdiff = D\n\
             [delta-tui \"bindings.root\"]\n\
             \tquit = q\n\
             \tquit = left\n\
             [delta-tui \"style.menu\"]\n\
             \tkey = bold green\n",
        )
        .unwrap();
        let git = || {
            let mut git = Command::new("git");
            git.current_dir(dir.path())
                .env("GIT_CONFIG_GLOBAL", &global)
                .env("GIT_CONFIG_NOSYSTEM", "1");
            git
        };

        let config = configured("", &migrated(&entries(&git).unwrap()));

        let general = &config.general;
        assert_eq!(general.file_header, crate::config::FileHeader::Stat);
        assert_eq!(general.log_args, ["--stat", "--pretty=commit-format"]);
        assert!(general.start_folded);
        assert_eq!(general.diff_renderer.builtin_themes, ["diff-so-fancy"]);
        assert_eq!(general.views["diff"], "git diff {commit}...");
        assert_eq!(
            general.commands["send"],
            "redis-cli XADD delta '*' message {input}"
        );
        assert_eq!(
            general
                .diff_renderer
                .feature_keys
                .iter()
                .collect::<Vec<_>>(),
            [(&"diff".to_owned(), &'D'), (&"send".to_owned(), &'@')]
        );
        let quit: Vec<&str> = config
            .bindings
            .list(&crate::menu::Menu::Root)
            .filter(|binding| binding.op == crate::ops::Op::Quit)
            .map(|binding| binding.raw.as_str())
            .collect();
        assert_eq!(quit, ["q", "left"]);
        let key = crate::style::Style::from(&config.style.menu.key);
        assert_eq!(key.fg, Some(Color::Green));
        assert_eq!(key.add_modifier, Modifier::BOLD);
    }
}
