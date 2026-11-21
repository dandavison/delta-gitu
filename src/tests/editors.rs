//! `E` switches the editor enter opens a file in, between the one the
//! environment sets and those in `general.editors`. Every test sets the
//! editor environment variables, so these run alone.

use super::*;
use crate::tests::helpers::{assert_passes_alone, running_alone};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

#[test]
fn e_switches_to_the_next_editor_and_back() {
    assert_passes_alone("tests::editors::switch_editors");
}

#[test]
#[ignore = "run alone by e_switches_to_the_next_editor_and_back"]
fn switch_editors() {
    if !running_alone() {
        return;
    }
    let editors = Editors::new();
    let mut ctx = TestContext::setup_clone("editors_switch");
    unsafe { std::env::set_var("GITU_SHOW_EDITOR", editors.command("env-editor")) };
    ctx.config().general.editors = vec![editors.command("config-editor")];

    let mut app = editors.open_diff(&mut ctx);
    ctx.update(&mut app, keys("<enter>E<enter>E<enter>"));

    assert_eq!(
        editors.opened(),
        [
            "env-editor file:1",
            "config-editor file:1",
            "env-editor file:1"
        ]
    );
}

#[test]
fn without_an_editor_in_the_environment_enter_opens_the_first_configured_one() {
    assert_passes_alone("tests::editors::open_the_first_configured_editor");
}

#[test]
#[ignore = "run alone by without_an_editor_in_the_environment_enter_opens_the_first_configured_one"]
fn open_the_first_configured_editor() {
    if !running_alone() {
        return;
    }
    let editors = Editors::new();
    let mut ctx = TestContext::setup_clone("editors_first_configured");
    for var in crate::ops::show::EDITOR_VARS {
        unsafe { std::env::remove_var(var) };
    }
    ctx.config().general.editors = vec![
        editors.command("first-editor"),
        editors.command("second-editor"),
    ];

    let mut app = editors.open_diff(&mut ctx);
    ctx.update(&mut app, keys("<enter>"));

    assert_eq!(editors.opened(), ["first-editor file:1"]);
}

/// Editors that write their name and arguments to a log instead of editing.
struct Editors {
    dir: temp_dir::TempDir,
}

impl Editors {
    fn new() -> Self {
        Self {
            dir: temp_dir::TempDir::new().unwrap(),
        }
    }

    /// The path of an editor called `name`, created on first use.
    fn command(&self, name: &str) -> String {
        let path = self.dir.path().join(name);
        let log = self.log();
        fs::write(
            &path,
            format!("#!/bin/sh\necho \"{name} $*\" >> '{}'\n", log.display()),
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path.to_str().unwrap().to_owned()
    }

    fn log(&self) -> std::path::PathBuf {
        self.dir.path().join("opened")
    }

    /// Each editor run, in order, as its name and arguments.
    fn opened(&self) -> Vec<String> {
        fs::read_to_string(self.log())
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// `git diff` of a changed file, with the cursor on it.
    fn open_diff(&self, ctx: &mut TestContext) -> crate::app::App {
        ctx.debug = false;
        commit(&ctx.dir, "file", "a\n");
        fs::write(Path::new(&ctx.dir).join("file"), "b\n").unwrap();
        ctx.init_app_as_pager_of(&["git", "diff"])
    }
}
