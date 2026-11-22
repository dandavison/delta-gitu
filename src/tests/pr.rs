//! Reviewing a GitHub PR: `O` checks it out and shows its diff, each file with
//! its Viewed checkbox from GitHub; `x` ticks or unticks it, and `o` opens the
//! line on GitHub to comment on it. Review threads are shown under the lines
//! they are on.

use super::*;
use crate::{
    app::App,
    error::Error,
    github::{Comment, GitHub, Pr, Side, Thread, ViewedState},
};
use std::{cell::RefCell, collections::HashMap, process::Command, rc::Rc};

const PR_URL: &str = "https://github.com/o/r/pull/7";
const LINES_TXT_HASH: &str = "123372fddad5407e07b7b77b3add7cd9ed231f91e8a71470a3efca35c13d2128";
const ADDED_TXT_HASH: &str = "005338a261920beed70d358c421c16850b4a5fb5e6ff7e97466a4a1b4126264b";

/// GitHub as the tests see it: one PR, the branch `feature` into `main`,
/// recording what gitu asks of it.
struct FakeGitHub {
    base_oid: String,
    head_oid: String,
    threads: RefCell<Vec<Thread>>,
    viewed: RefCell<HashMap<String, ViewedState>>,
    asked_for: RefCell<Vec<Option<String>>>,
    fetched_base: RefCell<bool>,
    opened: RefCell<Vec<String>>,
}

impl GitHub for FakeGitHub {
    fn pr(&self, selector: Option<&str>) -> Result<Pr, Error> {
        self.asked_for.borrow_mut().push(selector.map(String::from));
        Ok(Pr {
            id: "PR_id".into(),
            number: 7,
            url: PR_URL.into(),
            base_ref: "main".into(),
            base_oid: self.base_oid.clone(),
            head_oid: self.head_oid.clone(),
        })
    }

    fn checkout(&self, _pr: &Pr) -> Command {
        let mut cmd = Command::new("git");
        cmd.args(["checkout", "feature"]);
        cmd
    }

    fn fetch_base(&self, _pr: &Pr) -> Result<Command, Error> {
        *self.fetched_base.borrow_mut() = true;
        let mut cmd = Command::new("git");
        cmd.args(["fetch", "origin", "main"]);
        Ok(cmd)
    }

    fn viewed(&self, _pr: &Pr) -> Result<HashMap<String, ViewedState>, Error> {
        Ok(self.viewed.borrow().clone())
    }

    fn threads(&self, _pr: &Pr) -> Result<Vec<Thread>, Error> {
        Ok(self.threads.borrow().clone())
    }

    fn set_viewed(&self, _pr: &Pr, path: &str, viewed: bool) -> Result<(), Error> {
        let state = if viewed {
            ViewedState::Viewed
        } else {
            ViewedState::Unviewed
        };
        self.viewed.borrow_mut().insert(path.to_owned(), state);
        Ok(())
    }

    fn open(&self, url: &str) -> Result<(), Error> {
        self.opened.borrow_mut().push(url.to_owned());
        Ok(())
    }
}

/// A repository with the branch `feature`, which changes `lines.txt` and adds
/// `added.txt`, and `main`, which has moved on since with `main-only.txt`.
fn setup(ctx: &TestContext) -> Rc<FakeGitHub> {
    commit(&ctx.dir, "lines.txt", "keep\nold\nkeep\n");
    run(&ctx.dir, &["git", "checkout", "-b", "feature"]);
    commit(&ctx.dir, "lines.txt", "keep\nnew\nkeep\n");
    commit(&ctx.dir, "added.txt", "added\n");
    run(&ctx.dir, &["git", "checkout", "main"]);
    commit(&ctx.dir, "main-only.txt", "main\n");
    Rc::new(FakeGitHub {
        base_oid: run(&ctx.dir, &["git", "rev-parse", "main"])
            .trim()
            .to_owned(),
        head_oid: run(&ctx.dir, &["git", "rev-parse", "feature"])
            .trim()
            .to_owned(),
        threads: RefCell::new(vec![]),
        viewed: RefCell::new(HashMap::from([
            ("lines.txt".to_owned(), ViewedState::Viewed),
            ("added.txt".to_owned(), ViewedState::Unviewed),
        ])),
        asked_for: RefCell::new(vec![]),
        fetched_base: RefCell::new(false),
        opened: RefCell::new(vec![]),
    })
}

fn init_app(ctx: &mut TestContext, github: &Rc<FakeGitHub>) -> App {
    ctx.debug = false;
    let mut app = ctx.init_app();
    app.state.github = Rc::clone(github) as Rc<dyn GitHub>;
    app
}

fn head_branch(ctx: &TestContext) -> String {
    run(&ctx.dir, &["git", "branch", "--show-current"])
        .trim()
        .to_owned()
}

fn two_hours_ago() -> chrono::DateTime<chrono::FixedOffset> {
    (chrono::Utc::now() - chrono::Duration::hours(2)).fixed_offset()
}

/// What delta's config sets the options gitu asks it for to, as
/// `(name, value)`, whatever the features.
fn with_options(ctx: &mut TestContext, options: &'static [(&'static str, &'static str)]) {
    ctx.config().general.diff_renderer.options =
        crate::config::RendererOptions::new(move |_, _, names| {
            names
                .iter()
                .map(|name| {
                    options
                        .iter()
                        .find(|(option, _)| option == name)
                        .map(|(_, value)| (*value).to_owned())
                })
                .collect()
        });
}

/// A thread on `lines.txt`, its comments given as `(author, body)`, each made
/// two hours ago.
fn thread(
    id: &str,
    line: Option<(Side, u32)>,
    resolved: bool,
    comments: &[(&str, &str)],
) -> Thread {
    Thread {
        id: id.to_owned(),
        path: "lines.txt".to_owned(),
        line,
        resolved,
        comments: comments
            .iter()
            .enumerate()
            .map(|(i, (author, body))| Comment {
                author: (*author).to_owned(),
                body: (*body).to_owned(),
                url: format!("{PR_URL}#discussion_{id}_{i}"),
                created_at: two_hours_ago(),
            })
            .collect(),
    }
}

/// The lines of the screen after the one that shows `text`, as many as
/// `count`.
fn lines_after(ctx: &TestContext, text: &str, count: usize) -> Vec<String> {
    let screen = ctx.redact_buffer();
    let lines: Vec<&str> = screen.lines().collect();
    let i = lines
        .iter()
        .position(|line| line.contains(text))
        .unwrap_or_else(|| panic!("no line with {text}:\n{screen}"));
    lines[i + 1..]
        .iter()
        .take(count)
        .map(|line| line.to_string())
        .collect()
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
fn opening_a_pr_by_number_checks_it_out_and_shows_its_diff() {
    let mut ctx = setup_clone!();
    let github = setup(&ctx);
    let mut app = init_app(&mut ctx, &github);

    ctx.update(&mut app, keys("Oo7<enter>"));

    assert_eq!(*github.asked_for.borrow(), vec![Some("7".to_owned())]);
    assert_eq!(head_branch(&ctx), "feature");
    let screen = ctx.redact_buffer();
    assert!(screen.contains("lines.txt"), "{screen}");
    assert!(screen.contains("added.txt"), "{screen}");
    assert!(!screen.contains("main-only.txt"), "{screen}");
}

#[test]
fn opening_the_current_branch_pr_asks_gh_for_it() {
    let mut ctx = setup_clone!();
    let github = setup(&ctx);
    let mut app = init_app(&mut ctx, &github);

    ctx.update(&mut app, keys("OO"));

    assert_eq!(*github.asked_for.borrow(), vec![None]);
    assert_eq!(head_branch(&ctx), "feature");
}

#[test]
fn uncommitted_changes_stop_the_checkout() {
    let mut ctx = setup_clone!();
    let github = setup(&ctx);
    fs::write(ctx.dir.join("lines.txt"), "changed\n").unwrap();
    let mut app = init_app(&mut ctx, &github);

    ctx.update(&mut app, keys("OO"));

    assert_eq!(head_branch(&ctx), "main");
    let screen = ctx.redact_buffer();
    assert!(
        screen.contains("Cannot check out PR: working tree has uncommitted changes"),
        "{screen}"
    );
}

#[test]
fn untracked_files_do_not_stop_the_checkout() {
    let mut ctx = setup_clone!();
    let github = setup(&ctx);
    fs::write(ctx.dir.join("untracked.txt"), "new\n").unwrap();
    let mut app = init_app(&mut ctx, &github);

    ctx.update(&mut app, keys("OO"));

    assert_eq!(head_branch(&ctx), "feature");
}

#[test]
fn a_base_commit_not_in_the_repository_is_fetched() {
    let mut ctx = setup_clone!();
    let github = setup(&ctx);
    run(&ctx.dir, &["git", "push", "origin", "main"]);
    // main moves on on GitHub, after the repository last fetched it.
    let other = ctx.dir.parent().unwrap().join("other");
    let url = Url::from_file_path(&ctx.remote_dir).unwrap().to_string();
    run(
        ctx.dir.parent().unwrap(),
        &["git", "clone", &url, other.to_str().unwrap()],
    );
    commit(&other, "later.txt", "later\n");
    run(&other, &["git", "push", "origin", "main"]);
    let github = Rc::new(FakeGitHub {
        base_oid: run(&other, &["git", "rev-parse", "HEAD"]).trim().to_owned(),
        ..Rc::into_inner(github).unwrap()
    });
    let mut app = init_app(&mut ctx, &github);

    ctx.update(&mut app, keys("OO"));

    assert!(*github.fetched_base.borrow());
    let screen = ctx.redact_buffer();
    assert!(screen.contains("lines.txt"), "{screen}");
    assert!(!screen.contains("later.txt"), "{screen}");
}

#[test]
fn each_file_shows_its_viewed_checkbox() {
    let mut ctx = setup_clone!();
    let github = setup(&ctx);
    let mut app = init_app(&mut ctx, &github);

    ctx.update(&mut app, keys("OO"));

    assert!(line_with(&ctx, "lines.txt").contains("[x]"));
    assert!(line_with(&ctx, "added.txt").contains("[ ]"));
}

#[test]
fn x_ticks_and_unticks_the_file_on_github() {
    let mut ctx = setup_clone!();
    let github = setup(&ctx);
    let mut app = init_app(&mut ctx, &github);
    ctx.update(&mut app, keys("OO/added.txt<enter>"));

    ctx.update(&mut app, keys("x"));
    assert_eq!(github.viewed.borrow()["added.txt"], ViewedState::Viewed);
    assert!(line_with(&ctx, "added.txt").contains("[x]"));

    ctx.update(&mut app, keys("x"));
    assert_eq!(github.viewed.borrow()["added.txt"], ViewedState::Unviewed);
    assert!(line_with(&ctx, "added.txt").contains("[ ]"));
}

#[test]
fn refresh_reads_the_checkboxes_again() {
    let mut ctx = setup_clone!();
    let github = setup(&ctx);
    let mut app = init_app(&mut ctx, &github);
    ctx.update(&mut app, keys("OO"));

    // Ticked in the browser.
    github
        .viewed
        .borrow_mut()
        .insert("added.txt".to_owned(), ViewedState::Viewed);
    ctx.update(&mut app, keys("gr"));

    assert!(line_with(&ctx, "added.txt").contains("[x]"));
}

#[test]
fn o_opens_an_added_line_on_github() {
    let mut ctx = setup_clone!();
    let github = setup(&ctx);
    let mut app = init_app(&mut ctx, &github);
    ctx.update(&mut app, keys("OO/new<enter>"));

    ctx.update(&mut app, keys("o"));

    assert_eq!(
        *github.opened.borrow(),
        vec![format!("{PR_URL}/files#diff-{LINES_TXT_HASH}R2")]
    );
}

#[test]
fn o_opens_a_removed_line_on_github() {
    let mut ctx = setup_clone!();
    let github = setup(&ctx);
    let mut app = init_app(&mut ctx, &github);
    ctx.update(&mut app, keys("OO/old<enter>"));

    ctx.update(&mut app, keys("o"));

    assert_eq!(
        *github.opened.borrow(),
        vec![format!("{PR_URL}/files#diff-{LINES_TXT_HASH}L2")]
    );
}

#[test]
fn o_on_a_file_opens_the_file_on_github() {
    let mut ctx = setup_clone!();
    let github = setup(&ctx);
    let mut app = init_app(&mut ctx, &github);
    ctx.update(&mut app, keys("OO/added.txt<enter>"));

    ctx.update(&mut app, keys("o"));

    assert_eq!(
        *github.opened.borrow(),
        vec![format!("{PR_URL}/files#diff-{ADDED_TXT_HASH}")]
    );
}

#[test]
fn x_outside_a_pr_says_how_to_open_one() {
    let mut ctx = setup_clone!();
    ctx.debug = false;
    commit(&ctx.dir, "file", "a\n");
    fs::write(ctx.dir.join("file"), "b\n").unwrap();
    let mut app = ctx.init_app_as_pager_of(&["git", "diff"]);

    ctx.update(&mut app, keys("x"));

    let screen = ctx.redact_buffer();
    assert!(screen.contains("No PR here: O opens one"), "{screen}");
}

#[test]
fn a_thread_is_shown_under_its_added_line() {
    let mut ctx = setup_clone!();
    let github = setup(&ctx);
    *github.threads.borrow_mut() = vec![thread(
        "t1",
        Some((Side::New, 2)),
        false,
        &[("alice", "Why new?")],
    )];
    let mut app = init_app(&mut ctx, &github);

    ctx.update(&mut app, keys("OO"));

    let after = lines_after(&ctx, "+new", 1);
    assert!(
        after[0].contains("alice: Why new?"),
        "{}",
        ctx.redact_buffer()
    );
    assert!(
        !ctx.redact_buffer().contains("not the PR's head"),
        "{}",
        ctx.redact_buffer()
    );
}

#[test]
fn a_thread_on_the_old_side_is_shown_under_its_removed_line() {
    let mut ctx = setup_clone!();
    let github = setup(&ctx);
    *github.threads.borrow_mut() = vec![thread(
        "t1",
        Some((Side::Old, 2)),
        false,
        &[("alice", "Why not old?")],
    )];
    let mut app = init_app(&mut ctx, &github);

    ctx.update(&mut app, keys("OO"));

    let after = lines_after(&ctx, "-old", 1);
    assert!(
        after[0].contains("alice: Why not old?"),
        "{}",
        ctx.redact_buffer()
    );
}

#[test]
fn replies_are_shown_under_the_first_comment() {
    let mut ctx = setup_clone!();
    let github = setup(&ctx);
    *github.threads.borrow_mut() = vec![thread(
        "t1",
        Some((Side::New, 2)),
        false,
        &[("alice", "Why new?"), ("bob", "Because\nreasons")],
    )];
    let mut app = init_app(&mut ctx, &github);

    ctx.update(&mut app, keys("OO"));

    let after = lines_after(&ctx, "+new", 3);
    assert!(
        after[0].contains("alice: Why new?"),
        "{}",
        ctx.redact_buffer()
    );
    assert!(after[1].contains("bob: Because"), "{}", ctx.redact_buffer());
    assert!(after[2].contains("reasons"), "{}", ctx.redact_buffer());
}

#[test]
fn a_resolved_thread_starts_folded() {
    let mut ctx = setup_clone!();
    let github = setup(&ctx);
    *github.threads.borrow_mut() = vec![thread(
        "t1",
        Some((Side::New, 2)),
        true,
        &[("alice", "Why new?"), ("bob", "Because")],
    )];
    let mut app = init_app(&mut ctx, &github);

    ctx.update(&mut app, keys("OO"));

    let header = line_with(&ctx, "alice: Why new?");
    assert!(header.contains("resolved"), "{header}");
    assert!(
        !ctx.redact_buffer().contains("bob: Because"),
        "{}",
        ctx.redact_buffer()
    );

    ctx.update(&mut app, keys("/alice<enter><tab>"));
    assert!(
        ctx.redact_buffer().contains("bob: Because"),
        "{}",
        ctx.redact_buffer()
    );
}

#[test]
fn an_outdated_thread_is_shown_under_its_file() {
    let mut ctx = setup_clone!();
    let github = setup(&ctx);
    *github.threads.borrow_mut() = vec![thread("t1", None, false, &[("alice", "Why new?")])];
    let mut app = init_app(&mut ctx, &github);

    ctx.update(&mut app, keys("OO"));

    let after = lines_after(&ctx, "lines.txt", 1);
    assert!(
        after[0].contains("alice: Why new?"),
        "{}",
        ctx.redact_buffer()
    );
    assert!(after[0].contains("outdated"), "{}", ctx.redact_buffer());
}

#[test]
fn o_on_a_comment_opens_it_on_github() {
    let mut ctx = setup_clone!();
    let github = setup(&ctx);
    *github.threads.borrow_mut() = vec![thread(
        "t1",
        Some((Side::New, 2)),
        false,
        &[("alice", "Why new?"), ("bob", "Because")],
    )];
    let mut app = init_app(&mut ctx, &github);
    ctx.update(&mut app, keys("OO"));

    ctx.update(&mut app, keys("/alice<enter>o/bob<enter>o"));

    assert_eq!(
        *github.opened.borrow(),
        vec![
            format!("{PR_URL}#discussion_t1_0"),
            format!("{PR_URL}#discussion_t1_1")
        ]
    );
}

#[test]
fn refresh_reads_the_threads_again() {
    let mut ctx = setup_clone!();
    let github = setup(&ctx);
    let mut app = init_app(&mut ctx, &github);
    ctx.update(&mut app, keys("OO"));

    *github.threads.borrow_mut() = vec![thread(
        "t1",
        Some((Side::New, 2)),
        false,
        &[("alice", "Why new?")],
    )];
    ctx.update(&mut app, keys("gr"));

    assert!(
        ctx.redact_buffer().contains("alice: Why new?"),
        "{}",
        ctx.redact_buffer()
    );
}

#[test]
fn a_head_that_is_not_the_prs_on_github_is_warned_of() {
    let mut ctx = setup_clone!();
    let github = setup(&ctx);
    let github = Rc::new(FakeGitHub {
        head_oid: "0".repeat(40),
        ..Rc::into_inner(github).unwrap()
    });
    let mut app = init_app(&mut ctx, &github);

    ctx.update(&mut app, keys("OO"));

    assert!(
        ctx.redact_buffer().contains("not the PR's head on GitHub"),
        "{}",
        ctx.redact_buffer()
    );
}

/// The column, counting from 0 at the screen's left edge, of the bar drawn
/// before the comment showing `text`.
fn bar_column(ctx: &TestContext, text: &str) -> usize {
    line_with(ctx, text).chars().position(|c| c == '│').unwrap()
}

fn side_by_side(ctx: &mut TestContext) {
    ctx.config().general.diff_renderer.enabled_features =
        crate::config::EnabledFeatures::new(|_| vec!["side-by-side".into()]);
}

/// Where delta's right panel starts in side-by-side: halfway across the width
/// delta is given, which is the 80 columns but for the two gitu keeps for the
/// cursor and those it keeps for a file's checkbox and thread count. The
/// screen's column 0 is the cursor's.
const RIGHT_PANEL: usize = 1 + (80 - 2 - crate::github::FILE_PREFIX_WIDTH as usize) / 2;

/// In side-by-side, delta draws the new file in the right panel.
#[test]
fn side_by_side_draws_a_new_side_thread_in_the_right_panel() {
    let mut ctx = setup_clone!();
    side_by_side(&mut ctx);
    let github = setup(&ctx);
    *github.threads.borrow_mut() = vec![thread(
        "t1",
        Some((Side::New, 2)),
        false,
        &[("alice", "Why new?")],
    )];
    let mut app = init_app(&mut ctx, &github);

    ctx.update(&mut app, keys("OO"));

    assert_eq!(bar_column(&ctx, "alice: Why new?"), RIGHT_PANEL + 2);
}

#[test]
fn side_by_side_draws_an_old_side_thread_in_the_left_panel() {
    let mut ctx = setup_clone!();
    side_by_side(&mut ctx);
    let github = setup(&ctx);
    *github.threads.borrow_mut() = vec![thread(
        "t1",
        Some((Side::Old, 2)),
        false,
        &[("alice", "Why not old?")],
    )];
    let mut app = init_app(&mut ctx, &github);

    ctx.update(&mut app, keys("OO"));

    assert_eq!(bar_column(&ctx, "alice: Why not old?"), 1 + 2);
}

/// A comment longer than the room for it wraps, each row after the first
/// under the bar, and in side-by-side within its panel.
#[test]
fn a_long_comment_wraps_within_its_panel() {
    let mut ctx = setup_clone!();
    side_by_side(&mut ctx);
    let github = setup(&ctx);
    *github.threads.borrow_mut() = vec![thread(
        "t1",
        Some((Side::New, 2)),
        false,
        &[(
            "alice",
            "one two three four five six seven eight nine ten eleven twelve",
        )],
    )];
    let mut app = init_app(&mut ctx, &github);

    ctx.update(&mut app, keys("OO"));

    let first = line_with(&ctx, "alice: one");
    let last = line_with(&ctx, "twelve");
    assert_ne!(first, last, "{}", ctx.redact_buffer());
    assert_eq!(bar_column(&ctx, "twelve"), RIGHT_PANEL + 2);
    let width = last.trim_end_matches(['|', ' ']).chars().count();
    assert!(width <= 80, "{last}");
}

#[test]
fn a_long_comment_wraps_under_the_bar() {
    let mut ctx = setup_clone!();
    let github = setup(&ctx);
    let words = (1..=30)
        .map(|i| format!("w{i}"))
        .collect::<Vec<_>>()
        .join(" ");
    *github.threads.borrow_mut() = vec![thread(
        "t1",
        Some((Side::New, 2)),
        false,
        &[("alice", &words)],
    )];
    let mut app = init_app(&mut ctx, &github);

    ctx.update(&mut app, keys("OO"));

    assert_ne!(line_with(&ctx, "alice: w1 "), line_with(&ctx, "w30"));
    assert_eq!(bar_column(&ctx, "w30"), 1 + 2);
}

/// The cell drawn for the first `c` on the screen's row that shows `text`.
fn cell(ctx: &TestContext, text: &str, c: char) -> crate::term::TestCell {
    let crate::term::TermBackend::Test { buffer, .. } = &ctx.term else {
        unreachable!();
    };
    let rows: Vec<_> = buffer.cells.chunks(buffer.width as usize).collect();
    let row = rows
        .iter()
        .find(|row| {
            row.iter()
                .map(|cell| cell.symbol.as_str())
                .collect::<String>()
                .contains(text)
        })
        .unwrap_or_else(|| panic!("no row with {text}:\n{}", ctx.redact_buffer()));
    row.iter()
        .find(|cell| cell.symbol == c.to_string())
        .unwrap_or_else(|| panic!("no {c} in the row with {text}"))
        .clone()
}

#[test]
fn comments_are_drawn_in_the_comment_styles() {
    let mut ctx = setup_clone!();
    with_options(
        &mut ctx,
        &[
            ("comment-style", "red"),
            ("comment-author-style", "green bold"),
            ("comment-timestamp-style", "yellow"),
            ("comment-decoration-style", "blue"),
        ],
    );
    let github = setup(&ctx);
    *github.threads.borrow_mut() = vec![thread(
        "t1",
        None,
        false,
        &[("alice", "Why new?"), ("bob", "Because")],
    )];
    let mut app = init_app(&mut ctx, &github);

    ctx.update(&mut app, keys("OO"));

    use crate::style::{Color, Modifier};
    assert_eq!(cell(&ctx, "alice: Why new?", '│').fg, Color::Blue);
    assert_eq!(cell(&ctx, "alice: Why new?", 'W').fg, Color::Red);
    let author = cell(&ctx, "alice: Why new?", 'a');
    assert_eq!(author.fg, Color::Green);
    assert!(author.modifier.contains(Modifier::BOLD));
    assert_eq!(
        cell(&ctx, "alice: Why new?", 'g').fg,
        Color::Yellow,
        "timestamp"
    );
    assert_eq!(
        cell(&ctx, "alice: Why new?", 'u').fg,
        Color::Blue,
        "outdated"
    );
    assert_eq!(cell(&ctx, "bob: Because", 'B').fg, Color::Red);
}

/// The comment options are delta's, so a delta feature can set them, and they
/// change as the features in force do.
#[test]
fn comment_styles_follow_the_features_in_force() {
    let mut ctx = setup_clone!();
    ctx.config().general.diff_renderer.feature_keys = [("my-theme".into(), 'e')].into();
    ctx.config().general.diff_renderer.options =
        crate::config::RendererOptions::new(|_, features, names| {
            let themed = features.is_some_and(|features| features.contains("my-theme"));
            names
                .iter()
                .map(|name| (themed && *name == "comment-style").then(|| "red".to_owned()))
                .collect()
        });
    let github = setup(&ctx);
    *github.threads.borrow_mut() = vec![thread("t1", None, false, &[("alice", "Why new?")])];
    let mut app = init_app(&mut ctx, &github);
    ctx.update(&mut app, keys("OO"));
    assert_ne!(
        cell(&ctx, "alice: Why new?", 'W').fg,
        crate::style::Color::Red
    );

    ctx.update(&mut app, keys("e"));

    assert_eq!(
        cell(&ctx, "alice: Why new?", 'W').fg,
        crate::style::Color::Red
    );
}

#[test]
fn a_comment_style_that_is_not_a_style_is_reported() {
    let mut ctx = setup_clone!();
    with_options(&mut ctx, &[("comment-style", "notacolor")]);
    let github = setup(&ctx);
    *github.threads.borrow_mut() = vec![thread("t1", None, false, &[("alice", "Why new?")])];
    let mut app = init_app(&mut ctx, &github);

    ctx.update(&mut app, keys("OO"));

    let screen = ctx.redact_buffer();
    assert!(
        screen.contains("comment-style") && screen.contains("notacolor"),
        "{screen}"
    );
    assert!(screen.contains("alice: Why new?"), "{screen}");
}

/// Two comments on the added line, drawn with `comment-decoration-style` set
/// to `decoration`.
fn with_decoration(mut ctx: TestContext, decoration: &'static str, resolved: bool) -> TestContext {
    let options: &'static [(&str, &str)] =
        Box::leak(Box::new([("comment-decoration-style", decoration)]));
    with_options(&mut ctx, options);
    let github = setup(&ctx);
    *github.threads.borrow_mut() = vec![thread(
        "t1",
        Some((Side::New, 2)),
        resolved,
        &[("alice", "Why new?"), ("bob", "Because")],
    )];
    let mut app = init_app(&mut ctx, &github);
    ctx.update(&mut app, keys("OO"));
    ctx
}

/// The line of the screen before the one that shows `text`.
fn line_before(ctx: &TestContext, text: &str) -> String {
    let screen = ctx.redact_buffer();
    let lines: Vec<&str> = screen.lines().collect();
    let i = lines.iter().position(|line| line.contains(text)).unwrap();
    lines[i - 1].to_owned()
}

#[test]
fn a_box_decoration_draws_a_box_around_the_thread() {
    let ctx = with_decoration(setup_clone!(), "blue box", false);

    let screen = ctx.redact_buffer();
    let top = line_before(&ctx, "alice: Why new?");
    assert!(top.contains('┌') && top.contains('┐'), "{screen}");
    let header = line_with(&ctx, "alice: Why new?");
    assert_eq!(header.matches('│').count(), 2, "{screen}");
    let bottom = &lines_after(&ctx, "bob: Because", 1)[0];
    assert!(bottom.contains('└') && bottom.contains('┘'), "{screen}");
}

#[test]
fn a_bold_box_decoration_is_drawn_heavy() {
    let ctx = with_decoration(setup_clone!(), "bold box", false);

    assert!(
        line_before(&ctx, "alice: Why new?").contains('┏'),
        "{}",
        ctx.redact_buffer()
    );
}

#[test]
fn a_folded_boxed_thread_shows_its_first_row_alone() {
    let ctx = with_decoration(setup_clone!(), "box", true);

    let screen = ctx.redact_buffer();
    assert!(
        !line_before(&ctx, "alice: Why new?").contains('┌'),
        "{screen}"
    );
    assert!(line_with(&ctx, "alice: Why new?").contains('│'), "{screen}");
    assert!(!screen.contains('└'), "{screen}");
}

#[test]
fn an_underline_decoration_draws_a_line_under_the_thread() {
    let ctx = with_decoration(setup_clone!(), "ul", false);

    let screen = ctx.redact_buffer();
    let under = &lines_after(&ctx, "bob: Because", 1)[0];
    assert!(under.contains("───"), "{screen}");
    assert!(
        !line_with(&ctx, "alice: Why new?").contains('│'),
        "{screen}"
    );
}

#[test]
fn an_overline_decoration_draws_a_line_over_the_thread() {
    let ctx = with_decoration(setup_clone!(), "ol", false);

    assert!(
        line_before(&ctx, "alice: Why new?").contains("───"),
        "{}",
        ctx.redact_buffer()
    );
}

#[test]
fn a_comments_time_is_shown_as_how_long_ago() {
    let mut ctx = setup_clone!();
    let github = setup(&ctx);
    *github.threads.borrow_mut() = vec![thread("t1", None, false, &[("alice", "Why new?")])];
    let mut app = init_app(&mut ctx, &github);

    ctx.update(&mut app, keys("OO"));

    assert!(
        line_with(&ctx, "alice: Why new?").contains("2 hours ago"),
        "{}",
        ctx.redact_buffer()
    );
}

#[test]
fn a_comments_time_is_shown_in_the_timestamp_format() {
    let mut ctx = setup_clone!();
    with_options(&mut ctx, &[("comment-timestamp-format", "%Y-%m-%d")]);
    let github = setup(&ctx);
    *github.threads.borrow_mut() = vec![thread("t1", None, false, &[("alice", "Why new?")])];
    let mut app = init_app(&mut ctx, &github);

    ctx.update(&mut app, keys("OO"));

    let date = two_hours_ago()
        .with_timezone(&chrono::Local)
        .format("%Y-%m-%d")
        .to_string();
    assert!(
        line_with(&ctx, "alice: Why new?").contains(&date),
        "{}",
        ctx.redact_buffer()
    );
}

/// [`setup`], with the files folded to their lines of a diffstat, as
/// `file-header = stat` and `start-folded` show them, and the PR adding
/// `big.txt`, whose line of the diffstat takes the whole width.
fn setup_folded_to_stat(ctx: &mut TestContext) -> Rc<FakeGitHub> {
    ctx.config().general.file_header = crate::config::FileHeader::Stat;
    ctx.config().general.start_folded = true;
    let github = setup(ctx);
    run(&ctx.dir, &["git", "checkout", "feature"]);
    let lines = (1..=200).map(|i| format!("{i}\n")).collect::<String>();
    commit(&ctx.dir, "big.txt", &lines);
    run(&ctx.dir, &["git", "checkout", "main"]);
    Rc::new(FakeGitHub {
        head_oid: run(&ctx.dir, &["git", "rev-parse", "feature"])
            .trim()
            .to_owned(),
        ..Rc::into_inner(github).unwrap()
    })
}

#[test]
fn a_files_line_of_the_diffstat_fits_beside_its_checkbox() {
    let mut ctx = setup_clone!();
    let github = setup_folded_to_stat(&mut ctx);
    let mut app = init_app(&mut ctx, &github);

    ctx.update(&mut app, keys("OO"));

    let after = &lines_after(&ctx, "big.txt", 1)[0];
    assert!(
        after.contains("lines.txt"),
        "big.txt's row runs onto the next:\n{}",
        ctx.redact_buffer()
    );
}

#[test]
fn a_file_with_unresolved_threads_shows_how_many_comments_they_have() {
    let mut ctx = setup_clone!();
    let github = setup_folded_to_stat(&mut ctx);
    *github.threads.borrow_mut() = vec![
        thread(
            "t1",
            Some((Side::New, 2)),
            false,
            &[("alice", "Why?"), ("bob", "So")],
        ),
        thread("t2", None, false, &[("bob", "Hm")]),
        thread("t3", None, true, &[("carol", "Done")]),
    ];
    let mut app = init_app(&mut ctx, &github);

    ctx.update(&mut app, keys("OO"));

    let row = line_with(&ctx, "lines.txt");
    let name = row.find("lines.txt").unwrap();
    let count = row.find("[3 comments]").unwrap_or_else(|| panic!("{row}"));
    assert!(name < count, "{row}");
    assert!(
        !line_with(&ctx, "added.txt").contains("comment"),
        "{}",
        ctx.redact_buffer()
    );
}

#[test]
fn one_comment_is_counted_as_one_comment() {
    let mut ctx = setup_clone!();
    let github = setup_folded_to_stat(&mut ctx);
    *github.threads.borrow_mut() = vec![thread("t1", None, false, &[("bob", "Hm")])];
    let mut app = init_app(&mut ctx, &github);

    ctx.update(&mut app, keys("OO"));

    assert!(
        line_with(&ctx, "lines.txt").contains("[1 comment]"),
        "{}",
        ctx.redact_buffer()
    );
}

#[test]
fn a_files_comment_count_fits_on_its_row() {
    let mut ctx = setup_clone!();
    let github = setup_folded_to_stat(&mut ctx);
    *github.threads.borrow_mut() = vec![Thread {
        path: "big.txt".into(),
        ..thread("t1", None, false, &[("bob", "Hm")])
    }];
    let mut app = init_app(&mut ctx, &github);

    ctx.update(&mut app, keys("OO"));

    assert!(
        line_with(&ctx, "big.txt").contains("[1 comment]"),
        "{}",
        ctx.redact_buffer()
    );
    assert!(
        lines_after(&ctx, "big.txt", 1)[0].contains("lines.txt"),
        "big.txt's row runs onto the next:\n{}",
        ctx.redact_buffer()
    );
}

#[test]
fn a_file_whose_threads_are_all_resolved_shows_no_count() {
    let mut ctx = setup_clone!();
    let github = setup_folded_to_stat(&mut ctx);
    *github.threads.borrow_mut() = vec![thread("t1", None, true, &[("carol", "Done")])];
    let mut app = init_app(&mut ctx, &github);

    ctx.update(&mut app, keys("OO"));

    assert!(
        !line_with(&ctx, "lines.txt").contains("comment"),
        "{}",
        ctx.redact_buffer()
    );
}

#[test]
fn a_files_comment_count_is_drawn_in_the_comment_count_style() {
    let mut ctx = setup_clone!();
    with_options(&mut ctx, &[("comment-count-style", "magenta")]);
    let github = setup_folded_to_stat(&mut ctx);
    *github.threads.borrow_mut() = vec![thread("t1", None, false, &[("bob", "Hm")])];
    let mut app = init_app(&mut ctx, &github);

    ctx.update(&mut app, keys("OO"));

    // The checkbox's [ comes first in the row; the c of "comment" is the first c.
    assert_eq!(
        cell(&ctx, "lines.txt", 'c').fg,
        crate::style::Color::Magenta
    );
}
