//! Reviewing a local PR: `O O` opens the current branch's diff against `main`, as a PR that
//! exists only in the repository. Its Viewed checkboxes and review threads are stored in the
//! repository's git directory, shared by all its worktrees.

use super::pr::FakeGitHub;
use super::*;
use crate::review::github::GitHub;
use std::rc::Rc;

/// A repository with `feature` checked out, which changes `lines.txt` and adds `added.txt`, and
/// `main`, which has moved on since with `main-only.txt`.
fn setup(ctx: &mut TestContext) {
    ctx.debug = false;
    commit(&ctx.dir, "lines.txt", "keep\nold\nkeep\n");
    run(&ctx.dir, &["git", "checkout", "-b", "feature"]);
    commit(&ctx.dir, "lines.txt", "keep\nnew\nkeep\n");
    commit(&ctx.dir, "added.txt", "added\n");
    run(&ctx.dir, &["git", "checkout", "main"]);
    commit(&ctx.dir, "main-only.txt", "main\n");
    run(&ctx.dir, &["git", "checkout", "feature"]);
}

/// The app in the repository, with a GitHub on which `feature` has no PR, and whose default
/// branch is `main`.
fn init_app(ctx: &mut TestContext) -> crate::app::App {
    let dir = ctx.dir.clone();
    init_app_at(ctx, dir)
}

fn init_app_at(ctx: &mut TestContext, dir: std::path::PathBuf) -> crate::app::App {
    let mut app = ctx.init_app_at_path(dir);
    let github = FakeGitHub::new("", "");
    *github.has_pr.borrow_mut() = false;
    app.state.github = Rc::new(github);
    app
}

/// The line of the screen that shows `text`.
fn line_with(ctx: &TestContext, text: &str) -> String {
    let screen = ctx.redact_buffer();
    screen
        .lines()
        .find(|line| line.contains(text))
        .unwrap_or_else(|| panic!("no line with {text}:\n{screen}"))
        .to_owned()
}

#[test]
fn oo_opens_the_branch_against_main() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    let mut app = init_app(&mut ctx);

    ctx.update(&mut app, keys("OO"));

    let screen = ctx.redact_buffer();
    assert!(screen.contains("lines.txt"), "{screen}");
    assert!(screen.contains("added.txt"), "{screen}");
    assert!(!screen.contains("main-only.txt"), "{screen}");
    assert!(line_with(&ctx, "added.txt").contains("[ ]"), "{screen}");
    assert!(!screen.contains("not the PR's head"), "{screen}");
}

#[test]
fn oo_on_main_says_there_is_nothing_to_review() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    run(&ctx.dir, &["git", "checkout", "main"]);
    let mut app = init_app(&mut ctx);

    ctx.update(&mut app, keys("OO"));

    let screen = ctx.redact_buffer();
    assert!(
        screen.contains("No PR to review: main is the base branch"),
        "{screen}"
    );
}

#[test]
fn a_ticked_file_stays_ticked_when_the_pr_is_opened_again() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    let mut app = init_app(&mut ctx);
    ctx.update(&mut app, keys("OO/added.txt<enter>x"));
    assert!(line_with(&ctx, "added.txt").contains("[x]"));

    let mut app = init_app(&mut ctx);
    ctx.update(&mut app, keys("OO"));

    assert!(line_with(&ctx, "added.txt").contains("[x]"));
    assert!(line_with(&ctx, "lines.txt").contains("[ ]"));
}

#[test]
fn x_unticks_a_ticked_file() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    let mut app = init_app(&mut ctx);
    ctx.update(&mut app, keys("OO/added.txt<enter>x"));

    ctx.update(&mut app, keys("x"));

    assert!(line_with(&ctx, "added.txt").contains("[ ]"));
}

#[test]
fn a_ticked_file_changed_since_shows_as_changed() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    let mut app = init_app(&mut ctx);
    ctx.update(&mut app, keys("OO/added.txt<enter>x"));

    commit(&ctx.dir, "added.txt", "added\nmore\n");
    ctx.update(&mut app, keys("gr"));

    assert!(line_with(&ctx, "added.txt").contains("[~]"));
}

#[test]
fn ticks_are_shared_by_the_repositorys_worktrees() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    let mut app = init_app(&mut ctx);
    ctx.update(&mut app, keys("OO/added.txt<enter>x"));
    // `feature` can be checked out in only one worktree at a time.
    run(&ctx.dir, &["git", "checkout", "main"]);
    let worktree = ctx.dir.parent().unwrap().join("feature-worktree");
    run(
        &ctx.dir,
        &[
            "git",
            "worktree",
            "add",
            worktree.to_str().unwrap(),
            "feature",
        ],
    );

    let mut app = init_app_at(&mut ctx, worktree);
    ctx.update(&mut app, keys("OO"));

    assert!(line_with(&ctx, "added.txt").contains("[x]"));
}

/// Where the PR `feature` into `main` is recorded.
fn review_file_path(ctx: &TestContext) -> std::path::PathBuf {
    ctx.dir.join(".git/gitu/review/feature...main.json")
}

/// The JSON file the PR `feature` into `main` is recorded in.
fn review_file(ctx: &TestContext) -> serde_json::Value {
    serde_json::from_str(&fs::read_to_string(review_file_path(ctx)).unwrap()).unwrap()
}

#[test]
fn ctrl_s_adds_a_comment_on_the_line_to_a_draft_review() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    let mut app = init_app(&mut ctx);
    ctx.update(&mut app, keys("OO/new<enter>ihello"));

    ctx.update(&mut app, keys("<ctrl+s>"));

    let screen = ctx.redact_buffer();
    assert!(
        screen.contains("Comment added to your draft review"),
        "{screen}"
    );
    let header = line_with(&ctx, "CI [");
    assert!(header.contains("draft"), "{screen}");
    assert!(screen.contains("hello"), "{screen}");
    assert!(
        !screen.contains("New comment"),
        "the box is gone:\n{screen}"
    );
}

#[test]
fn a_comment_is_there_when_the_pr_is_opened_again() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    let mut app = init_app(&mut ctx);
    ctx.update(&mut app, keys("OO/new<enter>ihello<ctrl+s>"));

    let mut app = init_app(&mut ctx);
    ctx.update(&mut app, keys("OO"));

    let screen = ctx.redact_buffer();
    let lines: Vec<&str> = screen.lines().collect();
    let new = lines.iter().position(|line| line.contains("+new")).unwrap();
    assert!(lines[new + 1].contains("CI ["), "{screen}");
    assert!(lines[new + 2].contains("hello"), "{screen}");
}

#[test]
fn i_on_my_agents_comment_replies_to_it_in_the_same_draft_review() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    review_cli_with_input(
        &ctx.dir,
        &[
            "comment",
            "--path",
            "lines.txt",
            "--line",
            "2",
            "--agent",
            "agent",
        ],
        "hello",
    )
    .unwrap();
    let mut app = init_app(&mut ctx);
    ctx.update(&mut app, keys("OO"));

    ctx.update(&mut app, keys("/agent \\[<enter>iand more<ctrl+s>"));

    let screen = ctx.redact_buffer();
    assert!(screen.contains("hello"), "{screen}");
    assert!(screen.contains("and more"), "{screen}");
    let file = review_file(&ctx);
    assert_eq!(file["reviews"].as_array().unwrap().len(), 1, "{file}");
    let comments = file["threads"][0]["comments"].as_array().unwrap();
    assert_eq!(comments.len(), 2, "{file}");
    assert_eq!(comments[0]["agent"], "agent", "{file}");
    assert_eq!(comments[1]["agent"], serde_json::Value::Null, "{file}");
    assert_eq!(comments[0]["review"], comments[1]["review"], "{file}");
}

#[test]
fn the_review_file_records_the_comment_and_the_lines_it_is_on() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    let head = run(&ctx.dir, &["git", "rev-parse", "feature"])
        .trim()
        .to_owned();
    let mut app = init_app(&mut ctx);

    ctx.update(&mut app, keys("OO/new<enter>ihello<ctrl+s>"));

    let file = review_file(&ctx);
    let review = &file["reviews"][0];
    assert_eq!(review["author"], "CI", "{file}");
    assert_eq!(review["state"], "draft", "{file}");
    let thread = &file["threads"][0];
    assert_eq!(thread["resolved"], false, "{file}");
    let anchor = &thread["anchor"];
    assert_eq!(anchor["path"], "lines.txt", "{file}");
    assert_eq!(anchor["side"], "new", "{file}");
    assert_eq!(anchor["line"], 2, "{file}");
    assert_eq!(anchor["commit"], head.as_str(), "{file}");
    assert_eq!(anchor["text"], serde_json::json!(["new"]), "{file}");
    assert_eq!(anchor["before"], serde_json::json!(["keep"]), "{file}");
    assert_eq!(anchor["after"], serde_json::json!(["keep"]), "{file}");
    let comment = &thread["comments"][0];
    assert_eq!(comment["author"], "CI", "{file}");
    assert_eq!(comment["body"], "hello", "{file}");
    assert_eq!(comment["review"], review["id"], "{file}");
}

#[test]
fn a_comment_on_a_removed_line_is_shown_under_it() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    let mut app = init_app(&mut ctx);
    ctx.update(&mut app, keys("OO/old<enter>igone<ctrl+s>"));

    let mut app = init_app(&mut ctx);
    ctx.update(&mut app, keys("OO"));

    let screen = ctx.redact_buffer();
    let lines: Vec<&str> = screen.lines().collect();
    let old = lines.iter().position(|line| line.contains("-old")).unwrap();
    assert!(lines[old + 1].contains("CI ["), "{screen}");
    assert!(lines[old + 2].contains("gone"), "{screen}");
    assert_eq!(review_file(&ctx)["threads"][0]["anchor"]["side"], "old");
}

#[test]
fn comments_are_shared_by_the_repositorys_worktrees() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    let mut app = init_app(&mut ctx);
    ctx.update(&mut app, keys("OO/new<enter>ihello<ctrl+s>"));
    run(&ctx.dir, &["git", "checkout", "main"]);
    let worktree = ctx.dir.parent().unwrap().join("feature-worktree");
    run(
        &ctx.dir,
        &[
            "git",
            "worktree",
            "add",
            worktree.to_str().unwrap(),
            "feature",
        ],
    );

    let mut app = init_app_at(&mut ctx, worktree);
    ctx.update(&mut app, keys("OO"));

    assert!(
        ctx.redact_buffer().contains("hello"),
        "{}",
        ctx.redact_buffer()
    );
}

#[test]
fn i_on_my_draft_comment_edits_it_in_the_box() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    let mut app = init_app(&mut ctx);
    ctx.update(&mut app, keys("OO/new<enter>ihello<ctrl+s>/hello<enter>"));

    ctx.update(&mut app, keys("i"));
    let screen = ctx.redact_buffer();
    assert!(screen.contains("Edit comment"), "{screen}");
    assert!(line_with(&ctx, "hello").contains("hello"), "{screen}");

    ctx.update(&mut app, keys("<space>world<ctrl+s>"));
    let screen = ctx.redact_buffer();
    assert!(screen.contains("Comment updated"), "{screen}");
    assert!(screen.contains("hello world"), "{screen}");
    assert!(
        !screen.contains("Edit comment"),
        "the box is gone:\n{screen}"
    );
    let file = review_file(&ctx);
    let comments = file["threads"][0]["comments"].as_array().unwrap();
    assert_eq!(comments.len(), 1, "{file}");
    assert_eq!(comments[0]["body"], "hello world", "{file}");
}

#[test]
fn k_deletes_a_draft_comment() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    let mut app = init_app(&mut ctx);
    ctx.update(&mut app, keys("OO/new<enter>ihello<ctrl+s>/hello<enter>"));

    ctx.update(&mut app, keys("Ky"));

    let screen = ctx.redact_buffer();
    assert!(screen.contains("Comment deleted"), "{screen}");
    assert!(!screen.contains("CI ["), "{screen}");
    assert_eq!(review_file(&ctx)["threads"], serde_json::json!([]));
}

#[test]
fn deleting_a_reply_keeps_the_rest_of_the_thread() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    review_cli_with_input(
        &ctx.dir,
        &[
            "comment",
            "--path",
            "lines.txt",
            "--line",
            "2",
            "--agent",
            "agent",
        ],
        "hello",
    )
    .unwrap();
    let mut app = init_app(&mut ctx);
    ctx.update(
        &mut app,
        keys("OO/agent \\[<enter>iand more<ctrl+s>/and more<enter>"),
    );

    ctx.update(&mut app, keys("Ky"));

    let screen = ctx.redact_buffer();
    assert!(screen.contains("hello"), "{screen}");
    assert!(!screen.contains("and more"), "{screen}");
    let file = review_file(&ctx);
    let comments = file["threads"][0]["comments"].as_array().unwrap();
    assert_eq!(comments.len(), 1, "{file}");
    assert_eq!(comments[0]["body"], "hello", "{file}");
}

/// The line of the screen after the `n`th, counting from 0, that shows `text`.
fn line_after(ctx: &TestContext, text: &str, n: usize) -> String {
    let screen = ctx.redact_buffer();
    let lines: Vec<&str> = screen.lines().collect();
    let i = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.contains(text))
        .nth(n)
        .unwrap_or_else(|| panic!("no line {n} with {text}:\n{screen}"))
        .0;
    lines[i + 1].to_owned()
}

#[test]
fn a_thread_follows_its_line_when_lines_are_added_above_it() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    let mut app = init_app(&mut ctx);
    ctx.update(&mut app, keys("OO/new<enter>ihello<ctrl+s>"));

    commit(&ctx.dir, "lines.txt", "top\nkeep\nnew\nkeep\n");
    ctx.update(&mut app, keys("gr"));

    assert!(
        line_after(&ctx, "+new", 0).contains("CI ["),
        "{}",
        ctx.redact_buffer()
    );
    assert!(
        !ctx.redact_buffer().contains("outdated"),
        "{}",
        ctx.redact_buffer()
    );
}

#[test]
fn a_thread_whose_line_has_gone_is_outdated() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    let mut app = init_app(&mut ctx);
    ctx.update(&mut app, keys("OO/new<enter>ihello<ctrl+s>"));

    commit(&ctx.dir, "lines.txt", "keep\nnewer\nkeep\n");
    ctx.update(&mut app, keys("gr"));

    let header = line_after(&ctx, "lines.txt", 0);
    assert!(header.contains("CI ["), "{}", ctx.redact_buffer());
    assert!(header.contains("outdated"), "{}", ctx.redact_buffer());
}

#[test]
fn a_thread_on_a_repeated_line_follows_the_one_with_its_surrounding_lines() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    commit(&ctx.dir, "lines.txt", "keep\nnew\nkeep\nother\nnew\nend\n");
    let mut app = init_app(&mut ctx);
    ctx.update(&mut app, keys("OO/new<enter>nihello<ctrl+s>"));
    assert!(
        line_after(&ctx, "+new", 1).contains("CI ["),
        "{}",
        ctx.redact_buffer()
    );

    commit(
        &ctx.dir,
        "lines.txt",
        "top\nkeep\nnew\nkeep\nother\nnew\nend\n",
    );
    ctx.update(&mut app, keys("gr"));

    assert!(
        line_after(&ctx, "+new", 1).contains("CI ["),
        "{}",
        ctx.redact_buffer()
    );
    assert!(
        !line_after(&ctx, "+new", 0).contains("CI ["),
        "{}",
        ctx.redact_buffer()
    );
}

/// What `gitu review <args>` prints, run in `dir`.
fn review_cli(dir: &std::path::Path, args: &[&str]) -> String {
    review_cli_with_input(dir, args, "").unwrap()
}

/// What `gitu review <args>` prints, run in `dir` with `input` on stdin, with a GitHub on which
/// the branch has no PR.
fn review_cli_with_input(
    dir: &std::path::Path,
    args: &[&str],
    input: &str,
) -> Result<String, crate::error::Error> {
    let github = FakeGitHub::new("", "");
    *github.has_pr.borrow_mut() = false;
    review_cli_with_github(dir, args, input, Rc::new(github))
}

/// What `gitu review <args>` prints, run in `dir` with `input` on stdin and `github` as GitHub.
fn review_cli_with_github(
    dir: &std::path::Path,
    args: &[&str],
    input: &str,
    github: Rc<FakeGitHub>,
) -> Result<String, crate::error::Error> {
    use clap::Parser;
    let args = crate::cli::Args::try_parse_from(["gitu", "review"].iter().chain(args))
        .unwrap_or_else(|e| panic!("{e}"));
    let Some(crate::cli::Commands::Review { command }) = args.command else {
        unreachable!()
    };
    let mut out = vec![];
    crate::review::cli::run(
        &command,
        git2::Repository::open(dir).unwrap(),
        github,
        &mut input.as_bytes(),
        &mut out,
    )?;
    Ok(String::from_utf8(out).unwrap())
}

#[test]
fn review_show_prints_each_thread_with_its_lines_and_comments() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    review_cli_with_input(
        &ctx.dir,
        &[
            "comment",
            "--path",
            "lines.txt",
            "--line",
            "2",
            "--agent",
            "agent",
        ],
        "hello",
    )
    .unwrap();
    let mut app = init_app(&mut ctx);
    ctx.update(
        &mut app,
        keys("OO/agent \\[<enter>iand more<ctrl+s>/old<enter>igone<ctrl+s>"),
    );

    let markdown = review_cli(&ctx.dir, &["show"]);

    assert!(
        markdown.starts_with("# Review of feature into main\n"),
        "{markdown}"
    );
    let new = markdown.find("## lines.txt:2\n").expect(&markdown);
    let old = markdown
        .find("## lines.txt:2 (old side)\n")
        .expect(&markdown);
    let (new_section, old_section) = if new < old {
        (&markdown[new..old], &markdown[old..])
    } else {
        (&markdown[new..], &markdown[old..new])
    };
    assert!(
        new_section.contains("````\n  keep\n> new\n  keep\n````\n"),
        "{markdown}"
    );
    assert!(new_section.contains("**CI** (draft, "), "{markdown}");
    assert!(new_section.contains("hello\n"), "{markdown}");
    assert!(
        new_section.find("hello").unwrap() < new_section.find("and more").unwrap(),
        "{markdown}"
    );
    assert!(
        old_section.contains("````\n  keep\n> old\n  keep\n````\n"),
        "{markdown}"
    );
    assert!(old_section.contains("gone\n"), "{markdown}");
}

#[test]
fn review_show_says_where_an_outdated_thread_was() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    let mut app = init_app(&mut ctx);
    ctx.update(&mut app, keys("OO/new<enter>ihello<ctrl+s>"));
    commit(&ctx.dir, "lines.txt", "keep\nnewer\nkeep\n");

    let markdown = review_cli(&ctx.dir, &["show"]);

    assert!(
        markdown.contains("## lines.txt (outdated: was line 2)\n"),
        "{markdown}"
    );
}

#[test]
fn review_show_reads_the_pr_given_by_head_and_base() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    let mut app = init_app(&mut ctx);
    ctx.update(&mut app, keys("OO/new<enter>ihello<ctrl+s>"));
    run(&ctx.dir, &["git", "checkout", "main"]);

    let markdown = review_cli(&ctx.dir, &["show", "--head", "feature", "--base", "main"]);

    assert!(markdown.contains("hello"), "{markdown}");
}

#[test]
fn review_reply_with_an_agent_adds_the_reply_to_my_draft_review_with_its_name() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    let mut app = init_app(&mut ctx);
    ctx.update(&mut app, keys("OO/new<enter>ihello<ctrl+s>"));
    let thread = review_file(&ctx)["threads"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();

    review_cli_with_input(
        &ctx.dir,
        &["reply", &thread, "--agent", "claude"],
        "Agreed.\n",
    )
    .unwrap();

    let file = review_file(&ctx);
    let comments = file["threads"][0]["comments"].as_array().unwrap();
    assert_eq!(comments.len(), 2, "{file}");
    assert_eq!(comments[1]["author"], "CI", "{file}");
    assert_eq!(comments[1]["agent"], "claude", "{file}");
    assert_eq!(comments[1]["body"], "Agreed.", "{file}");
    let reviews = file["reviews"].as_array().unwrap();
    assert_eq!(reviews.len(), 1, "{file}");
    assert_eq!(reviews[0]["author"], "CI", "{file}");
    assert_eq!(reviews[0]["state"], "draft", "{file}");
    assert_eq!(comments[1]["review"], reviews[0]["id"], "{file}");
    let markdown = review_cli(&ctx.dir, &["show"]);
    assert!(markdown.contains("**claude** (draft, "), "{markdown}");
    assert!(markdown.contains("Agreed.\n"), "{markdown}");
}

#[test]
fn review_reply_by_default_is_by_gits_user() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    let mut app = init_app(&mut ctx);
    ctx.update(&mut app, keys("OO/new<enter>ihello<ctrl+s>"));
    let thread = review_file(&ctx)["threads"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();

    review_cli_with_input(&ctx.dir, &["reply", &thread], "more").unwrap();

    let file = review_file(&ctx);
    assert_eq!(file["reviews"].as_array().unwrap().len(), 1, "{file}");
    assert_eq!(file["threads"][0]["comments"][1]["author"], "CI", "{file}");
}

#[test]
fn review_reply_to_a_thread_that_is_not_there_is_an_error() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);

    let result = review_cli_with_input(&ctx.dir, &["reply", "nope"], "hi");

    let error = result.unwrap_err().to_string();
    assert!(error.contains("no thread nope"), "{error}");
    let file = review_file(&ctx);
    assert_eq!(
        file["reviews"],
        serde_json::json!([]),
        "no review is started: {file}"
    );
}

#[test]
fn review_reply_that_is_empty_is_an_error() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    let mut app = init_app(&mut ctx);
    ctx.update(&mut app, keys("OO/new<enter>ihello<ctrl+s>"));
    let thread = review_file(&ctx)["threads"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let result = review_cli_with_input(&ctx.dir, &["reply", &thread], " \n");

    let error = result.unwrap_err().to_string();
    assert!(error.contains("Nothing to reply"), "{error}");
    assert_eq!(
        review_file(&ctx)["threads"][0]["comments"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn review_comment_with_an_agent_starts_a_thread_in_my_draft_review() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);

    review_cli_with_input(
        &ctx.dir,
        &[
            "comment",
            "--path",
            "lines.txt",
            "--line",
            "2",
            "--agent",
            "claude",
        ],
        "Why new?\n",
    )
    .unwrap();

    let file = review_file(&ctx);
    let thread = &file["threads"][0];
    assert_eq!(thread["anchor"]["path"], "lines.txt", "{file}");
    assert_eq!(thread["anchor"]["side"], "new", "{file}");
    assert_eq!(thread["anchor"]["line"], 2, "{file}");
    assert_eq!(
        thread["anchor"]["text"],
        serde_json::json!(["new"]),
        "{file}"
    );
    assert_eq!(thread["comments"][0]["author"], "CI", "{file}");
    assert_eq!(thread["comments"][0]["agent"], "claude", "{file}");
    assert_eq!(thread["comments"][0]["body"], "Why new?", "{file}");
    assert_eq!(file["reviews"][0]["author"], "CI", "{file}");
    assert_eq!(file["reviews"][0]["state"], "draft", "{file}");
    let mut app = init_app(&mut ctx);
    ctx.update(&mut app, keys("OO"));
    assert!(
        line_after(&ctx, "+new", 0).contains("claude ["),
        "{}",
        ctx.redact_buffer()
    );
}

#[test]
fn review_comment_on_the_old_side_is_on_the_removed_line() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);

    review_cli_with_input(
        &ctx.dir,
        &[
            "comment",
            "--path",
            "lines.txt",
            "--line",
            "2",
            "--side",
            "old",
        ],
        "Why not old?",
    )
    .unwrap();

    let file = review_file(&ctx);
    assert_eq!(file["threads"][0]["anchor"]["side"], "old", "{file}");
    assert_eq!(
        file["threads"][0]["anchor"]["text"],
        serde_json::json!(["old"]),
        "{file}"
    );
    assert_eq!(file["threads"][0]["comments"][0]["author"], "CI", "{file}");
}

#[test]
fn review_comment_on_a_line_not_in_the_diff_is_an_error() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    // long.txt is where the PR branched from, so only its first lines are in the diff.
    run(&ctx.dir, &["git", "checkout", "main"]);
    commit(&ctx.dir, "long.txt", &"line\n".repeat(20));
    run(&ctx.dir, &["git", "checkout", "feature"]);
    run(&ctx.dir, &["git", "merge", "--no-edit", "main"]);
    commit(
        &ctx.dir,
        "long.txt",
        &format!("changed\n{}", "line\n".repeat(19)),
    );

    for (path, line) in [
        ("long.txt", "15"),
        ("main-only.txt", "1"),
        ("lines.txt", "9"),
    ] {
        let result =
            review_cli_with_input(&ctx.dir, &["comment", "--path", path, "--line", line], "hi");

        let error = result.unwrap_err().to_string();
        assert!(
            error.contains(&format!("{path}:{line} is not in the PR's diff")),
            "{error}"
        );
    }
    let path = ctx.dir.join(".git/gitu/review/feature...main.json");
    assert!(!path.exists(), "nothing is recorded");
}

#[test]
fn review_comment_that_is_empty_is_an_error() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);

    let result = review_cli_with_input(
        &ctx.dir,
        &["comment", "--path", "lines.txt", "--line", "2"],
        "\n",
    );

    let error = result.unwrap_err().to_string();
    assert!(error.contains("Nothing to comment"), "{error}");
}

/// GitHub with a PR of `feature` into `main` whose head is where `feature` is now, with its
/// commits.
fn github_at_head(ctx: &TestContext) -> Rc<FakeGitHub> {
    let github = FakeGitHub::new(
        run(&ctx.dir, &["git", "rev-parse", "main"]).trim(),
        run(&ctx.dir, &["git", "rev-parse", "feature"]).trim(),
    );
    *github.commits.borrow_mut() = run(&ctx.dir, &["git", "rev-list", "--count", "main..feature"])
        .trim()
        .parse()
        .unwrap();
    Rc::new(github)
}

#[test]
fn review_sync_pushes_the_branch_and_creates_a_draft_pr_when_there_is_none() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    let mut app = init_app(&mut ctx);
    ctx.update(&mut app, keys("OO/new<enter>ihello<ctrl+s>"));
    let github = github_at_head(&ctx);
    *github.has_pr.borrow_mut() = false;

    review_cli_with_github(&ctx.dir, &["sync", "--create-pr"], "", Rc::clone(&github)).unwrap();

    assert_eq!(
        *github.created.borrow(),
        vec![("feature".to_owned(), "main".to_owned())]
    );
    let pushed = run(&ctx.remote_dir, &["git", "rev-parse", "feature"]);
    let local = run(&ctx.dir, &["git", "rev-parse", "feature"]);
    assert_eq!(pushed, local);
    assert_eq!(github.posted.borrow().len(), 1);
}

/// A thread GitHub doesn't add is not posted: `O s` says so, and the comment stays a draft.
#[test]
fn os_says_so_when_github_does_not_add_a_thread_and_keeps_it_a_draft() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    let github = github_at_head(&ctx);
    *github.refuses_threads.borrow_mut() = true;
    let mut app = init_app(&mut ctx);
    app.state.github = Rc::clone(&github) as Rc<dyn GitHub>;
    ctx.update(&mut app, keys("OO/new<enter>ihello<ctrl+s>"));

    ctx.update(&mut app, keys("Os"));

    let screen = ctx.redact_buffer();
    assert!(screen.contains("Couldn't add the thread"), "{screen}");
    assert!(!screen.contains("Posted"), "{screen}");
    let file = review_file(&ctx);
    assert_eq!(file["threads"][0]["comments"][0]["body"], "hello", "{file}");
    let reviews = file["reviews"].as_array().unwrap();
    assert!(reviews.iter().any(|r| r["state"] == "draft"), "{file}");
}

/// Creating a PR on GitHub makes the branch visible to everyone who can see the repository, so
/// posting from a branch with no PR creates one only when told to.
#[test]
fn review_sync_on_a_branch_with_no_github_pr_needs_create_pr() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    comment_with_base(&ctx, None);
    let github = github_at_head(&ctx);
    *github.has_pr.borrow_mut() = false;

    let result = review_cli_with_github(&ctx.dir, &["sync"], "", Rc::clone(&github));

    let error = result.unwrap_err().to_string();
    assert!(error.contains("--create-pr"), "{error}");
    assert!(github.created.borrow().is_empty());
    assert!(github.posted.borrow().is_empty());
}

/// `O s` on a branch with no GitHub PR says it would create one, which everyone who can see the
/// repository sees, and does nothing unless told to.
#[test]
fn os_on_a_branch_with_no_github_pr_asks_before_creating_one() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    comment_with_base(&ctx, None);
    let github = github_at_head(&ctx);
    *github.has_pr.borrow_mut() = false;
    let mut app = init_app(&mut ctx);
    app.state.github = Rc::clone(&github) as Rc<dyn GitHub>;

    ctx.update(&mut app, keys("Os"));
    let screen = ctx.redact_buffer();
    assert!(
        screen.contains("No PR on GitHub: create one? [y/n]"),
        "{screen}"
    );

    let mut app = init_app(&mut ctx);
    app.state.github = Rc::clone(&github) as Rc<dyn GitHub>;
    ctx.update(&mut app, keys("Osn"));
    assert!(github.created.borrow().is_empty());
    assert!(github.posted.borrow().is_empty());
}

/// Uncommitted changes don't stop a sync that doesn't need to move the branch.
#[test]
fn syncing_a_branch_at_githubs_head_with_uncommitted_changes_posts() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    comment_with_base(&ctx, None);
    std::fs::write(ctx.dir.join("lines.txt"), "uncommitted\n").unwrap();
    let github = github_at_head(&ctx);

    review_cli_with_github(&ctx.dir, &["sync"], "", Rc::clone(&github)).unwrap();

    assert_eq!(github.posted.borrow().len(), 1);
}

/// The comment in the review file whose text is `body`.
fn stored_comment(ctx: &TestContext, body: &str) -> serde_json::Value {
    let file = review_file(ctx);
    file["threads"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|t| t["comments"].as_array().unwrap())
        .find(|c| c["body"] == body)
        .unwrap_or_else(|| panic!("no comment {body}: {file}"))
        .clone()
}

/// The id of the comment whose text is `body`, as the review command line takes it.
fn comment_id(ctx: &TestContext, body: &str) -> String {
    stored_comment(ctx, body)["id"].as_str().unwrap().to_owned()
}

/// A local-only comment is in no review, and sync never posts it.
#[test]
fn a_local_only_comment_is_never_posted() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    review_cli_with_input(
        &ctx.dir,
        &["comment", "--path", "lines.txt", "--line", "2", "--local"],
        "just between us",
    )
    .unwrap();
    let github = github_at_head(&ctx);

    review_cli_with_github(&ctx.dir, &["sync"], "", Rc::clone(&github)).unwrap();

    assert!(github.posted.borrow().is_empty());
    assert_eq!(
        stored_comment(&ctx, "just between us")["review"],
        serde_json::Value::Null
    );
    let show = &["show", "--format", "json"];
    let json = review_cli_with_github(&ctx.dir, show, "", Rc::clone(&github)).unwrap();
    let json: serde_json::Value = serde_json::from_str(&json).unwrap();
    let comment = &json["threads"][0]["comments"][0];
    assert_eq!(comment["local_only"], true, "{json}");
}

/// A reply in a thread whose first comment is local-only is local-only too, unless said.
#[test]
fn a_reply_in_a_local_only_thread_is_local_only_unless_said() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    review_cli_with_input(
        &ctx.dir,
        &["comment", "--path", "lines.txt", "--line", "2", "--local"],
        "just between us",
    )
    .unwrap();
    let thread = review_file(&ctx)["threads"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();

    review_cli_with_input(&ctx.dir, &["reply", &thread], "agreed").unwrap();
    review_cli_with_input(&ctx.dir, &["reply", &thread, "--draft"], "for GitHub").unwrap();

    assert_eq!(
        stored_comment(&ctx, "agreed")["review"],
        serde_json::Value::Null
    );
    assert_ne!(
        stored_comment(&ctx, "for GitHub")["review"],
        serde_json::Value::Null
    );
}

/// A comment moves between the draft review and local-only, and sync posts it only from the
/// draft review.
#[test]
fn review_move_moves_a_comment_between_the_draft_review_and_local_only() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    comment_with_base(&ctx, None);
    let id = comment_id(&ctx, "hello");
    let github = github_at_head(&ctx);

    review_cli_with_input(&ctx.dir, &["move", &id, "--local"], "").unwrap();
    review_cli_with_github(&ctx.dir, &["sync"], "", Rc::clone(&github)).unwrap();
    assert!(github.posted.borrow().is_empty());

    review_cli_with_github(&ctx.dir, &["move", &id, "--draft"], "", Rc::clone(&github)).unwrap();
    review_cli_with_github(&ctx.dir, &["sync"], "", Rc::clone(&github)).unwrap();
    assert_eq!(github.posted.borrow().len(), 1);
}

/// A draft reply in a thread whose first comment is local-only can't reach GitHub before its
/// thread: sync posts nothing, and says which reply it is.
#[test]
fn a_draft_reply_in_a_local_only_thread_blocks_sync() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    review_cli_with_input(
        &ctx.dir,
        &["comment", "--path", "lines.txt", "--line", "2", "--local"],
        "just between us",
    )
    .unwrap();
    let thread = review_file(&ctx)["threads"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    review_cli_with_input(&ctx.dir, &["reply", &thread, "--draft"], "for GitHub").unwrap();
    let github = github_at_head(&ctx);

    let error = review_cli_with_github(&ctx.dir, &["sync"], "", Rc::clone(&github))
        .unwrap_err()
        .to_string();

    assert!(error.contains("local-only"), "{error}");
    assert!(error.contains("for GitHub"), "{error}");
    assert!(github.posted.borrow().is_empty());
}

/// p on a comment moves it between the draft review and local-only, and the screen says which
/// it is in.
#[test]
fn p_moves_a_comment_to_local_only_and_back() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    comment_with_base(&ctx, None);
    let mut app = init_app(&mut ctx);
    ctx.update(&mut app, keys("OO/hello<enter>"));

    ctx.update(&mut app, keys("p"));
    assert!(
        line_with(&ctx, "CI [").contains("local-only"),
        "{}",
        ctx.redact_buffer()
    );
    assert_eq!(
        stored_comment(&ctx, "hello")["review"],
        serde_json::Value::Null
    );

    ctx.update(&mut app, keys("p"));
    assert!(
        line_with(&ctx, "CI [").contains("draft"),
        "{}",
        ctx.redact_buffer()
    );
    assert_ne!(
        stored_comment(&ctx, "hello")["review"],
        serde_json::Value::Null
    );
}

/// A comment GitHub added, though its reply was lost, as when the network fails just after GitHub
/// has made the change, is not posted again by the next sync: it is found in the pending review.
#[test]
fn a_comment_whose_reply_from_github_was_lost_is_not_posted_twice() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    let github = github_at_head(&ctx);
    comment_with_base(&ctx, None);
    *github.loses_replies.borrow_mut() = 1;

    review_cli_with_github(&ctx.dir, &["sync"], "", Rc::clone(&github)).unwrap_err();
    assert_eq!(github.posted.borrow().len(), 1);

    review_cli_with_github(&ctx.dir, &["sync"], "", Rc::clone(&github)).unwrap();

    assert_eq!(github.posted.borrow().len(), 1);
    let file = review_file(&ctx);
    let comments = file["threads"][0]["comments"].as_array().unwrap();
    assert_eq!(comments.len(), 1, "{file}");
    assert_eq!(comments[0]["github"], "new0_0", "{file}");
}

/// A comment a sync was posting when it stopped may be in the pending review on GitHub already. It
/// can't be edited, deleted or moved to local-only until a sync finishes: the next sync would not
/// find it there and would post it twice, or it would be left there.
#[test]
fn a_comment_a_stopped_sync_was_posting_cant_be_changed_until_a_sync_finishes() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    let github = github_at_head(&ctx);
    comment_with_base(&ctx, None);
    *github.loses_replies.borrow_mut() = 1;
    review_cli_with_github(&ctx.dir, &["sync"], "", Rc::clone(&github)).unwrap_err();
    let id = comment_id(&ctx, "hello");

    for args in [
        vec!["edit", id.as_str()],
        vec!["delete", id.as_str()],
        vec!["move", id.as_str(), "--local"],
    ] {
        let error = review_cli_with_github(&ctx.dir, &args, "changed", Rc::clone(&github))
            .unwrap_err()
            .to_string();
        assert!(error.contains("sync again"), "{args:?}: {error}");
    }
    assert_ne!(
        stored_comment(&ctx, "hello")["review"],
        serde_json::Value::Null
    );

    review_cli_with_github(&ctx.dir, &["sync"], "", Rc::clone(&github)).unwrap();
    assert_eq!(github.posted.borrow().len(), 1);
}

/// A comment GitHub added though its reply was lost, and whose review I then submitted on GitHub,
/// is not posted again by the next sync: it is found among my comments, though no longer pending.
#[test]
fn a_comment_whose_reply_was_lost_is_not_posted_twice_once_its_review_is_submitted() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    let github = github_at_head(&ctx);
    comment_with_base(&ctx, None);
    *github.loses_replies.borrow_mut() = 1;
    review_cli_with_github(&ctx.dir, &["sync"], "", Rc::clone(&github)).unwrap_err();
    assert_eq!(github.posted.borrow().len(), 1);

    // On GitHub, I submit the pending review.
    for comment in github
        .threads
        .borrow_mut()
        .iter_mut()
        .flat_map(|t| &mut t.comments)
    {
        comment.draft = false;
        comment.submitted = true;
    }
    *github.submitted_by_me.borrow_mut() = true;
    *github.pending.borrow_mut() = None;

    review_cli_with_github(&ctx.dir, &["sync"], "", Rc::clone(&github)).unwrap();

    assert_eq!(github.posted.borrow().len(), 1);
    // Nor is a pending review started, which would be left with nothing in it.
    assert_eq!(*github.reviews_started.borrow(), 1);
}

/// Of two identical draft comments on a line, one posted though its reply was lost, the other
/// is still posted: only a comment from an attempt that didn't finish is looked for.
#[test]
fn two_identical_drafts_are_both_posted_after_a_lost_reply() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    let github = github_at_head(&ctx);
    comment_with_base(&ctx, None);
    comment_with_base(&ctx, None);
    *github.loses_replies.borrow_mut() = 1;

    review_cli_with_github(&ctx.dir, &["sync"], "", Rc::clone(&github)).unwrap_err();
    review_cli_with_github(&ctx.dir, &["sync"], "", Rc::clone(&github)).unwrap();

    let posted = github.posted.borrow();
    assert_eq!(posted.len(), 2, "{posted:?}");
    assert!(posted.iter().all(|(_, body)| body == "hello"), "{posted:?}");
}

/// Changing my comment in a submitted review changes what others see: the review command line
/// does it only when given --publish.
#[test]
fn review_edit_of_my_comment_in_a_submitted_review_needs_publish() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    let github = github_at_head(&ctx);
    *github.submitted_by_me.borrow_mut() = true;
    *github.threads.borrow_mut() = vec![super::pr::thread(
        "t1",
        Some((crate::review::Side::New, 2)),
        false,
        &[("me", "Why new?")],
    )];
    review_cli_with_github(&ctx.dir, &["sync"], "", Rc::clone(&github)).unwrap();

    let refused = review_cli_with_github(
        &ctx.dir,
        &["edit", "github-t1_0"],
        "Why new? really",
        Rc::clone(&github),
    );

    let error = refused.unwrap_err().to_string();
    assert!(error.contains("--publish"), "{error}");
    assert!(github.edited.borrow().is_empty());

    review_cli_with_github(
        &ctx.dir,
        &["edit", "github-t1_0", "--publish"],
        "Why new? really",
        Rc::clone(&github),
    )
    .unwrap();
    assert_eq!(
        *github.edited.borrow(),
        vec![("t1_0".to_owned(), "Why new? really".to_owned())]
    );
}

/// My comment was in my pending review when fetched, and I then submitted that review on
/// GitHub. The comment is now public, so editing it without --publish must be refused.
#[test]
fn editing_my_comment_whose_review_was_submitted_on_github_since_the_fetch_needs_publish() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    let github = github_at_head(&ctx);
    let mut t = super::pr::thread(
        "t1",
        Some((crate::review::Side::New, 2)),
        false,
        &[("me", "Why new?")],
    );
    t.comments[0].draft = true;
    t.comments[0].submitted = false;
    *github.threads.borrow_mut() = vec![t];
    review_cli_with_github(&ctx.dir, &["sync"], "", Rc::clone(&github)).unwrap();

    // On GitHub, I submit the pending review. delta has not fetched since.
    github.threads.borrow_mut()[0].comments[0].draft = false;
    github.threads.borrow_mut()[0].comments[0].submitted = true;
    *github.submitted_by_me.borrow_mut() = true;
    *github.pending.borrow_mut() = None;

    let result = review_cli_with_github(
        &ctx.dir,
        &["edit", "github-t1_0"],
        "Why new? really",
        Rc::clone(&github),
    );
    assert!(
        result.is_err() && github.edited.borrow().is_empty(),
        "edited a now-public comment without --publish: {:?}",
        github.edited.borrow()
    );
}

/// I edited my pending comment on GitHub after delta last fetched. Editing it in delta
/// from the stale copy must not silently overwrite the edit made on GitHub.
#[test]
fn editing_my_comment_from_a_copy_older_than_github_is_refused() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    let github = github_at_head(&ctx);
    let mut t = super::pr::thread(
        "t1",
        Some((crate::review::Side::New, 2)),
        false,
        &[("me", "Why new?")],
    );
    t.comments[0].draft = true;
    t.comments[0].submitted = false;
    *github.threads.borrow_mut() = vec![t];
    review_cli_with_github(&ctx.dir, &["sync"], "", Rc::clone(&github)).unwrap();

    // On GitHub, I edit the comment. delta has not fetched since.
    github.threads.borrow_mut()[0].comments[0].body = "Why new? (edited on GitHub)".to_owned();

    let result = review_cli_with_github(
        &ctx.dir,
        &["edit", "github-t1_0"],
        "Why new? edited in delta",
        Rc::clone(&github),
    );
    assert!(
        result.is_err() && github.edited.borrow().is_empty(),
        "overwrote an edit made on GitHub since the last fetch: {:?}",
        github.edited.borrow()
    );
}

/// My reply was in my pending review when fetched, and I then submitted that review on GitHub.
/// The reply is now public, so deleting it without --publish must be refused.
#[test]
fn deleting_my_reply_whose_review_was_submitted_on_github_since_the_fetch_needs_publish() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    let github = github_at_head(&ctx);
    let mut t = super::pr::thread(
        "t1",
        Some((crate::review::Side::New, 2)),
        false,
        &[("me", "Why new?"), ("me", "And more")],
    );
    for comment in &mut t.comments {
        comment.draft = true;
        comment.submitted = false;
    }
    *github.threads.borrow_mut() = vec![t];
    review_cli_with_github(&ctx.dir, &["sync"], "", Rc::clone(&github)).unwrap();

    // On GitHub, I submit the pending review. delta has not fetched since.
    for comment in &mut github.threads.borrow_mut()[0].comments {
        comment.draft = false;
        comment.submitted = true;
    }
    *github.submitted_by_me.borrow_mut() = true;
    *github.pending.borrow_mut() = None;

    let result =
        review_cli_with_github(&ctx.dir, &["delete", "github-t1_1"], "", Rc::clone(&github));
    assert!(
        result.is_err() && github.deleted.borrow().is_empty(),
        "deleted a now-public comment without --publish: {:?}",
        github.deleted.borrow()
    );
}

/// Sync never pushes: a branch with commits GitHub's PR doesn't have is refused, with nothing
/// pushed or posted.
#[test]
fn review_sync_with_commits_github_doesnt_have_refuses_and_pushes_nothing() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    run(&ctx.dir, &["git", "push", "-u", "origin", "feature"]);
    let pushed = run(&ctx.remote_dir, &["git", "rev-parse", "feature"]);
    let github = github_at_head(&ctx);
    comment_with_base(&ctx, None);
    commit(&ctx.dir, "added.txt", "added\nmore\n");

    let result = review_cli_with_github(&ctx.dir, &["sync"], "", Rc::clone(&github));

    let error = result.unwrap_err().to_string();
    assert!(error.contains("push them first"), "{error}");
    assert_eq!(
        run(&ctx.remote_dir, &["git", "rev-parse", "feature"]),
        pushed
    );
    assert!(github.posted.borrow().is_empty());
}

/// When gitu can't yet open a branch's PR, it says how to see the drafts, which are kept.
#[test]
fn a_pr_not_handled_yet_says_how_to_see_its_drafts() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    let github = github_at_head(&ctx);
    review_cli_with_github(
        &ctx.dir,
        &["comment", "--path", "lines.txt", "--line", "2"],
        "hello",
        Rc::clone(&github),
    )
    .unwrap();
    *github.has_pr.borrow_mut() = false;

    let error = review_cli_with_github(&ctx.dir, &["show"], "", Rc::clone(&github))
        .unwrap_err()
        .to_string();

    assert!(error.contains("no longer has open"), "{error}");
    assert!(
        error.contains("delta --tui review show --head feature --base main"),
        "{error}"
    );
}

/// With more than one local PR for a branch, gitu says how to see each one's drafts.
#[test]
fn two_local_prs_for_a_branch_say_how_to_see_each_ones_drafts() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    run(&ctx.dir, &["git", "branch", "release", "main"]);
    comment_with_base(&ctx, None);
    comment_with_base(&ctx, Some("release"));

    let error = review_cli_with_input(&ctx.dir, &["show"], "")
        .unwrap_err()
        .to_string();

    assert!(
        error.contains("delta --tui review show --head feature --base main"),
        "{error}"
    );
    assert!(
        error.contains("delta --tui review show --head feature --base release"),
        "{error}"
    );
}

#[test]
fn review_sync_refuses_when_githubs_head_is_not_the_branchs() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    let mut app = init_app(&mut ctx);
    ctx.update(&mut app, keys("OO/new<enter>ihello<ctrl+s>"));
    let main = run(&ctx.dir, &["git", "rev-parse", "main"]);
    let github = Rc::new(FakeGitHub::new(main.trim(), &"0".repeat(40)));

    let result = review_cli_with_github(&ctx.dir, &["sync"], "", Rc::clone(&github));

    let error = result.unwrap_err().to_string();
    assert!(error.contains("not at GitHub's PR #7's head"), "{error}");
    assert!(github.posted.borrow().is_empty());
    assert_eq!(review_file(&ctx)["reviews"][0]["state"], "draft");
}

#[test]
fn os_posts_the_review() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    let github = github_at_head(&ctx);
    let mut app = init_app(&mut ctx);
    app.state.github = Rc::clone(&github) as Rc<dyn GitHub>;
    ctx.update(&mut app, keys("OO/new<enter>ihello<ctrl+s>"));

    ctx.update(&mut app, keys("Os"));

    assert_eq!(github.posted.borrow().len(), 1);
    let screen = ctx.redact_buffer();
    assert!(screen.contains("Posted 1 thread"), "{screen}");
    // Fetched back from GitHub, where it is in my pending review, by my GitHub login.
    assert!(!screen.contains("CI ["), "{screen}");
    assert!(!line_with(&ctx, "me [").contains("draft"), "{screen}");
}

#[test]
fn review_sync_refuses_when_the_github_prs_base_is_not_the_local_prs() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    let mut app = init_app(&mut ctx);
    ctx.update(&mut app, keys("OO/new<enter>ihello<ctrl+s>"));
    let github = github_at_head(&ctx);
    *github.base_ref.borrow_mut() = "release".to_owned();

    let result = review_cli_with_github(
        &ctx.dir,
        &["sync", "--base", "main"],
        "",
        Rc::clone(&github),
    );

    let error = result.unwrap_err().to_string();
    assert!(error.contains("is into release, not main"), "{error}");
    assert!(github.posted.borrow().is_empty());
}

#[test]
fn the_base_is_the_github_prs_base() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    run(&ctx.dir, &["git", "branch", "release", "main"]);
    let github = github_at_head(&ctx);
    *github.base_ref.borrow_mut() = "release".to_owned();

    let markdown = review_cli_with_github(&ctx.dir, &["show"], "", github).unwrap();

    assert!(
        markdown.starts_with("# Review of feature into release\n"),
        "{markdown}"
    );
}

#[test]
fn without_a_github_pr_the_base_is_githubs_default_branch() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    run(&ctx.dir, &["git", "branch", "trunk", "main"]);
    let github = Rc::new(FakeGitHub::new("", ""));
    *github.has_pr.borrow_mut() = false;
    *github.default_branch.borrow_mut() = "trunk".to_owned();

    let markdown = review_cli_with_github(&ctx.dir, &["show"], "", github).unwrap();

    assert!(
        markdown.starts_with("# Review of feature into trunk\n"),
        "{markdown}"
    );
}

#[test]
fn when_gh_fails_the_base_must_be_given() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    let github = Rc::new(FakeGitHub::new("", ""));
    *github.fails.borrow_mut() = true;

    let error = review_cli_with_github(&ctx.dir, &["show"], "", Rc::clone(&github))
        .unwrap_err()
        .to_string();
    let given = review_cli_with_github(&ctx.dir, &["show", "--base", "main"], "", github);

    assert!(error.contains("--base"), "{error}");
    assert!(
        error.contains("error connecting to api.github.com"),
        "{error}"
    );
    assert!(
        given
            .unwrap()
            .starts_with("# Review of feature into main\n")
    );
}

#[test]
fn fetching_a_thread_whose_commit_is_not_in_the_repository_is_an_error() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    let main = run(&ctx.dir, &["git", "rev-parse", "main"]);
    let github = Rc::new(FakeGitHub::new(main.trim(), &"0".repeat(40)));
    *github.threads.borrow_mut() = vec![super::pr::thread(
        "t1",
        Some((crate::review::Side::New, 2)),
        false,
        &[("alice", "Why new?")],
    )];

    let result = review_cli_with_github(&ctx.dir, &["fetch"], "", github);

    let error = result.unwrap_err().to_string();
    assert!(error.contains("is not in the repository"), "{error}");
}

/// Every comment GitHub gives is in one of the reviews it lists. One that is not would be lost
/// from the local copy, so fetching it is an error.
#[test]
fn fetching_a_comment_in_no_review_is_an_error() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    let github = github_at_head(&ctx);
    *github.threads.borrow_mut() = vec![super::pr::thread(
        "t1",
        Some((crate::review::Side::New, 2)),
        false,
        &[("alice", "Why new?")],
    )];
    *github.unreviewed.borrow_mut() = vec!["t1_0".to_owned()];

    let result = review_cli_with_github(&ctx.dir, &["fetch"], "", github);

    let error = result.unwrap_err().to_string();
    assert!(error.contains("t1_0"), "{error}");
    assert!(error.contains("in no review"), "{error}");
}

/// A thread on a file that is not in the PR's diff has nowhere to be shown, so the screen says
/// it is not shown.
#[test]
fn a_thread_on_a_file_not_in_the_diff_is_said_not_to_be_shown() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    review_cli_with_input(
        &ctx.dir,
        &["comment", "--path", "added.txt", "--line", "1"],
        "hello",
    )
    .unwrap();
    let mut file = review_file(&ctx);
    file["threads"][0]["anchor"]["path"] = "elsewhere.txt".into();
    fs::write(
        review_file_path(&ctx),
        serde_json::to_string_pretty(&file).unwrap(),
    )
    .unwrap();
    let mut app = init_app(&mut ctx);

    ctx.update(&mut app, keys("OO"));

    let screen = ctx.redact_buffer();
    assert!(
        screen.contains("1 thread on elsewhere.txt is not shown"),
        "{screen}"
    );
}

/// A pending review on GitHub with no comments has no threads to show it by, so the PR's summary
/// says it is there.
#[test]
fn the_summary_shows_a_pending_review_on_github_with_no_comments() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    let github = github_at_head(&ctx);
    *github.pending.borrow_mut() = Some("R1".to_owned());
    review_cli_with_github(&ctx.dir, &["fetch"], "", Rc::clone(&github)).unwrap();
    let mut app = init_app(&mut ctx);
    app.state.github = github;

    ctx.update(&mut app, keys("OO"));

    let screen = ctx.redact_buffer();
    assert!(
        screen.contains("pending review on GitHub with no comments"),
        "{screen}"
    );
    assert!(!screen.contains("won't post"), "{screen}");
}

/// Sync adds the draft comments to the user's pending review on GitHub, whoever started it.
#[test]
fn review_sync_adds_to_a_pending_review_it_did_not_start() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    let github = github_at_head(&ctx);
    *github.pending.borrow_mut() = Some("R1".to_owned());
    review_cli_with_input(
        &ctx.dir,
        &["comment", "--path", "lines.txt", "--line", "2"],
        "hello",
    )
    .unwrap();

    review_cli_with_github(&ctx.dir, &["sync"], "", Rc::clone(&github)).unwrap();

    assert_eq!(*github.reviews_started.borrow(), 0);
    assert_eq!(
        *github.posted.borrow(),
        vec![("R1 lines.txt:(New, 2)".to_owned(), "hello".to_owned())]
    );
}

/// With no draft comments and no PR on GitHub, there is nothing to sync.
#[test]
fn review_sync_with_nothing_to_post_and_no_github_pr_is_an_error() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    let github = github_at_head(&ctx);
    *github.has_pr.borrow_mut() = false;

    let result = review_cli_with_github(&ctx.dir, &["sync"], "", Rc::clone(&github));

    let error = result.unwrap_err().to_string();
    assert!(error.contains("Nothing to sync"), "{error}");
    assert!(github.created.borrow().is_empty());
}

/// A repository whose `main` is behind `origin/main`, with `feature` branched from
/// `origin/main` and pushed: `main` lacks `upstream.txt`, which is on `origin/main`, and
/// `feature` adds `feature.txt`.
fn setup_stale_main(ctx: &mut TestContext) {
    ctx.debug = false;
    commit(&ctx.dir, "upstream.txt", "upstream\n");
    run(&ctx.dir, &["git", "push", "origin", "main"]);
    run(&ctx.dir, &["git", "reset", "--hard", "HEAD~1"]);
    run(
        &ctx.dir,
        &["git", "checkout", "-b", "feature", "origin/main"],
    );
    commit(&ctx.dir, "feature.txt", "feature\n");
    run(&ctx.dir, &["git", "push", "-u", "origin", "feature"]);
}

/// The PR's base is where its base branch is on the remote when the local branch is behind it,
/// as it often is: the PR's diff has only the branch's own changes.
#[test]
fn oo_with_main_behind_origin_main_diffs_against_origin_main() {
    let mut ctx = setup_clone!();
    setup_stale_main(&mut ctx);
    let mut app = init_app(&mut ctx);

    ctx.update(&mut app, keys("OO"));

    let screen = ctx.redact_buffer();
    assert!(screen.contains("feature.txt"), "{screen}");
    assert!(!screen.contains("upstream.txt"), "{screen}");
}

/// When the local base branch and the remote's have each moved on, which the PR is into can't
/// be told, so opening it is an error.
#[test]
fn oo_with_main_and_origin_main_diverged_is_an_error() {
    let mut ctx = setup_clone!();
    setup_stale_main(&mut ctx);
    run(&ctx.dir, &["git", "checkout", "main"]);
    commit(&ctx.dir, "local-only.txt", "local\n");
    run(&ctx.dir, &["git", "checkout", "feature"]);
    let mut app = init_app(&mut ctx);

    ctx.update(&mut app, keys("OO"));

    let screen = ctx.redact_buffer();
    assert!(screen.contains("have diverged"), "{screen}");
    assert!(!screen.contains("added: feature.txt"), "{screen}");
}

/// When the branch has a PR on GitHub, the local PR must branch from its base where GitHub's
/// does, so that its diff is the GitHub PR's.
#[test]
fn oo_branched_from_elsewhere_than_on_github_is_an_error() {
    let mut ctx = setup_clone!();
    setup_stale_main(&mut ctx);
    let stale = run(&ctx.dir, &["git", "rev-parse", "main"]);
    let feature = run(&ctx.dir, &["git", "rev-parse", "feature"]);
    let mut app = init_app(&mut ctx);
    app.state.github = Rc::new(FakeGitHub::new(stale.trim(), feature.trim()));

    ctx.update(&mut app, keys("OO"));

    let screen = ctx.redact_buffer();
    assert!(screen.contains("on GitHub, so its diff"), "{screen}");
    assert!(!screen.contains("added: feature.txt"), "{screen}");
}

/// GitHub's base for a PR is where the base branch was when the PR was last updated, which the
/// base branch has moved on from since: the PR still branched from where GitHub has it, and opens.
#[test]
fn oo_when_main_has_moved_on_since_the_pr_was_updated_opens_it() {
    let mut ctx = setup_clone!();
    setup_stale_main(&mut ctx);
    let githubs_base = run(&ctx.dir, &["git", "rev-parse", "origin/main"]);
    let feature = run(&ctx.dir, &["git", "rev-parse", "feature"]);
    run(&ctx.dir, &["git", "checkout", "main"]);
    run(&ctx.dir, &["git", "reset", "--hard", "origin/main"]);
    commit(&ctx.dir, "later.txt", "later\n");
    run(&ctx.dir, &["git", "push", "origin", "main"]);
    run(&ctx.dir, &["git", "checkout", "feature"]);
    let github = FakeGitHub::new(githubs_base.trim(), feature.trim());
    *github.commits.borrow_mut() = 1;
    let mut app = init_app(&mut ctx);
    app.state.github = Rc::new(github);

    ctx.update(&mut app, keys("OO"));

    let screen = ctx.redact_buffer();
    assert!(screen.contains("added: feature.txt"), "{screen}");
    assert!(!screen.contains("later.txt"), "{screen}");
}

/// A base that doesn't have GitHub's base commit is behind GitHub's, and must be fetched.
#[test]
fn oo_with_a_base_behind_githubs_is_an_error() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    run(&ctx.dir, &["git", "checkout", "-b", "githubs-main", "main"]);
    commit(&ctx.dir, "newer.txt", "newer\n");
    let githubs_base = run(&ctx.dir, &["git", "rev-parse", "githubs-main"]);
    run(&ctx.dir, &["git", "checkout", "feature"]);
    let feature = run(&ctx.dir, &["git", "rev-parse", "feature"]);
    let mut app = init_app(&mut ctx);
    app.state.github = Rc::new(FakeGitHub::new(githubs_base.trim(), feature.trim()));

    ctx.update(&mut app, keys("OO"));

    let screen = ctx.redact_buffer();
    assert!(screen.contains("which main doesn't have"), "{screen}");
    assert!(!screen.contains("added: added.txt"), "{screen}");
}

/// When the branch is where GitHub has the PR's head, the local PR has the GitHub PR's commits,
/// as many as GitHub says.
#[test]
fn oo_with_a_commit_count_not_githubs_is_an_error() {
    let mut ctx = setup_clone!();
    setup_stale_main(&mut ctx);
    let base = run(&ctx.dir, &["git", "rev-parse", "origin/main"]);
    let feature = run(&ctx.dir, &["git", "rev-parse", "feature"]);
    let github = FakeGitHub::new(base.trim(), feature.trim());
    *github.commits.borrow_mut() = 26;
    let mut app = init_app(&mut ctx);
    app.state.github = Rc::new(github);

    ctx.update(&mut app, keys("OO"));

    let screen = ctx.redact_buffer();
    assert!(screen.contains("GitHub's PR has 26 commits"), "{screen}");
    assert!(!screen.contains("added: feature.txt"), "{screen}");
}

/// `setup`, with a commit on `feature` by another author.
fn setup_two_authors(ctx: &mut TestContext) {
    setup(ctx);
    fs::write(ctx.dir.join("other.txt"), "other\n").unwrap();
    run(&ctx.dir, &["git", "add", "other.txt"]);
    run(
        &ctx.dir,
        &[
            "git",
            "commit",
            "-m",
            "add other.txt",
            "--author",
            "Other Person <other@example.com>",
        ],
    );
}

/// A PR whose commits are by more than one author may have a wrong base, so its authors are
/// listed, and it is opened only when the user says to go on.
#[test]
fn oo_with_commits_by_several_authors_asks_before_opening() {
    let mut ctx = setup_clone!();
    setup_two_authors(&mut ctx);
    let mut app = init_app(&mut ctx);

    ctx.update(&mut app, keys("OO"));

    let prompt = ctx.prompt_line();
    assert!(prompt.contains("Author Name"), "{prompt}");
    assert!(prompt.contains("Other Person"), "{prompt}");

    let mut app = init_app(&mut ctx);
    ctx.update(&mut app, keys("OOn"));
    assert!(
        !ctx.redact_buffer().contains("added: other.txt"),
        "{}",
        ctx.redact_buffer()
    );

    let mut app = init_app(&mut ctx);
    ctx.update(&mut app, keys("OOy"));
    assert!(
        ctx.redact_buffer().contains("added: other.txt"),
        "{}",
        ctx.redact_buffer()
    );
}

/// A branch behind its remote's would be reviewed as it was, not as it is, so opening its PR
/// is an error.
#[test]
fn oo_on_a_branch_behind_its_remote_branch_is_an_error() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    run(&ctx.dir, &["git", "push", "-u", "origin", "feature"]);
    commit(&ctx.dir, "more.txt", "more\n");
    run(&ctx.dir, &["git", "push"]);
    run(&ctx.dir, &["git", "reset", "--hard", "HEAD~1"]);
    let mut app = init_app(&mut ctx);

    ctx.update(&mut app, keys("OO"));

    let screen = ctx.redact_buffer();
    assert!(
        screen.contains("origin/feature has 1 commit that feature doesn't"),
        "{screen}"
    );
    assert!(!screen.contains("added: added.txt"), "{screen}");
}

/// A branch with no commits since its base, as once it is merged, has nothing to review.
#[test]
fn oo_on_a_branch_with_no_commits_since_its_base_is_an_error() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    run(&ctx.dir, &["git", "checkout", "-b", "empty", "main"]);
    let mut app = init_app(&mut ctx);

    ctx.update(&mut app, keys("OO"));

    let screen = ctx.redact_buffer();
    assert!(
        screen.contains("No PR to review: empty has no commits since main"),
        "{screen}"
    );
}

/// Where the PR `feature` into `release` is recorded.
fn release_file_path(ctx: &TestContext) -> std::path::PathBuf {
    ctx.dir.join(".git/gitu/review/feature...release.json")
}

/// A comment on `feature`, as the review command line writes it, into the base `base` if given.
fn comment_with_base(ctx: &TestContext, base: Option<&str>) {
    let mut args = vec!["comment", "--path", "lines.txt", "--line", "2"];
    if let Some(base) = base {
        args.extend(["--base", base]);
    }
    review_cli_with_input(&ctx.dir, &args, "hello").unwrap();
}

/// Opening the branch's PR when GitHub has one links the local PR to it: the link is recorded,
/// so that the local PR stays that GitHub PR's.
#[test]
fn oo_records_the_github_pr_it_links_to() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    let github = github_at_head(&ctx);
    let mut app = init_app(&mut ctx);
    app.state.github = Rc::clone(&github) as Rc<dyn GitHub>;

    ctx.update(&mut app, keys("OO"));

    let file = review_file(&ctx);
    assert_eq!(file["github_pr"]["number"], 7, "{file}");
    assert_eq!(
        file["github_pr"]["url"], "https://github.com/o/r/pull/7",
        "{file}"
    );
}

/// A local PR made before its GitHub PR is linked to it once GitHub has it, keeping its drafts.
#[test]
fn oo_links_a_local_pr_made_before_its_github_pr() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    comment_with_base(&ctx, None);
    let github = github_at_head(&ctx);
    let mut app = init_app(&mut ctx);
    app.state.github = Rc::clone(&github) as Rc<dyn GitHub>;

    ctx.update(&mut app, keys("OO"));

    let file = review_file(&ctx);
    assert_eq!(file["github_pr"]["number"], 7, "{file}");
    assert!(
        ctx.redact_buffer().contains("hello"),
        "{}",
        ctx.redact_buffer()
    );
}

/// A local PR linked to a GitHub PR that GitHub no longer has open is not handled yet.
#[test]
fn oo_on_a_branch_whose_github_pr_is_no_longer_open_is_an_error() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    let github = github_at_head(&ctx);
    let mut app = init_app(&mut ctx);
    app.state.github = Rc::clone(&github) as Rc<dyn GitHub>;
    ctx.update(&mut app, keys("OO"));
    *github.has_pr.borrow_mut() = false;

    let mut app = init_app(&mut ctx);
    app.state.github = Rc::clone(&github) as Rc<dyn GitHub>;
    ctx.update(&mut app, keys("OO"));

    let screen = ctx.redact_buffer();
    assert!(
        screen.contains("linked to #7, which GitHub no longer"),
        "{screen}"
    );
}

/// A local PR linked to one GitHub PR, when GitHub's PR for the branch is another, is not
/// handled yet.
#[test]
fn oo_on_a_branch_linked_to_another_github_pr_is_an_error() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    let github = github_at_head(&ctx);
    let mut app = init_app(&mut ctx);
    app.state.github = Rc::clone(&github) as Rc<dyn GitHub>;
    ctx.update(&mut app, keys("OO"));
    let mut file = review_file(&ctx);
    file["github_pr"]["number"] = 8.into();
    fs::write(
        review_file_path(&ctx),
        serde_json::to_string_pretty(&file).unwrap(),
    )
    .unwrap();

    let mut app = init_app(&mut ctx);
    app.state.github = Rc::clone(&github) as Rc<dyn GitHub>;
    ctx.update(&mut app, keys("OO"));

    let screen = ctx.redact_buffer();
    assert!(
        screen.contains("linked to #8, but GitHub's PR for it is"),
        "{screen}"
    );
}

/// A local PR into another base than its GitHub PR's is not handled yet.
#[test]
fn oo_with_a_local_pr_into_another_base_than_githubs_is_an_error() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    run(&ctx.dir, &["git", "branch", "release", "main"]);
    comment_with_base(&ctx, Some("release"));
    let github = github_at_head(&ctx);
    let mut app = init_app(&mut ctx);
    app.state.github = Rc::clone(&github) as Rc<dyn GitHub>;

    ctx.update(&mut app, keys("OO"));

    let screen = ctx.redact_buffer();
    assert!(
        screen.contains("is into release, but GitHub's PR #7 is"),
        "{screen}"
    );
}

/// A branch with more than one local PR, into different bases, is not handled yet.
#[test]
fn oo_on_a_branch_with_two_local_prs_is_an_error() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    run(&ctx.dir, &["git", "branch", "release", "main"]);
    comment_with_base(&ctx, None);
    comment_with_base(&ctx, Some("release"));
    assert!(release_file_path(&ctx).exists());
    let mut app = init_app(&mut ctx);

    ctx.update(&mut app, keys("OO"));

    let screen = ctx.redact_buffer();
    assert!(
        screen.contains("feature has local PRs into main and release"),
        "{screen}"
    );
}

/// Without GitHub, the branch's local PR opens, saying it was not checked against GitHub.
#[test]
fn oo_without_github_opens_the_local_pr_and_says_github_was_not_checked() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    comment_with_base(&ctx, None);
    let github = FakeGitHub::new("", "");
    *github.fails.borrow_mut() = true;
    let mut app = init_app(&mut ctx);
    app.state.github = Rc::new(github);

    ctx.update(&mut app, keys("OO"));

    let screen = ctx.redact_buffer();
    assert!(screen.contains("hello"), "{screen}");
    assert!(screen.contains("Not checked against GitHub"), "{screen}");
}

/// Without GitHub, and with no local PR for the branch, its base can't be known.
#[test]
fn oo_without_github_or_a_local_pr_is_an_error() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    let github = FakeGitHub::new("", "");
    *github.fails.borrow_mut() = true;
    let mut app = init_app(&mut ctx);
    app.state.github = Rc::new(github);

    ctx.update(&mut app, keys("OO"));

    let screen = ctx.redact_buffer();
    assert!(
        screen.contains("Couldn't find feature's PR: there is none in this repository"),
        "{screen}"
    );
}

/// The status view shows the current branch's local PR, as one line: its base, whether it is on
/// GitHub, and how many commits and draft comments it has. Its commits are not listed: they are
/// in its log.
#[test]
fn the_status_view_shows_the_branchs_pr_as_a_summary() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    comment_with_base(&ctx, None);

    let mut app = init_app(&mut ctx);
    ctx.update(&mut app, keys(""));

    let header = line_with(&ctx, "PR into main");
    assert!(header.contains("not on GitHub"), "{header}");
    assert!(header.contains("2 commits"), "{header}");
    assert!(header.contains("1 draft comment"), "{header}");
    let screen = ctx.redact_buffer();
    let lines: Vec<&str> = screen.lines().collect();
    let at = lines
        .iter()
        .position(|line| line.contains("PR into main"))
        .unwrap();
    assert!(!lines[at + 1].contains("add added.txt"), "{screen}");
}

/// The PR's screen starts with the same summary of the PR.
#[test]
fn the_pr_screen_starts_with_a_summary_of_the_pr() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    comment_with_base(&ctx, None);
    let mut app = init_app(&mut ctx);

    ctx.update(&mut app, keys("OO"));

    let screen = ctx.redact_buffer();
    let first = screen.lines().next().unwrap();
    assert!(
        first.contains("PR into main · not on GitHub · 2 commits · 1 draft comment"),
        "{screen}"
    );
}

/// The summary atop the PR's screen is drawn in `pr-summary-style`, and followed by a blank line.
#[test]
fn the_pr_summary_is_drawn_in_its_style_and_followed_by_a_blank_line() {
    let ctx = setup_clone!();
    let mut ctx =
        super::delta_config::with_delta_config(ctx, "[delta]\n\tpr-summary-style = red\n");
    setup(&mut ctx);
    comment_with_base(&ctx, None);
    let mut app = init_app(&mut ctx);

    ctx.update(&mut app, keys("OO"));

    let screen = ctx.redact_buffer();
    let lines: Vec<&str> = screen.lines().collect();
    assert!(lines[0].contains("PR into main"), "{screen}");
    assert!(lines[1].trim_end_matches('|').trim().is_empty(), "{screen}");
    assert_eq!(
        super::pr::cell(&ctx, "PR into main", 'P').fg,
        crate::style::Color::Red
    );
}

/// On the PR's screen, the log is the PR's: its commits, as the user's log arguments show them,
/// on top of the PR's screen.
#[test]
fn ll_on_the_pr_screen_shows_the_prs_log() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    ctx.config().general.log_args = vec!["--format=logged %s".to_owned()];
    let mut app = init_app(&mut ctx);
    ctx.update(&mut app, keys("OO"));

    ctx.update(&mut app, keys("ll"));

    let screen = ctx.redact_buffer();
    assert!(screen.contains("logged add added.txt"), "{screen}");
    assert!(screen.contains("logged modify lines.txt"), "{screen}");
    assert!(!screen.contains("logged add lines.txt"), "{screen}");
    let screens = app.state.screens.len();

    ctx.update(&mut app, keys("q"));

    assert_eq!(app.state.screens.len(), screens - 1);
    assert!(
        ctx.redact_buffer().contains("added: added.txt"),
        "{}",
        ctx.redact_buffer()
    );
}

/// A local PR linked to a GitHub PR shows the GitHub PR's number in the status view.
#[test]
fn the_status_view_shows_the_github_pr_a_pr_is_linked_to() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    let github = github_at_head(&ctx);
    let mut app = init_app(&mut ctx);
    app.state.github = Rc::clone(&github) as Rc<dyn GitHub>;
    ctx.update(&mut app, keys("OO"));

    let mut app = init_app(&mut ctx);
    ctx.update(&mut app, keys(""));

    assert!(
        line_with(&ctx, "PR into main").contains("#7"),
        "{}",
        ctx.redact_buffer()
    );
}

/// `enter` on the status view's PR opens it.
#[test]
fn enter_on_the_status_views_pr_opens_it() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    comment_with_base(&ctx, None);
    let mut app = init_app(&mut ctx);

    ctx.update(&mut app, keys("/PR into<enter><enter>"));

    let screen = ctx.redact_buffer();
    assert!(screen.contains("added: added.txt"), "{screen}");
    assert!(screen.contains("hello"), "{screen}");
}

/// A branch with no local PR has no PR in the status view.
#[test]
fn the_status_view_shows_no_pr_for_a_branch_without_one() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    let mut app = init_app(&mut ctx);

    ctx.update(&mut app, keys(""));

    let screen = ctx.redact_buffer();
    assert!(!screen.contains("PR into"), "{screen}");
}

/// What can't be shown of the branch's PR in the status view is said there.
#[test]
fn the_status_view_says_when_a_branch_has_two_local_prs() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    run(&ctx.dir, &["git", "branch", "release", "main"]);
    comment_with_base(&ctx, None);
    comment_with_base(&ctx, Some("release"));
    let mut app = init_app(&mut ctx);

    ctx.update(&mut app, keys(""));

    let screen = ctx.redact_buffer();
    assert!(
        screen.contains("feature has local PRs into main and release"),
        "{screen}"
    );
}

#[test]
fn oo_without_gits_user_name_says_to_set_it() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    run(&ctx.dir, &["git", "config", "--unset", "user.name"]);
    let mut app = init_app(&mut ctx);

    ctx.update(&mut app, keys("OO"));

    let screen = ctx.redact_buffer();
    assert!(screen.contains("user.name"), "{screen}");
}

#[test]
fn k_deletes_an_agents_draft_comment() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    review_cli_with_input(
        &ctx.dir,
        &[
            "comment",
            "--path",
            "lines.txt",
            "--line",
            "2",
            "--agent",
            "agent",
        ],
        "hello",
    )
    .unwrap();
    let mut app = init_app(&mut ctx);
    ctx.update(&mut app, keys("OO/agent \\[<enter>"));

    ctx.update(&mut app, keys("Ky"));

    let screen = ctx.redact_buffer();
    assert!(screen.contains("Comment deleted"), "{screen}");
    assert!(!screen.contains("hello"), "{screen}");
}

#[test]
fn syncing_posts_an_agents_draft_comments_too() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    review_cli_with_input(
        &ctx.dir,
        &[
            "comment",
            "--path",
            "lines.txt",
            "--line",
            "2",
            "--agent",
            "agent",
        ],
        "hello",
    )
    .unwrap();
    review_cli_with_input(
        &ctx.dir,
        &[
            "comment",
            "--path",
            "lines.txt",
            "--line",
            "2",
            "--side",
            "old",
        ],
        "mine",
    )
    .unwrap();

    let github = github_at_head(&ctx);

    review_cli_with_github(&ctx.dir, &["sync"], "", Rc::clone(&github)).unwrap();

    let mut bodies: Vec<String> = github
        .posted
        .borrow()
        .iter()
        .map(|(_, body)| body.clone())
        .collect();
    bodies.sort();
    assert_eq!(bodies, ["hello", "mine"]);
}

/// A store written by a gitu that kept a draft review for each author has one draft review once
/// read: the other authors' comments are in it, labelled with their names.
#[test]
fn a_draft_review_by_another_author_in_an_old_store_becomes_part_of_mine() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    let mut app = init_app(&mut ctx);
    ctx.update(&mut app, keys("OO/new<enter>ihello<ctrl+s>"));
    let thread = review_file(&ctx)["threads"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    review_cli_with_input(&ctx.dir, &["reply", &thread], "Agreed.\n").unwrap();
    // As the gitu before labels wrote it: the reply in claude's own draft review.
    let mut old = review_file(&ctx);
    old["version"] = 1.into();
    let mut claudes = old["reviews"][0].clone();
    claudes["id"] = "claudes".into();
    claudes["author"] = "claude".into();
    old["reviews"].as_array_mut().unwrap().push(claudes);
    let reply = &mut old["threads"][0]["comments"][1];
    reply["review"] = "claudes".into();
    reply["author"] = "claude".into();
    let path = review_file_path(&ctx);
    fs::write(&path, serde_json::to_string_pretty(&old).unwrap()).unwrap();

    let json: serde_json::Value =
        serde_json::from_str(&review_cli(&ctx.dir, &["show", "--format", "json"])).unwrap();

    let reviews = json["reviews"].as_array().unwrap();
    assert_eq!(reviews.len(), 1, "{json}");
    assert_eq!(reviews[0]["author"], "CI", "{json}");
    let comments = json["threads"][0]["comments"].as_array().unwrap();
    assert_eq!(comments[1]["review"], reviews[0]["id"], "{json}");
    assert_eq!(comments[1]["author"], "CI", "{json}");
    assert_eq!(comments[1]["agent"], "claude", "{json}");
}

/// With `refresh_on_file_change`, an agent's reply written while the PR is open
/// shows without `g r`.
#[test]
fn an_agents_reply_shows_without_refreshing() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    ctx.config().general.refresh_on_file_change.enabled = true;
    let mut app = init_app(&mut ctx);
    ctx.update(&mut app, keys("OO/new<enter>ihello<ctrl+s>"));
    let thread = review_file(&ctx)["threads"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    // The watcher starts on a thread of its own.
    std::thread::sleep(std::time::Duration::from_millis(500));

    review_cli_with_input(
        &ctx.dir,
        &["reply", &thread, "--agent", "claude"],
        "Agreed.\n",
    )
    .unwrap();

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !ctx.redact_buffer().contains("Agreed.") {
        assert!(
            std::time::Instant::now() < deadline,
            "{}",
            ctx.redact_buffer()
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
        app.update(&mut ctx.term, None).unwrap();
    }
}
