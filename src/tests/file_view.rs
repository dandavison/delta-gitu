//! The file view: a file whole, as committed, opened with `.`, whose lines take comments. They are
//! kept in the branch's files review, apart from its PR, for an agent to answer.

use super::local_pr::{answer_as_printed, init_app, review_cli, setup};
use super::*;

/// `setup`, with `notes.md` committed on `feature`.
fn setup_notes(ctx: &mut TestContext) {
    setup(ctx);
    commit(&ctx.dir, "notes.md", "# Notes\nhello world\nthe end\n");
}

/// The row the cursor is on.
fn cursor_row(ctx: &TestContext) -> String {
    let screen = ctx.redact_buffer();
    screen
        .lines()
        .find(|line| line.starts_with('▌'))
        .unwrap_or_else(|| panic!("no cursor:\n{screen}"))
        .to_owned()
}

/// Where `feature`'s files review is recorded.
fn files_review(ctx: &TestContext) -> serde_json::Value {
    let path = ctx.dir.join(".git/delta/review/feature...feature.json");
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

/// `.` on a line of a diff opens its file whole, as committed, at that line.
#[test]
fn dot_on_a_diff_line_opens_the_file_whole_at_the_line() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    let mut app = init_app(&mut ctx);
    ctx.update(&mut app, keys("OO/new<enter>"));

    ctx.update(&mut app, keys("."));

    let screen = ctx.redact_buffer();
    assert!(cursor_row(&ctx).contains("new"), "{screen}");
    assert!(!screen.contains("added.txt"), "{screen}");
}

/// `.` anywhere else opens a picker of the files git tracks, and the one picked opens.
#[test]
fn dot_elsewhere_picks_a_file_to_open() {
    let mut ctx = setup_clone!();
    setup_notes(&mut ctx);
    let mut app = init_app(&mut ctx);

    ctx.update(&mut app, keys("."));
    let screen = ctx.redact_buffer();
    assert!(screen.contains("Open file"), "{screen}");
    assert!(screen.contains("notes.md"), "{screen}");
    assert!(screen.contains("lines.txt"), "{screen}");

    ctx.update(&mut app, keys(".notes<enter>"));
    let screen = ctx.redact_buffer();
    assert!(screen.contains("hello world"), "{screen}");
}

/// In the file view, `.` picks another file.
#[test]
fn dot_in_the_file_view_picks_a_file() {
    let mut ctx = setup_clone!();
    setup_notes(&mut ctx);
    let mut app = init_app(&mut ctx);
    ctx.update(&mut app, keys(".notes<enter>/hello<enter>"));

    ctx.update(&mut app, keys("."));

    let screen = ctx.redact_buffer();
    assert!(screen.contains("Open file"), "{screen}");
}

/// The file view shows the file as committed, and says when it has changes not committed.
#[test]
fn the_file_view_says_when_the_file_has_changes_not_committed() {
    let mut ctx = setup_clone!();
    setup_notes(&mut ctx);
    fs::write(ctx.dir.join("notes.md"), "# Notes\nchanged\n").unwrap();
    let mut app = init_app(&mut ctx);

    ctx.update(&mut app, keys(".notes<enter>"));

    let screen = super::pr::screen_text(&ctx);
    assert!(screen.contains("hello world"), "{screen}");
    assert!(
        screen.contains("notes.md has changes not committed, which aren't shown"),
        "{screen}"
    );
}

/// A comment on a line of the file view is local-only, in the branch's files review: there is no
/// PR to post it to.
#[test]
fn a_comment_in_the_file_view_is_local_only_in_the_files_review() {
    let mut ctx = setup_clone!();
    setup_notes(&mut ctx);
    let mut app = init_app(&mut ctx);
    ctx.update(&mut app, keys(".notes<enter>/hello<enter>"));

    ctx.update(&mut app, keys("imake it louder<ctrl+s>"));

    let screen = ctx.redact_buffer();
    assert!(screen.contains("make it louder"), "{screen}");
    let file = files_review(&ctx);
    let thread = &file["threads"][0];
    assert_eq!(thread["anchor"]["path"], "notes.md", "{file}");
    assert_eq!(thread["anchor"]["line"], 2, "{file}");
    assert!(thread["comments"][0]["review"].is_null(), "{file}");

    let shown = review_cli(&ctx.dir, &["show", "--base", "feature"]);
    assert!(shown.contains("## notes.md:2"), "{shown}");
    assert!(shown.contains("make it louder"), "{shown}");
}

/// A comment in the files review can't move to a draft review: there is no PR to post it to.
#[test]
fn a_comment_in_the_files_review_stays_local_only() {
    let mut ctx = setup_clone!();
    setup_notes(&mut ctx);
    let mut app = init_app(&mut ctx);
    ctx.update(
        &mut app,
        keys(".notes<enter>/hello<enter>imake it louder<ctrl+s>/louder<enter>"),
    );

    ctx.update(&mut app, keys("p"));

    let screen = super::pr::screen_text(&ctx);
    assert!(
        screen.contains("A comment on a file is local-only"),
        "{screen}"
    );
    assert!(files_review(&ctx)["threads"][0]["comments"][0]["review"].is_null());
}

/// `@` in the file view sends a message that `listen` prints, saying how to answer it: with
/// `--base` the branch, which is the files review's.
#[test]
fn at_in_the_file_view_sends_a_message_listen_prints_with_how_to_answer() {
    let mut ctx = setup_clone!();
    setup_notes(&mut ctx);
    let mut app = init_app(&mut ctx);
    ctx.update(
        &mut app,
        keys(".notes<enter>/hello<enter>imake it louder<ctrl+s>/louder<enter>"),
    );
    let thread = files_review(&ctx)["threads"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();

    ctx.update(&mut app, keys("@<enter>"));

    let printed = review_cli(&ctx.dir, &["listen"]);
    assert!(printed.contains("the files of feature"), "{printed}");
    assert!(printed.contains("--base feature"), "{printed}");
    assert!(printed.contains(&thread), "{printed}");
    assert!(printed.contains("## notes.md:2"), "{printed}");
}

/// The command a message from the file view says to answer with replies in the files review.
#[test]
fn a_message_from_the_file_view_says_how_to_answer_it() {
    let mut ctx = setup_clone!();
    setup_notes(&mut ctx);
    let mut app = init_app(&mut ctx);
    ctx.update(
        &mut app,
        keys(".notes<enter>/hello<enter>imake it louder<ctrl+s>/louder<enter>@<enter>"),
    );
    let printed = review_cli(&ctx.dir, &["listen"]);

    answer_as_printed(&ctx.dir, &printed, "", "🤖 louder now").unwrap();

    let file = files_review(&ctx);
    let reply = &file["threads"][0]["comments"][1];
    assert_eq!(reply["body"], "🤖 louder now", "{file}");
    assert_eq!(reply["agent"], "bot", "{file}");
}

/// The files review is not a PR: the branch's PR opens as before.
#[test]
fn the_files_review_is_not_a_pr() {
    let mut ctx = setup_clone!();
    setup_notes(&mut ctx);
    let mut app = init_app(&mut ctx);
    ctx.update(
        &mut app,
        keys(".notes<enter>/hello<enter>imake it louder<ctrl+s>"),
    );

    let mut app = init_app(&mut ctx);
    ctx.update(&mut app, keys("OO"));

    let screen = ctx.redact_buffer();
    assert!(screen.contains("added: added.txt"), "{screen}");
}

/// On the base branch, which has no PR, the file view takes comments too.
#[test]
fn the_file_view_takes_comments_on_the_base_branch() {
    let mut ctx = setup_clone!();
    setup_notes(&mut ctx);
    run(&ctx.dir, &["git", "checkout", "main"]);
    let mut app = init_app(&mut ctx);

    ctx.update(&mut app, keys(".lines<enter>/old<enter>ifix this<ctrl+s>"));

    let shown = review_cli(&ctx.dir, &["show", "--base", "main"]);
    assert!(shown.contains("## lines.txt:2"), "{shown}");
    assert!(shown.contains("fix this"), "{shown}");
}

/// The files review has nothing to do with GitHub: an agent answers a comment in it whatever
/// GitHub has of the branch, as when GitHub's `main` is at a commit this repository hasn't fetched.
#[test]
fn an_agent_answers_in_the_files_review_whatever_github_has() {
    let mut ctx = setup_clone!();
    setup_notes(&mut ctx);
    run(&ctx.dir, &["git", "checkout", "main"]);
    let mut app = init_app(&mut ctx);
    ctx.update(&mut app, keys(".lines<enter>/old<enter>ifix this<ctrl+s>"));
    let file = ctx.dir.join(".git/delta/review/main...main.json");
    let file: serde_json::Value = serde_json::from_str(&fs::read_to_string(file).unwrap()).unwrap();
    let thread = file["threads"][0]["id"].as_str().unwrap().to_owned();

    super::local_pr::review_cli_with_input(
        &ctx.dir,
        &["reply", &thread, "--base", "main", "--agent", "claude"],
        "Done.\n",
    )
    .unwrap();

    let shown = review_cli(&ctx.dir, &["show", "--base", "main"]);
    assert!(shown.contains("Done."), "{shown}");
}
