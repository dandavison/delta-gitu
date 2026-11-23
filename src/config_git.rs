//! gitu's settings in delta's git config. They are keys in `[delta]` and in a
//! `[delta "feature"]` section, beside delta's own options, and delta resolves
//! them as it does its own (see [`RendererOptions`]): a value in `[delta]` beats a
//! feature's, a list is the key given once per item, and a list takes the items
//! of `[delta]` and of every feature in force.
//!
//! A setting's name is its path in gitu's config file with dashes for
//! underscores and dots, less `general`: `general.diff_renderer.theme` is
//! `diff-renderer-theme`. A style's name ends with `-style`:
//! `style.menu.active_arg` is `menu-active-arg-style`. gitu's `style.hunk_header`
//! cannot be set here, since `hunk-header-style` is delta's.
//!
//! A table of named entries is a list whose items start with the name:
//! `command = html {command} | aha`, `view = diff git diff {commit}...` and
//! `feature-key = side-by-side S`. Where two items have the same name, the one
//! of `[delta]`, or of the feature later in force, wins. A binding is
//! `bind = root.quit q`, one key an item; a binding's keys are those of every
//! item for it.
//!
//! Git config values are untyped, so each takes the type of gitu's default for
//! it: a boolean or integer is read as git reads one, and a style is written as
//! delta writes one.

use std::{path::Path, str::FromStr};

use figment::{
    Figment,
    value::{Dict, Empty, Value},
};

use crate::{
    Res,
    config::RendererOptions,
    error::Error,
    style::{Color, Modifier, Style},
};

/// Where feature keys are in gitu's config.
pub(crate) const FEATURE_KEYS: &str = "general.diff_renderer.feature_keys";

/// Where commands are in gitu's config.
pub(crate) const COMMANDS: &str = "general.commands";

/// Where views are in gitu's config.
pub(crate) const VIEWS: &str = "general.views";

/// The setting a feature key is in git config.
pub(crate) const FEATURE_KEY: &str = "feature-key";

/// The setting the theme is in git config.
pub(crate) const THEME: &str = "diff-renderer-theme";

/// The settings that have no value by default, so are not in gitu's defaults.
const UNSET_BY_DEFAULT: [&str; 2] = ["general.alt_editor", "general.diff_renderer.theme"];

/// The names gitu's styles would have that are delta's options.
const DELTAS: [&str; 1] = ["hunk-header-style"];

/// A setting gitu takes from git config.
struct Setting {
    /// Its name in git config.
    name: String,
    /// Where it is in gitu's config.
    path: Vec<String>,
    kind: Kind,
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Bool,
    Int,
    Str,
    Style,
    List,
    /// A table of named entries: commands, views and feature keys.
    Table,
    Bindings,
}

/// Every setting gitu takes from git config, from gitu's `defaults`.
fn settings(defaults: &Figment) -> Vec<Setting> {
    let mut settings = vec![];
    if let Ok(Value::Dict(_, general)) = defaults.find_value("general") {
        general_settings(&general, &mut vec!["general".to_owned()], &mut settings);
    }
    for path in UNSET_BY_DEFAULT {
        let path: Vec<String> = path.split('.').map(String::from).collect();
        settings.push(Setting {
            name: dashed(&path[1..]),
            path,
            kind: Kind::Str,
        });
    }
    if let Ok(Value::Dict(_, style)) = defaults.find_value("style") {
        style_settings(&style, &mut vec!["style".to_owned()], &mut settings);
    }
    settings.push(Setting {
        name: "bind".to_owned(),
        path: vec!["bindings".to_owned()],
        kind: Kind::Bindings,
    });
    settings
}

fn general_settings(dict: &Dict, path: &mut Vec<String>, settings: &mut Vec<Setting>) {
    for (key, value) in dict {
        path.push(key.clone());
        let tables = [
            (COMMANDS, "command"),
            (VIEWS, "view"),
            (FEATURE_KEYS, FEATURE_KEY),
        ];
        if let Some((_, name)) = tables.iter().find(|(at, _)| *at == path.join(".")) {
            settings.push(Setting {
                name: (*name).to_owned(),
                path: path.clone(),
                kind: Kind::Table,
            });
        } else if let Value::Dict(_, dict) = value {
            general_settings(dict, path, settings);
        } else {
            let kind = match value {
                Value::Bool(..) => Kind::Bool,
                Value::Num(..) => Kind::Int,
                Value::Array(..) => Kind::List,
                _ => Kind::Str,
            };
            settings.push(Setting {
                name: dashed(&path[1..]),
                path: path.clone(),
                kind,
            });
        }
        path.pop();
    }
}

fn style_settings(dict: &Dict, path: &mut Vec<String>, settings: &mut Vec<Setting>) {
    for (key, value) in dict {
        let Value::Dict(_, dict) = value else {
            continue;
        };
        path.push(key.clone());
        if is_style(dict) {
            let name = format!("{}-style", dashed(&path[1..]));
            if !DELTAS.contains(&name.as_str()) {
                settings.push(Setting {
                    name,
                    path: path.clone(),
                    kind: Kind::Style,
                });
            }
        } else {
            style_settings(dict, path, settings);
        }
        path.pop();
    }
}

fn dashed(path: &[String]) -> String {
    path.join("-").replace('_', "-")
}

/// The names of the settings gitu takes from git config.
pub(crate) fn names() -> Vec<String> {
    let defaults = Figment::from(
        <figment::providers::Toml as figment::providers::Format>::string(
            crate::config::DEFAULT_CONFIG,
        ),
    );
    settings(&defaults)
        .into_iter()
        .map(|setting| setting.name)
        .collect()
}

/// gitu's settings in the renderer's git config in the repository at `dir`, with `features` in
/// force as `DELTA_FEATURES` gives them, as gitu config. `defaults` gives each its type.
pub(crate) fn read(
    options: &RendererOptions,
    dir: &Path,
    features: Option<&str>,
    defaults: &Figment,
) -> Res<Dict> {
    let _ = (options, dir, features, settings(defaults));
    Ok(Dict::new())
}

/// An item of a table or of the bindings: its name, and the rest after the space.
fn named<'a>(value: &'a str, name: &str) -> Res<(&'a str, &'a str)> {
    value
        .trim()
        .split_once(char::is_whitespace)
        .map(|(entry, rest)| (entry, rest.trim()))
        .filter(|(_, rest)| !rest.is_empty())
        .ok_or_else(|| config_error(&format!("{name}: '{value}' is not a name and a value")))
}

fn not_a_binding(value: &str, name: &str) -> Error {
    config_error(&format!(
        "{name}: '{value}' is not a menu and an action, such as root.quit, and a key"
    ))
}

/// Where the binding `menu.action` is in gitu's config: dashes are underscores, but for a
/// menu argument's own, such as `--all`.
fn binding_path(action: &str) -> Option<Vec<String>> {
    let (menu, action) = action.split_once('.')?;
    let action = if action.starts_with('-') {
        action.to_owned()
    } else {
        underscored(action)
    };
    Some(vec!["bindings".to_owned(), underscored(menu), action])
}

fn underscored(name: &str) -> String {
    name.replace('-', "_")
}

fn insert(config: &mut Dict, path: &[String], value: Value) {
    match path {
        [] => {}
        [key] => {
            config.insert(key.clone(), value);
        }
        [key, rest @ ..] => {
            let entry = config
                .entry(key.clone())
                .or_insert_with(|| Value::from(Dict::new()));
            if !matches!(entry, Value::Dict(..)) {
                *entry = Value::from(Dict::new());
            }
            if let Value::Dict(_, dict) = entry {
                insert(dict, rest, value);
            }
        }
    }
}

/// Add `value` to the list at `path`.
fn push(config: &mut Dict, path: &[String], value: Value) {
    let (last, parents) = path.split_last().expect("a path");
    let mut dict = config;
    for key in parents {
        let entry = dict
            .entry(key.clone())
            .or_insert_with(|| Value::from(Dict::new()));
        if !matches!(entry, Value::Dict(..)) {
            *entry = Value::from(Dict::new());
        }
        let Value::Dict(_, inner) = entry else {
            unreachable!()
        };
        dict = inner;
    }
    match dict
        .entry(last.clone())
        .or_insert_with(|| Value::from(Vec::<Value>::new()))
    {
        Value::Array(_, items) => items.push(value),
        other => *other = Value::from(vec![value]),
    }
}

/// Whether git config holds anything in the `[delta-tui]` sections gitu read before its
/// settings moved to `[delta]`.
pub(crate) fn has_old_section(_git_config: &git2::Config) -> Res<bool> {
    Ok(false)
}

/// As git reads a boolean, where a key with no value is true.
fn parse_bool(value: &Option<String>, name: &str) -> Res<bool> {
    let Some(value) = value else {
        return Ok(true);
    };
    match value.to_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Ok(true),
        "false" | "no" | "off" | "0" | "" => Ok(false),
        _ => Err(config_error(&format!("{name}: '{value}' is not a boolean"))),
    }
}

/// As git reads an integer, where `k`, `m` and `g` multiply by powers of 1024.
fn parse_int(value: &Option<String>, name: &str) -> Res<i64> {
    let value = value.as_deref().unwrap_or_default();
    let (digits, scale) = match value.char_indices().last() {
        Some((i, 'k' | 'K')) => (&value[..i], 1 << 10),
        Some((i, 'm' | 'M')) => (&value[..i], 1 << 20),
        Some((i, 'g' | 'G')) => (&value[..i], 1 << 30),
        _ => (value, 1),
    };
    digits
        .trim()
        .parse::<i64>()
        .ok()
        .and_then(|n| n.checked_mul(scale))
        .ok_or_else(|| config_error(&format!("{name}: '{value}' is not an integer")))
}

/// Whether gitu's default `config` is a style.
fn is_style(config: &Dict) -> bool {
    config
        .keys()
        .all(|key| ["fg", "bg", "mods", "symbol"].contains(&key.as_str()))
}

/// A delta style: attributes, and a foreground then a background color, where
/// `normal` is no color. A color it leaves out is unset, rather than left as
/// gitu's default, as delta does.
fn parse_style(value: &Option<String>, name: &str) -> Res<Dict> {
    let value = value.as_deref().unwrap_or_default();
    let bad = |why: &str| config_error(&format!("{name}: '{value}' is not a style: {why}"));
    let mut colors = vec![];
    let mut mods = vec![];
    for word in value.split_whitespace() {
        if let Some((attribute, _)) = attribute(word) {
            mods.push(attribute);
        } else if word.eq_ignore_ascii_case("normal") {
            colors.push(Value::from(Empty::None));
        } else if Color::from_str(word).is_ok() {
            colors.push(Value::from(word));
        } else {
            return Err(bad(&format!("unknown word '{word}'")));
        }
    }
    if colors.len() > 2 {
        return Err(bad("more than two colors"));
    }
    let mut colors = colors.into_iter();
    let mut style = Dict::new();
    for key in ["fg", "bg"] {
        style.insert(
            key.into(),
            colors.next().unwrap_or(Value::from(Empty::None)),
        );
    }
    style.insert("mods".into(), Value::from(mods.join("|")));
    Ok(style)
}

/// The attribute a word of a delta style is, by the name gitu's config gives it and as a
/// modifier.
fn attribute(word: &str) -> Option<(&'static str, Modifier)> {
    Some(match word.to_lowercase().as_str() {
        "bold" => ("BOLD", Modifier::BOLD),
        "dim" => ("DIM", Modifier::DIM),
        "italic" => ("ITALIC", Modifier::ITALIC),
        "ul" | "underline" => ("UNDERLINED", Modifier::UNDERLINED),
        "blink" => ("SLOW_BLINK", Modifier::SLOW_BLINK),
        "reverse" => ("REVERSED", Modifier::REVERSED),
        "hidden" => ("HIDDEN", Modifier::HIDDEN),
        "strike" => ("CROSSED_OUT", Modifier::CROSSED_OUT),
        _ => return None,
    })
}

/// A delta decoration style: a style, and which of delta's decorations it asks for.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct DecorationStyle {
    pub style: Style,
    pub boxed: bool,
    pub underline: bool,
    pub overline: bool,
}

/// `value`, a delta style, as gitu draws with it: attributes, and a foreground then a
/// background color, where `normal` is no color. With `decoration`, `box`, `ul` and `ol` are
/// delta's decorations rather than attributes. delta's `auto`, `raw`, `syntax` and `omit` mean
/// nothing for what gitu draws, and are refused with the other words it does not know.
pub(crate) fn parse_delta_style(value: &str, decoration: bool) -> Result<DecorationStyle, String> {
    let mut parsed = DecorationStyle::default();
    let mut colors = vec![];
    for word in value.split_whitespace() {
        let lower = word.to_lowercase();
        if decoration && lower == "box" {
            parsed.boxed = true;
        } else if decoration && lower == "ul" {
            parsed.underline = true;
        } else if decoration && lower == "ol" {
            parsed.overline = true;
        } else if let Some((_, modifier)) = attribute(word) {
            parsed.style.add_modifier |= modifier;
        } else if lower == "normal" {
            colors.push(None);
        } else if let Ok(color) = Color::from_str(word) {
            colors.push(Some(color));
        } else {
            return Err(format!("'{value}' is not a style: unknown word '{word}'"));
        }
    }
    if colors.len() > 2 {
        return Err(format!("'{value}' is not a style: more than two colors"));
    }
    let mut colors = colors.into_iter();
    parsed.style.fg = colors.next().flatten();
    parsed.style.bg = colors.next().flatten();
    Ok(parsed)
}

fn config_error(message: &str) -> Error {
    Error::Config(Box::new(figment::Error::from(message.to_owned())))
}

#[cfg(test)]
mod delta_style_tests {
    use super::*;

    #[test]
    fn a_delta_style_has_colors_attributes_and_decorations() {
        let parsed = parse_delta_style("bold blue box ul", true).unwrap();
        assert_eq!(parsed.style.fg, Some(Color::Blue));
        assert_eq!(parsed.style.add_modifier, Modifier::BOLD);
        assert!(parsed.boxed && parsed.underline && !parsed.overline);
        let parsed = parse_delta_style("ul normal red", false).unwrap();
        assert_eq!(parsed.style.add_modifier, Modifier::UNDERLINED);
        assert_eq!((parsed.style.fg, parsed.style.bg), (None, Some(Color::Red)));
        assert!(parse_delta_style("box", false).is_err());
        assert!(parse_delta_style("syntax", false).is_err());
    }
}
