//! An editor that fails says why, and which environment variable sets it.
//! Every test sets GITU_SHOW_EDITOR, so these run alone.

use super::*;
use crate::tests::helpers::{assert_passes_alone, running_alone};
use std::os::unix::fs::PermissionsExt;

#[test]
fn a_failing_editor_shows_its_stderr_and_the_variable_that_sets_it() {
    assert_passes_alone("tests::editor_failure::open_an_editor_that_fails");
}

#[test]
#[ignore = "run alone by a_failing_editor_shows_its_stderr_and_the_variable_that_sets_it"]
fn open_an_editor_that_fails() {
    if !running_alone() {
        return;
    }
    let rows = error_opening_the_file_with(
        "failing-editor",
        Some("echo 'cannot open the file' >&2; exit 1"),
    );
    assert_eq!(
        rows,
        [
            "! The editor set by GITU_SHOW_EDITOR failed:",
            "  failing-editor file:1",
            "  exit code: 1",
            "  stderr:",
            "  cannot open the file",
        ]
    );
}

#[test]
fn an_editor_that_cannot_be_started_shows_the_variable_that_sets_it() {
    assert_passes_alone("tests::editor_failure::open_an_editor_that_does_not_exist");
}

#[test]
#[ignore = "run alone by an_editor_that_cannot_be_started_shows_the_variable_that_sets_it"]
fn open_an_editor_that_does_not_exist() {
    if !running_alone() {
        return;
    }
    let rows = error_opening_the_file_with("no-such-editor", None);
    assert_eq!(
        rows,
        [
            "! Couldn't start the editor set by GITU_SHOW_EDITOR: No such file or directory (os error 2)"
        ]
    );
}

/// The error rows after `enter` on a file in `git diff`, with GITU_SHOW_EDITOR set to
/// `editor`, a program on PATH running `script` if there is one.
fn error_opening_the_file_with(editor: &str, script: Option<&str>) -> Vec<String> {
    let bin = temp_dir::TempDir::new().unwrap();
    if let Some(script) = script {
        let path = bin.path().join(editor);
        fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let mut ctx = TestContext::setup_clone_wide(&format!("editor_failure_{editor}"));
    ctx.debug = false;
    commit(&ctx.dir, "file", "a\n");
    fs::write(ctx.dir.join("file"), "b\n").unwrap();
    let path = format!(
        "{}:{}",
        bin.path().display(),
        std::env::var("PATH").unwrap()
    );
    unsafe {
        std::env::set_var("PATH", path);
        std::env::set_var("GITU_SHOW_EDITOR", editor);
    }

    let mut app = ctx.init_app_as_pager_of(&["git", "diff"]);
    ctx.update(&mut app, keys("<enter>"));

    let buffer = ctx.redact_buffer();
    buffer
        .lines()
        .skip_while(|row| !row.starts_with('!'))
        .map(|row| row.trim_end_matches('|').trim_end().to_owned())
        .take_while(|row| row.starts_with('!') || row.starts_with("  "))
        .collect()
}
