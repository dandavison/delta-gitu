//! `delta tui --show-config` lists every setting the TUI takes from git config, with what it
//! does and its value.

use super::*;
use crate::{
    cli::{Args, ConfigFormat},
    config_git,
    screen::review::OPTION_NAMES,
    show_config::{self, Shown},
};
use clap::Parser;

#[test]
fn show_config_is_a_flag_with_an_optional_format() {
    let format = |args: &[&str]| Args::parse_from(args).show_config;
    assert_eq!(format(&["gitu", "--show-config"]), Some(ConfigFormat::Text));
    assert_eq!(
        format(&["gitu", "--show-config=json"]),
        Some(ConfigFormat::Json)
    );
    assert_eq!(format(&["gitu"]), None);
}

/// The settings in `ctx`'s repository, with `options` set in git config.
fn shown(ctx: &mut TestContext, options: &'static [(&'static str, &'static str)]) -> Vec<Shown> {
    super::pr::with_options(ctx, options);
    let dir = ctx.dir.clone();
    show_config::settings(ctx.config(), &dir).unwrap()
}

fn find<'a>(shown: &'a [Shown], name: &str) -> &'a Shown {
    shown
        .iter()
        .find(|setting| setting.name == name)
        .unwrap_or_else(|| panic!("{name} is not shown"))
}

/// Every setting the TUI reads from git config is listed, each with what it does.
#[test]
fn every_setting_is_listed_with_what_it_does() {
    let mut ctx = setup_clone!();
    let shown = shown(&mut ctx, &[]);
    for name in config_git::names()
        .iter()
        .map(String::as_str)
        .chain(OPTION_NAMES)
    {
        let setting = find(&shown, name);
        assert!(
            !setting.description.trim().is_empty(),
            "{name} does not say what it does"
        );
    }
}

/// A setting set in git config is shown with its value, as set; one not set is shown with its
/// default. A style is written as delta writes one, and a key binding as `bind` takes one.
#[test]
fn a_setting_is_shown_with_its_value() {
    let mut ctx = setup_clone!();
    let shown = shown(
        &mut ctx,
        &[
            ("section-header-style", "bold red"),
            ("comment-style", "blue"),
        ],
    );

    let header = find(&shown, "section-header-style");
    assert_eq!(header.values, ["bold red"]);
    assert!(header.set);
    let info = find(&shown, "info-msg-style");
    assert_eq!(info.values, ["bold green"]);
    assert_eq!(info.default, ["bold green"]);
    assert!(!info.set);
    let comment = find(&shown, "comment-style");
    assert_eq!(comment.values, ["blue"]);
    assert!(comment.set);
    assert!(
        find(&shown, "bind")
            .values
            .contains(&"root.quit q".to_owned()),
        "{:?}",
        find(&shown, "bind").values
    );
}

/// The text is git config: each setting under a comment saying what it does, and a setting with
/// no value commented out.
#[test]
fn the_text_is_git_config_saying_what_each_setting_does() {
    let mut ctx = setup_clone!();
    let shown = shown(&mut ctx, &[("section-header-style", "bold red")]);

    let text = show_config::text(&shown);

    let lines: Vec<&str> = text.lines().collect();
    let at = lines
        .iter()
        .position(|line| *line == "section-header-style = bold red")
        .unwrap_or_else(|| panic!("{text}"));
    assert!(lines[at - 1].starts_with("# "), "{text}");
    assert!(lines.contains(&"# alt-editor ="), "{text}");
}

#[test]
fn the_json_has_each_settings_name_value_default_and_description() {
    let mut ctx = setup_clone!();
    let shown = shown(&mut ctx, &[("section-header-style", "bold red")]);

    let json = show_config::json(&shown);

    let header = json
        .as_array()
        .unwrap()
        .iter()
        .find(|setting| setting["name"] == "section-header-style")
        .unwrap_or_else(|| panic!("{json}"));
    assert_eq!(header["values"], serde_json::json!(["bold red"]));
    assert_eq!(header["set"], true);
    assert!(header["default"].is_array(), "{header}");
    assert!(
        header["description"]
            .as_str()
            .is_some_and(|d| !d.is_empty())
    );
}
