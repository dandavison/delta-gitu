//! Reviewing a GitHub PR: `O` checks it out and shows its diff, each file with
//! its Viewed checkbox from GitHub; `x` ticks or unticks it, and `o` opens the
//! line on GitHub to comment on it. Review threads are shown under the lines
//! they are on.

use super::*;
use crate::review::remote::WriteToken;
use crate::{
    app::App,
    error::Error,
    review::{
        Comment, Place, Side, Subject, Thread, ViewedState,
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
    /// How many commits GitHub says the PR has: by default, the 2 that `setup`'s `feature` has.
    pub(super) commits: RefCell<usize>,
    /// Whether the submitted review is mine rather than alice's.
    pub(super) submitted_by_me: RefCell<bool>,
    /// How many of the next threads and replies GitHub adds but whose reply is lost, as when the
    /// network fails just after GitHub has made the change.
    pub(super) loses_replies: RefCell<usize>,
    /// How long GitHub takes to say what a PR is, as over a slow network.
    pub(super) delay: RefCell<std::time::Duration>,
    /// How long GitHub takes to give a PR's threads.
    pub(super) threads_delay: RefCell<std::time::Duration>,
    /// Where branches are on GitHub, by name, other than the PR's base, which is at `base_oid`
    /// unless given here.
    pub(super) branches: RefCell<HashMap<String, String>>,
}

impl FakeGitHub {
    /// An error, once GitHub has made the change, if its reply is to be lost.
    fn reply_lost(&self) -> Result<(), Error> {
        let mut lost = self.loses_replies.borrow_mut();
        if *lost == 0 {
            return Ok(());
        }
        *lost -= 1;
        Err(Error::Gh("error connecting to api.github.com".to_owned()))
    }
}

impl GitHub for FakeGitHub {
    fn pr(&self, selector: Option<&str>) -> Result<GitHubPr, Error> {
        std::thread::sleep(*self.delay.borrow());
        self.asked_for.borrow_mut().push(selector.map(String::from));
        Ok(GitHubPr {
            id: "PR_id".into(),
            number: 7,
            url: PR_URL.into(),
            base_ref: self.base_ref.borrow().clone(),
            base_oid: self.base_oid.clone(),
            head_oid: self.head_oid.clone(),
            commits: *self.commits.borrow(),
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
        let mine = *self.submitted_by_me.borrow();
        let mut reviews = vec![GitHubReview {
            id: "submitted".into(),
            author: if mine { "me" } else { "alice" }.into(),
            state: GitHubReviewState::Submitted,
            comments: comments(false),
            mine,
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
        std::thread::sleep(*self.threads_delay.borrow());
        Ok(self.threads.borrow().clone())
    }

    fn default_branch(&self) -> Result<String, Error> {
        if *self.fails.borrow() {
            return Err(Error::Gh("error connecting to api.github.com".to_owned()));
        }
        Ok(self.default_branch.borrow().clone())
    }

    fn branch_commit(&self, branch: &str) -> Result<Option<String>, Error> {
        if *self.fails.borrow() {
            return Err(Error::Gh("error connecting to api.github.com".to_owned()));
        }
        if let Some(at) = self.branches.borrow().get(branch) {
            return Ok(Some(at.clone()));
        }
        Ok((branch == *self.base_ref.borrow()).then(|| self.base_oid.clone()))
    }

    fn pr_for_branch(&self, head: &str) -> Result<Option<GitHubPr>, Error> {
        if *self.fails.borrow() {
            return Err(Error::Gh("error connecting to api.github.com".to_owned()));
        }
        if !*self.has_pr.borrow() {
            return Ok(None);
        }
        self.pr(Some(head)).map(Some)
    }

    fn create_pr(&self, _: WriteToken, head: &str, base: &str) -> Result<GitHubPr, Error> {
        self.created
            .borrow_mut()
            .push((head.to_owned(), base.to_owned()));
        *self.has_pr.borrow_mut() = true;
        self.pr(None)
    }

    fn pending_review(&self, _pr: &GitHubPr) -> Result<Option<String>, Error> {
        Ok(self.pending.borrow().clone())
    }

    fn start_review(&self, _: WriteToken, _pr: &GitHubPr) -> Result<String, Error> {
        *self.reviews_started.borrow_mut() += 1;
        let id = format!("R{}", self.reviews_started.borrow());
        *self.pending.borrow_mut() = Some(id.clone());
        Ok(id)
    }

    fn add_thread(
        &self,
        _: WriteToken,
        review_id: &str,
        path: &str,
        place: Place,
        body: &str,
    ) -> Result<String, Error> {
        if *self.refuses_threads.borrow() {
            return Err(Error::Gh(format!(
                "Couldn't add the thread on {path} at {place:?}: GitHub did nothing"
            )));
        }
        let (line, subject, at) = match place {
            Place::Line(side, n) => (Some((side, n)), Subject::Line, format!("{:?}", (side, n))),
            Place::File => (None, Subject::File, "file".to_owned()),
        };
        self.posted
            .borrow_mut()
            .push((format!("{review_id} {path}:{at}"), body.to_owned()));
        let id = format!("new{}", self.threads.borrow().len());
        let mut new = Thread {
            path: path.to_owned(),
            subject,
            ..thread(&id, line, false, &[("me", body)])
        };
        new.comments[0].draft = true;
        new.comments[0].editable = true;
        new.comments[0].deletable = true;
        new.comments[0].created_at = chrono::Utc::now().fixed_offset();
        let comment_id = new.comments[0].id.clone();
        self.threads.borrow_mut().push(new);
        self.reply_lost()?;
        Ok(comment_id)
    }

    fn add_reply(
        &self,
        _: WriteToken,
        review_id: &str,
        comment_id: &str,
        body: &str,
    ) -> Result<String, Error> {
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
            created_at: chrono::Utc::now().fixed_offset(),
            draft: true,
            editable: true,
            deletable: true,
            submitted: false,
            local_only: false,
            pending: false,
            reactions: vec![],
            agent: None,
        });
        drop(threads);
        self.reply_lost()?;
        Ok("reply".to_owned())
    }

    fn edit_comment(&self, _: WriteToken, comment_id: &str, body: &str) -> Result<(), Error> {
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

    fn comment_now(
        &self,
        comment_id: &str,
    ) -> Result<Option<crate::review::github::CommentNow>, Error> {
        let mine = *self.submitted_by_me.borrow();
        Ok(self
            .threads
            .borrow()
            .iter()
            .flat_map(|t| &t.comments)
            .find(|c| c.id == comment_id)
            .map(|c| crate::review::github::CommentNow {
                body: c.body.clone(),
                // As `reviews` has it: a draft is in my pending review, and the rest in the
                // submitted one, which is mine if `submitted_by_me`.
                public: !c.draft,
                mine: c.draft || mine,
            }))
    }

    fn delete_comment(&self, _: WriteToken, comment_id: &str) -> Result<(), Error> {
        self.deleted.borrow_mut().push(comment_id.to_owned());
        let mut threads = self.threads.borrow_mut();
        for thread in threads.iter_mut() {
            thread.comments.retain(|comment| comment.id != comment_id);
        }
        threads.retain(|thread| !thread.comments.is_empty());
        Ok(())
    }

    fn set_viewed(
        &self,
        _: WriteToken,
        _pr: &GitHubPr,
        path: &str,
        viewed: bool,
    ) -> Result<(), Error> {
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
pub(super) fn setup(ctx: &TestContext) -> Rc<FakeGitHub> {
    commit(&ctx.dir, "lines.txt", "keep\nold\nkeep\n");
    run(&ctx.dir, &["git", "checkout", "-b", "feature"]);
    commit(&ctx.dir, "lines.txt", "keep\nnew\nkeep\n");
    commit(&ctx.dir, "added.txt", "added\n");
    run(&ctx.dir, &["git", "checkout", "main"]);
    commit(&ctx.dir, "main-only.txt", "main\n");
    run(&ctx.dir, &["git", "checkout", "feature"]);
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
            commits: RefCell::new(2),
            submitted_by_me: RefCell::new(false),
            loses_replies: RefCell::new(0),
            delay: RefCell::new(std::time::Duration::ZERO),
            threads_delay: RefCell::new(std::time::Duration::ZERO),
            branches: RefCell::new(HashMap::new()),
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

pub(super) fn init_app(ctx: &mut TestContext, github: &Rc<FakeGitHub>) -> App {
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
pub(super) fn with_options(
    ctx: &mut TestContext,
    options: &'static [(&'static str, &'static str)],
) {
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
        start_line: None,
        subject: Subject::Line,
        resolved,
        deleted_on_github: false,
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
                submitted: true,
                local_only: false,
                pending: false,
                reactions: vec![],
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
    run(&ctx.dir, &["git", "checkout", "main"]);
    let mut app = init_app(&mut ctx, &github);

    ctx.update(&mut app, keys("Oo7<enter>"));

    assert_eq!(github.asked_for.borrow()[0], Some("7".to_owned()));
    assert_eq!(head_branch(&ctx), "feature");
    let screen = ctx.redact_buffer();
    assert!(screen.contains("lines.txt"), "{screen}");
    assert!(screen.contains("added.txt"), "{screen}");
    assert!(!screen.contains("main-only.txt"), "{screen}");
}

/// `O O` opens the current branch's PR, asking gh for the branch's, and checks nothing out.
#[test]
fn opening_the_current_branch_pr_asks_gh_for_the_branchs() {
    let mut ctx = setup_clone!();
    let github = setup(&ctx);
    let mut app = init_app(&mut ctx, &github);

    ctx.update(&mut app, keys("OO"));

    assert_eq!(*github.asked_for.borrow(), vec![Some("feature".to_owned())]);
    assert_eq!(head_branch(&ctx), "feature");
    let screen = ctx.redact_buffer();
    assert!(screen.contains("added.txt"), "{screen}");
}

#[test]
fn opening_the_pr_of_the_base_branch_says_there_is_none() {
    let mut ctx = setup_clone!();
    let github = setup(&ctx);
    *github.has_pr.borrow_mut() = false;
    run(&ctx.dir, &["git", "checkout", "main"]);
    let mut app = init_app(&mut ctx, &github);

    ctx.update(&mut app, keys("OO"));

    let screen = ctx.redact_buffer();
    assert!(
        screen.contains("No PR to review: main is the base branch"),
        "{screen}"
    );
}

#[test]
fn uncommitted_changes_stop_the_checkout() {
    let mut ctx = setup_clone!();
    let github = setup(&ctx);
    run(&ctx.dir, &["git", "checkout", "main"]);
    fs::write(ctx.dir.join("lines.txt"), "changed\n").unwrap();
    let mut app = init_app(&mut ctx, &github);

    ctx.update(&mut app, keys("Oo7<enter>"));

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
    run(&ctx.dir, &["git", "checkout", "main"]);
    fs::write(ctx.dir.join("untracked.txt"), "new\n").unwrap();
    let mut app = init_app(&mut ctx, &github);

    ctx.update(&mut app, keys("Oo7<enter>"));

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

    let screen = screen_text(&ctx);
    assert!(
        screen.contains("which GitHub's PR #7's base is on"),
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

/// o on the PR's summary, atop its screen, opens the PR's page on GitHub.
#[test]
fn o_on_the_summary_opens_the_pr_on_github() {
    let mut ctx = setup_clone!();
    let github = setup(&ctx);
    let mut app = init_app(&mut ctx, &github);
    ctx.update(&mut app, keys("OOgg"));

    ctx.update(&mut app, keys("o"));

    assert_eq!(*github.opened.borrow(), vec![PR_URL.to_owned()]);
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
        head_oid: run(&ctx.dir, &["git", "rev-parse", "feature~1"])
            .trim()
            .to_owned(),
        ..Rc::into_inner(github).unwrap()
    });
    let mut app = init_app(&mut ctx, &github);

    ctx.update(&mut app, keys("OO"));

    let screen = screen_text(&ctx);
    assert!(
        screen.contains("Your local branch feature has 1 commit that GitHub's PR #7 doesn't"),
        "{screen}"
    );
}

/// The column, counting from 0 at the screen's left edge, of the bar drawn
/// before the comment showing `text`.
fn bar_column(ctx: &TestContext, text: &str) -> usize {
    line_with(ctx, text).chars().position(|c| c == '│').unwrap()
}

/// A renderer that draws every hunk side by side, as delta does with `side-by-side`.
fn side_by_side(ctx: &mut TestContext) {
    ctx.config().general.diff_renderer.command = super::pager::layout_renderer("always", 0);
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

/// In side-by-side, a row with a removed line and an added line has `i` comment on the added
/// line, in the right panel, and the box says which line it is.
#[test]
fn i_on_a_side_by_side_row_with_an_added_line_comments_on_the_added_line() {
    let mut ctx = setup_clone!();
    side_by_side(&mut ctx);
    let github = setup(&ctx);
    let mut app = init_app(&mut ctx, &github);
    ctx.update(&mut app, keys("OO/new<enter>"));

    ctx.update(&mut app, keys("i"));

    let screen = ctx.redact_buffer();
    assert!(screen.contains("New comment on line 2 "), "{screen}");
    assert_eq!(bar_column(&ctx, "New comment"), RIGHT_PANEL + 2);
}

/// A comment goes where its line is drawn, as the renderer's OSC 1717 records place it, not as
/// the side-by-side setting says: with delta's `side-by-side = hunk`, a hunk that only adds lines
/// is drawn unified, and a thread on it is as in a unified diff, while one on a hunk that removes
/// and adds lines is in its side's panel.
#[test]
fn a_thread_goes_where_the_renderer_draws_its_line() {
    let mut ctx = setup_clone!();
    ctx.config().general.diff_renderer.command = super::pager::layout_renderer("hunk", 0);
    let github = setup(&ctx);
    let mut added = thread("t2", Some((Side::New, 1)), false, &[("bob", "Why added?")]);
    added.path = "added.txt".to_owned();
    *github.threads.borrow_mut() = vec![
        thread("t1", Some((Side::New, 2)), false, &[("alice", "Why new?")]),
        added,
    ];
    let mut app = init_app(&mut ctx, &github);

    ctx.update(&mut app, keys("OO"));

    assert_eq!(bar_column(&ctx, ALICE), RIGHT_PANEL + 2);
    assert_eq!(bar_column(&ctx, "bob ["), 1 + 2);
}

/// A thread on a line the renderer draws on several rows goes after all of them.
#[test]
fn a_thread_goes_after_every_row_of_a_wrapped_line() {
    let mut ctx = setup_clone!();
    ctx.config().general.diff_renderer.command = super::pager::layout_renderer("never", 2);
    let github = setup(&ctx);
    *github.threads.borrow_mut() = vec![thread(
        "t1",
        Some((Side::New, 2)),
        false,
        &[("alice", "Why?")],
    )];
    let mut app = init_app(&mut ctx, &github);

    ctx.update(&mut app, keys("OO"));

    let screen = ctx.redact_buffer();
    let row = |text: &str| {
        screen
            .lines()
            .position(|line| line.trim_end_matches([' ', '|']).ends_with(text))
            .unwrap_or_else(|| panic!("no row ending {text}:\n{screen}"))
    };
    assert_eq!(row("+n") + 1, row("ew"), "{screen}");
    assert!(row("ew") < row("Why?"), "{screen}");
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
/// The screen's text as one line, its rows joined with spaces, so that a message wrapped onto
/// several rows reads as it was written.
pub(super) fn screen_text(ctx: &TestContext) -> String {
    ctx.redact_buffer()
        .lines()
        .map(|line| line.trim_end_matches('|').trim())
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

pub(super) fn cell(ctx: &TestContext, text: &str, c: char) -> crate::term::TestCell {
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

/// A comment in a submitted review, which everyone can see, is drawn in `comment-submitted-style`
/// over `comment-style`; a comment in a pending review, which only its author sees, is not.
#[test]
fn comments_in_a_submitted_review_are_drawn_in_the_submitted_style() {
    let mut ctx = setup_clone!();
    with_options(
        &mut ctx,
        &[
            ("comment-style", "red"),
            ("comment-submitted-style", "green"),
        ],
    );
    let github = setup(&ctx);
    let mut pending = thread("t2", None, false, &[("me", "Pending words")]);
    pending.comments[0].draft = true;
    *github.threads.borrow_mut() = vec![
        thread("t1", None, false, &[("alice", "Submitted words")]),
        pending,
    ];
    let mut app = init_app(&mut ctx, &github);

    ctx.update(&mut app, keys("OO"));

    use crate::style::Color;
    assert_eq!(cell(&ctx, "Submitted words", 'S').fg, Color::Green);
    assert_eq!(cell(&ctx, "Pending words", 'P').fg, Color::Red);
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
    // As on GitHub, every file of the PR has a Viewed state.
    github
        .viewed
        .borrow_mut()
        .insert("big.txt".to_owned(), ViewedState::Unviewed);
    // big.txt's commit is the PR's third.
    *github.commits.borrow_mut() = 3;
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

/// A markdown link in a comment shows its text, underlined and linked to its URL, which isn't
/// shown.
#[test]
fn a_markdown_link_in_a_comment_shows_its_text_linked_to_its_url() {
    let mut ctx = setup_clone!();
    let github = setup(&ctx);
    *github.threads.borrow_mut() = vec![thread(
        "t1",
        Some((Side::New, 2)),
        false,
        &[("alice", "See [the check](https://example.com/check) here.")],
    )];
    let mut app = init_app(&mut ctx, &github);

    ctx.update(&mut app, keys("OO"));

    let screen = ctx.redact_buffer();
    assert!(screen.contains("See the check here."), "{screen}");
    assert!(!screen.contains("example.com"), "{screen}");
    use crate::style::Modifier;
    let k = cell(&ctx, "See the check", 'k');
    assert_eq!(k.link.as_deref(), Some("https://example.com/check"));
    assert!(k.modifier.contains(Modifier::UNDERLINED));
    let s = cell(&ctx, "See the check", 'S');
    assert_eq!(s.link, None);
    assert!(!s.modifier.contains(Modifier::UNDERLINED));
}

/// Under a comment are its reactions: an emoji for each person who reacted with it, linked to
/// their page, without their name, and how many more reacted than GitHub gave.
#[test]
fn each_reaction_links_to_the_page_of_who_reacted() {
    use crate::review::{Reaction, Reactor};
    let reactor = |login: &str| Reactor {
        login: login.to_owned(),
        url: format!("https://github.com/{login}"),
    };
    let mut ctx = setup_clone!();
    let github = setup(&ctx);
    let mut t = thread("t1", Some((Side::New, 2)), false, &[("alice", "Why new?")]);
    t.comments[0].reactions = vec![
        Reaction {
            emoji: "👍".to_owned(),
            people: vec![reactor("bob"), reactor("carol")],
            count: 2,
        },
        Reaction {
            emoji: "🎉".to_owned(),
            people: vec![reactor("dave")],
            count: 12,
        },
    ];
    *github.threads.borrow_mut() = vec![t];
    let mut app = init_app(&mut ctx, &github);

    ctx.update(&mut app, keys("OO"));

    let screen = ctx.redact_buffer();
    assert!(screen.contains("👍👍  🎉 +11"), "{screen}");
    assert!(!screen.contains("bob"), "{screen}");
    let crate::term::TermBackend::Test { buffer, .. } = &ctx.term else {
        unreachable!();
    };
    let row = buffer
        .cells
        .chunks(buffer.width as usize)
        .find(|row| {
            row.iter()
                .map(|c| c.symbol.as_str())
                .collect::<String>()
                .contains("+11")
        })
        .unwrap();
    let links = |emoji: &str| -> Vec<Option<String>> {
        row.iter()
            .filter(|c| c.symbol == emoji)
            .map(|c| c.link.clone())
            .collect()
    };
    let page = |login: &str| Some(format!("https://github.com/{login}"));
    assert_eq!(links("👍"), [page("bob"), page("carol")]);
    assert_eq!(links("🎉"), [page("dave")]);
}

/// A link's text wraps as other words do, each row of it linked, however long its URL.
#[test]
fn a_long_markdown_link_wraps_by_its_text() {
    let mut ctx = setup_clone!();
    let github = setup(&ctx);
    let url = format!("https://example.com/{}", "a".repeat(300));
    let body = format!("[alpha {}omega]({url})", "word ".repeat(30));
    *github.threads.borrow_mut() = vec![thread(
        "t1",
        Some((Side::New, 2)),
        false,
        &[("alice", &body)],
    )];
    let mut app = init_app(&mut ctx, &github);

    ctx.update(&mut app, keys("OO"));

    let screen = ctx.redact_buffer();
    assert!(!screen.contains("aaaaaaaa"), "{screen}");
    assert_eq!(cell(&ctx, "alpha", 'p').link, Some(url.clone()));
    assert_eq!(cell(&ctx, "omega", 'g').link, Some(url));
}

/// A link in code is code: it is shown as written.
#[test]
fn a_markdown_link_in_code_is_shown_as_written() {
    let mut ctx = setup_clone!();
    let github = setup(&ctx);
    *github.threads.borrow_mut() = vec![thread(
        "t1",
        Some((Side::New, 2)),
        false,
        &[("alice", "Write `[text](https://example.com)` for a link")],
    )];
    let mut app = init_app(&mut ctx, &github);

    ctx.update(&mut app, keys("OO"));

    let screen = ctx.redact_buffer();
    assert!(
        screen.contains("Write `[text](https://example.com)` for a link"),
        "{screen}"
    );
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

/// esc in a box with nothing written in it removes the box, and leaves the PR open.
#[test]
fn esc_removes_an_empty_box() {
    let mut ctx = setup_clone!();
    let github = setup(&ctx);
    let mut app = init_app(&mut ctx, &github);
    ctx.update(&mut app, keys("OO/new<enter>i"));
    assert!(
        ctx.redact_buffer().contains("New comment"),
        "{}",
        ctx.redact_buffer()
    );
    let screens = app.state.screens.len();

    ctx.update(&mut app, keys("<esc>"));

    assert!(
        !ctx.redact_buffer().contains("New comment"),
        "{}",
        ctx.redact_buffer()
    );
    assert_eq!(app.state.screens.len(), screens);
    assert!(app.screen().pr.is_some());
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
        head_oid: run(&ctx.dir, &["git", "rev-parse", "feature~1"])
            .trim()
            .to_owned(),
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
        screen.contains("Only a local comment, or your own on GitHub, can be changed"),
        "{screen}"
    );
    assert!(screen.contains("Why new?"), "{screen}");
}

#[test]
fn os_posts_the_draft_review_into_one_pending_review() {
    let mut ctx = setup_clone!();
    let github = setup(&ctx);
    let mut app = init_app(&mut ctx, &github);
    ctx.update(
        &mut app,
        keys("OO/new<enter>ione<ctrl+s>/old<enter>itwo<ctrl+s>"),
    );

    ctx.update(&mut app, keys("Os"));

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
fn os_adds_to_a_pending_review_it_did_not_start() {
    let mut ctx = setup_clone!();
    let github = setup(&ctx);
    *github.pending.borrow_mut() = Some("R0".into());
    let mut app = init_app(&mut ctx, &github);
    ctx.update(&mut app, keys("OO/new<enter>ihello<ctrl+s>"));

    ctx.update(&mut app, keys("Os"));

    assert_eq!(*github.reviews_started.borrow(), 0);
    assert_eq!(
        *github.posted.borrow(),
        vec![("R0 lines.txt:(New, 2)".to_owned(), "hello".to_owned())]
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

/// Changing my comment in a submitted review changes what others see, so it is asked first.
#[test]
fn editing_my_comment_in_a_submitted_review_asks_first() {
    let mut ctx = setup_clone!();
    let github = setup(&ctx);
    *github.submitted_by_me.borrow_mut() = true;
    *github.threads.borrow_mut() = vec![thread(
        "t1",
        Some((Side::New, 2)),
        false,
        &[("me", "Why new?")],
    )];
    let mut app = init_app(&mut ctx, &github);

    ctx.update(&mut app, keys("OO/Why new<enter>i<space>really<ctrl+s>"));
    let screen = ctx.redact_buffer();
    assert!(screen.contains("public on GitHub"), "{screen}");

    let mut app = init_app(&mut ctx, &github);
    ctx.update(&mut app, keys("OO/Why new<enter>i<space>really<ctrl+s>n"));
    assert!(github.edited.borrow().is_empty());

    let mut app = init_app(&mut ctx, &github);
    ctx.update(&mut app, keys("OO/Why new<enter>i<space>really<ctrl+s>y"));
    assert_eq!(
        *github.edited.borrow(),
        vec![("t1_0".to_owned(), "Why new? really".to_owned())]
    );
}

/// Deleting my comment in a submitted review changes what others see, so it is asked first, as
/// such.
#[test]
fn deleting_my_comment_in_a_submitted_review_asks_first() {
    let mut ctx = setup_clone!();
    let github = setup(&ctx);
    *github.submitted_by_me.borrow_mut() = true;
    *github.threads.borrow_mut() = vec![thread(
        "t1",
        Some((Side::New, 2)),
        false,
        &[("alice", "Why new?"), ("me", "Because")],
    )];
    let mut app = init_app(&mut ctx, &github);

    ctx.update(&mut app, keys("OO/Because<enter>K"));
    let screen = ctx.redact_buffer();
    assert!(screen.contains("public on GitHub"), "{screen}");

    let mut app = init_app(&mut ctx, &github);
    ctx.update(&mut app, keys("OO/Because<enter>Ky"));
    assert_eq!(*github.deleted.borrow(), vec!["t1_1".to_owned()]);
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
    for key in ["Toggle viewed", "Open in GitHub", "Write a comment"] {
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
        "Open in GitHub",
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

/// `setup`, with `long.txt`, whose line 10 the PR changes, and git's `diff.context` set to 7, so
/// that the PR's screen shows lines 3 to 17, though GitHub's diff, which has 3 lines of context,
/// has only lines 7 to 13.
fn setup_wide_context(ctx: &TestContext) -> Rc<FakeGitHub> {
    let lines = |ten: &str| {
        (1..=20)
            .map(|i| match i {
                10 => format!("{ten}\n"),
                i => format!("line-{i:02}\n"),
            })
            .collect::<String>()
    };
    commit(&ctx.dir, "long.txt", &lines("line-10"));
    run(&ctx.dir, &["git", "checkout", "-b", "feature"]);
    commit(&ctx.dir, "long.txt", &lines("changed"));
    // As many commits as FakeGitHub's PR has.
    commit(&ctx.dir, "added.txt", "added\n");
    run(&ctx.dir, &["git", "config", "diff.context", "7"]);
    Rc::new(FakeGitHub::new(
        run(&ctx.dir, &["git", "rev-parse", "main"]).trim(),
        run(&ctx.dir, &["git", "rev-parse", "feature"]).trim(),
    ))
}

/// The JSON file the PR `feature` into `main` is recorded in.
fn feature_review_file(ctx: &TestContext) -> std::path::PathBuf {
    ctx.dir.join(".git/delta/review/feature...main.json")
}

/// A new comment on a line the screen shows but GitHub's diff doesn't, where GitHub can't place
/// it, is local-only, and says why.
#[test]
fn a_comment_on_a_line_outside_githubs_diff_is_local_only() {
    let mut ctx = setup_clone!();
    let github = setup_wide_context(&ctx);
    let mut app = init_app(&mut ctx, &github);
    ctx.update(&mut app, keys("OO/line-15<enter>"));

    ctx.update(&mut app, keys("ihello<ctrl+s>"));

    let screen = ctx.redact_buffer();
    assert!(
        screen.contains("local-only: GitHub can't place a comment on long.txt:15"),
        "{screen}"
    );
    let file: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(feature_review_file(&ctx)).unwrap()).unwrap();
    let comment = &file["threads"][0]["comments"][0];
    assert_eq!(comment["body"], "hello", "{file}");
    assert!(comment["review"].is_null(), "{file}");
}

/// A comment on a line in GitHub's diff, though not a changed one, goes into the draft review.
#[test]
fn a_comment_on_a_context_line_in_githubs_diff_is_a_draft() {
    let mut ctx = setup_clone!();
    let github = setup_wide_context(&ctx);
    let mut app = init_app(&mut ctx, &github);
    ctx.update(&mut app, keys("OO/line-13<enter>"));

    ctx.update(&mut app, keys("ihello<ctrl+s>"));

    let file: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(feature_review_file(&ctx)).unwrap()).unwrap();
    assert!(
        !file["threads"][0]["comments"][0]["review"].is_null(),
        "{file}\n{}",
        ctx.redact_buffer()
    );
}

/// `p` doesn't move a comment GitHub can't place into the draft review, and says why.
#[test]
fn p_does_not_put_a_comment_github_cant_place_in_the_draft_review() {
    let mut ctx = setup_clone!();
    let github = setup_wide_context(&ctx);
    let mut app = init_app(&mut ctx, &github);
    ctx.update(
        &mut app,
        keys("OO/line-15<enter>ihello<ctrl+s>/hello<enter>"),
    );

    ctx.update(&mut app, keys("p"));

    let screen = ctx.redact_buffer();
    assert!(
        screen.contains("GitHub can't place a comment on long.txt:15"),
        "{screen}"
    );
    let file: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(feature_review_file(&ctx)).unwrap()).unwrap();
    assert!(
        file["threads"][0]["comments"][0]["review"].is_null(),
        "{file}"
    );
}

/// Sync posts nothing while a draft thread is on a line GitHub can't place, as one can come to be
/// when the code changes, and says which.
#[test]
fn os_posts_nothing_while_a_draft_thread_is_outside_githubs_diff() {
    let mut ctx = setup_clone!();
    let github = setup_wide_context(&ctx);
    let mut app = init_app(&mut ctx, &github);
    ctx.update(
        &mut app,
        keys("OO/line-13<enter>ione<ctrl+s>/line-11<enter>itwo<ctrl+s>"),
    );
    // The first thread on line 15, where it would be had the code moved.
    let path = feature_review_file(&ctx);
    let mut file: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    file["threads"][0]["anchor"]["line"] = 15.into();
    file["threads"][0]["anchor"]["text"] = serde_json::json!(["line-15"]);
    fs::write(&path, serde_json::to_string_pretty(&file).unwrap()).unwrap();
    ctx.update(&mut app, keys("gr"));

    ctx.update(&mut app, keys("Os"));

    assert_eq!(*github.posted.borrow(), vec![]);
    let screen = ctx.redact_buffer();
    assert!(screen.contains("Posted nothing"), "{screen}");
    assert!(screen.contains("long.txt:15"), "{screen}");
}

/// The rows the cursor is on, which are all of a thread's when it is on one.
fn cursor_rows(ctx: &TestContext) -> String {
    let screen = ctx.redact_buffer();
    let rows: Vec<&str> = screen
        .lines()
        .filter(|line| line.starts_with('▌'))
        .collect();
    assert!(!rows.is_empty(), "no cursor:\n{screen}");
    rows.join("\n")
}

/// The PR open, with a thread on `lines.txt`'s new line 2, `on new`, which has an agent's reply,
/// `a reply about drafts`, and one on its old line 2, `on old`.
fn with_two_threads(ctx: &mut TestContext) -> App {
    let github = setup(ctx);
    let mut app = init_app(ctx, &github);
    ctx.update(
        &mut app,
        keys("OO/new<enter>ion new<ctrl+s>/old<enter>ion old<ctrl+s>"),
    );
    // `i` on my own comment edits it, so the reply is an agent's.
    let file: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(ctx.dir.join(".git/delta/review/feature...main.json")).unwrap(),
    )
    .unwrap();
    let thread = file["threads"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["comments"][0]["body"] == "on new")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    super::local_pr::review_cli_with_input(
        &ctx.dir,
        &[
            "reply", &thread, "--agent", "claude", "--head", "feature", "--base", "main",
        ],
        "a reply about drafts",
    )
    .unwrap();
    ctx.update(&mut app, keys("gr"));
    let screen = ctx.redact_buffer();
    assert!(screen.contains("a reply about drafts"), "{screen}");
    assert!(screen.contains("on old"), "{screen}");
    app
}

/// `O t` shows the PR's threads, each under where it is, as on the PR's screen.
#[test]
fn o_t_shows_the_prs_threads() {
    let mut ctx = setup_clone!();
    let mut app = with_two_threads(&mut ctx);

    ctx.update(&mut app, keys("Ot"));

    let screen = ctx.redact_buffer();
    assert!(screen.contains("lines.txt:2"), "{screen}");
    assert!(screen.contains("on new"), "{screen}");
    assert!(screen.contains("a reply about drafts"), "{screen}");
    assert!(screen.contains("on old"), "{screen}");
    // Not the diff.
    assert!(!screen.contains("added: added.txt"), "{screen}");
}

/// Typing narrows the threads shown to those any of whose comments match each word.
#[test]
fn typing_in_the_thread_view_narrows_it() {
    let mut ctx = setup_clone!();
    let mut app = with_two_threads(&mut ctx);

    ctx.update(&mut app, keys("Otdrafts"));

    let screen = ctx.redact_buffer();
    assert!(screen.contains("on new"), "{screen}");
    assert!(!screen.contains("on old"), "{screen}");
}

/// `enter` on a thread goes to it in the PR's diff.
#[test]
fn enter_in_the_thread_view_goes_to_the_thread_in_the_diff() {
    let mut ctx = setup_clone!();
    let mut app = with_two_threads(&mut ctx);
    ctx.update(&mut app, keys("gg"));

    ctx.update(&mut app, keys("Otold<enter>"));

    let screen = ctx.redact_buffer();
    assert!(screen.contains("added: added.txt"), "{screen}");
    assert!(cursor_rows(&ctx).contains("on old"), "{screen}");
}

/// `right` does as `enter` does, and the thread's file is unfolded to show it.
#[test]
fn right_in_the_thread_view_goes_to_the_thread_in_its_folded_file() {
    let mut ctx = setup_clone!();
    let mut app = with_two_threads(&mut ctx);
    ctx.update(&mut app, keys("gg/lines.txt<enter><tab>gg"));
    assert!(!ctx.redact_buffer().contains("on new"));

    ctx.update(&mut app, keys("Otdrafts<right>"));

    let screen = ctx.redact_buffer();
    assert!(cursor_rows(&ctx).contains("on new"), "{screen}");
}

/// `esc` clears what was typed, and then closes the view, staying where the PR's screen was.
#[test]
fn esc_in_the_thread_view_clears_the_filter_then_closes_it() {
    let mut ctx = setup_clone!();
    let mut app = with_two_threads(&mut ctx);

    ctx.update(&mut app, keys("Otdrafts<esc>"));
    let screen = ctx.redact_buffer();
    assert!(screen.contains("on old"), "{screen}");
    assert!(!screen.contains("added: added.txt"), "{screen}");

    ctx.update(&mut app, keys("<esc>"));
    let screen = ctx.redact_buffer();
    assert!(screen.contains("added: added.txt"), "{screen}");
}

/// A branch with commits GitHub's PR doesn't have is reviewed as it is, with a row saying how it
/// stands against GitHub, as delta says elsewhere, and what to do about it.
#[test]
fn a_branch_ahead_of_its_pr_says_so_and_how_to_push() {
    let mut ctx = setup_clone!();
    let github = setup(&ctx);
    commit(&ctx.dir, "more.txt", "more\n");
    let mut app = init_app(&mut ctx, &github);

    ctx.update(&mut app, keys("OO"));

    let screen = screen_text(&ctx);
    assert!(
        screen.contains("Your local branch feature has 1 commit that GitHub's PR #7 doesn't"),
        "{screen}"
    );
    assert!(
        screen.contains("links to GitHub may be on the wrong lines"),
        "{screen}"
    );
    assert!(screen.contains("git push origin feature"), "{screen}");
}

/// Alt+up in the thread view's filter steps back through what was typed there before, and narrows
/// the view to it.
#[test]
fn alt_up_in_the_thread_view_gives_the_filter_typed_before() {
    let mut ctx = setup_clone!();
    let mut app = with_two_threads(&mut ctx);
    ctx.update(&mut app, keys("Otold<enter>"));

    ctx.update(&mut app, keys("Ot<alt+up>"));

    assert_eq!(ctx.prompt_line(), "Threads: old|");
    let screen = ctx.redact_buffer();
    assert!(screen.contains("on old"), "{screen}");
    assert!(!screen.contains("on new"), "{screen}");
}

/// `-` and `+` leave the cursor on the comment it was on, though the hunk the comment is under is
/// cut differently with less context.
#[test]
fn changing_the_context_keeps_the_cursor_on_its_comment() {
    let mut ctx = setup_clone!();
    let github = setup_wide_context(&ctx);
    let mut app = init_app(&mut ctx, &github);
    ctx.update(
        &mut app,
        keys("OO/changed<enter>ihello<ctrl+s>/hello<enter>"),
    );
    assert!(
        cursor_rows(&ctx).contains("hello"),
        "{}",
        ctx.redact_buffer()
    );

    ctx.update(&mut app, keys("-"));

    assert!(
        cursor_rows(&ctx).contains("hello"),
        "{}",
        ctx.redact_buffer()
    );
}

/// From a screen with no PR, `O t` opens the current branch's PR, as `O O` does, and shows its
/// threads; the one chosen is then shown in the PR's diff.
#[test]
fn o_t_with_no_pr_on_screen_opens_the_current_branchs() {
    let mut ctx = setup_clone!();
    let mut app = with_two_threads(&mut ctx);
    ctx.update(&mut app, keys("q"));
    assert!(!ctx.redact_buffer().contains("added: added.txt"));

    ctx.update(&mut app, keys("Ot"));
    let screen = ctx.redact_buffer();
    assert!(screen.contains("on old"), "{screen}");
    assert!(!screen.contains("added: added.txt"), "{screen}");

    // Keys run out at the end of each update, which closes the view: it is opened again, from the
    // status screen, and the thread chosen in one update.
    ctx.update(&mut app, keys("qOtold<enter>"));
    let screen = ctx.redact_buffer();
    assert!(screen.contains("added: added.txt"), "{screen}");
    assert!(cursor_rows(&ctx).contains("on old"), "{screen}");
}

/// Down in the thread view moves to the next thread, which enter then goes to.
#[test]
fn down_in_the_thread_view_moves_to_the_next_thread() {
    let mut ctx = setup_clone!();
    let mut app = with_two_threads(&mut ctx);

    ctx.update(&mut app, keys("Ot<down><enter>"));

    assert!(
        cursor_rows(&ctx).contains("on old"),
        "{}",
        ctx.redact_buffer()
    );
}
