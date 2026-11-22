//! gitu follows the review lifecycle's state machine ([`crate::review::lifecycle`]): driven
//! through random sequences of the model's actions, it is in the model's state after each one,
//! and refuses what the model refuses. Since every state of the model keeps the lifecycle's
//! invariants, so does gitu, for the sequences tried.
//!
//! Three repositories, each a clone of one remote: my laptop (where my agent writes too, as me),
//! my desktop, and my colleague's. Everyone acts as an agent would, through `delta --tui review`;
//! on my laptop, every other comment is written with `--agent`, which must change nothing the
//! model sees. GitHub is [`Server`], a fake that keeps reviews for several users, and
//! that can fail part way through a post.
//!
//! The number of sequences is `GITU_LIFECYCLE_SEQUENCES` (default 6), each of
//! `GITU_LIFECYCLE_STEPS` actions (default 10), from the seed `GITU_LIFECYCLE_SEED`.

use super::*;
use crate::{
    error::Error,
    review::{
        Comment, Side, Thread, ViewedState,
        github::{GitHub, GitHubPr, GitHubReview, GitHubReviewState},
        lifecycle::{Action, CommentRef, Id, Model, Person, Repo, ReviewState, Rules},
    },
};
use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap},
    path::PathBuf,
    process::Command,
    rc::Rc,
};

/// The file every thread is on, one thread to a line, every line changed by the PR.
const FILE: &str = "lines.txt";
const LINES: u32 = 12;

fn login(person: Person) -> &'static str {
    match person {
        Person::Me => "me",
        Person::Colleague => "colleague",
    }
}

fn state_name(state: ReviewState) -> &'static str {
    match state {
        ReviewState::Pending => "pending",
        ReviewState::Submitted => "submitted",
        ReviewState::Dismissed => "dismissed",
    }
}

/// GitHub, for several users: one PR, of `feature` into `main` in `remote`.
struct Server {
    remote: PathBuf,
    reviews: Vec<ServerReview>,
    threads: Vec<ServerThread>,
    next: u32,
    /// How many more threads and replies can be added before adding one fails, as when the
    /// network fails part way through a post; `None` for no limit.
    fail_after: Option<usize>,
}

struct ServerReview {
    id: String,
    author: &'static str,
    state: ReviewState,
    comments: Vec<String>,
}

struct ServerThread {
    id: String,
    line: u32,
    /// Whether a commit has removed the thread's line, so that GitHub has it outdated.
    outdated: bool,
    /// (id, author, body), in order.
    comments: Vec<(String, &'static str, String)>,
}

impl Server {
    fn new_id(&mut self, prefix: &str) -> String {
        self.next += 1;
        format!("{prefix}{}", self.next)
    }

    fn pending_of(&self, user: &str) -> Option<usize> {
        self.reviews
            .iter()
            .position(|r| r.author == user && r.state == ReviewState::Pending)
    }

    fn visible_to(&self, user: &str) -> impl Iterator<Item = &ServerReview> {
        self.reviews
            .iter()
            .filter(move |r| r.state != ReviewState::Pending || r.author == user)
    }

    fn spend(&mut self) -> Result<(), Error> {
        match &mut self.fail_after {
            Some(0) => Err(Error::Gh("The network failed".to_owned())),
            Some(n) => {
                *n -= 1;
                Ok(())
            }
            None => Ok(()),
        }
    }

    fn add_comment(
        &mut self,
        review: &str,
        thread: usize,
        author: &'static str,
        body: &str,
    ) -> String {
        let id = self.new_id("C");
        self.threads[thread]
            .comments
            .push((id.clone(), author, body.to_owned()));
        let review = self.reviews.iter_mut().find(|r| r.id == review).unwrap();
        review.comments.push(id.clone());
        id
    }
}

/// [`Server`], as one user sees it through `gh`.
struct User {
    server: Rc<RefCell<Server>>,
    login: &'static str,
}

impl GitHub for User {
    fn pr(&self, _selector: Option<&str>) -> Result<GitHubPr, Error> {
        let remote = self.server.borrow().remote.clone();
        let oid = |branch: &str| {
            run(&remote, &["git", "rev-parse", branch])
                .trim()
                .to_owned()
        };
        Ok(GitHubPr {
            id: "PR".into(),
            number: 7,
            url: "https://github.com/o/r/pull/7".into(),
            base_ref: "main".into(),
            base_oid: oid("main"),
            head_oid: oid("feature"),
        })
    }

    fn checkout(&self, _pr: &GitHubPr) -> Command {
        unreachable!("the lifecycle does not check PRs out")
    }

    fn viewed(&self, _pr: &GitHubPr) -> Result<HashMap<String, ViewedState>, Error> {
        Ok(HashMap::new())
    }

    fn threads(&self, _pr: &GitHubPr) -> Result<Vec<Thread>, Error> {
        let server = self.server.borrow();
        let visible: HashMap<&str, bool> = server
            .visible_to(self.login)
            .flat_map(|r| {
                r.comments
                    .iter()
                    .map(move |c| (c.as_str(), r.state == ReviewState::Pending))
            })
            .collect();
        Ok(server
            .threads
            .iter()
            .filter(|t| {
                t.comments
                    .iter()
                    .any(|(id, ..)| visible.contains_key(id.as_str()))
            })
            .map(|t| Thread {
                id: t.id.clone(),
                path: FILE.to_owned(),
                line: (!t.outdated).then_some((Side::New, t.line)),
                resolved: false,
                comments: t
                    .comments
                    .iter()
                    .filter_map(|(id, author, body)| {
                        Some(Comment {
                            id: id.clone(),
                            author: (*author).to_owned(),
                            body: body.clone(),
                            url: None,
                            created_at: chrono::Utc::now().fixed_offset(),
                            draft: *visible.get(id.as_str())?,
                            editable: *visible.get(id.as_str())?,
                            deletable: *visible.get(id.as_str())?,
                            agent: None,
                        })
                    })
                    .collect(),
            })
            .collect())
    }

    fn reviews(&self, _pr: &GitHubPr) -> Result<Vec<GitHubReview>, Error> {
        Ok(self
            .server
            .borrow()
            .visible_to(self.login)
            .map(|r| GitHubReview {
                id: r.id.clone(),
                author: r.author.to_owned(),
                state: match r.state {
                    ReviewState::Pending => GitHubReviewState::Pending,
                    ReviewState::Submitted => GitHubReviewState::Submitted,
                    ReviewState::Dismissed => GitHubReviewState::Dismissed,
                },
                comments: r.comments.clone(),
                mine: r.author == self.login,
            })
            .collect())
    }

    fn pending_review(&self, _pr: &GitHubPr) -> Result<Option<String>, Error> {
        let server = self.server.borrow();
        Ok(server
            .pending_of(self.login)
            .map(|i| server.reviews[i].id.clone()))
    }

    fn start_review(&self, _pr: &GitHubPr) -> Result<String, Error> {
        let mut server = self.server.borrow_mut();
        assert!(
            server.pending_of(self.login).is_none(),
            "one pending review per user"
        );
        let id = server.new_id("R");
        server.reviews.push(ServerReview {
            id: id.clone(),
            author: self.login,
            state: ReviewState::Pending,
            comments: vec![],
        });
        Ok(id)
    }

    fn add_thread(
        &self,
        review_id: &str,
        path: &str,
        line: (Side, u32),
        body: &str,
    ) -> Result<String, Error> {
        assert_eq!((path, line.0), (FILE, Side::New));
        let mut server = self.server.borrow_mut();
        server.spend()?;
        let id = server.new_id("T");
        server.threads.push(ServerThread {
            id,
            line: line.1,
            outdated: false,
            comments: vec![],
        });
        let thread = server.threads.len() - 1;
        Ok(server.add_comment(review_id, thread, self.login, body))
    }

    fn add_reply(&self, review_id: &str, comment_id: &str, body: &str) -> Result<String, Error> {
        let mut server = self.server.borrow_mut();
        server.spend()?;
        let thread = server
            .threads
            .iter()
            .position(|t| t.comments.first().is_some_and(|(id, ..)| id == comment_id))
            .expect("a reply is to a thread's first comment");
        Ok(server.add_comment(review_id, thread, self.login, body))
    }

    fn edit_comment(&self, comment_id: &str, body: &str) -> Result<(), Error> {
        let mut server = self.server.borrow_mut();
        let Some(comment) = server
            .threads
            .iter_mut()
            .flat_map(|t| &mut t.comments)
            .find(|(id, ..)| id == comment_id)
        else {
            return Err(Error::Gh("Could not resolve to a node".to_owned()));
        };
        assert_eq!(comment.1, self.login, "only a comment's author edits it");
        comment.2 = body.to_owned();
        Ok(())
    }

    fn delete_comment(&self, comment_id: &str) -> Result<(), Error> {
        let mut server = self.server.borrow_mut();
        for thread in &mut server.threads {
            thread.comments.retain(|(id, ..)| id != comment_id);
        }
        server.threads.retain(|t| !t.comments.is_empty());
        for review in &mut server.reviews {
            review.comments.retain(|id| id != comment_id);
        }
        Ok(())
    }

    fn default_branch(&self) -> Result<String, Error> {
        Ok("main".to_owned())
    }

    fn pr_for_branch(&self, head: &str) -> Result<Option<GitHubPr>, Error> {
        assert_eq!(head, "feature");
        self.pr(None).map(Some)
    }

    fn create_pr(&self, _head: &str, _base: &str) -> Result<GitHubPr, Error> {
        unreachable!("the PR exists")
    }

    fn set_viewed(&self, _pr: &GitHubPr, _path: &str, _viewed: bool) -> Result<(), Error> {
        Ok(())
    }

    fn open(&self, _url: &str) -> Result<(), Error> {
        Ok(())
    }
}

/// What the lifecycle's state is made of, as both gitu and the model can be seen: a comment is
/// its text and its line, `None` once its line has gone, and a review is its author, its state
/// and its comments.
#[derive(Debug, PartialEq, Eq)]
struct Seen {
    github: Vec<SeenReview>,
    repos: BTreeMap<Repo, SeenRepo>,
}

type SeenComment = (String, Option<u32>);
type SeenReview = (String, String, Vec<SeenComment>);

#[derive(Debug, PartialEq, Eq)]
struct SeenRepo {
    /// The draft reviews, by author.
    drafts: BTreeMap<String, Vec<SeenComment>>,
    /// The local copy of GitHub's reviews.
    copies: Vec<SeenReview>,
}

/// The repositories, the fake GitHub, and which line each of the model's threads is on.
struct World {
    _ctx: TestContext,
    dirs: BTreeMap<Repo, PathBuf>,
    server: Rc<RefCell<Server>>,
    lines: BTreeMap<Id, u32>,
    next_line: u32,
    rules: Rules,
}

impl World {
    fn new(test_name: &str) -> Self {
        let ctx = TestContext::setup_clone(test_name);
        let laptop = ctx.dir.clone();
        let old: String = (1..=LINES).map(|n| format!("old{n}\n")).collect();
        let new: String = (1..=LINES).map(|n| format!("new{n}\n")).collect();
        commit(&laptop, FILE, &old);
        run(&laptop, &["git", "push", "origin", "main"]);
        run(&laptop, &["git", "checkout", "-b", "feature"]);
        commit(&laptop, FILE, &new);
        run(&laptop, &["git", "push", "-u", "origin", "feature"]);
        run(&laptop, &["git", "config", "user.name", "me"]);
        let mut dirs = BTreeMap::from([(Repo::Laptop, laptop)]);
        for (repo, user) in [(Repo::Desktop, "me"), (Repo::Colleague, "colleague")] {
            let dir = ctx.dir.parent().unwrap().join(format!("{repo:?}"));
            let url = url::Url::from_file_path(&ctx.remote_dir)
                .unwrap()
                .to_string();
            run(
                ctx.dir.parent().unwrap(),
                &["git", "clone", &url, dir.to_str().unwrap()],
            );
            run(&dir, &["git", "config", "user.name", user]);
            run(&dir, &["git", "config", "user.email", "ci@example.com"]);
            run(&dir, &["git", "checkout", "feature"]);
            dirs.insert(repo, dir);
        }
        for dir in dirs.values() {
            run(dir, &["git", "remote", "set-head", "origin", "main"]);
        }
        let server = Rc::new(RefCell::new(Server {
            remote: ctx.remote_dir.clone(),
            reviews: vec![],
            threads: vec![],
            next: 0,
            fail_after: None,
        }));
        World {
            _ctx: ctx,
            dirs,
            server,
            lines: BTreeMap::new(),
            next_line: 1,
            rules: Rules::default(),
        }
    }

    /// What `delta --tui review <args>` prints in `repo`, with `input` on stdin.
    fn review(&self, repo: Repo, args: &[&str], input: &str) -> Result<String, Error> {
        use clap::Parser;
        let args = crate::cli::Args::try_parse_from(["gitu", "review"].iter().chain(args))
            .unwrap_or_else(|e| panic!("{e}"));
        let Some(crate::cli::Commands::Review { command }) = args.command else {
            unreachable!()
        };
        let github = Rc::new(User {
            server: Rc::clone(&self.server),
            login: login(repo.github_user()),
        });
        let mut out = vec![];
        crate::review::cli::run(
            &command,
            git2::Repository::open(&self.dirs[&repo]).unwrap(),
            github,
            &mut input.as_bytes(),
            &mut out,
        )?;
        Ok(String::from_utf8(out).unwrap())
    }

    fn json(&self, repo: Repo) -> serde_json::Value {
        let out = self
            .review(repo, &["show", "--format", "json"], "")
            .unwrap_or_else(|e| panic!("show --format json in {repo:?}: {e}"));
        serde_json::from_str(&out).unwrap()
    }

    /// The id, in `repo`'s local PR, of the comment whose text is `body`.
    fn comment_id(&self, repo: Repo, body: &str) -> String {
        let json = self.json(repo);
        json["threads"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|t| t["comments"].as_array().unwrap())
            .find(|c| c["body"] == body)
            .unwrap_or_else(|| panic!("no comment {body} in {repo:?}: {json}"))["id"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    /// The id, in `repo`'s local PR, of the thread with a comment whose text is one of `bodies`.
    fn thread_id(&self, repo: Repo, bodies: &[String]) -> String {
        let json = self.json(repo);
        json["threads"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| {
                t["comments"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|c| bodies.iter().any(|b| c["body"] == b.as_str()))
            })
            .unwrap_or_else(|| panic!("no thread with {bodies:?} in {repo:?}: {json}"))["id"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    fn seen(&self) -> Seen {
        let server = self.server.borrow();
        let line_of = |thread: &ServerThread| (!thread.outdated).then_some(thread.line);
        let mut github: Vec<SeenReview> = server
            .reviews
            .iter()
            .map(|r| {
                let mut comments: Vec<SeenComment> = r
                    .comments
                    .iter()
                    .map(|id| {
                        let thread = server
                            .threads
                            .iter()
                            .find(|t| t.comments.iter().any(|(c, ..)| c == id))
                            .unwrap();
                        let (_, _, body) = thread.comments.iter().find(|(c, ..)| c == id).unwrap();
                        (body.clone(), line_of(thread))
                    })
                    .collect();
                comments.sort();
                (
                    r.author.to_owned(),
                    state_name(r.state).to_owned(),
                    comments,
                )
            })
            .collect();
        github.sort();
        drop(server);
        let repos = Repo::ALL
            .into_iter()
            .map(|repo| (repo, seen_repo(&self.json(repo))))
            .collect();
        Seen { github, repos }
    }

    /// Do `action`, which the model takes from `before` to `after`, or refuses when `after` is
    /// `None`, in gitu or on the fake GitHub.
    fn act(&mut self, before: &Model, action: &Action, after: Option<&Model>) {
        match (action, after) {
            (Action::Write { repo, thread }, Some(after)) => {
                let cid = *after.written.last().unwrap();
                let body = cid.to_string();
                let agent: &[&str] = if *repo == Repo::Laptop && cid % 2 == 0 {
                    &["--agent", "agent"]
                } else {
                    &[]
                };
                match thread {
                    None => {
                        let t = *after.threads.keys().last().unwrap();
                        let line = self.new_line(t);
                        let line = line.to_string();
                        let mut args = vec!["comment", "--path", FILE, "--line", &line];
                        args.extend(agent);
                        self.review(*repo, &args, &body).unwrap();
                    }
                    Some(t) => {
                        let bodies: Vec<String> = before.threads[t]
                            .comments
                            .iter()
                            .map(|c| body_of(before, *c))
                            .collect();
                        let thread = self.thread_id(*repo, &bodies);
                        let mut args = vec!["reply", thread.as_str()];
                        args.extend(agent);
                        self.review(*repo, &args, &body).unwrap();
                    }
                }
            }
            (Action::Write { .. }, None) => {}
            (Action::Delete { repo, comment }, after) => {
                let id = self.comment_id(*repo, &comment.to_string());
                let result = self.review(*repo, &["delete", &id], "");
                assert_eq!(result.is_ok(), after.is_some(), "{action}: {result:?}");
            }
            (Action::Post { repo, stop_after }, after) => {
                self.server.borrow_mut().fail_after = *stop_after;
                let result = self.review(*repo, &["post"], "");
                self.server.borrow_mut().fail_after = None;
                let finished = after.is_some_and(|after| after.repos[repo].draft.is_none());
                assert_eq!(result.is_ok(), finished, "{action}: {result:?}");
            }
            (Action::Fetch { repo }, _) => {
                self.review(*repo, &["fetch"], "").unwrap();
            }
            (Action::Submit { user }, Some(_)) => {
                let mut server = self.server.borrow_mut();
                let i = server.pending_of(login(*user)).unwrap();
                server.reviews[i].state = ReviewState::Submitted;
            }
            (Action::BrowserComment { user }, Some(after)) => {
                let t = *after.threads.keys().last().unwrap();
                let gid = *after.gh_comments.keys().last().unwrap();
                let line = self.new_line(t);
                let github = User {
                    server: Rc::clone(&self.server),
                    login: login(*user),
                };
                let pr = github.pr(None).unwrap();
                let review = match github.pending_review(&pr).unwrap() {
                    Some(review) => review,
                    None => github.start_review(&pr).unwrap(),
                };
                github
                    .add_thread(&review, FILE, (Side::New, line), &gid.to_string())
                    .unwrap();
            }
            (Action::Dismiss { review }, Some(_)) => {
                let bodies = review_bodies(before, *review);
                let mut server = self.server.borrow_mut();
                let server = &mut *server;
                let found = server.reviews.iter_mut().find(|r| {
                    let mut ours: Vec<String> = r
                        .comments
                        .iter()
                        .map(|id| server_threads_body(&server.threads, id))
                        .collect();
                    ours.sort();
                    r.state == ReviewState::Submitted && ours == bodies
                });
                found.expect("the review to dismiss").state = ReviewState::Dismissed;
            }
            (Action::ChangeCode { thread }, Some(_)) => {
                let line = self.lines[thread];
                let laptop = &self.dirs[&Repo::Laptop];
                let text = std::fs::read_to_string(laptop.join(FILE)).unwrap();
                let text = text.replace(&format!("new{line}\n"), &format!("changed{line}\n"));
                commit(laptop, FILE, &text);
                run(laptop, &["git", "push", "origin", "feature"]);
                for repo in [Repo::Desktop, Repo::Colleague] {
                    run(&self.dirs[&repo], &["git", "pull", "--ff-only"]);
                }
                for t in &mut self.server.borrow_mut().threads {
                    if t.line == line {
                        t.outdated = true;
                    }
                }
            }
            (Action::EditOnGitHub { repo, comment }, after) => {
                let body = text(before, *comment, before.repos[repo].copied_edits[comment]);
                let edited = github_body(before, *comment) + "+";
                let id = self.comment_id(*repo, &body);
                let result = self.review(*repo, &["edit", &id], &edited);
                assert_eq!(result.is_ok(), after.is_some(), "{action}: {result:?}");
            }
            (Action::DeleteOnGitHub { repo, comment }, after) => {
                let body = text(before, *comment, before.repos[repo].copied_edits[comment]);
                let id = self.comment_id(*repo, &body);
                let result = self.review(*repo, &["delete", &id], "");
                assert_eq!(result.is_ok(), after.is_some(), "{action}: {result:?}");
            }
            (Action::Submit { .. } | Action::BrowserComment { .. }, None)
            | (Action::Dismiss { .. } | Action::ChangeCode { .. }, None) => {}
        }
    }

    fn new_line(&mut self, thread: Id) -> u32 {
        let line = self.next_line;
        assert!(line <= LINES, "more threads than lines");
        self.next_line += 1;
        self.lines.insert(thread, line);
        line
    }

    /// The model's state, seen as [`World::seen`] sees gitu's.
    fn model_seen(&self, m: &Model) -> Seen {
        let line = |thread: Id| m.threads[&thread].placeable.then(|| self.lines[&thread]);
        // A review on GitHub, with each comment's text as `edits` gives it.
        let review = |r: &crate::review::lifecycle::GitHubReview, edits: &dyn Fn(Id) -> u32| {
            let mut comments: Vec<SeenComment> = r
                .comments
                .iter()
                .map(|g| (text(m, *g, edits(*g)), line(m.gh_comments[g].thread)))
                .collect();
            comments.sort();
            (
                login(r.author).to_owned(),
                state_name(r.state).to_owned(),
                comments,
            )
        };
        let now = |g: Id| m.gh_comments[&g].edits;
        let mut github: Vec<SeenReview> = m.gh_reviews.values().map(|r| review(r, &now)).collect();
        github.sort();
        let repos = m
            .repos
            .iter()
            .map(|(repo, r)| {
                let drafts = r
                    .draft
                    .iter()
                    .map(|d| {
                        let mut comments: Vec<SeenComment> = d
                            .comments
                            .iter()
                            .map(|c| (c.to_string(), line(r.comments[c].thread)))
                            .collect();
                        comments.sort();
                        (login(repo.github_user()).to_owned(), comments)
                    })
                    .collect();
                let fetched = |g: Id| r.copied_edits[&g];
                let mut copies: Vec<SeenReview> = r
                    .copy
                    .iter()
                    .flatten()
                    .map(|(_, review_)| review(review_, &fetched))
                    .collect();
                copies.sort();
                (*repo, SeenRepo { drafts, copies })
            })
            .collect();
        Seen { github, repos }
    }
}

fn server_threads_body(threads: &[ServerThread], id: &str) -> String {
    threads
        .iter()
        .flat_map(|t| &t.comments)
        .find(|(c, ..)| c == id)
        .unwrap()
        .2
        .clone()
}

/// The text of a comment in the model.
fn body_of(m: &Model, comment: CommentRef) -> String {
    match comment {
        CommentRef::Local(c) => c.to_string(),
        CommentRef::GitHub(g) => github_body(m, g),
    }
}

/// The text of a comment on GitHub in the model: the local comment it was posted as, or its own
/// id, with a `+` for each time it has been edited.
fn github_body(m: &Model, g: Id) -> String {
    text(m, g, m.gh_comments[&g].edits)
}

/// The text of the comment on GitHub `g` once edited `edits` times.
fn text(m: &Model, g: Id, edits: u32) -> String {
    let c = &m.gh_comments[&g];
    format!("{}{}", c.origin.unwrap_or(g), "+".repeat(edits as usize))
}

/// The texts of a review's comments in the model, sorted.
fn review_bodies(m: &Model, review: Id) -> Vec<String> {
    let mut bodies: Vec<String> = m.gh_reviews[&review]
        .comments
        .iter()
        .map(|g| body_of(m, CommentRef::GitHub(*g)))
        .collect();
    bodies.sort();
    bodies
}

/// A repository's local PR, as `delta --tui review show --format json` prints it.
fn seen_repo(json: &serde_json::Value) -> SeenRepo {
    let mut by_review: BTreeMap<String, Vec<SeenComment>> = BTreeMap::new();
    for thread in json["threads"].as_array().unwrap() {
        let line = thread["line"].as_u64().map(|n| n as u32);
        for comment in thread["comments"].as_array().unwrap() {
            by_review
                .entry(comment["review"].as_str().unwrap().to_owned())
                .or_default()
                .push((comment["body"].as_str().unwrap().to_owned(), line));
        }
    }
    let mut drafts = BTreeMap::new();
    let mut copies = vec![];
    for review in json["reviews"].as_array().unwrap() {
        let mut comments = by_review
            .remove(review["id"].as_str().unwrap())
            .unwrap_or_default();
        comments.sort();
        let author = review["author"].as_str().unwrap().to_owned();
        match review["state"].as_str().unwrap() {
            "draft" => {
                drafts.insert(author, comments);
            }
            state => copies.push((author, state.to_owned(), comments)),
        }
    }
    assert!(by_review.is_empty(), "comments in no review: {json}");
    copies.sort();
    SeenRepo { drafts, copies }
}

/// A small, seeded random number generator, so that a failing sequence can be run again.
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % n as u64) as usize
    }
}

fn env_number(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

#[test]
fn gitu_follows_the_review_lifecycle() {
    let sequences = env_number("GITU_LIFECYCLE_SEQUENCES", 6);
    let steps = env_number("GITU_LIFECYCLE_STEPS", 10);
    let seed = env_number("GITU_LIFECYCLE_SEED", 0x9e37_79b9_7f4a_7c15);
    let mut rng = Rng(seed);
    for sequence in 0..sequences {
        let mut world = World::new(&format!("gitu_follows_the_review_lifecycle_{sequence}"));
        let mut model = Model::default();
        let mut done: Vec<String> = vec![];
        for _ in 0..steps {
            let actions = model.actions(3);
            let action = actions[rng.below(actions.len())].clone();
            let after = model.apply(&action, world.rules);
            done.push(format!(
                "{action}{}",
                if after.is_some() { "" } else { " (refused)" }
            ));
            world.act(&model, &action, after.as_ref());
            if let Some(after) = after {
                model = after;
            }
            let expected = world.model_seen(&model);
            let seen = world.seen();
            assert_eq!(
                seen,
                expected,
                "gitu is not in the model's state after:\n  - {}",
                done.join("\n  - ")
            );
        }
    }
}
