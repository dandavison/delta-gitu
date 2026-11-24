use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::config_git::{self, FEATURE_KEYS};
use crate::style::{Color, Modifier, Style};
use crate::{Bindings, Res, error::Error, key_parser, menu::Menu, ops::Op};
use crossterm::event::{KeyCode, KeyModifiers};
use etcetera::{BaseStrategy, choose_base_strategy};
use figment::{
    Figment,
    providers::{Format, Serialized, Toml},
};
use serde::Deserialize;

pub(crate) const DEFAULT_CONFIG: &str = include_str!("default_config.toml");

pub struct Config {
    pub general: GeneralConfig,
    pub style: StyleConfig,
    pub bindings: Bindings,
    pub picker_bindings: PickerBindings,
    /// gitu's config file, which [`Config::resolved`] puts git config over, over gitu's
    /// defaults.
    pub(crate) file: Figment,
}

#[derive(Default, Deserialize)]
pub(crate) struct PickerBindingsConfig {
    #[serde(default)]
    pub next: Vec<String>,
    #[serde(default)]
    pub previous: Vec<String>,
    #[serde(default)]
    pub done: Vec<String>,
    #[serde(default)]
    pub cancel: Vec<String>,
    #[serde(default)]
    pub toggle_list: Vec<String>,
    #[serde(default)]
    pub quit: Vec<String>,
}

#[derive(Default, Deserialize)]
pub(crate) struct BindingsConfig {
    #[serde(flatten)]
    pub menus: BTreeMap<Menu, BTreeMap<Op, Vec<String>>>,
    #[serde(default)]
    pub picker: PickerBindingsConfig,
}

#[derive(Default, Deserialize)]
/// Only used to deserialise configurations with `figment`. This should be
/// parsed to be turned into a useful [`Config`].
pub(crate) struct FigmentConfig {
    pub general: GeneralConfig,
    pub style: StyleConfig,
    pub bindings: BindingsConfig,
}

#[derive(Default, Debug, Deserialize)]
pub struct GeneralConfig {
    pub always_show_help: BoolConfigEntry,
    pub confirm_quit: BoolConfigEntry,
    pub refresh_on_file_change: BoolConfigEntry,
    pub confirm_discard: ConfirmDiscardOption,
    pub collapsed_sections: Vec<String>,
    pub stash_list_limit: usize,
    pub recent_commits_limit: usize,
    pub log_author_width: usize,
    pub mouse_support: bool,
    pub mouse_scroll_lines: usize,
    /// The most input gitu will read, as a pager or when putting a command
    /// again. It is read in one pass before anything is drawn, so this is what
    /// stops a `git log -p` over a whole history from showing nothing for
    /// minutes. What is past the limit is dropped, not deferred.
    pub max_input_bytes: u64,
    /// Whether the branch section is 'Branch: {name}' over a line placing it
    /// relative to its upstream, or the branch name alone.
    pub verbose_branch_status: bool,
    /// Globs whose files are dropped from a paged view, as if the patch never
    /// had them. Generated code and fixtures recur, so hiding them is worth
    /// saying once rather than per session.
    #[serde(default)]
    pub hide: Vec<String>,
    /// Whether the cursor stops on a diff's unchanged lines, as reading a patch
    /// line by line wants. Nothing can be staged there.
    #[serde(default)]
    pub visit_context_lines: bool,
    /// Whether the status screen opens with the cursor on the first unstaged
    /// file, or the first staged one when nothing is unstaged.
    #[serde(default)]
    pub status_cursor_on_changes: bool,
    #[serde(default)]
    pub diff_renderer: DiffRendererConfig,
    /// How gitu runs difftastic, while the delta feature named difftastic is enabled.
    #[serde(default)]
    pub difftastic: DifftasticConfig,
    /// Arguments given to the `git log` gitu runs for the log view.
    #[serde(default)]
    pub log_args: Vec<String>,
    /// Arguments given to the `git show` gitu runs for a commit it opens.
    #[serde(default)]
    pub show_args: Vec<String>,
    #[serde(default)]
    pub file_header: FileHeader,
    /// Whether a patch starts with its files folded: `git diff`, `git show`,
    /// `git log -p`, and a commit opened from the log.
    #[serde(default)]
    pub start_folded: bool,
    /// Whether gitu leaves the repository and git config as it finds them.
    #[serde(default)]
    pub read_only: ReadOnly,
    /// Shell commands, by name, run by the keys `diff_renderer.feature_keys`
    /// gives them, with `{commit}`, `{file}` and `{branch}` filled in from what
    /// is under the cursor, `{command}` from the git command the screen is
    /// showing, and `{input}` from a prompt.
    #[serde(default)]
    pub commands: BTreeMap<String, String>,
    /// git commands, by name, opened as a screen by the keys
    /// `diff_renderer.feature_keys` gives them, filled in as commands are.
    #[serde(default)]
    pub views: BTreeMap<String, String>,
    #[serde(default)]
    pub alt_editor: Option<String>,
}

/// Whether gitu runs commands that change the repository (stage, commit,
/// rebase, fetch, ...) and saves what is chosen in-session (a theme, feature
/// keys, prompt history).
#[derive(Default, Debug, PartialEq, Eq, Clone, Copy)]
pub enum ReadOnly {
    /// Read-only in a repository jj also manages (one with a `.jj` directory),
    /// where a git command can contradict jj's own state.
    #[default]
    Auto,
    On,
    Off,
}

impl ReadOnly {
    pub fn is_on(self, repo: &git2::Repository) -> bool {
        match self {
            ReadOnly::Auto => repo.workdir().is_some_and(|dir| dir.join(".jj").is_dir()),
            ReadOnly::On => true,
            ReadOnly::Off => false,
        }
    }
}

/// A boolean, or `auto`. Git config gives a boolean as a string, written as
/// git accepts one.
impl<'de> Deserialize<'de> for ReadOnly {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Bool(bool),
            Str(String),
        }
        match Raw::deserialize(deserializer)? {
            Raw::Bool(true) => Ok(ReadOnly::On),
            Raw::Bool(false) => Ok(ReadOnly::Off),
            Raw::Str(value) => match value.to_lowercase().as_str() {
                "auto" => Ok(ReadOnly::Auto),
                "true" | "yes" | "on" | "1" | "" => Ok(ReadOnly::On),
                "false" | "no" | "off" | "0" => Ok(ReadOnly::Off),
                _ => Err(serde::de::Error::custom(format!(
                    "read_only: '{value}' is neither a boolean nor auto"
                ))),
            },
        }
    }
}

/// What heads each file of a patch, folded or not.
#[derive(Default, Debug, Deserialize, PartialEq, Eq, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum FileHeader {
    /// The renderer's file header.
    #[default]
    Delta,
    /// The file's line of a diffstat, counted by gitu from the patch.
    Stat,
}

#[derive(Default, Debug, Deserialize)]
pub struct DifftasticConfig {
    /// difftastic's command as `[executable, ...args]`. gitu adds `--display=json` and the two
    /// versions of a file.
    #[serde(default)]
    pub command: Vec<String>,
}

#[derive(Default, Debug, Deserialize)]
pub struct DiffRendererConfig {
    /// Renderer command as `[executable, ...args]`, set by `delta --gitu` to
    /// itself rather than read from config. It receives a unified diff on stdin
    /// and emits ANSI-colored output carrying OSC-1717 diff-line metadata
    /// (<https://github.com/stefanhaller/diff-line-metadata-spec/blob/main/diff-line-metadata-osc-1717.md>).
    #[serde(skip)]
    pub command: Vec<String>,
    /// Named renderer features offered by `renderer_features`, to turn on and
    /// off while viewing a diff.
    #[serde(default)]
    pub features: Vec<String>,
    /// Keys that toggle a feature from `renderer_features` before anything is
    /// typed to filter it, by feature, or run a command from
    /// `general.commands` or open a view from `general.views`, by its name.
    #[serde(default)]
    pub feature_keys: BTreeMap<String, char>,
    /// The theme overlaid on the user's own delta configuration: a feature that
    /// sets `dark` or `light`, as delta defines a theme.
    #[serde(default)]
    pub theme: Option<String>,
    /// delta's builtin features to offer as themes after those in git config,
    /// for either mode, such as `diff-so-fancy`.
    #[serde(default)]
    pub builtin_themes: Vec<String>,
    /// The mode delta renders in, set by `delta --gitu`, which settles it before
    /// gitu starts. `None` when gitu is not run by delta.
    #[serde(skip)]
    pub color_mode: Option<ColorMode>,
    /// Which features the user's own configuration enables, by delta's name, asked each time the
    /// answer is needed, so that a change to the user's git config during a session counts. Set
    /// by `delta --gitu`; without it, the user's configuration enables none.
    #[serde(skip)]
    pub enabled_features: EnabledFeatures,
    /// The git config the renderer reads in the repository at a path, which gitu uses for what it
    /// offers from git config, such as themes. Set by `delta --gitu`; without it, gitu reads the
    /// repository's own.
    #[serde(skip)]
    pub git_config: RendererGitConfig,
    /// The values the renderer's config gives options it does not use itself but gitu draws with,
    /// such as `comment-style`, with the features in force. Set by `delta --gitu`, so that they are
    /// set where delta's own options are and taken from features as delta's are; without it, gitu
    /// uses its defaults.
    #[serde(skip)]
    pub options: RendererOptions,
    /// Whether the renderer draws side by side at a width. Set by `delta --gitu`, since delta's
    /// `side-by-side = N` draws side by side only at a width of at least `N`; without it, gitu
    /// takes the renderer to draw side by side when its `side-by-side` feature is enabled.
    #[serde(skip)]
    pub side_by_side: RendererSideBySide,
}

/// Asks the renderer what its config sets options to, in the repository at a path, with features
/// in force as `DELTA_FEATURES` gives them, for each of several option names: the value of an
/// option, or every item of a list, which takes those of the renderer's own section and of
/// every feature in force, lowest precedence first.
#[derive(Clone, Default)]
pub struct RendererOptions {
    values: Option<Arc<AskOptions>>,
    lists: Option<Arc<AskLists>>,
}

type AskOptions = dyn Fn(&Path, Option<&str>, &[&str]) -> Vec<Option<String>> + Send + Sync;
type AskLists = dyn Fn(&Path, Option<&str>, &[&str]) -> Vec<Vec<String>> + Send + Sync;

impl RendererOptions {
    pub fn new(
        values: impl Fn(&Path, Option<&str>, &[&str]) -> Vec<Option<String>> + Send + Sync + 'static,
        lists: impl Fn(&Path, Option<&str>, &[&str]) -> Vec<Vec<String>> + Send + Sync + 'static,
    ) -> Self {
        Self {
            values: Some(Arc::new(values)),
            lists: Some(Arc::new(lists)),
        }
    }

    /// The value of each of `names`, `None` for one not set, or for all of them when the
    /// renderer has not said how to ask.
    pub fn get(&self, dir: &Path, features: Option<&str>, names: &[&str]) -> Vec<Option<String>> {
        match &self.values {
            Some(ask) => ask(dir, features, names),
            None => vec![None; names.len()],
        }
    }

    /// The items of each of `names`, lowest precedence first: none when the renderer has not
    /// said how to ask.
    pub fn get_all(&self, dir: &Path, features: Option<&str>, names: &[&str]) -> Vec<Vec<String>> {
        match &self.lists {
            Some(ask) => ask(dir, features, names),
            None => vec![vec![]; names.len()],
        }
    }

    /// Whether the renderer has said how to ask.
    pub fn is_set(&self) -> bool {
        self.values.is_some()
    }
}

impl std::fmt::Debug for RendererOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.is_set() {
            "RendererOptions(..)"
        } else {
            "RendererOptions(none)"
        })
    }
}

/// Asks the renderer whether it draws side by side in the repository at a path, with features in
/// force as `DELTA_FEATURES` gives them, at a width of a number of columns, as `COLUMNS` gives it.
#[derive(Clone, Default)]
pub struct RendererSideBySide(Option<Arc<AskSideBySide>>);

type AskSideBySide = dyn Fn(&Path, Option<&str>, usize) -> bool + Send + Sync;

impl RendererSideBySide {
    pub fn new(ask: impl Fn(&Path, Option<&str>, usize) -> bool + Send + Sync + 'static) -> Self {
        Self(Some(Arc::new(ask)))
    }

    /// Whether the renderer draws side by side, or `None` if the renderer has not said how to ask.
    pub fn get(&self, dir: &Path, features: Option<&str>, columns: usize) -> Option<bool> {
        self.0.as_ref().map(|ask| ask(dir, features, columns))
    }
}

impl std::fmt::Debug for RendererSideBySide {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_some() {
            "RendererSideBySide(..)"
        } else {
            "RendererSideBySide(none)"
        })
    }
}

/// Opens the git config the renderer reads in the repository at a path.
#[derive(Clone, Default)]
pub struct RendererGitConfig(Option<Arc<OpenGitConfig>>);

type OpenGitConfig = dyn Fn(&Path) -> Option<git2::Config> + Send + Sync;

impl RendererGitConfig {
    pub fn new(open: impl Fn(&Path) -> Option<git2::Config> + Send + Sync + 'static) -> Self {
        Self(Some(Arc::new(open)))
    }

    /// The renderer's git config at `dir`, or `None` if the renderer has not said how it opens it.
    pub fn open(&self, dir: &Path) -> Option<Option<git2::Config>> {
        self.0.as_ref().map(|open| open(dir))
    }
}

impl std::fmt::Debug for RendererGitConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_some() {
            "RendererGitConfig(..)"
        } else {
            "RendererGitConfig(none)"
        })
    }
}

/// Asks the renderer which features the user's own configuration, as it is now, enables in the
/// repository at a path.
#[derive(Clone, Default)]
pub struct EnabledFeatures(Option<Arc<AskEnabledFeatures>>);

type AskEnabledFeatures = dyn Fn(&Path) -> Vec<String> + Send + Sync;

impl EnabledFeatures {
    pub fn new(ask: impl Fn(&Path) -> Vec<String> + Send + Sync + 'static) -> Self {
        Self(Some(Arc::new(ask)))
    }

    pub fn get(&self, dir: &Path) -> Vec<String> {
        self.0.as_ref().map(|ask| ask(dir)).unwrap_or_default()
    }
}

impl std::fmt::Debug for EnabledFeatures {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_some() {
            "EnabledFeatures(..)"
        } else {
            "EnabledFeatures(none)"
        })
    }
}

/// Whether delta renders for a dark or a light terminal background.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorMode {
    Dark,
    Light,
}

#[derive(Default, Debug, Deserialize)]
pub struct BoolConfigEntry {
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Default, Debug, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum ConfirmDiscardOption {
    #[default]
    Line,
    Hunk,
    File,
    Never,
}

#[derive(Default, Debug, Deserialize)]
pub struct StyleConfig {
    pub separator: StyleConfigEntry,

    pub info_msg: StyleConfigEntry,
    pub error_msg: StyleConfigEntry,
    pub command: StyleConfigEntry,

    #[serde(default)]
    pub menu: MenuStyleConfig,

    pub prompt: StyleConfigEntry,

    pub section_header: StyleConfigEntry,
    pub file_header: StyleConfigEntry,
    pub hunk_header: StyleConfigEntry,

    pub pr_summary: StyleConfigEntry,

    #[serde(default)]
    pub picker: PickerStyleConfig,

    pub cursor: SymbolStyleConfigEntry,
    pub selection_bar: SymbolStyleConfigEntry,
    pub selection_line: StyleConfigEntry,
    pub selection_area: StyleConfigEntry,

    #[serde(default)]
    pub search_match: StyleConfigEntry,

    pub hash: StyleConfigEntry,
    /// The instruction keywords in the interactive rebase todo view.
    #[serde(default)]
    pub rebase_todo: RebaseTodoStyleConfig,
    pub branch: StyleConfigEntry,
    pub remote: StyleConfigEntry,
    pub tag: StyleConfigEntry,
    pub author: StyleConfigEntry,
    pub age: StyleConfigEntry,

    #[serde(default)]
    pub blame: BlameStyleConfig,
}

#[derive(Default, Debug, Deserialize)]
pub struct RebaseTodoStyleConfig {
    #[serde(default)]
    pub reword: StyleConfigEntry,
    #[serde(default)]
    pub edit: StyleConfigEntry,
    #[serde(default)]
    pub squash: StyleConfigEntry,
    #[serde(default)]
    pub fixup: StyleConfigEntry,
    #[serde(default)]
    pub drop: StyleConfigEntry,
}

#[derive(Default, Debug, Deserialize)]
pub struct BlameStyleConfig {
    #[serde(default)]
    pub line_num: StyleConfigEntry,
    #[serde(default)]
    pub code_line: StyleConfigEntry,
}

#[derive(Default, Debug, Deserialize)]
pub struct MenuStyleConfig {
    #[serde(default)]
    pub heading: StyleConfigEntry,
    #[serde(default)]
    pub key: StyleConfigEntry,
    /// Active argument value display (e.g., "--interactive")
    #[serde(default)]
    pub active_arg: StyleConfigEntry,
    /// Inactive argument value display
    #[serde(default)]
    pub inactive_arg: StyleConfigEntry,
}

#[derive(Default, Debug, Deserialize)]
pub struct PickerStyleConfig {
    #[serde(default)]
    pub prompt: StyleConfigEntry,
    #[serde(default)]
    pub info: StyleConfigEntry,
    #[serde(default)]
    pub selection_line: StyleConfigEntry,
    #[serde(default)]
    pub matched: StyleConfigEntry,
}

#[derive(Default, Debug, Deserialize)]
pub struct StyleConfigEntry {
    #[serde(default)]
    fg: Option<Color>,
    #[serde(default)]
    bg: Option<Color>,
    #[serde(default)]
    mods: Option<Modifier>,
}

#[derive(Default, Debug, Deserialize)]
pub struct SymbolStyleConfigEntry {
    #[serde(default)]
    pub symbol: char,
    #[serde(default)]
    fg: Option<Color>,
    #[serde(default)]
    bg: Option<Color>,
    #[serde(default)]
    mods: Option<Modifier>,
}

impl From<&StyleConfigEntry> for Style {
    fn from(val: &StyleConfigEntry) -> Self {
        Style {
            fg: val.fg,
            bg: val.bg,
            add_modifier: val.mods.unwrap_or(Modifier::empty()),
            sub_modifier: Modifier::empty(),
        }
    }
}

impl From<&SymbolStyleConfigEntry> for Style {
    fn from(val: &SymbolStyleConfigEntry) -> Self {
        Style {
            fg: val.fg,
            bg: val.bg,
            add_modifier: val.mods.unwrap_or(Modifier::empty()),
            sub_modifier: Modifier::empty(),
        }
    }
}

pub struct PickerBindings {
    pub next: Vec<Vec<(KeyModifiers, KeyCode)>>,
    pub previous: Vec<Vec<(KeyModifiers, KeyCode)>>,
    pub done: Vec<Vec<(KeyModifiers, KeyCode)>>,
    pub cancel: Vec<Vec<(KeyModifiers, KeyCode)>>,
    pub toggle_list: Vec<Vec<(KeyModifiers, KeyCode)>>,
    pub quit: Vec<Vec<(KeyModifiers, KeyCode)>>,
}

impl TryFrom<PickerBindingsConfig> for PickerBindings {
    type Error = crate::error::Error;

    fn try_from(config: PickerBindingsConfig) -> Result<Self, Self::Error> {
        let mut bad_bindings = Vec::new();

        let next = parse_picker_keys(&config.next, "picker.next", &mut bad_bindings);
        let previous = parse_picker_keys(&config.previous, "picker.previous", &mut bad_bindings);
        let done = parse_picker_keys(&config.done, "picker.done", &mut bad_bindings);
        let cancel = parse_picker_keys(&config.cancel, "picker.cancel", &mut bad_bindings);
        let toggle_list =
            parse_picker_keys(&config.toggle_list, "picker.toggle_list", &mut bad_bindings);
        let quit = parse_picker_keys(&config.quit, "picker.quit", &mut bad_bindings);

        if !bad_bindings.is_empty() {
            return Err(Error::Bindings {
                bad_key_bindings: bad_bindings,
            });
        }

        Ok(Self {
            next,
            previous,
            done,
            cancel,
            toggle_list,
            quit,
        })
    }
}

fn parse_picker_keys(
    raw_keys: &[String],
    action_name: &str,
    bad_bindings: &mut Vec<String>,
) -> Vec<Vec<(KeyModifiers, KeyCode)>> {
    raw_keys
        .iter()
        .filter_map(|keys| {
            if let Ok(("", parsed)) = key_parser::parse_config_keys(keys) {
                Some(parsed)
            } else {
                bad_bindings.push(format!("- {} = {} (not a key)", action_name, keys));
                None
            }
        })
        .collect()
}

/// gitu's config file at `path`, or at its default place, over gitu's defaults. Settings in git
/// config are put over these by [`Config::resolved`], once the renderer has said how to ask for
/// them.
pub fn init_config(path: Option<PathBuf>) -> Res<Config> {
    init_config_from(path.unwrap_or_else(config_path))
}

/// The config in the gitu config file at `config_path`, over gitu's defaults.
pub(crate) fn init_config_from(config_path: PathBuf) -> Res<Config> {
    if config_path.exists() {
        log::info!("Loading config file at {config_path:?}");
    } else {
        log::info!("No config file at {config_path:?}");
    }
    build(Figment::from(Toml::file(&config_path)), Figment::new())
}

/// gitu's defaults.
impl Default for Config {
    fn default() -> Self {
        build(Figment::new(), Figment::new()).expect("gitu's defaults are a valid config")
    }
}

/// The names of the settings gitu takes from git config, which are delta options delta does
/// not use itself.
pub fn delta_option_names() -> Vec<String> {
    config_git::names()
}

impl Config {
    /// This config with the settings in the renderer's git config in the repository at `dir` put
    /// over it, as the renderer resolves them with `features` in force (as `DELTA_FEATURES`
    /// gives them, or with `None` as the renderer's environment does). What the renderer has
    /// told gitu, such as how to ask it, is kept. Git config's old `[delta-tui]` sections are an
    /// error, rather than being silently ignored.
    pub fn resolved(&self, dir: &Path, features: Option<&str>) -> Res<Config> {
        let renderer = &self.general.diff_renderer;
        if let Some(Some(git_config)) = renderer.git_config.open(dir)
            && config_git::has_old_section(&git_config)?
        {
            return Err(Error::Config(Box::new(figment::Error::from(
                "git config's [delta-tui] sections are no longer read: their settings go in \
                 [delta] or a [delta \"feature\"] section. \
                 `delta --tui --migrate-old-config` prints them as they go in [delta]. \
                 See https://dandavison.github.io/delta/tui.html"
                    .to_owned(),
            ))));
        }
        let defaults = Figment::from(Toml::string(DEFAULT_CONFIG));
        let git = config_git::read(&renderer.options, dir, features, &defaults)?;
        let mut config = build(self.file.clone(), Figment::from(Serialized::defaults(git)))?;
        let resolved = &mut config.general.diff_renderer;
        resolved.command = renderer.command.clone();
        resolved.color_mode = renderer.color_mode;
        resolved.enabled_features = renderer.enabled_features.clone();
        resolved.git_config = renderer.git_config.clone();
        resolved.options = renderer.options.clone();
        resolved.side_by_side = renderer.side_by_side.clone();
        Ok(config)
    }
}

/// gitu's defaults, with `file`, gitu's config file, over them, and `git`, the settings in git
/// config, over that.
fn build(file: Figment, git: Figment) -> Res<Config> {
    let defaults = Figment::from(Toml::string(DEFAULT_CONFIG));
    let default_bindings: BindingsConfig = defaults
        .extract_inner("bindings")
        .map_err(Box::new)
        .map_err(Error::Config)?;
    let FigmentConfig {
        mut general,
        style,
        bindings: bindings_config,
    } = defaults
        .clone()
        .merge(file.clone())
        .merge(git.clone())
        .extract()
        .map_err(Box::new)
        .map_err(Error::Config)?;
    let users = |path: &str| git.contains(path) || file.contains(path);
    if let Some(feature_keys) = users_feature_keys(&git)?.or(users_feature_keys(&file)?) {
        general.diff_renderer.feature_keys = feature_keys;
    }
    let bindings = Bindings::try_from(bindings_config.menus)?
        .set_by_user(&default_bindings.menus, |path| {
            users(&format!("bindings.{path}"))
        });
    let picker_bindings = PickerBindings::try_from(bindings_config.picker)?;
    check_feature_keys(&general.diff_renderer.feature_keys, &bindings)?;

    Ok(Config {
        general,
        style,
        bindings,
        picker_bindings,
        file,
    })
}

/// The feature keys in one layer of the user's config, which are all of them
/// rather than additions to those under it, so that a key can be unset.
fn users_feature_keys(figment: &Figment) -> Res<Option<BTreeMap<String, char>>> {
    if !figment.contains(FEATURE_KEYS) {
        return Ok(None);
    }
    figment
        .extract_inner(FEATURE_KEYS)
        .map(Some)
        .map_err(Box::new)
        .map_err(Error::Config)
}

/// A feature's key is a top-level key, so one that is bound already would do two
/// things.
fn check_feature_keys(feature_keys: &BTreeMap<String, char>, bindings: &Bindings) -> Res<()> {
    let taken: Vec<String> = feature_keys
        .iter()
        .filter(|(_, key)| bindings.binds_at_top_level(**key))
        .map(|(feature, key)| {
            format!("- general.diff_renderer.feature_keys.{feature} = {key} (bound already)")
        })
        .collect();
    if !taken.is_empty() {
        return Err(Error::Bindings {
            bad_key_bindings: taken,
        });
    }
    Ok(())
}

pub fn config_path() -> PathBuf {
    choose_base_strategy()
        .expect("Unable to find the config directory!")
        .config_dir()
        .join("gitu/config.toml")
}

#[cfg(test)]
pub(crate) const TEST_SEARCH_HIGHLIGHT_BG: Color = Color::LightYellow;

#[cfg(test)]
pub(crate) fn init_test_config() -> Res<Config> {
    let FigmentConfig {
        mut general,
        mut style,
        bindings: bindings_config,
    } = Figment::new()
        .merge(Toml::string(DEFAULT_CONFIG))
        .extract()
        .map_err(Box::new)
        .map_err(Error::Config)?;

    general.always_show_help.enabled = false;
    general.refresh_on_file_change.enabled = false;
    general.status_cursor_on_changes = false;
    general.diff_renderer.command = crate::tests::helpers::ui::renderer();

    style.search_match = StyleConfigEntry {
        fg: None,
        bg: Some(TEST_SEARCH_HIGHLIGHT_BG),
        mods: None,
    };

    Ok(Config {
        general,
        style,
        bindings: Bindings::try_from(bindings_config.menus).unwrap(),
        picker_bindings: PickerBindings::try_from(bindings_config.picker).unwrap(),
        file: Figment::new(),
    })
}

/// delta's options in the git config at `path`, as delta resolves them, simplified: the features
/// in force are those `[delta] features` lists, then those `DELTA_FEATURES` turns on (`+name`)
/// less those it turns off (`-name`). A value in `[delta]` beats a feature's, and a later
/// feature's an earlier one's; a list takes the items of each feature in turn, then of
/// `[delta]`.
#[cfg(test)]
pub(crate) fn options_in(path: PathBuf) -> RendererOptions {
    fn sections(git_config: &git2::Config, overlay: Option<&str>) -> Vec<String> {
        let listed = git_config.get_string("delta.features").unwrap_or_default();
        let mut features: Vec<String> = listed.split_whitespace().map(String::from).collect();
        let overlay = overlay.unwrap_or_default();
        for word in overlay.trim_start_matches('+').split_whitespace() {
            match word.strip_prefix('-') {
                Some(off) => features.retain(|feature| feature != off),
                None => features.push(word.to_owned()),
            }
        }
        features
            .iter()
            .map(|feature| format!("delta.{feature}"))
            .chain(["delta".to_owned()])
            .collect()
    }
    fn all(git_config: &git2::Config, key: &str) -> Vec<String> {
        let mut values = vec![];
        if let Ok(mut entries) = git_config.multivar(key, None) {
            while let Some(Ok(entry)) = entries.next() {
                values.extend(entry.value().map(String::from));
            }
        }
        values
    }
    let lists_path = path.clone();
    RendererOptions::new(
        move |_, overlay, names| {
            let git_config = git2::Config::open(&path).unwrap();
            let sections = sections(&git_config, overlay);
            names
                .iter()
                .map(|name| {
                    sections
                        .iter()
                        .rev()
                        .find_map(|section| all(&git_config, &format!("{section}.{name}")).pop())
                })
                .collect()
        },
        move |_, overlay, names| {
            let git_config = git2::Config::open(&lists_path).unwrap();
            let sections = sections(&git_config, overlay);
            names
                .iter()
                .map(|name| {
                    sections
                        .iter()
                        .flat_map(|section| all(&git_config, &format!("{section}.{name}")))
                        .collect()
                })
                .collect()
        },
    )
}

/// `config` told, as delta tells gitu, about the git config at `path`: what it sets delta's
/// options to (see [`options_in`]), how to open it, and the features it enables.
#[cfg(test)]
pub(crate) fn told_about(config: &mut Config, path: PathBuf) {
    let renderer = &mut config.general.diff_renderer;
    renderer.options = options_in(path.clone());
    let enabled = path.clone();
    renderer.enabled_features = EnabledFeatures::new(move |_| {
        git2::Config::open(&enabled)
            .and_then(|git_config| git_config.get_string("delta.features"))
            .unwrap_or_default()
            .split_whitespace()
            .map(String::from)
            .collect()
    });
    renderer.git_config = RendererGitConfig::new(move |_| git2::Config::open(&path).ok());
}

/// The config read from a gitu config file holding `toml` and git config holding `gitconfig`,
/// with `features` in force as `DELTA_FEATURES` gives them.
#[cfg(test)]
pub(crate) fn configured_with(toml: &str, gitconfig: &str, features: Option<&str>) -> Res<Config> {
    let dir = temp_dir::TempDir::new().unwrap();
    let toml_path = dir.path().join("config.toml");
    std::fs::write(&toml_path, toml).unwrap();
    let git_path = dir.path().join("gitconfig");
    std::fs::write(&git_path, gitconfig).unwrap();
    let mut config = init_config_from(toml_path)?;
    told_about(&mut config, git_path);
    config.resolved(dir.path(), features)
}

/// The config read from a gitu config file holding `toml` and git config holding `gitconfig`.
#[cfg(test)]
pub(crate) fn configured(toml: &str, gitconfig: &str) -> Config {
    configured_with(toml, gitconfig, None).unwrap()
}

#[cfg(test)]
mod tests {

    use crate::style::{Color, Modifier};
    use figment::{
        Figment,
        providers::{Format, Toml},
    };

    use super::{DEFAULT_CONFIG, FigmentConfig, configured, configured_with};

    #[test]
    fn config_merges() {
        let config: FigmentConfig = Figment::new()
            .merge(Toml::string(DEFAULT_CONFIG))
            .merge(Toml::string(
                r#"
                [style]
                hunk_header.bg = "light green"
                "#,
            ))
            .extract()
            .unwrap();

        assert_eq!(config.style.hunk_header.bg, Some(Color::LightGreen));
        assert_eq!(config.style.hunk_header.fg, Some(Color::Blue));
    }

    fn config_error(gitconfig: &str) -> String {
        match configured_with("", gitconfig, None) {
            Ok(_) => panic!("{gitconfig} was accepted"),
            Err(e) => e.to_string(),
        }
    }

    /// A binding that is not a key says so, and where to fix it.
    #[test]
    fn a_binding_that_is_not_a_key_says_so() {
        assert_eq!(
            config_error("[delta]\n\tbind = root.quit ctrl+\n"),
            "Fix these key bindings in git config or gitu's config file:\n\
             - root.quit = ctrl+ (not a key)"
        );
    }

    #[test]
    fn a_binding_that_is_not_a_menu_action_and_a_key_says_so() {
        assert!(config_error("[delta]\n\tbind = quit\n").contains("root.quit"));
    }

    #[test]
    fn git_config_sets_general_settings() {
        let config = configured(
            "",
            "[delta]\n\
             \tstart-folded = true\n\
             \tfile-header = stat\n\
             \tstash-list-limit = 3\n",
        );
        assert!(config.general.start_folded);
        assert_eq!(config.general.file_header, super::FileHeader::Stat);
        assert_eq!(config.general.stash_list_limit, 3);
    }

    /// As git reads booleans and integers.
    #[test]
    fn git_config_values_are_read_as_git_reads_them() {
        let config = configured(
            "",
            "[delta]\n\
             \tstart-folded = yes\n\
             \tvisit-context-lines = no\n\
             \tmax-input-bytes = 4m\n",
        );
        assert!(config.general.start_folded);
        assert!(!config.general.visit_context_lines);
        assert_eq!(config.general.max_input_bytes, 4 * 1024 * 1024);
    }

    /// A list is a key given once per item, as git's own lists are.
    #[test]
    fn git_config_lists_are_repeated_keys() {
        let config = configured(
            "",
            "[delta]\n\
             \tlog-args = --stat\n\
             \tlog-args = --pretty=format:%h %s\n\
             \tshow-args = --stat\n",
        );
        assert_eq!(config.general.log_args, ["--stat", "--pretty=format:%h %s"]);
        assert_eq!(config.general.show_args, ["--stat"]);
    }

    #[test]
    fn git_config_overrides_the_gitu_config_file() {
        let config = configured(
            "[general]\nstart_folded = true\nstash_list_limit = 3\n",
            "[delta]\n\tstart-folded = false\n",
        );
        assert!(!config.general.start_folded);
        assert_eq!(config.general.stash_list_limit, 3);
    }

    /// A nested setting's name is its path, joined with dashes.
    #[test]
    fn nested_settings_are_named_by_their_path() {
        let config = configured(
            "",
            "[delta]\n\
             \tdiff-renderer-features = side-by-side\n\
             \tdiff-renderer-features = my-*\n\
             \tconfirm-quit-enabled = true\n",
        );
        assert_eq!(
            config.general.diff_renderer.features,
            ["side-by-side", "my-*"]
        );
        assert!(config.general.confirm_quit.enabled);
    }

    /// Builtin themes are a list, one key per item, as other lists are.
    #[test]
    fn git_config_lists_builtin_themes() {
        let config = configured(
            "",
            "[delta]\n\tdiff-renderer-builtin-themes = diff-so-fancy\n",
        );
        assert_eq!(
            config.general.diff_renderer.builtin_themes,
            ["diff-so-fancy"]
        );
    }

    /// A feature key is the feature's own name, dashes and all, then the key, and as in the gitu
    /// config file they replace the default ones.
    #[test]
    fn git_config_sets_feature_keys_by_feature_name() {
        let config = configured("", "[delta]\n\tfeature-key = my-feature e\n");
        assert_eq!(
            config
                .general
                .diff_renderer
                .feature_keys
                .into_iter()
                .collect::<Vec<_>>(),
            [("my-feature".to_string(), 'e')]
        );
    }

    /// A command is its own name, dashes and all, then the command.
    #[test]
    fn git_config_sets_commands_by_name() {
        let config = configured(
            "",
            "[delta]\n\tcommand = open-pr gh pr view --web {commit}\n",
        );
        assert_eq!(
            config.general.commands.into_iter().collect::<Vec<_>>(),
            [(
                "open-pr".to_string(),
                "gh pr view --web {commit}".to_string()
            )]
        );
    }

    /// A view is its own name, dashes and all, then the git command, as a command is.
    #[test]
    fn git_config_sets_views_by_name() {
        let config = configured("", "[delta]\n\tview = diff-since git diff {commit}...\n");
        assert_eq!(
            config.general.views.into_iter().collect::<Vec<_>>(),
            [("diff-since".to_string(), "git diff {commit}...".to_string())]
        );
    }

    #[test]
    fn git_config_sets_bindings() {
        let config = configured(
            "",
            "[delta]\n\
             \tbind = root.quit x\n\
             \tbind = root.quit esc\n",
        );
        assert!(config.bindings.binds_at_top_level('x'));
        assert!(!config.bindings.binds_at_top_level('q'));
    }

    /// A menu argument's binding keeps its dashes, as its name is its command line's.
    #[test]
    fn git_config_binds_a_menu_argument() {
        let config = configured("", "[delta]\n\tbind = commit-menu.--all -x\n");
        let menu = crate::menu::PendingMenu::init(crate::menu::Menu::Commit);
        let raw: Vec<&str> = config
            .bindings
            .arg_list(&menu)
            .filter(|binding| binding.op == crate::ops::Op::ToggleArg("--all".into()))
            .map(|binding| binding.raw.as_str())
            .collect();
        assert_eq!(raw, ["-x"]);
    }

    /// A style is written as delta writes one: attributes, then a foreground
    /// and a background color. A color it leaves out is unset.
    #[test]
    fn git_config_styles_are_delta_styles() {
        let config = configured(
            "",
            "[delta]\n\
             \tfile-header-style = bold italic \"#ff0000\" 237\n\
             \tsection-header-style = normal blue\n\
             \thash-style = ul\n",
        );
        let style = &config.style;
        assert_eq!(style.file_header.fg, Some(Color::Rgb(255, 0, 0)));
        assert_eq!(style.file_header.bg, Some(Color::Indexed(237)));
        assert_eq!(
            style.file_header.mods,
            Some(Modifier::BOLD | Modifier::ITALIC)
        );
        assert_eq!(style.section_header.fg, None);
        assert_eq!(style.section_header.bg, Some(Color::Blue));
        assert_eq!(style.hash.fg, None);
        assert_eq!(style.hash.mods, Some(Modifier::UNDERLINED));
    }

    /// delta's `hunk-header-style` is delta's, which gitu's hunk header style is not.
    #[test]
    fn deltas_hunk_header_style_is_not_gitus() {
        let config = configured(
            "",
            "[delta]\n\thunk-header-style = file line-number syntax\n",
        );
        assert_eq!(config.style.hunk_header.fg, Some(Color::Blue));
    }

    #[test]
    fn a_git_config_style_keeps_its_symbol() {
        let config = configured("", "[delta]\n\tcursor-style = red\n");
        assert_eq!(config.style.cursor.symbol, '▌');
        assert_eq!(config.style.cursor.fg, Some(Color::Red));
    }

    #[test]
    fn a_git_config_style_inside_a_group_of_styles() {
        let config = configured("", "[delta]\n\tmenu-key-style = green\n");
        assert_eq!(config.style.menu.key.fg, Some(Color::Green));
    }

    #[test]
    fn a_bad_git_config_value_is_an_error_naming_the_key() {
        assert!(config_error("[delta]\n\tstart-folded = maybe\n").contains("start-folded"));
        assert!(
            config_error("[delta]\n\tfile-header-style = bold sky\n").contains("file-header-style")
        );
    }

    /// The settings are delta's options, so a delta feature in force sets them.
    #[test]
    fn a_feature_in_force_sets_a_setting() {
        let gitconfig = "[delta]\n\tfeatures = tui\n[delta \"tui\"]\n\tstart-folded = true\n";
        assert!(configured("", gitconfig).general.start_folded);
        let off = "[delta \"tui\"]\n\tstart-folded = true\n";
        assert!(!configured("", off).general.start_folded);
        assert!(
            configured_with("", off, Some("+tui"))
                .unwrap()
                .general
                .start_folded
        );
    }

    /// As for delta's own options, `[delta]` beats a feature.
    #[test]
    fn the_delta_section_beats_a_feature() {
        let config = configured(
            "",
            "[delta]\n\tfeatures = tui\n\tstash-list-limit = 3\n\
             [delta \"tui\"]\n\tstash-list-limit = 5\n",
        );
        assert_eq!(config.general.stash_list_limit, 3);
    }

    /// A list takes the items of `[delta]` and of each feature in force. Of two commands of the
    /// same name, `[delta]`'s wins.
    #[test]
    fn a_list_takes_the_items_of_every_section_in_force() {
        let config = configured(
            "",
            "[delta]\n\tfeatures = tui\n\
             \tcommand = a delta-a\n\tcommand = both delta-both\n\
             \tbind = root.quit x\n\
             [delta \"tui\"]\n\
             \tcommand = b tui-b\n\tcommand = both tui-both\n\
             \tbind = root.quit y\n",
        );
        assert_eq!(
            config.general.commands.into_iter().collect::<Vec<_>>(),
            [
                ("a".to_string(), "delta-a".to_string()),
                ("b".to_string(), "tui-b".to_string()),
                ("both".to_string(), "delta-both".to_string()),
            ]
        );
        assert!(config.bindings.binds_at_top_level('x'));
        assert!(config.bindings.binds_at_top_level('y'));
    }

    /// Settings in the old `[delta-tui]` sections are not silently ignored.
    #[test]
    fn a_delta_tui_section_is_an_error() {
        let error = config_error("[delta-tui]\n\tstart-folded = true\n");
        assert!(error.contains("[delta-tui]"), "{error}");
        assert!(error.contains("[delta]"), "{error}");
        assert!(
            error.contains("delta --tui --migrate-old-config"),
            "{error}"
        );
    }
}
