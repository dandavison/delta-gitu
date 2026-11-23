//! Changes gitu makes to the user's git config, through `git config` so that
//! everything else in it stays as they wrote it and a linked file is written
//! through the link.
//!
//! gitu's settings are in `[delta]` and in `[delta "feature"]` sections (see
//! [`crate::config_git`]). A setting gitu writes goes in the section and file
//! of its last entry, the one git reads last, or else in the file holding the
//! `[delta]` section, or else in the global git config.

use crate::{Res, config_git, error::Error};
use std::{collections::BTreeMap, path::Path, process::Command};

/// Make the feature keys in git config go from `before` to `after`: a feature whose key changed
/// loses its `feature-key` entries in every section, and gets one with its new key, if it has
/// one. An entry for a feature whose key did not change stays as written, so a feature key
/// kept in a feature not in force is left alone. With no feature keys in git config, all of
/// `after` is written, since the keys in git config replace gitu's default ones.
pub(crate) fn set_feature_keys(
    dir: &Path,
    before: &BTreeMap<String, char>,
    after: &BTreeMap<String, char>,
) -> Res<()> {
    set_feature_keys_with(&git_in(dir), before, after)
}

/// Make the theme in git config `theme`, or unset it, where its last entry is (see the module
/// documentation). Unsetting it unsets that entry only: a theme also set in a file git reads
/// earlier, or in another section, then has its way.
pub(crate) fn set_theme(dir: &Path, theme: Option<&str>) -> Res<()> {
    set_theme_with(&git_in(dir), theme)
}

fn git_in(dir: &Path) -> impl Fn() -> Command {
    move || {
        let mut git = Command::new("git");
        git.current_dir(dir);
        git
    }
}

/// An entry in git config: the file holding it, its full name and its value.
struct Entry {
    file: String,
    name: String,
    value: String,
}

/// Every entry of the setting `setting`, in `[delta]` or a feature's section, in the order git
/// reads them.
fn entries(git: &dyn Fn() -> Command, setting: &str) -> Res<Vec<Entry>> {
    let regex = format!("^delta\\.(.+\\.)?{}$", setting.replace('.', "\\."));
    let output = run(git().args(["config", "--show-origin", "--null", "--get-regexp", &regex]))?;
    // Each entry is its origin, then its name and value.
    let fields: Vec<&str> = output.split('\0').collect();
    Ok(fields
        .chunks(2)
        .filter_map(|entry| {
            let file = entry[0].strip_prefix("file:")?;
            let (name, value) = entry
                .get(1)?
                .split_once('\n')
                .unwrap_or((entry.get(1)?, ""));
            Some(Entry {
                file: file.to_owned(),
                name: name.to_owned(),
                value: value.to_owned(),
            })
        })
        .collect())
}

/// Where to write `setting`: the `git config` arguments picking the file, and the full name.
fn target(
    git: &dyn Fn() -> Command,
    existing: &[Entry],
    setting: &str,
) -> Res<(Vec<String>, String)> {
    if let Some(last) = existing.last() {
        return Ok((
            vec!["--file".to_owned(), last.file.clone()],
            last.name.clone(),
        ));
    }
    let name = format!("delta.{setting}");
    Ok(match origin(git, "^delta\\.[^.]+$")? {
        Some(file) => (vec!["--file".to_owned(), file], name),
        None => (vec!["--global".to_owned()], name),
    })
}

fn set_theme_with(git: &dyn Fn() -> Command, theme: Option<&str>) -> Res<()> {
    let existing = entries(git, config_git::THEME)?;
    let (file, name) = target(git, &existing, config_git::THEME)?;
    match theme {
        Some(theme) => run(git().arg("config").args(&file).args([&name, theme]))?,
        None => run(git().arg("config").args(&file).args(["--unset-all", &name]))?,
    };
    Ok(())
}

fn set_feature_keys_with(
    git: &dyn Fn() -> Command,
    before: &BTreeMap<String, char>,
    after: &BTreeMap<String, char>,
) -> Res<()> {
    let changed: Vec<&String> = before
        .keys()
        .chain(after.keys())
        .filter(|feature| before.get(*feature) != after.get(*feature))
        .collect();
    let existing = entries(git, config_git::FEATURE_KEY)?;
    let (file, name) = target(git, &existing, config_git::FEATURE_KEY)?;
    // With none in git config, the default keys are in force, and the keys written are all of
    // them, since they replace the defaults.
    let changed: Vec<&String> = if existing.is_empty() {
        after.keys().collect()
    } else {
        changed
    };
    let feature_of = |entry: &Entry| entry.value.split_whitespace().next().map(str::to_owned);
    for entry in &existing {
        if feature_of(entry).is_some_and(|feature| changed.contains(&&feature)) {
            run(git()
                .args([
                    "config",
                    "--file",
                    &entry.file,
                    "--fixed-value",
                    "--unset-all",
                ])
                .args([&entry.name, &entry.value]))?;
        }
    }
    let mut added = vec![];
    for feature in changed {
        if let Some(key) = after.get(feature)
            && !added.contains(&feature)
        {
            added.push(feature);
            run(git().args(["config"]).args(&file).args([
                "--add",
                &name,
                &format!("{feature} {key}"),
            ]))?;
        }
    }
    Ok(())
}

/// The file of the last entry whose name matches `regex`: the one git reads
/// last, so the one that has its way.
fn origin(git: &dyn Fn() -> Command, regex: &str) -> Res<Option<String>> {
    let output = run(git().args(["config", "--show-origin", "--null", "--get-regexp", regex]))?;
    // Each entry is its origin, then its name and value.
    let fields: Vec<&str> = output.split('\0').collect();
    Ok(fields
        .chunks(2)
        .filter_map(|entry| entry[0].strip_prefix("file:"))
        .next_back()
        .map(str::to_owned))
}

/// What `git` writes, where finding nothing (status 1) is no error.
fn run(git: &mut Command) -> Res<String> {
    let output = git.output().map_err(|e| Error::EditConfig(e.to_string()))?;
    if !output.status.success() && output.status.code() != Some(1) {
        return Err(Error::EditConfig(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }
    String::from_utf8(output.stdout).map_err(|e| Error::EditConfig(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::configured_with;
    use std::{fs, path::PathBuf};
    use temp_dir::TempDir;

    fn keys(keys: &[(&str, char)]) -> BTreeMap<String, char> {
        keys.iter()
            .map(|(feature, key)| ((*feature).to_owned(), *key))
            .collect()
    }

    /// A repository whose global git config is `global.gitconfig` beside it,
    /// holding `global`.
    struct Repo {
        dir: TempDir,
    }

    impl Repo {
        fn new(global: &str) -> Self {
            let dir = TempDir::new().unwrap();
            fs::create_dir(dir.path().join("repo")).unwrap();
            fs::write(dir.path().join("global.gitconfig"), global).unwrap();
            let repo = Self { dir };
            assert!(repo.git().arg("init").output().unwrap().status.success());
            repo
        }

        fn path(&self, name: &str) -> PathBuf {
            self.dir.path().join(name)
        }

        fn git(&self) -> Command {
            let mut git = Command::new("git");
            git.current_dir(self.path("repo"))
                .env("GIT_CONFIG_GLOBAL", self.path("global.gitconfig"))
                .env("GIT_CONFIG_NOSYSTEM", "1");
            git
        }

        fn set_feature_keys(&self, before: &[(&str, char)], after: &[(&str, char)]) {
            set_feature_keys_with(&|| self.git(), &keys(before), &keys(after)).unwrap();
        }

        /// The feature keys gitu would read from git config file `name`, with `features` in
        /// force as `DELTA_FEATURES` gives them.
        fn feature_keys_with(&self, name: &str, features: Option<&str>) -> Vec<(String, char)> {
            configured_with("", &self.read(name), features)
                .unwrap()
                .general
                .diff_renderer
                .feature_keys
                .into_iter()
                .collect()
        }

        fn feature_keys(&self, name: &str) -> Vec<(String, char)> {
            self.feature_keys_with(name, None)
        }

        fn read(&self, name: &str) -> String {
            fs::read_to_string(self.path(name)).unwrap()
        }
    }

    /// With no `[delta]` section anywhere, the keys go in the global config, as
    /// `git config --global` would put them.
    #[test]
    fn keys_go_in_the_global_git_config_by_default() {
        let repo = Repo::new("");

        repo.set_feature_keys(&[], &[("side-by-side", 'e')]);

        assert_eq!(
            repo.feature_keys("global.gitconfig"),
            [("side-by-side".into(), 'e')]
        );
    }

    /// Where the user keeps their delta config, often a file of its own included
    /// from their git config.
    #[test]
    fn keys_go_in_the_file_holding_the_delta_section() {
        let repo = Repo::new("[include]\n\tpath = delta.gitconfig\n");
        fs::write(
            repo.path("delta.gitconfig"),
            "[delta]\n\tstart-folded = true\n",
        )
        .unwrap();

        repo.set_feature_keys(&[], &[("side-by-side", 'e')]);

        assert_eq!(
            repo.read("global.gitconfig"),
            "[include]\n\tpath = delta.gitconfig\n"
        );
        assert_eq!(
            repo.feature_keys("delta.gitconfig"),
            [("side-by-side".into(), 'e')]
        );
    }

    /// The feature keys a user keeps in a feature of their own stay there.
    #[test]
    fn keys_go_in_the_section_holding_feature_keys() {
        let repo = Repo::new(
            "[delta]\n\tfeatures = tui\n\
             [delta \"tui\"]\n\tfeature-key = line-numbers w\n",
        );

        repo.set_feature_keys(
            &[("line-numbers", 'w')],
            &[("line-numbers", 'w'), ("side-by-side", 'e')],
        );

        let written = repo.read("global.gitconfig");
        assert!(
            written.ends_with("\tfeature-key = line-numbers w\n\tfeature-key = side-by-side e\n"),
            "{written}"
        );
    }

    /// The file is the user's: what they wrote, and how, stays.
    #[test]
    fn everything_else_in_the_file_is_kept_as_written() {
        let mine = "# My settings\n\
                    [delta]\n\
                    \t# Offered by -.\n\
                    \tvisit-context-lines = true  # mine\n\
                    \tfeature-key = line-numbers w  # mine\n";
        let repo = Repo::new(mine);

        repo.set_feature_keys(
            &[("line-numbers", 'w')],
            &[("line-numbers", 'w'), ("side-by-side", 'e')],
        );

        let written = repo.read("global.gitconfig");
        assert!(written.starts_with(mine), "{written}");
        assert_eq!(
            repo.feature_keys("global.gitconfig"),
            [("line-numbers".into(), 'w'), ("side-by-side".into(), 'e')]
        );
    }

    #[test]
    fn a_feature_left_out_loses_its_key() {
        let repo = Repo::new("");

        repo.set_feature_keys(&[], &[("side-by-side", 'e'), ("line-numbers", 'w')]);
        repo.set_feature_keys(
            &[("side-by-side", 'e'), ("line-numbers", 'w')],
            &[("line-numbers", 'w')],
        );

        assert_eq!(
            repo.feature_keys("global.gitconfig"),
            [("line-numbers".into(), 'w')]
        );
    }

    #[test]
    fn a_key_moves_to_the_feature_it_is_set_for() {
        let repo = Repo::new("");

        repo.set_feature_keys(&[], &[("side-by-side", 'e'), ("line-numbers", 'w')]);
        repo.set_feature_keys(
            &[("side-by-side", 'e'), ("line-numbers", 'w')],
            &[("line-numbers", 'e')],
        );

        assert_eq!(
            repo.feature_keys("global.gitconfig"),
            [("line-numbers".into(), 'e')]
        );
    }

    /// A feature key set in a feature that is not in force is not one gitu was showing, so a
    /// change to the others leaves it alone.
    #[test]
    fn a_key_in_a_feature_not_in_force_is_kept() {
        let repo = Repo::new(
            "[delta]\n\tfeature-key = line-numbers w\n\
             [delta \"other\"]\n\tfeature-key = side-by-side W\n",
        );

        repo.set_feature_keys(&[("line-numbers", 'w')], &[("line-numbers", 'Q')]);

        assert_eq!(
            repo.feature_keys_with("global.gitconfig", Some("+other")),
            [("line-numbers".into(), 'Q'), ("side-by-side".into(), 'W')]
        );
    }

    /// A config kept in a dotfiles repo is often a link into it.
    #[cfg(unix)]
    #[test]
    fn a_linked_config_file_is_written_through_the_link() {
        let repo = Repo::new("[include]\n\tpath = delta.gitconfig\n");
        fs::write(
            repo.path("dotfiles.gitconfig"),
            "[delta]\n\tstart-folded = true\n",
        )
        .unwrap();
        std::os::unix::fs::symlink(
            repo.path("dotfiles.gitconfig"),
            repo.path("delta.gitconfig"),
        )
        .unwrap();

        repo.set_feature_keys(&[], &[("side-by-side", 'e')]);

        assert!(
            fs::symlink_metadata(repo.path("delta.gitconfig"))
                .unwrap()
                .is_symlink()
        );
        assert_eq!(
            repo.feature_keys("dotfiles.gitconfig"),
            [("side-by-side".into(), 'e')]
        );
    }
}
