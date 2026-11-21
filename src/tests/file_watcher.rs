use std::{
    fs,
    path::Path,
    thread,
    time::{Duration, Instant},
};

use stdext::function_name;

use crate::{file_watcher::FileWatcher, git};

use crate::{
    repo_setup_clone,
    tests::helpers::{RepoTestContext, run},
};

/// Starts a watcher on `workdir`, discards events from setup, runs `cmd` in
/// `workdir` and reports whether the watcher saw a change.
fn watcher_sees(workdir: &Path, cmd: &[&str]) -> bool {
    watcher_sees_effect_of(workdir, || {
        run(workdir, cmd);
    })
}

fn watcher_sees_effect_of(workdir: &Path, action: impl FnOnce()) -> bool {
    let watcher = FileWatcher::new(&git2::Repository::open(workdir).unwrap()).unwrap();
    thread::sleep(Duration::from_millis(500));
    watcher.pending_updates();

    action();

    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if watcher.pending_updates() {
            return true;
        }
        thread::sleep(Duration::from_millis(50));
    }
    false
}

#[test]
fn commit_in_main_checkout_triggers_refresh() {
    let ctx = repo_setup_clone!();
    fs::write(ctx.dir.join("initial-file"), "changed").unwrap();

    assert!(watcher_sees(&ctx.dir, &["git", "commit", "-am", "change"]));
}

/// Adds a linked worktree on a new branch next to `dir`. The path is relative
/// because on Windows `dir` has the `\\?\` prefix from `fs::canonicalize`,
/// which git cannot create directories under.
fn add_worktree(dir: &Path) {
    run(dir, &["git", "worktree", "add", "../worktree", "-b", "wt"]);
}

#[test]
fn commit_in_linked_worktree_triggers_refresh() {
    let ctx = repo_setup_clone!();
    let worktree = ctx.dir.parent().unwrap().join("worktree");
    add_worktree(&ctx.dir);
    fs::write(worktree.join("initial-file"), "changed").unwrap();

    assert!(watcher_sees(&worktree, &["git", "commit", "-am", "change"]));
}

#[test]
fn checkout_in_linked_worktree_triggers_refresh() {
    let ctx = repo_setup_clone!();
    let worktree = ctx.dir.parent().unwrap().join("worktree");
    add_worktree(&ctx.dir);
    run(&worktree, &["git", "branch", "other"]);

    assert!(watcher_sees(&worktree, &["git", "switch", "other"]));
}

#[test]
fn reading_status_does_not_trigger_refresh() {
    let ctx = repo_setup_clone!();
    // A tracked file whose mtime no longer matches the index makes `git status`
    // and `git diff` rewrite the index unless told not to.
    thread::sleep(Duration::from_millis(1100));
    run(&ctx.dir, &["touch", "initial-file"]);

    assert!(!watcher_sees_effect_of(&ctx.dir, || {
        git::status(&ctx.dir).unwrap();
        git::diff_unstaged(&ctx.local_repo, None).unwrap();
        git::diff_staged(&ctx.local_repo, None).unwrap();
    }));
}
