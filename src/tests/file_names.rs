//! Files whose names git quotes, or that look like syntax in git's output or
//! on a git command line. Each test runs every name in a repo of its own and
//! reports every name it fails for.

use super::*;
use std::panic::{AssertUnwindSafe, catch_unwind};

const NAMES: &[&str] = &[
    "plain",
    "with space",
    " leading-space",
    "trailing-space ",
    "a -> b",
    "...",
    "[...lang]",
    "[...lang]/index.astro",
    "x b/y",
    "*",
    "?",
    "[ab]",
    ":(top)magic",
    "-n",
    "\"quoted\"",
    "back\\slash",
    "tab\there",
    "new\nline",
    "carriage\rreturn",
    "ünïcödé",
    "#hash",
    "@@ -1 +1 @@",
    "diff --git a b",
    "+++ b",
];

/// Files beside the one under test, which a name taken as a glob, as
/// pathspec magic, or as an option would also reach.
const DECOYS: &[&str] = &["a", "l", "magic", "z"];

#[test]
fn untracked_file_is_listed() {
    for_each_name(function_name!(), |ctx, name| {
        write(ctx, name, "content\n");
        assert_lists(ctx, name);
    });
}

#[test]
fn unstaged_file_is_listed() {
    for_each_name(function_name!(), |ctx, name| {
        write(ctx, name, "content\n");
        commit_all(ctx);
        write(ctx, name, "changed\n");
        assert_lists(ctx, name);
    });
}

#[test]
fn staged_file_is_listed() {
    for_each_name(function_name!(), |ctx, name| {
        write(ctx, name, "content\n");
        run(&ctx.dir, &["git", "add", "-A"]);
        assert_lists(ctx, name);
    });
}

#[test]
fn renamed_file_is_listed() {
    for_each_name(function_name!(), |ctx, name| {
        write(ctx, "old", "content\n");
        commit_all(ctx);
        fs::remove_file(ctx.dir.join("old")).unwrap();
        write(ctx, name, "content\n");
        run(&ctx.dir, &["git", "add", "-A"]);
        assert_lists(ctx, name);
    });
}

#[test]
fn commit_lists_the_file() {
    for_each_name(function_name!(), |ctx, name| {
        write(ctx, name, "content\n");
        commit_all(ctx);
        let mut app = ctx.init_app();
        ctx.update(&mut app, keys("ll<enter>"));
        assert_shows(ctx, name);
    });
}

#[test]
fn stage_untracked_file() {
    for_each_name(function_name!(), |ctx, name| {
        write_with_decoys(ctx, name);
        let mut app = ctx.init_app();
        ctx.update(&mut app, keys(&format!("{}s", select(name))));
        assert_status(ctx, name, "A ", "??");
    });
}

#[test]
fn stage_unstaged_file() {
    for_each_name(function_name!(), |ctx, name| {
        write_with_decoys(ctx, name);
        commit_all(ctx);
        change_with_decoys(ctx, name);
        let mut app = ctx.init_app();
        ctx.update(&mut app, keys(&format!("{}s", select(name))));
        assert_status(ctx, name, "M ", " M");
    });
}

#[test]
fn unstage_staged_file() {
    for_each_name(function_name!(), |ctx, name| {
        write_with_decoys(ctx, name);
        commit_all(ctx);
        change_with_decoys(ctx, name);
        run(&ctx.dir, &["git", "add", "-A"]);
        let mut app = ctx.init_app();
        ctx.update(&mut app, keys(&format!("{}u", select(name))));
        assert_status(ctx, name, " M", "M ");
    });
}

#[test]
fn discard_untracked_file() {
    for_each_name(function_name!(), |ctx, name| {
        write_with_decoys(ctx, name);
        let mut app = ctx.init_app();
        ctx.update(&mut app, keys(&format!("{}Ky", select(name))));
        assert_status(ctx, name, "", "??");
    });
}

#[test]
fn discard_unstaged_file() {
    for_each_name(function_name!(), |ctx, name| {
        write_with_decoys(ctx, name);
        commit_all(ctx);
        change_with_decoys(ctx, name);
        let mut app = ctx.init_app();
        ctx.update(&mut app, keys(&format!("{}Ky", select(name))));
        assert_status(ctx, name, "", " M");
    });
}

/// Run `test` for each name in a repo of its own, then fail with the names it
/// failed for.
fn for_each_name(test_name: &str, test: impl Fn(&mut TestContext, &str)) {
    let failures: Vec<String> = NAMES
        .iter()
        .enumerate()
        .filter_map(|(i, name)| {
            let mut ctx = TestContext::setup_clone(&format!("{test_name}-{i}"));
            // Lists untracked files one by one, rather than their directory.
            run(
                &ctx.dir,
                &["git", "config", "status.showUntrackedFiles", "all"],
            );
            catch_unwind(AssertUnwindSafe(|| test(&mut ctx, name)))
                .err()
                .map(|e| format!("{name:?}: {}", panic_message(&*e)))
        })
        .collect();
    assert!(
        failures.is_empty(),
        "failed for {} of {} names:\n{}",
        failures.len(),
        NAMES.len(),
        failures.join("\n")
    );
}

fn panic_message(e: &(dyn std::any::Any + Send)) -> String {
    e.downcast_ref::<String>()
        .cloned()
        .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_default()
}

fn write(ctx: &TestContext, name: &str, contents: &str) {
    let path = ctx.dir.join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn write_with_decoys(ctx: &TestContext, name: &str) {
    for file in files(name) {
        write(ctx, file, "content\n");
    }
}

fn change_with_decoys(ctx: &TestContext, name: &str) {
    for file in files(name) {
        write(ctx, file, "changed\n");
    }
}

fn commit_all(ctx: &TestContext) {
    run(&ctx.dir, &["git", "add", "-A"]);
    run(&ctx.dir, &["git", "commit", "-m", "commit"]);
}

/// The file under test and the decoys, in the order git lists them.
fn files(name: &str) -> Vec<&str> {
    let mut files: Vec<&str> = DECOYS.iter().copied().chain([name]).collect();
    files.sort();
    files
}

/// The keys that move the cursor from the branch to `name`, in a status whose
/// only section holds `name` and the decoys.
fn select(name: &str) -> String {
    let i = files(name).iter().position(|f| *f == name).unwrap();
    "j".repeat(2 + i)
}

/// `name` has status `code` and every decoy `decoy_code`, as
/// `git status --porcelain` gives them; an empty code means the file is gone.
fn assert_status(ctx: &TestContext, name: &str, code: &str, decoy_code: &str) {
    let mut expected: Vec<String> = DECOYS
        .iter()
        .map(|decoy| format!("{decoy_code} {decoy}"))
        .chain((!code.is_empty()).then(|| format!("{code} {name}")))
        .collect();
    expected.sort();
    assert_eq!(status(ctx), expected, "{}", ctx.redact_buffer());
}

fn status(ctx: &TestContext) -> Vec<String> {
    let mut entries: Vec<String> = run(&ctx.dir, &["git", "status", "--porcelain", "-z"])
        .split('\0')
        .filter(|entry| !entry.is_empty())
        .map(String::from)
        .collect();
    entries.sort();
    entries
}

fn assert_lists(ctx: &mut TestContext, name: &str) {
    ctx.init_app();
    assert_shows(ctx, name);
}

/// The screen shows `name`, unless it holds a character the terminal cannot
/// show as itself.
fn assert_shows(ctx: &TestContext, name: &str) {
    if name.chars().any(char::is_control) {
        return;
    }
    let buffer = ctx.redact_buffer();
    assert!(buffer.contains(name), "{buffer}");
}
