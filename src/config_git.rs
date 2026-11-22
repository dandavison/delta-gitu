//! The `delta-tui` section of git config, read as a layer of gitu's config over
//! its config file, so that `delta --tui` is configured where delta is.
//!
//! A key's place in gitu's config is its subsection, split at dots, then its
//! name, with dashes for underscores: `[delta-tui "style.menu"] active-arg` is
//! `style.menu.active_arg`. A key with no subsection is under `general`. A
//! feature key keeps its name, which is the feature's, and a command or view
//! its own.
//!
//! Git config values are untyped, so each takes the type of gitu's default for
//! it: a boolean or integer is read as git reads one, a list is the key given
//! once per item, and a style is written as delta writes one. Git variable
//! names cannot start with `-` or a digit, so the bindings of menu arguments
//! (`commit_menu.--all`) and of screen lines cannot be set here, and since git
//! lowercases them, neither can the key of a feature whose name has capitals.

use std::str::FromStr;

use figment::{
    Figment,
    value::{Dict, Empty, Value},
};

use crate::{
    Res,
    error::Error,
    style::{Color, Modifier, Style},
};

pub(crate) const SECTION: &str = "delta-tui";

/// Where feature keys are in gitu's config.
pub(crate) const FEATURE_KEYS: &str = "general.diff_renderer.feature_keys";

/// Where commands are in gitu's config.
pub(crate) const COMMANDS: &str = "general.commands";

/// Where views are in gitu's config.
pub(crate) const VIEWS: &str = "general.views";

/// Where feature keys are in git config.
pub(crate) const FEATURE_KEYS_SECTION: &str = "delta-tui.general.diff-renderer.feature-keys";

/// Where the theme is in git config.
pub(crate) const THEME: &str = "delta-tui.general.diff-renderer.theme";

/// The `delta-tui` section of `git_config`, as gitu config. `defaults` gives
/// each key its type.
pub(crate) fn read(git_config: &git2::Config, defaults: &Figment) -> Res<Dict> {
    let mut config = Dict::new();
    for name in names(git_config)? {
        let path = config_path(&name);
        let value = match defaults.find_value(&path.join(".")).ok() {
            Some(Value::Array(..)) => Value::from(values(git_config, &name)?),
            Some(Value::Bool(..)) => Value::from(parse_bool(&value(git_config, &name)?, &name)?),
            Some(Value::Num(..)) => Value::from(parse_int(&value(git_config, &name)?, &name)?),
            Some(Value::Dict(_, style)) if path[0] == "style" && is_style(&style) => {
                Value::from(parse_style(&value(git_config, &name)?, &name)?)
            }
            _ => Value::from(value(git_config, &name)?.unwrap_or_default()),
        };
        insert(&mut config, &path, value);
    }
    Ok(config)
}

/// The names of the section's keys, each once.
fn names(git_config: &git2::Config) -> Res<Vec<String>> {
    let mut names = Vec::<String>::new();
    let mut entries = git_config
        .entries(Some(&format!("^{SECTION}\\.")))
        .map_err(Error::ReadGitConfig)?;
    while let Some(entry) = entries.next() {
        let entry = entry.map_err(Error::ReadGitConfig)?;
        let name = entry.name().ok_or_else(|| bad_name(entry.name_bytes()))?;
        if !names.iter().any(|seen| seen == name) {
            names.push(name.to_owned());
        }
    }
    Ok(names)
}

fn bad_name(name: &[u8]) -> Error {
    config_error(&format!(
        "{}: not UTF-8",
        String::from_utf8_lossy(name).into_owned()
    ))
}

/// The value git gives `name`: the last one set. A key with no `=` has none.
fn value(git_config: &git2::Config, name: &str) -> Res<Option<String>> {
    let entry = git_config.get_entry(name).map_err(Error::ReadGitConfig)?;
    if !entry.has_value() {
        return Ok(None);
    }
    entry
        .value()
        .map(|value| Some(value.to_owned()))
        .ok_or_else(|| config_error(&format!("{name}: not UTF-8")))
}

/// Every value of `name`, in the order git gives them.
fn values(git_config: &git2::Config, name: &str) -> Res<Vec<String>> {
    let mut values = vec![];
    let mut entries = git_config
        .multivar(name, None)
        .map_err(Error::ReadGitConfig)?;
    while let Some(entry) = entries.next() {
        let entry = entry.map_err(Error::ReadGitConfig)?;
        values.push(
            entry
                .value()
                .ok_or_else(|| config_error(&format!("{name}: not UTF-8")))?
                .to_owned(),
        );
    }
    Ok(values)
}

/// Where the git config key `name` is in gitu's config.
fn config_path(name: &str) -> Vec<String> {
    let key = &name[SECTION.len() + 1..];
    let (subsection, variable) = match key.rsplit_once('.') {
        Some((subsection, variable)) => (subsection, variable),
        None => ("general", key),
    };
    let mut path: Vec<String> = subsection.split('.').map(underscored).collect();
    if [FEATURE_KEYS, COMMANDS, VIEWS].contains(&path.join(".").as_str()) {
        path.push(variable.to_owned());
    } else {
        path.push(underscored(variable));
    }
    path
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
