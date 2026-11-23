//! Reviewing a GitHub PR: `O` checks it out and shows its diff, each file with
//! its Viewed checkbox from GitHub; `x` ticks or unticks it, and `o` opens the
//! line on GitHub to comment on it. Review threads are shown under the lines
//! they are on.

use super::*;
use crate::{
    app::App,
    error::Error,
    review::{
        Comment, Side, Thread, ViewedState,
        github::{GitHub, GitHubPr},
    },
};
use std::{cell::RefCell, collections::HashMap, process::Command, rc::Rc};

const PR_URL: &str = "https://github.com/o/r/pull/7";
const LINES_TXT_HASH: &str = "123372fddad5407e07b7b77b3add7cd9ed231f91e8a71470a3efca35c13d2128";
const ADDED_TXT_HASH: &str = "005338a261920beed70d358c421c16850b4a5fb5e6ff7e97466a4a1b4126264b";

/// GitHub as the tests see it: one PR, the branch `feature` into `main`,
/// recording what gitu asks of it.
pub(super) struct FakeGitHub {
    pub(super) base_oid: String,
    pub(super) head_oid: String,
    /// The branch the PR is into.
    pub(super) base_ref: RefCell<String>,
    pub(super) threads: RefCell<Vec<Thread>>,
    pub(super) viewed: RefCell<HashMap<String, ViewedState>>,
    pub(super) asked_for: RefCell<Vec<Option<String>>>,
    pub(super) opened: RefCell<Vec<String>>,
    /// The comments posted, as (where, body): a thread's id, or a path and line.
    pub(super) posted: RefCell<Vec<(String, String)>>,
    /// The id of the user's pending review, if they have one.
    pub(super) pending: RefCell<Option<String>>,
    /// How many pending reviews gitu has started.
    pub(super) reviews_started: RefCell<usize>,
    /// The comments edited, as (id, body), and deleted, by id.
    pub(super) edited: RefCell<Vec<(String, String)>>,
    pub(super) deleted: RefCell<Vec<String>>,
    /// Whether the branch has a PR on GitHub.
    pub(super) has_pr: RefCell<bool>,
    /// The PRs created, as (head, base).
    pub(super) created: RefCell<Vec<(String, String)>>,
    /// The default branch of the repository on GitHub.
    pub(super) default_branch: RefCell<String>,
    /// Whether gh fails, as without the network.
    pub(super) fails: RefCell<bool>,
    /// Whether GitHub refuses to add threads, as for a line it can't place in the PR's diff.
    pub(super) refuses_threads: RefCell<bool>,
    /// The ids of comments that are in no review GitHub lists.
    pub(super) unreviewed: RefCell<Vec<String>>,
}

impl GitHub for FakeGitHub {
    fn pr(&self, selector: Option<&str>) -> Result<GitHubPr, Error> {
        self.asked_for.borrow_mut().push(selector.map(String::from));
        Ok(GitHubPr {
            id: "PR_id".into(),
            number: 7,
            url: PR_URL.into(),
            base_ref: self.base_ref.borrow().clone(),
            base_oid: self.base_oid.clone(),
            head_oid: self.head_oid.clone(),
        })
    }

    fn checkout(&self, _pr: &GitHubPr) -> Command {
        let mut cmd = Command::new("git");
        cmd.args(["checkout", "feature"]);
        cmd
    }

    fn viewed(&self, _pr: &GitHubPr) -> Result<HashMap<String, ViewedState>, Error> {
        Ok(self.viewed.borrow().clone())
    }

    /// One review for the comments in my pending review, and one for the rest. A test may set
    /// up a comment in my pending review without saying which review that is: it is R0.
    fn reviews(&self, _pr: &GitHubPr) -> Result<Vec<crate::review::github::GitHubReview>, Error> {
        use crate::review::github::{GitHubReview, GitHubReviewState};
        let comments = |draft: bool| -> Vec<String> {
            self.threads
                .borrow()
                .iter()
                .flat_map(|t| &t.comments)
                .filter(|c| c.draft == draft)
                .filter(|c| !self.unreviewed.borrow().contains(&c.id))
                .map(|c| c.id.clone())
                .collect()
        };
        let mut reviews = vec![GitHubReview {
            id: "submitted".into(),
            author: "alice".into(),
            state: GitHubReviewState::Submitted,
            comments: comments(false),
            mine: false,
        }];
        let pending = self.pending.borrow().clone();
        if pending.is_some() || !comments(true).is_empty() {
            reviews.push(GitHubReview {
                id: pending.unwrap_or_else(|| "R0".to_owned()),
                author: "me".into(),
                state: GitHubReviewState::Pending,
                comments: comments(true),
                mine: true,
            });
        }
        Ok(reviews)
    }

    fn threads(&self, _pr: &GitHubPr) -> Result<Vec<Thread>, Error> {
        Ok(self.threads.borrow().clone())
    }

    fn default_branch(&self) -> Result<String, Error> {
        if *self.fails.borrow() {
            return Err(Error::Gh("error connecting to api.github.com".to_owned()));
        }
        Ok(self.default_branch.borrow().clone())
    }

    fn pr_for_branch(&self, _head: &str) -> Result<Option<GitHubPr>, Error> {
        if *self.fails.borrow() {
            return Err(Error::Gh("error connecting to api.github.com".to_owned()));
        }
        if !*self.has_pr.borrow() {
            return Ok(None);
        }
        self.pr(None).map(Some)
    }

    fn create_pr(&self, head: &str, base: &str) -> Result<GitHubPr, Error> {
        self.created
            .borrow_mut()
            .push((head.to_owned(), base.to_owned()));
        *self.has_pr.borrow_mut() = true;
        self.pr(None)
    }

    fn pending_review(&self, _pr: &GitHubPr) -> Result<Option<String>, Error> {
        Ok(self.pending.borrow().clone())
    }

    fn start_review(&self, _pr: &GitHubPr) -> Result<String, Error> {
        *self.reviews_started.borrow_mut() += 1;
        let id = format!("R{}", self.reviews_started.borrow());
        *self.pending.borrow_mut() = Some(id.clone());
        Ok(id)
    }

    fn add_thread(
        &self,
        review_id: &str,
        path: &str,
        line: (Side, u32),
        body: &str,
    ) -> Result<String, Error> {
        if *self.refuses_threads.borrow() {
            return Err(Error::Gh(format!(
                "Couldn't add the thread on {path}:{}: GitHub did nothing",
                line.1
            )));
        }
        self.posted
            .borrow_mut()
            .push((format!("{review_id} {path}:{line:?}"), body.to_owned()));
        let id = format!("new{}", self.threads.borrow().len());
        let mut new = Thread {
            path: path.to_owned(),
            ..thread(&id, Some(line), false, &[("me", body)])
        };
        new.comments[0].draft = true;
        new.comments[0].editable = true;
        new.comments[0].deletable = true;
        let comment_id = new.comments[0].id.clone();
        self.threads.borrow_mut().push(new);
        Ok(comment_id)
    }

    fn add_reply(&self, review_id: &str, comment_id: &str, body: &str) -> Result<String, Error> {
        self.posted
            .borrow_mut()
            .push((format!("{review_id} {comment_id}"), body.to_owned()));
        let mut threads = self.threads.borrow_mut();
        let thread = threads
            .iter_mut()
            .find(|thread| thread.comments[0].id == comment_id)
            .unwrap();
        thread.comments.push(Comment {
            id: "reply".into(),
            author: "me".into(),
            body: body.to_owned(),
            url: Some(format!("{PR_URL}#reply")),
            created_at: two_hours_ago(),
            draft: true,
            editable: true,
            deletable: true,
            agent: None,
        });
        Ok("reply".to_owned())
    }

    fn edit_comment(&self, comment_id: &str, body: &str) -> Result<(), Error> {
        self.edited
            .borrow_mut()
            .push((comment_id.to_owned(), body.to_owned()));
        for thread in self.threads.borrow_mut().iter_mut() {
            for comment in &mut thread.comments {
                if comment.id == comment_id {
                    comment.body = body.to_owned();
                }
            }
        }
        Ok(())
    }

    fn delete_comment(&self, comment_id: &str) -> Result<(), Error> {
        self.deleted.borrow_mut().push(comment_id.to_owned());
        let mut threads = self.threads.borrow_mut();
        for thread in threads.iter_mut() {
            thread.comments.retain(|comment| comment.id != comment_id);
        }
        threads.retain(|thread| !thread.comments.is_empty());
        Ok(())
    }

    fn set_viewed(&self, _pr: &GitHubPr, path: &str, viewed: bool) -> Result<(), Error> {
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
    Rc::new(FakeGitHub::new(
        run(&ctx.dir, &["git", "rev-parse", "main"]).trim(),
        run(&ctx.dir, &["git", "rev-parse", "feature"]).trim(),
    ))
}

impl FakeGitHub {
    /// GitHub with one PR, whose base and head are at `base_oid` and `head_oid`, with no
    /// threads, `lines.txt` ticked and `added.txt` not.
    pub(super) fn new(base_oid: &str, head_oid: &str) -> Self {
        FakeGitHub {
            base_oid: base_oid.to_owned(),
            head_oid: head_oid.to_owned(),
            base_ref: RefCell::new("main".to_owned()),
            threads: RefCell::new(vec![]),
            viewed: RefCell::new(HashMap::from([
                ("lines.txt".to_owned(), ViewedState::Viewed),
                ("added.txt".to_owned(), ViewedState::Unviewed),
            ])),
            asked_for: RefCell::new(vec![]),
            opened: RefCell::new(vec![]),
            posted: RefCell::new(vec![]),
            pending: RefCell::new(None),
            reviews_started: RefCell::new(0),
            edited: RefCell::new(vec![]),
            deleted: RefCell::new(vec![]),
            has_pr: RefCell::new(true),
            created: RefCell::new(vec![]),
            default_branch: RefCell::new("main".to_owned()),
            fails: RefCell::new(false),
            refuses_threads: RefCell::new(false),
            unreviewed: RefCell::new(vec![]),
        }
    }
}

/// A thread on a file GitHub doesn't have in the PR, as one added by a commit not pushed, is
/// shown under the file, and not said to be not shown.
#[test]
fn a_thread_on_a_file_not_pushed_is_shown_and_not_said_not_to_be() {
    let mut ctx = setup_clone!();
    let github = setup(&ctx);
    run(&ctx.dir, &["git", "checkout", "feature"]);
    commit(&ctx.dir, "local.txt", "local\n");
    *github.threads.borrow_mut() = vec![Thread {
        path: "local.txt".into(),
        ..thread("t1", None, false, &[("bob", "Hm")])
    }];
    let mut app = init_app(&mut ctx, &github);

    ctx.update(&mut app, keys("OO"));
    let top = ctx.redact_buffer();
    ctx.update(&mut app, keys("/local.txt<enter>"));

    assert!(!top.contains("not shown"), "{top}");
    let screen = ctx.redact_buffer();
    assert!(screen.contains("Hm"), "{screen}");
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

/// The first row of a comment by alice or bob, as [`thread`] makes them.
const ALICE: &str = "alice [2 hours ago]";
const BOB: &str = "bob [2 hours ago]";

pub(super) fn two_hours_ago() -> chrono::DateTime<chrono::FixedOffset> {
    (chrono::Utc::now() - chrono::Duration::hours(2)).fixed_offset()
}

/// What delta's config sets the options gitu asks it for to, as
/// `(name, value)`, whatever the features.
fn with_options(ctx: &mut TestContext, options: &'static [(&'static str, &'static str)]) {
    ctx.config().general.diff_renderer.options = crate::config::RendererOptions::new(
        move |_, _, names| {
            names
                .iter()
                .map(|name| {
                    options
                        .iter()
                        .find(|(option, _)| option == name)
                        .map(|(_, value)| (*value).to_owned())
                })
                .collect()
        },
        |_, _, names| vec![vec![]; names.len()],
    );
}

/// A thread on `lines.txt`, its comments given as `(author, body)`, each made
/// two hours ago.
pub(super) fn thread(
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
                id: format!("{id}_{i}"),
                author: (*author).to_owned(),
                body: (*body).to_owned(),
                url: Some(format!("{PR_URL}#discussion_{id}_{i}")),
                created_at: two_hours_ago(),
                draft: false,
                editable: false,
                deletable: false,
                agent: None,
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
fn a_base_commit_not_in_the_repository_is_an_error() {
    let mut ctx = setup_clone!();
    let github = setup(&ctx);
    run(&ctx.dir, &["git", "push", "origin", "main"]);
    // main moves on on GitHub, after the repository last fetched it.
    let other = ctx.dir.parent().unwrap().join("other");
    let url = Url::from_file_path(&ctx.remote_dir).unwrap().to_string();
    // Relative to where it runs: on Windows the absolute path has a `\\?\` prefix git cannot use.
    run(ctx.dir.parent().unwrap(), &["git", "clone", &url, "other"]);
    commit(&other, "later.txt", "later\n");
    run(&other, &["git", "push", "origin", "main"]);
    let github = Rc::new(FakeGitHub {
        base_oid: run(&other, &["git", "rev-parse", "HEAD"]).trim().to_owned(),
        ..Rc::into_inner(github).unwrap()
    });
    let mut app = init_app(&mut ctx, &github);

    ctx.update(&mut app, keys("OO"));

    let screen = ctx.redact_buffer();
    assert!(
        screen.contains("is not in the repository: fetch main"),
        "{screen}"
    );
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

    let after = lines_after(&ctx, "+new", 2);
    assert!(after[1].contains("Why new?"), "{}", ctx.redact_buffer());
    assert!(after[0].contains(ALICE), "{}", ctx.redact_buffer());
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

    let after = lines_after(&ctx, "-old", 2);
    assert!(after[1].contains("Why not old?"), "{}", ctx.redact_buffer());
    assert!(after[0].contains(ALICE), "{}", ctx.redact_buffer());
}

#[test]
fn replies_are_shown_under_the_first_comment_after_a_blank_row() {
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

    let after = lines_after(&ctx, "+new", 6);
    let screen = ctx.redact_buffer();
    assert!(after[0].contains(ALICE), "{screen}");
    assert!(after[1].contains("Why new?"), "{screen}");
    // A blank row in the thread's box or bar, between one comment and the next.
    let blank = after[2].trim_end_matches('|').trim_end();
    assert!(blank.chars().all(|c| c == ' ' || c == '│'), "{screen}");
    assert!(after[3].contains(BOB), "{screen}");
    assert!(after[4].contains("Because"), "{screen}");
    assert!(after[5].contains("reasons"), "{screen}");
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

    let header = line_with(&ctx, ALICE);
    assert!(header.contains("resolved"), "{header}");
    assert!(
        !ctx.redact_buffer().contains("Because"),
        "{}",
        ctx.redact_buffer()
    );

    ctx.update(&mut app, keys("/alice<enter><tab>"));
    assert!(
        ctx.redact_buffer().contains("Because"),
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
    assert!(after[0].contains(ALICE), "{}", ctx.redact_buffer());
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
        ctx.redact_buffer().contains(ALICE),
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
/// cursor and those it keeps for a file's checkbox. The screen's column 0 is
/// the cursor's.
const RIGHT_PANEL: usize = 1 + (80 - 2 - crate::review::FILE_PREFIX_WIDTH as usize) / 2;

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

    assert_eq!(bar_column(&ctx, ALICE), RIGHT_PANEL + 2);
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

    assert_eq!(bar_column(&ctx, ALICE), 1 + 2);
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

    let first = line_with(&ctx, "one two");
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

    assert_ne!(line_with(&ctx, "w1 w2"), line_with(&ctx, "w30"));
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
    assert_eq!(cell(&ctx, ALICE, '│').fg, Color::Blue);
    assert_eq!(cell(&ctx, "Why new?", 'W').fg, Color::Red);
    let author = cell(&ctx, ALICE, 'a');
    assert_eq!(author.fg, Color::Green);
    assert!(author.modifier.contains(Modifier::BOLD));
    assert_eq!(cell(&ctx, ALICE, 'g').fg, Color::Yellow, "timestamp");
    assert_eq!(cell(&ctx, ALICE, 'd').fg, Color::Blue, "outdated");
    assert_eq!(cell(&ctx, "Because", 'B').fg, Color::Red);
}

/// The comment options are delta's, so a delta feature can set them, and they
/// change as the features in force do.
#[test]
fn comment_styles_follow_the_features_in_force() {
    let mut ctx = setup_clone!();
    ctx.config().general.diff_renderer.feature_keys = [("my-theme".into(), 'e')].into();
    ctx.config().general.diff_renderer.options = crate::config::RendererOptions::new(
        |_, features, names| {
            let themed = features.is_some_and(|features| features.contains("my-theme"));
            names
                .iter()
                .map(|name| (themed && *name == "comment-style").then(|| "red".to_owned()))
                .collect()
        },
        |_, _, names| vec![vec![]; names.len()],
    );
    let github = setup(&ctx);
    *github.threads.borrow_mut() = vec![thread("t1", None, false, &[("alice", "Why new?")])];
    let mut app = init_app(&mut ctx, &github);
    ctx.update(&mut app, keys("OO"));
    assert_ne!(cell(&ctx, "Why new?", 'W').fg, crate::style::Color::Red);

    ctx.update(&mut app, keys("e"));

    assert_eq!(cell(&ctx, "Why new?", 'W').fg, crate::style::Color::Red);
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
    assert!(screen.contains("Why new?"), "{screen}");
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
    let top = line_before(&ctx, ALICE);
    assert!(top.contains('┌') && top.contains('┐'), "{screen}");
    let header = line_with(&ctx, ALICE);
    assert_eq!(header.matches('│').count(), 2, "{screen}");
    let bottom = &lines_after(&ctx, "Because", 1)[0];
    assert!(bottom.contains('└') && bottom.contains('┘'), "{screen}");
}

#[test]
fn a_bold_box_decoration_is_drawn_heavy() {
    let ctx = with_decoration(setup_clone!(), "bold box", false);

    assert!(
        line_before(&ctx, ALICE).contains('┏'),
        "{}",
        ctx.redact_buffer()
    );
}

#[test]
/// Folded, a boxed thread is its first row in a closed box.
fn a_folded_boxed_thread_is_its_first_row_in_a_box() {
    let ctx = with_decoration(setup_clone!(), "box", true);

    let screen = ctx.redact_buffer();
    assert!(line_before(&ctx, ALICE).contains('┌'), "{screen}");
    assert!(line_with(&ctx, ALICE).contains('│'), "{screen}");
    assert!(lines_after(&ctx, ALICE, 1)[0].contains('└'), "{screen}");
    assert!(!screen.contains("Because"), "{screen}");
}

/// Folded, a thread between lines keeps both lines.
#[test]
fn a_folded_thread_keeps_its_lines_over_and_under_it() {
    let ctx = with_decoration(setup_clone!(), "ul ol", true);

    let screen = ctx.redact_buffer();
    assert!(line_before(&ctx, ALICE).contains("───"), "{screen}");
    assert!(lines_after(&ctx, ALICE, 1)[0].contains("───"), "{screen}");
}

#[test]
fn an_underline_decoration_draws_a_line_under_the_thread() {
    let ctx = with_decoration(setup_clone!(), "ul", false);

    let screen = ctx.redact_buffer();
    let under = &lines_after(&ctx, "Because", 1)[0];
    assert!(under.contains("───"), "{screen}");
    assert!(!line_with(&ctx, ALICE).contains('│'), "{screen}");
}

#[test]
fn an_overline_decoration_draws_a_line_over_the_thread() {
    let ctx = with_decoration(setup_clone!(), "ol", false);

    assert!(
        line_before(&ctx, ALICE).contains("───"),
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
        line_with(&ctx, ALICE).contains("2 hours ago"),
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
        line_with(&ctx, "alice [").contains(&format!("[{date}]")),
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
    // As on GitHub, every file of the PR has a Viewed state.
    github
        .viewed
        .borrow_mut()
        .insert("big.txt".to_owned(), ViewedState::Unviewed);
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

/// On a file's line of the diffstat the count goes straight after its name,
/// and the files' `|`s stay in a column.
#[test]
fn a_files_comment_count_goes_after_its_name_in_the_diffstat() {
    let mut ctx = setup_clone!();
    let github = setup_folded_to_stat(&mut ctx);
    *github.threads.borrow_mut() = vec![thread("t1", None, false, &[("bob", "Hm")])];
    let mut app = init_app(&mut ctx, &github);

    ctx.update(&mut app, keys("OO"));

    let row = line_with(&ctx, "lines.txt");
    assert!(row.contains("lines.txt [1 comment]"), "{row}");
    let bar = |text: &str| line_with(&ctx, text).chars().position(|c| c == '|');
    assert_eq!(
        bar("lines.txt"),
        bar("added.txt"),
        "{}",
        ctx.redact_buffer()
    );
    assert_eq!(bar("big.txt"), bar("added.txt"), "{}", ctx.redact_buffer());
}

#[test]
fn a_comments_author_and_time_head_its_text() {
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

    let header = line_with(&ctx, ALICE);
    assert!(!header.contains("Why"), "{}", ctx.redact_buffer());
    assert!(
        lines_after(&ctx, ALICE, 1)[0].contains("Why new?"),
        "{}",
        ctx.redact_buffer()
    );
}

/// The cell at `column` of the screen's row that shows `text`.
fn cell_at(ctx: &TestContext, text: &str, column: usize) -> crate::term::TestCell {
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
    row[column].clone()
}

/// A comment's background fills its rows to the edge of the room for it, so
/// that a thread stands out as a block.
#[test]
fn a_comments_background_fills_its_rows() {
    let mut ctx = setup_clone!();
    with_options(&mut ctx, &[("comment-style", "normal blue")]);
    let github = setup(&ctx);
    *github.threads.borrow_mut() = vec![thread(
        "t1",
        Some((Side::New, 2)),
        false,
        &[("alice", "Why new?")],
    )];
    let mut app = init_app(&mut ctx, &github);

    ctx.update(&mut app, keys("OO"));

    use crate::style::Color;
    assert_eq!(cell_at(&ctx, "Why new?", 70).bg, Color::Blue);
    assert_eq!(cell_at(&ctx, ALICE, 70).bg, Color::Blue);
    assert_eq!(cell(&ctx, ALICE, 'a').bg, Color::Blue, "the author");
}

/// Without a comment-style of the user's, comments have GitHub's background
/// for them, for the light or dark background delta renders for.
#[test]
fn comments_have_a_background_by_default() {
    let mut ctx = setup_clone!();
    ctx.config().general.diff_renderer.color_mode = Some(crate::config::ColorMode::Light);
    let github = setup(&ctx);
    *github.threads.borrow_mut() = vec![thread("t1", None, false, &[("alice", "Why new?")])];
    let mut app = init_app(&mut ctx, &github);

    ctx.update(&mut app, keys("OO"));

    assert_eq!(
        cell(&ctx, "Why new?", 'W').bg,
        crate::style::Color::Rgb(0xf6, 0xf8, 0xfa)
    );
}

/// Each comment's time links to the comment on GitHub.
#[test]
fn a_comments_time_links_to_it_on_github() {
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

    // The g of "ago".
    assert_eq!(
        cell(&ctx, ALICE, 'g').link,
        Some(format!("{PR_URL}#discussion_t1_0"))
    );
    assert_eq!(
        cell(&ctx, BOB, 'g').link,
        Some(format!("{PR_URL}#discussion_t1_1"))
    );
    assert_eq!(cell(&ctx, ALICE, 'a').link, None, "the author");
}

/// On a file's line of the diffstat the count is right-aligned in the names'
/// column, so the counts end in a column against the `|`.
#[test]
fn comment_counts_are_aligned_against_the_diffstats_bar() {
    let mut ctx = setup_clone!();
    let github = setup_folded_to_stat(&mut ctx);
    *github.threads.borrow_mut() = vec![
        thread("t1", None, false, &[("alice", "Why?"), ("bob", "So")]),
        Thread {
            path: "big.txt".into(),
            ..thread("t2", None, false, &[("bob", "Hm")])
        },
    ];
    let mut app = init_app(&mut ctx, &github);

    ctx.update(&mut app, keys("OO"));

    assert!(line_with(&ctx, "lines.txt").contains("lines.txt [2 comments] |"));
    let big = line_with(&ctx, "big.txt");
    assert!(big.contains("[1 comment] |"), "{}", ctx.redact_buffer());
    assert!(big.contains("big.txt    ["), "{}", ctx.redact_buffer());
}

/// delta works out which features the user's config enables by settling all of
/// its options, so a PR's screen asks it once for each set of features in
/// force, not each time the PR's rows are built again.
#[test]
fn delta_is_not_asked_which_features_are_enabled_on_every_rebuild() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let mut ctx = setup_clone!();
    let asked = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&asked);
    ctx.config().general.diff_renderer.enabled_features =
        crate::config::EnabledFeatures::new(move |_| {
            counter.fetch_add(1, Ordering::SeqCst);
            vec![]
        });
    let github = setup(&ctx);
    *github.threads.borrow_mut() = vec![thread(
        "t1",
        Some((Side::New, 2)),
        false,
        &[("alice", "Why new?")],
    )];
    let mut app = init_app(&mut ctx, &github);
    // How many times delta is asked while a wider diff context builds the rows again, with the
    // same features. Any screen asks once per rebuild, to know whether difftastic is on.
    let asked_while_widening = |ctx: &mut TestContext, app: &mut App| {
        let before = asked.load(Ordering::SeqCst);
        ctx.update(app, keys("++"));
        asked.load(Ordering::SeqCst) - before
    };
    ctx.update(&mut app, keys("OO"));
    let on_the_pr = asked_while_widening(&mut ctx, &mut app);
    ctx.update(&mut app, keys("q"));
    let on_the_status = asked_while_widening(&mut ctx, &mut app);

    assert_eq!(on_the_pr, on_the_status);
}

#[test]
fn i_on_a_diff_line_opens_a_comment_box_under_it() {
    let mut ctx = setup_clone!();
    let github = setup(&ctx);
    let mut app = init_app(&mut ctx, &github);
    ctx.update(&mut app, keys("OO/new<enter>"));

    ctx.update(&mut app, keys("ihello"));

    let after = lines_after(&ctx, "+new", 2);
    let screen = ctx.redact_buffer();
    assert!(after[0].contains("New comment"), "{screen}");
    assert!(after[1].contains("hello"), "{screen}");
}

#[test]
fn i_on_a_thread_opens_a_reply_box_after_it() {
    let mut ctx = setup_clone!();
    let github = setup(&ctx);
    *github.threads.borrow_mut() = vec![thread(
        "t1",
        Some((Side::New, 2)),
        false,
        &[("alice", "Why new?")],
    )];
    let mut app = init_app(&mut ctx, &github);
    ctx.update(&mut app, keys("OO/alice<enter>"));

    ctx.update(&mut app, keys("isure"));

    let after = lines_after(&ctx, "Why new?", 2);
    let screen = ctx.redact_buffer();
    assert!(after[0].contains("Reply"), "{screen}");
    assert!(after[1].contains("sure"), "{screen}");
}

/// While the box has focus, keys are text: `j` is not a move, nor `q` a quit.
#[test]
fn keys_are_text_while_the_box_has_focus() {
    let mut ctx = setup_clone!();
    let github = setup(&ctx);
    let mut app = init_app(&mut ctx, &github);
    ctx.update(&mut app, keys("OO/new<enter>"));

    ctx.update(&mut app, keys("ijq"));

    assert!(!app.state.quit);
    assert!(
        ctx.redact_buffer().contains("jq"),
        "{}",
        ctx.redact_buffer()
    );
}

#[test]
fn enter_in_the_box_starts_a_new_line() {
    let mut ctx = setup_clone!();
    let github = setup(&ctx);
    let mut app = init_app(&mut ctx, &github);
    ctx.update(&mut app, keys("OO/new<enter>"));

    ctx.update(&mut app, keys("ione<enter>two"));

    let after = lines_after(&ctx, "one", 1);
    assert!(after[0].contains("two"), "{}", ctx.redact_buffer());
    assert!(!line_with(&ctx, "one").contains("two"));
}

/// esc leaves the box, keeping what was written, and `i` goes back to it.
#[test]
fn esc_leaves_the_box_and_i_goes_back_to_it() {
    let mut ctx = setup_clone!();
    let github = setup(&ctx);
    let mut app = init_app(&mut ctx, &github);
    ctx.update(&mut app, keys("OO/new<enter>"));
    ctx.update(&mut app, keys("ihi<esc>"));

    // A key of the screen's again.
    ctx.update(&mut app, keys("gg"));
    assert!(
        ctx.redact_buffer().contains("hi"),
        "{}",
        ctx.redact_buffer()
    );

    ctx.update(&mut app, keys("i!"));
    assert!(
        ctx.redact_buffer().contains("hi!"),
        "{}",
        ctx.redact_buffer()
    );
}

#[test]
fn a_paste_goes_into_the_box() {
    let mut ctx = setup_clone!();
    let github = setup(&ctx);
    let mut app = init_app(&mut ctx, &github);
    ctx.update(&mut app, keys("OO/new<enter>i"));

    ctx.update(
        &mut app,
        vec![crossterm::event::Event::Paste("pasted\nlines".into())],
    );

    assert!(
        lines_after(&ctx, "pasted", 1)[0].contains("lines"),
        "{}",
        ctx.redact_buffer()
    );
}

/// The box is drawn in comment-editor-style, which is comment-style unless set.
#[test]
fn the_box_is_drawn_in_the_comment_styles_unless_editor_styles_are_set() {
    let mut ctx = setup_clone!();
    with_options(&mut ctx, &[("comment-style", "normal blue")]);
    let github = setup(&ctx);
    let mut app = init_app(&mut ctx, &github);
    ctx.update(&mut app, keys("OO/new<enter>ihello"));
    assert_eq!(cell(&ctx, "hello", 'h').bg, crate::style::Color::Blue);
}

#[test]
fn the_box_is_drawn_in_the_editor_styles() {
    let mut ctx = setup_clone!();
    with_options(
        &mut ctx,
        &[
            ("comment-style", "normal blue"),
            ("comment-editor-style", "normal red"),
        ],
    );
    let github = setup(&ctx);
    let mut app = init_app(&mut ctx, &github);
    ctx.update(&mut app, keys("OO/new<enter>ihello"));
    assert_eq!(cell(&ctx, "hello", 'h').bg, crate::style::Color::Red);
}

#[test]
fn ctrl_s_adds_a_reply_to_your_draft_review_not_to_github() {
    let mut ctx = setup_clone!();
    let github = setup(&ctx);
    *github.threads.borrow_mut() = vec![thread(
        "t1",
        Some((Side::New, 2)),
        false,
        &[("alice", "Why new?")],
    )];
    let mut app = init_app(&mut ctx, &github);
    ctx.update(&mut app, keys("OO/alice<enter>isure<enter>thing"));

    ctx.update(&mut app, keys("<ctrl+s>"));

    assert!(github.posted.borrow().is_empty());
    let screen = ctx.redact_buffer();
    assert!(!screen.contains("Reply"), "the box is gone:\n{screen}");
    assert!(line_with(&ctx, "CI [").contains("draft"), "{screen}");
    assert!(
        screen.contains("Comment added to your draft review"),
        "{screen}"
    );
}

#[test]
fn ctrl_s_adds_a_new_comment_on_the_line_to_your_draft_review() {
    let mut ctx = setup_clone!();
    let github = setup(&ctx);
    let mut app = init_app(&mut ctx, &github);
    ctx.update(&mut app, keys("OO/new<enter>ihello"));

    ctx.update(&mut app, keys("<ctrl+s>"));

    assert!(github.posted.borrow().is_empty());
    let screen = ctx.redact_buffer();
    assert!(
        !screen.contains("New comment"),
        "the box is gone:\n{screen}"
    );
    assert!(line_with(&ctx, "CI [").contains("draft"), "{screen}");
}

/// A new comment's line is numbered against the PR's head on GitHub, so it is
/// not posted from another commit; what was written is kept.
#[test]
fn a_new_comment_is_not_posted_while_head_is_not_the_prs() {
    let mut ctx = setup_clone!();
    let github = setup(&ctx);
    let github = Rc::new(FakeGitHub {
        head_oid: "0".repeat(40),
        ..Rc::into_inner(github).unwrap()
    });
    let mut app = init_app(&mut ctx, &github);
    ctx.update(&mut app, keys("OO/new<enter>ihello"));

    ctx.update(&mut app, keys("<ctrl+s>"));

    assert!(github.posted.borrow().is_empty());
    let screen = ctx.redact_buffer();
    assert!(screen.contains("hello"), "{screen}");
    assert!(screen.contains("Cannot comment"), "{screen}");
}

#[test]
fn an_empty_comment_is_not_posted() {
    let mut ctx = setup_clone!();
    let github = setup(&ctx);
    let mut app = init_app(&mut ctx, &github);
    ctx.update(&mut app, keys("OO/new<enter>i"));

    ctx.update(&mut app, keys("<ctrl+s>"));

    assert!(github.posted.borrow().is_empty());
    assert!(
        ctx.redact_buffer().contains("Nothing to post"),
        "{}",
        ctx.redact_buffer()
    );
}

#[test]
fn i_on_someone_elses_comment_replies_to_it() {
    let mut ctx = setup_clone!();
    let github = setup(&ctx);
    *github.threads.borrow_mut() = vec![thread(
        "t1",
        Some((Side::New, 2)),
        false,
        &[("alice", "Why new?")],
    )];
    let mut app = init_app(&mut ctx, &github);
    ctx.update(&mut app, keys("OO/Why new<enter>"));

    ctx.update(&mut app, keys("i"));

    let screen = ctx.redact_buffer();
    assert!(screen.contains("Reply"), "{screen}");
    assert!(!screen.contains("Edit comment"), "{screen}");
}

#[test]
fn k_on_a_comment_not_in_a_draft_review_says_it_cannot_be_changed() {
    let mut ctx = setup_clone!();
    let github = setup(&ctx);
    *github.threads.borrow_mut() = vec![thread(
        "t1",
        Some((Side::New, 2)),
        false,
        &[("alice", "Why new?")],
    )];
    let mut app = init_app(&mut ctx, &github);
    ctx.update(&mut app, keys("OO/Why new<enter>"));

    ctx.update(&mut app, keys("K"));

    let screen = ctx.redact_buffer();
    assert!(
        screen.contains("Only a comment in a draft review can be changed"),
        "{screen}"
    );
    assert!(screen.contains("Why new?"), "{screen}");
}

#[test]
fn op_posts_the_draft_review_into_one_pending_review() {
    let mut ctx = setup_clone!();
    let github = setup(&ctx);
    let mut app = init_app(&mut ctx, &github);
    ctx.update(
        &mut app,
        keys("OO/new<enter>ione<ctrl+s>/old<enter>itwo<ctrl+s>"),
    );

    ctx.update(&mut app, keys("Opy"));

    assert_eq!(*github.reviews_started.borrow(), 1);
    assert_eq!(
        *github.posted.borrow(),
        vec![
            ("R1 lines.txt:(New, 2)".to_owned(), "one".to_owned()),
            ("R1 lines.txt:(Old, 2)".to_owned(), "two".to_owned()),
        ]
    );
    let screen = ctx.redact_buffer();
    assert!(screen.contains("Posted 2 threads"), "{screen}");
}

#[test]
fn op_refuses_while_you_have_a_pending_review_it_did_not_start() {
    let mut ctx = setup_clone!();
    let github = setup(&ctx);
    *github.pending.borrow_mut() = Some("R0".into());
    let mut app = init_app(&mut ctx, &github);
    ctx.update(&mut app, keys("OO/new<enter>ihello<ctrl+s>"));

    ctx.update(&mut app, keys("Opy"));

    assert!(github.posted.borrow().is_empty());
    let screen = ctx.redact_buffer();
    assert!(
        screen
            .contains("Couldn't post: you have a pending review on https://github.com/o/r/pull/7"),
        "{screen}"
    );
}

/// A thread whose one comment, by me, is in my pending review.
fn pending_thread() -> Thread {
    let mut pending = thread("t1", Some((Side::New, 2)), false, &[("me", "Why new?")]);
    pending.comments[0].draft = true;
    pending.comments[0].editable = true;
    pending.comments[0].deletable = true;
    pending
}

#[test]
fn i_on_my_comment_on_github_edits_it_there() {
    let mut ctx = setup_clone!();
    let github = setup(&ctx);
    *github.threads.borrow_mut() = vec![pending_thread()];
    let mut app = init_app(&mut ctx, &github);
    ctx.update(&mut app, keys("OO/Why new<enter>"));

    ctx.update(&mut app, keys("i<space>really<ctrl+s>"));

    assert_eq!(
        *github.edited.borrow(),
        vec![("t1_0".to_owned(), "Why new? really".to_owned())]
    );
    let screen = ctx.redact_buffer();
    assert!(screen.contains("Why new? really"), "{screen}");
    assert!(screen.contains("Comment updated"), "{screen}");
}

#[test]
fn k_deletes_your_reply_on_github() {
    let mut ctx = setup_clone!();
    let github = setup(&ctx);
    let mut pending = thread(
        "t1",
        Some((Side::New, 2)),
        false,
        &[("alice", "Why new?"), ("me", "Because")],
    );
    pending.comments[1].draft = true;
    *github.threads.borrow_mut() = vec![pending];
    *github.pending.borrow_mut() = Some("R0".into());
    let mut app = init_app(&mut ctx, &github);
    ctx.update(&mut app, keys("OO/Because<enter>"));

    ctx.update(&mut app, keys("Ky"));

    assert_eq!(*github.deleted.borrow(), vec!["t1_1".to_owned()]);
    let screen = ctx.redact_buffer();
    assert!(!screen.contains("Because"), "{screen}");
    assert!(screen.contains("Why new?"), "{screen}");
}

#[test]
fn k_does_not_delete_a_threads_first_comment_on_github() {
    let mut ctx = setup_clone!();
    let github = setup(&ctx);
    *github.threads.borrow_mut() = vec![pending_thread()];
    *github.pending.borrow_mut() = Some("R0".into());
    let mut app = init_app(&mut ctx, &github);
    ctx.update(&mut app, keys("OO/Why new<enter>"));

    ctx.update(&mut app, keys("Ky"));

    assert!(github.deleted.borrow().is_empty());
    let screen = ctx.redact_buffer();
    assert!(screen.contains("starts a thread on GitHub"), "{screen}");
}

/// The help's rows, from the one with the heading `heading` down to the next blank row, from
/// the column the heading starts in. A heading has two spaces or more before it, which tells it
/// from an entry such as `O PR` in Submenus.
fn help_section(ctx: &TestContext, heading: &str) -> Vec<String> {
    let screen = ctx.redact_buffer();
    let lines: Vec<&str> = screen.lines().collect();
    let pattern = regex::Regex::new(&format!(r"(?:^ ?|  ){heading}  ")).unwrap();
    let Some((row, column)) = lines.iter().enumerate().find_map(|(i, line)| {
        let found = pattern.find(line)?;
        let start = found.end() - heading.len() - 2;
        Some((i, line[..start].chars().count()))
    }) else {
        return vec![];
    };
    lines[row + 1..]
        .iter()
        .map(|line| line.chars().skip(column).take(40).collect::<String>())
        .take_while(|cell| !cell.trim().is_empty())
        .collect()
}

#[test]
fn the_help_lists_the_pr_keys_under_pr_on_a_prs_screen() {
    let mut ctx = setup_clone!();
    let github = setup(&ctx);
    let mut app = init_app(&mut ctx, &github);
    ctx.update(&mut app, keys("OO/new<enter>"));
    ctx.resize(&mut app, &[HELP_FITS]);

    ctx.update(&mut app, keys(HELP));

    let pr = help_section(&ctx, "PR").join("\n");
    for key in ["Toggle viewed", "Show on GitHub", "Write a comment"] {
        assert!(pr.contains(key), "{key} under PR:\n{}", ctx.redact_buffer());
    }
    let line = help_section(&ctx, "Line").join("\n");
    assert!(!line.contains("Toggle viewed"), "{}", ctx.redact_buffer());
}

#[test]
fn the_help_lists_deleting_a_comment_under_pr_on_a_comment() {
    let mut ctx = setup_clone!();
    let github = setup(&ctx);
    *github.threads.borrow_mut() = vec![pending_thread()];
    let mut app = init_app(&mut ctx, &github);
    ctx.update(&mut app, keys("OO/Why new<enter>"));
    ctx.resize(&mut app, &[HELP_FITS]);

    ctx.update(&mut app, keys(HELP));

    let pr = help_section(&ctx, "PR").join("\n");
    for key in ["Delete comment", "Edit comment"] {
        assert!(pr.contains(key), "{key} under PR:\n{}", ctx.redact_buffer());
    }
    assert!(
        !ctx.redact_buffer().contains("Discard"),
        "{}",
        ctx.redact_buffer()
    );
}

#[test]
fn the_help_lists_no_pr_keys_outside_a_pr() {
    let mut ctx = setup_clone!();
    ctx.debug = false;
    commit(&ctx.dir, "file", "a\n");
    fs::write(ctx.dir.join("file"), "b\n").unwrap();
    let mut app = ctx.init_app_as_pager_of(&["git", "diff"]);
    ctx.update(&mut app, keys("/b<enter>"));
    ctx.resize(&mut app, &[HELP_FITS]);

    ctx.update(&mut app, keys(HELP));

    let screen = ctx.redact_buffer();
    for key in [
        "Toggle viewed",
        "Show on GitHub",
        "Write a comment",
        "Edit comment",
        "Reply",
    ] {
        assert!(!screen.contains(key), "{key}:\n{screen}");
    }
    assert!(help_section(&ctx, "PR").is_empty(), "{screen}");
}

#[test]
fn the_help_says_i_replies_on_someone_elses_comment() {
    let mut ctx = setup_clone!();
    let github = setup(&ctx);
    *github.threads.borrow_mut() = vec![thread(
        "t1",
        Some((Side::New, 2)),
        false,
        &[("alice", "Why new?")],
    )];
    let mut app = init_app(&mut ctx, &github);
    ctx.update(&mut app, keys("OO/Why new<enter>"));
    ctx.resize(&mut app, &[HELP_FITS]);

    ctx.update(&mut app, keys(HELP));

    let pr = help_section(&ctx, "PR").join("\n");
    assert!(pr.contains("Reply"), "{}", ctx.redact_buffer());
    assert!(!pr.contains("Edit comment"), "{}", ctx.redact_buffer());
}
