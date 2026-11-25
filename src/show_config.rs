//! `delta tui --show-config`: every setting the TUI takes from git config's `[delta]` section,
//! what it does, and its value in the repository it is run in.
//!
//! The settings are those [`config_git`] reads, named as it names them, and the delta options
//! gitu draws comments with ([`crate::screen::review::OPTIONS`]). What a setting does is the
//! comment over it in gitu's defaults, which the defaults keep for each one.

use crate::{
    Res,
    config::{Config, DEFAULT_CONFIG},
    config_git::{self, Kind},
    config_migrate::quoted,
    error::Error,
    screen::review::{OPTION_NAMES, OPTIONS, comment_style_default},
};
use figment::{
    Figment,
    providers::{Format, Serialized, Toml},
    value::{Dict, Value},
};
use std::{collections::HashMap, path::Path};

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

/// What `bind` does, which has no comment of its own in gitu's defaults.
const BIND: &str = "A key for an action, as menu.action key, one key an entry. An action's keys \
                    replace its default keys. ? lists every action.";

/// Every setting the TUI takes from git config, with its value in the repository at `dir` with
/// `config`.
pub(crate) fn settings(config: &Config, dir: &Path) -> Res<Vec<Shown>> {
    let defaults = Figment::from(Toml::string(DEFAULT_CONFIG));
    let renderer = &config.general.diff_renderer;
    let git = Figment::from(Serialized::defaults(config_git::read(
        &renderer.options,
        dir,
        None,
        &defaults,
    )?));
    let values = defaults
        .clone()
        .merge(config.file.clone())
        .merge(git.clone());
    let descriptions = descriptions(DEFAULT_CONFIG);

    let mut shown = vec![];
    for setting in config_git::settings(&defaults) {
        let path = setting.path.join(".");
        let description = if setting.kind == Kind::Bindings {
            BIND.to_owned()
        } else {
            descriptions.get(&path).cloned().unwrap_or_default()
        };
        shown.push(Shown {
            values: strings(values.find_value(&path).ok(), setting.kind, &setting.name)?,
            default: strings(defaults.find_value(&path).ok(), setting.kind, &setting.name)?,
            set: git.contains(&path) || config.file.contains(&path),
            name: setting.name,
            description,
        });
    }

    let options = renderer.options.get(dir, None, &OPTION_NAMES);
    for (option, value) in OPTIONS.iter().zip(options) {
        let default = match option.name {
            "comment-style" => Some(comment_style_default(renderer.color_mode)),
            _ => option.default,
        }
        .filter(|default| !default.is_empty())
        .map(str::to_owned);
        shown.push(Shown {
            name: option.name.to_owned(),
            description: option.description.to_owned(),
            set: value.is_some(),
            values: value.or(default.clone()).into_iter().collect(),
            default: default.into_iter().collect(),
        });
    }
    Ok(shown)
}

/// What each setting in gitu's `defaults` does, by its path in gitu's config: the comment over
/// it. A comment says what the run of settings under it does, up to a blank line or the next
/// comment. A setting with no default, which is only an example in a comment, takes the comment
/// it is in.
fn descriptions(defaults: &str) -> HashMap<String, String> {
    let mut descriptions = HashMap::new();
    let mut section = String::new();
    let mut comment: Vec<&str> = vec![];
    let mut run: Option<String> = None;
    for line in defaults.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            comment.clear();
            run = None;
        } else if let Some(text) = line.strip_prefix('#') {
            run = None;
            let text = text.strip_prefix(' ').unwrap_or(text);
            comment.push(text);
            for unset in config_git::UNSET_BY_DEFAULT {
                let (unset_section, key) = unset.split_once('.').unwrap_or_default();
                if unset_section == section && setting_of(text.trim()) == Some(key) {
                    descriptions.insert(unset.to_owned(), comment.join("\n"));
                }
            }
        } else if let Some(name) = line
            .strip_prefix('[')
            .and_then(|line| line.strip_suffix(']'))
        {
            section = name.to_owned();
            comment.clear();
            run = None;
        } else if let Some(key) = setting_of(line) {
            if !comment.is_empty() {
                run = Some(comment.join("\n"));
                comment.clear();
            }
            if let Some(description) = &run {
                descriptions.insert(format!("{section}.{key}"), description.clone());
            }
        }
    }
    descriptions
}

/// The setting `line` sets, as `key = value` or `a.b = value`.
fn setting_of(line: &str) -> Option<&str> {
    let (key, _) = line.split_once('=')?;
    let key = key.trim();
    key.chars()
        .all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c))
        .then_some(key)
        .filter(|key| !key.is_empty())
}

/// `value` as git config gives the setting `name` of `kind`: an item for each of a list's items.
fn strings(value: Option<Value>, kind: Kind, name: &str) -> Res<Vec<String>> {
    let Some(value) = value else {
        return Ok(vec![]);
    };
    let unexpected = || {
        Error::Config(Box::new(figment::Error::from(format!(
            "{name}: gitu's config has {value:?}, which is not a {kind:?}"
        ))))
    };
    Ok(match (kind, &value) {
        (Kind::Bool | Kind::Int | Kind::Str, _) => scalar(&value).into_iter().collect(),
        (Kind::List, Value::Array(_, items)) => items.iter().filter_map(scalar).collect(),
        (Kind::Style, Value::Dict(_, style)) => vec![style_string(style, name)?],
        (Kind::Table, Value::Dict(_, table)) => table
            .iter()
            .filter_map(|(entry, value)| Some(format!("{entry} {}", scalar(value)?)))
            .collect(),
        (Kind::Bindings, Value::Dict(_, menus)) => bindings(menus),
        _ => return Err(unexpected()),
    })
}

/// A string, number or boolean as git config gives it, or `None` for no value.
fn scalar(value: &Value) -> Option<String> {
    match value {
        Value::String(_, value) => Some(value.clone()),
        Value::Char(_, value) => Some(value.to_string()),
        Value::Bool(_, value) => Some(value.to_string()),
        Value::Num(_, number) => number
            .to_i128()
            .map(|number| number.to_string())
            .or_else(|| number.to_f64().map(|number| number.to_string())),
        _ => None,
    }
}

/// A style in gitu's config, `{ fg, bg, mods }`, as delta writes one: its attributes, then its
/// foreground and background colors, with `normal` for no foreground before a background.
fn style_string(style: &Dict, name: &str) -> Res<String> {
    let mut words: Vec<String> = vec![];
    if let Some(Value::String(_, mods)) = style.get("mods") {
        for modifier in mods.split('|').map(str::trim).filter(|m| !m.is_empty()) {
            let word = match modifier {
                "BOLD" => "bold",
                "DIM" => "dim",
                "ITALIC" => "italic",
                "UNDERLINED" => "ul",
                "SLOW_BLINK" => "blink",
                "REVERSED" => "reverse",
                "HIDDEN" => "hidden",
                "CROSSED_OUT" => "strike",
                _ => {
                    return Err(Error::Config(Box::new(figment::Error::from(format!(
                        "{name}: delta's styles have no word for {modifier}"
                    )))));
                }
            };
            words.push(word.to_owned());
        }
    }
    // A color as one word, as delta reads one: `light red` is `lightred`.
    let color = |key: &str| match style.get(key) {
        Some(Value::String(_, color)) => Some(color.replace(' ', "")),
        _ => None,
    };
    match (color("fg"), color("bg")) {
        (fg, Some(bg)) => {
            words.push(fg.unwrap_or_else(|| "normal".to_owned()));
            words.push(bg);
        }
        (Some(fg), None) => words.push(fg),
        (None, None) => {}
    }
    Ok(if words.is_empty() {
        "normal".to_owned()
    } else {
        words.join(" ")
    })
}

/// Key bindings in gitu's config, by menu and action, as `bind` takes them: `menu.action key`,
/// with dashes for underscores but in a menu argument, such as `--all`.
fn bindings(menus: &Dict) -> Vec<String> {
    let mut bindings = vec![];
    for (menu, actions) in menus {
        let Value::Dict(_, actions) = actions else {
            continue;
        };
        for (action, keys) in actions {
            let Value::Array(_, keys) = keys else {
                continue;
            };
            let action = if action.starts_with('-') {
                action.clone()
            } else {
                action.replace('_', "-")
            };
            for key in keys.iter().filter_map(scalar) {
                bindings.push(format!("{}.{action} {key}", menu.replace('_', "-")));
            }
        }
    }
    bindings
}

/// `settings` as git config's `[delta]` section: each setting under a comment saying what it
/// does, a list as an entry for each item, a setting with no value commented out, and the
/// default of one the user set to something else after it.
pub(crate) fn text(settings: &[Shown]) -> String {
    let mut out = String::from(
        "# delta tui's settings, as they go in git config's [delta] section, with their values \
         here.\n\n[delta]\n",
    );
    for setting in settings {
        out.push('\n');
        for line in setting.description.lines() {
            out.push_str(format!("# {line}").trim_end());
            out.push('\n');
        }
        if setting.values.is_empty() {
            out.push_str(&format!("# {} =\n", setting.name));
        }
        for value in &setting.values {
            out.push_str(&format!("{} = {}\n", setting.name, quoted(value)));
        }
        if setting.set && setting.values != setting.default {
            let default = match setting.default.as_slice() {
                [] => "none".to_owned(),
                default => default.join(", "),
            };
            out.push_str(&format!("# default: {default}\n"));
        }
    }
    out
}

/// `settings` as JSON: for each, its name, what it does, its value and its default, each as a
/// list of git config values, and whether the user set it.
pub(crate) fn json(settings: &[Shown]) -> serde_json::Value {
    settings
        .iter()
        .map(|setting| {
            serde_json::json!({
                "name": setting.name,
                "description": setting.description,
                "values": setting.values,
                "default": setting.default,
                "set": setting.set,
            })
        })
        .collect()
}
