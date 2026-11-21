//! alt+enter opens a file, hunk or line in `general.alt_editor`, as enter
//! opens it in the editor the environment sets.

use super::*;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

/// `ctx` with `general.alt_editor` set to a program that writes its arguments
/// to `opened` in the repository, and a changed file under the cursor of `git
/// diff`.
#[cfg(unix)]
fn with_recording_alt_editor(mut ctx: TestContext) -> (TestContext, crate::app::App) {
    let bin = ctx.dir.join(".git").join("alt-editor");
    fs::write(&bin, "#!/bin/sh\necho \"$*\" >> opened\n").unwrap();
    fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
    ctx.config().general.alt_editor = Some(bin.to_str().unwrap().to_owned());
    ctx.debug = false;
    commit(&ctx.dir, "file", "a\nb\nc\n");
    fs::write(ctx.dir.join("file"), "a\nB\nc\n").unwrap();
    let app = ctx.init_app_as_pager_of(&["git", "diff"]);
    (ctx, app)
}

#[cfg(unix)]
#[test]
fn alt_enter_opens_the_hunk_in_the_alt_editor() {
    let (mut ctx, mut app) = with_recording_alt_editor(setup_clone!());

    ctx.update(&mut app, keys("<alt+enter>"));

    assert_eq!(
        fs::read_to_string(ctx.dir.join("opened")).unwrap_or_default(),
        "file:2\n",
        "{}",
        ctx.redact_buffer()
    );
}

#[test]
fn alt_enter_without_an_alt_editor_says_how_to_set_one() {
    let mut ctx = setup_clone!();
    ctx.debug = false;
    commit(&ctx.dir, "file", "a\n");
    fs::write(ctx.dir.join("file"), "b\n").unwrap();
    let mut app = ctx.init_app_as_pager_of(&["git", "diff"]);

    ctx.update(&mut app, keys("<alt+enter>"));

    assert!(
        ctx.redact_buffer().contains("general.alt_editor"),
        "{}",
        ctx.redact_buffer()
    );
}
