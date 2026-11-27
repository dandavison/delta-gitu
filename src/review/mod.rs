//! Reviewing a PR: its diff, with a Viewed checkbox on each file, review threads under the lines
//! they are on, and a box in the diff to write comments in. Where the PR and its threads come
//! from, and where comments go, is the PR's [`Backend`].

/// Instructions for an AI agent on reviewing code with a person, which
/// `delta tui --agent-instructions` prints.
pub(crate) const AGENT_INSTRUCTIONS: &str = include_str!("agent_instructions.md");

pub(crate) mod diagram;
pub(crate) mod github;
#[cfg(test)]
pub(crate) mod lifecycle;
pub(crate) mod local;
pub(crate) mod post;
pub(crate) mod remote;
pub(crate) mod standing;

use crate::screen::review::CommentOptions;
use crate::text_area::TextArea;
use crate::{Res, git::diff::Diff, gitu_diff::Status};
use git2::Repository;
use std::{cell::RefCell, collections::HashMap, rc::Rc};

/// What to tell the user when the PR's `which` commit (`base` or `head`), `oid` of the branch
/// `branch` on GitHub, isn't in the repository.
pub(crate) fn missing_commit(which: &str, oid: &str, branch: &str) -> String {
    format!(
        "The PR's {which} commit {} is not in the repository: fetch {branch} from the PR's \
         repository, and try again",
        &oid[..oid.len().min(7)]
    )
}

/// What to tell the user once a comment is added to their draft review.
pub(crate) const ADDED_TO_DRAFT: &str = "Comment added to your draft review";

/// Where a PR's review threads and Viewed checkboxes are read from and written to. A backend is
/// for one PR.
pub(crate) trait Backend {
    /// What to give `git diff` for the PR's diff: from where its head branched from its base,
    /// `<base>...<head>`.
    fn diff_range(&self) -> String;
    /// Whether HEAD is not the commit the PR's lines are numbered against, for a PR reviewed
    /// checked out, so that its diff, comments and links may not match.
    fn head_mismatch(&self, repo: &Repository) -> bool;
    /// The checkbox state of each file of the PR, by path.
    fn viewed(&self) -> Res<HashMap<String, ViewedState>>;
    /// Tick or untick the checkbox of the file at `path`.
    fn set_viewed(&self, path: &str, viewed: bool) -> Res<()>;
    /// The PR's review threads.
    fn threads(&self) -> Res<Vec<Thread>>;
    /// Add `body` as a reply to the review thread `thread_id`, returning what to tell the user.
    fn reply(&self, thread_id: &str, body: &str) -> Res<&'static str>;
    /// Add `body` as a new review comment on `line` of the file at `path`, numbered as in the
    /// PR's head commit for the new side and its base for the old, returning what to tell the
    /// user.
    fn comment(&self, path: &str, line: (Side, u32), body: &str) -> Res<String>;
    /// Change what the comment `comment_id`, in the thread `thread_id`, says to `body`,
    /// returning what to tell the user. A local comment can be changed, and the user's own on
    /// GitHub, publicly only with `consent` (see [`remote::write`]).
    fn edit_comment(
        &self,
        thread_id: &str,
        comment_id: &str,
        body: &str,
        consent: remote::Consent,
    ) -> Res<&'static str>;
    /// Delete the comment `comment_id`, in the thread `thread_id`, and the thread with it if it
    /// was the thread's only comment, returning what to tell the user, as
    /// [`Backend::edit_comment`] changes one.
    fn delete_comment(
        &self,
        thread_id: &str,
        comment_id: &str,
        consent: remote::Consent,
    ) -> Res<&'static str>;
    /// Read what the backend copies from elsewhere again, as `g r` asks.
    fn refresh(&self) -> Res<()> {
        Ok(())
    }
    /// The backend, if it is a local PR's.
    fn as_local(&self) -> Option<&local::LocalBackend> {
        None
    }
    /// The user's pending review on GitHub, as last fetched, if they have one.
    fn pending_review(&self) -> Res<Option<PendingReview>> {
        Ok(None)
    }
    /// The messages agents are working on: printed by `delta tui review listen`, and the agent
    /// not yet listening for the next.
    fn working(&self) -> Res<Vec<Working>> {
        Ok(vec![])
    }
    /// The PR in one line, as the top of its screen shows it, if the backend can say.
    fn summary(&self) -> Res<Option<String>> {
        Ok(None)
    }
    /// The page showing the file at `path` in the PR's diff, at `line` when given, or `None`
    /// when the backend has no pages.
    fn file_url(&self, path: &str, line: Option<(Side, u32)>) -> Option<String>;
    /// The PR's page, or `None` when the backend has no pages.
    fn url(&self) -> Option<String>;
}

/// The Viewed checkbox on each file of a PR. Each reviewer has their own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ViewedState {
    Viewed,
    Unviewed,
    /// Marked viewed, but the file has changed since.
    Dismissed,
}

/// The side of a PR's diff a line is on: the old file's, for a removed line,
/// or the new file's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Side {
    Old,
    New,
}

/// What a thread is on: a line, or, as GitHub allows, a whole file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Subject {
    #[default]
    Line,
    File,
}

/// Where a new thread goes on GitHub: on a line of a side, or on its whole file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Place {
    Line(Side, u32),
    File,
}

/// A review thread: comments on a line of the PR's diff, and the replies.
#[derive(Clone, Debug)]
pub(crate) struct Thread {
    /// The id the backend knows the thread by.
    pub id: String,
    pub path: String,
    /// The line the thread is on, numbered as in the PR's head commit for the new side and its
    /// base for the old. None for a thread on a whole file, and once the line has changed since
    /// the thread was started (GitHub calls the thread outdated).
    pub line: Option<(Side, u32)>,
    /// For a thread on several lines, the first, numbered as `line` is; `line` is the last.
    pub start_line: Option<u32>,
    pub subject: Subject,
    pub resolved: bool,
    /// Whether the thread was on GitHub and has been deleted there, leaving only draft replies.
    pub deleted_on_github: bool,
    pub comments: Vec<Comment>,
}

#[derive(Clone, Debug)]
pub(crate) struct Comment {
    /// The id the backend knows the comment by.
    pub id: String,
    /// The author's login: "ghost" for a GitHub account since deleted, as GitHub shows it.
    pub author: String,
    /// The name of the agent that wrote it as its author, if one did.
    pub agent: Option<String>,
    pub body: String,
    /// The comment's page, when the backend has pages.
    pub url: Option<String>,
    pub created_at: chrono::DateTime<chrono::FixedOffset>,
    /// Whether the comment is in a draft review, which only its reviewer sees.
    pub draft: bool,
    /// Whether the comment is local-only: in no review, so that sync never posts it.
    pub local_only: bool,
    /// The reactions to it on GitHub, one for each emoji.
    pub reactions: Vec<Reaction>,
    /// Whether the comment is in the user's pending review on GitHub, which only they see until
    /// they submit it.
    pub pending: bool,
    /// Whether the comment is in a submitted review, which everyone who can see the PR sees.
    pub submitted: bool,
    /// Whether `i` on it edits it, rather than replying: the user's own comment, local (in the
    /// draft review, or local-only) or in a review on GitHub, when the PR's backend can change
    /// that, and not written by an agent.
    pub editable: bool,
    /// Whether the user can delete it: any local comment, in the draft review or local-only, or
    /// their own comment in a review on GitHub, when the PR's backend can change that.
    pub deletable: bool,
}

/// The people who reacted to a comment on GitHub with one emoji.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Reaction {
    pub emoji: String,
    /// Who reacted, as many as GitHub gave.
    pub people: Vec<Reactor>,
    /// How many reacted, which may be more than `people` has.
    pub count: usize,
}

/// Someone who reacted to a comment, and their page on GitHub.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Reactor {
    pub login: String,
    pub url: String,
}

/// `backend`'s threads: a local PR's placed in `version`, its commits as the screen shows them.
fn threads_in(backend: &dyn Backend, version: Option<local::Version>) -> Res<Vec<Thread>> {
    match (backend.as_local(), version) {
        (Some(local), Some(version)) => local.threads_at(&version),
        _ => backend.threads(),
    }
}

/// A message an agent is working on.
#[derive(Clone, Debug)]
pub(crate) struct Working {
    /// The thread the message is about, or none for the whole PR.
    pub thread: Option<String>,
    /// When the agent started: when `listen` printed the message.
    pub since: chrono::DateTime<chrono::FixedOffset>,
}

/// The user's pending review on GitHub: only they can see its comments until they submit it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PendingReview {
    pub comments: usize,
}

/// A PR opened for review on a screen, with the checkbox state of each of its files, by path, and
/// its review threads.
pub(crate) struct OpenPr {
    pub backend: Rc<dyn Backend>,
    pub viewed: RefCell<HashMap<String, ViewedState>>,
    pub threads: RefCell<Vec<Thread>>,
    /// The messages agents are working on.
    pub working: RefCell<Vec<Working>>,
    pub pending_review: RefCell<Option<PendingReview>>,
    /// The PR in one line (see [`Backend::summary`]).
    pub summary: RefCell<Option<String>>,
    /// The options comments were last drawn with, and the features in force they were asked
    /// with, as `DELTA_FEATURES` gives them; `None` until they are first asked for, and once the
    /// PR is read again, so that a change to delta's config counts.
    pub options: RefCell<Option<(Option<String>, Rc<CommentOptions>)>>,
    /// The comment being written, if one is.
    pub editor: RefCell<Option<CommentEditor>>,
    /// The time to draw the PR at, when a test sets it: otherwise, the current time.
    pub now: std::cell::Cell<Option<chrono::DateTime<chrono::FixedOffset>>>,
    /// Whether a thread was pulsing when the PR was last drawn, so that it is drawn again on each
    /// tick until none is.
    pub pulsing: std::cell::Cell<bool>,
    /// For a local PR, the commits its diff on screen is of, as last read, which its threads are
    /// placed in; none once the diff's range is changed with `:`, as it is then not the PR's.
    pub version: std::cell::Cell<Option<local::Version>>,
    /// The git command the PR's screen shows its diff with, whose range [`OpenPr::repin`] keeps
    /// at `version`.
    pub git: RefCell<Option<Rc<RefCell<crate::calling_process::GitCommand>>>>,
}

/// A comment being written, in a box in the diff.
pub(crate) struct CommentEditor {
    pub target: EditorTarget,
    pub text: TextArea,
    /// Whether keys go to it.
    pub focused: bool,
    /// The columns its text was last wrapped to, which moving up and down goes by.
    pub width: usize,
}

/// What a comment being written is: a reply to a review thread, a new comment on a line of the
/// diff, or a draft comment being changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum EditorTarget {
    Reply {
        thread_id: String,
    },
    Line {
        path: String,
        line: (Side, u32),
    },
    Edit {
        thread_id: String,
        comment_id: String,
    },
}

/// How many columns [`OpenPr::checkbox`] takes before a file's row. A PR's diff is rendered
/// that much narrower, so that a row taking the whole width still fits beside it.
pub(crate) const FILE_PREFIX_WIDTH: u16 = 4;

impl OpenPr {
    /// Open the PR `backend` is for, reading its checkboxes and threads.
    pub fn new(backend: Rc<dyn Backend>) -> Res<Self> {
        let version = backend
            .as_local()
            .map(local::LocalBackend::version)
            .transpose()?;
        Ok(Self {
            viewed: RefCell::new(backend.viewed()?),
            threads: RefCell::new(threads_in(backend.as_ref(), version)?),
            version: std::cell::Cell::new(version),
            git: RefCell::new(None),
            working: RefCell::new(backend.working()?),
            pending_review: RefCell::new(backend.pending_review()?),
            summary: RefCell::new(backend.summary()?),
            options: RefCell::new(None),
            editor: RefCell::new(None),
            now: std::cell::Cell::new(None),
            pulsing: std::cell::Cell::new(false),
            backend,
        })
    }

    /// Read the threads, the messages agents are working on, the user's pending review on GitHub,
    /// and the summary of the PR from the backend again.
    pub fn reread_threads(&self) -> Res<()> {
        *self.threads.borrow_mut() = threads_in(self.backend.as_ref(), self.version.get())?;
        *self.working.borrow_mut() = self.backend.working()?;
        *self.pending_review.borrow_mut() = self.backend.pending_review()?;
        *self.summary.borrow_mut() = self.backend.summary()?;
        Ok(())
    }

    /// Read a local PR's commits again, as its branch may have moved, have its screen's diff show
    /// them, and read its threads in them, as [`OpenPr::reread_threads`] does. The screen's rows
    /// are to be built again after.
    pub fn repin(&self) -> Res<()> {
        if let (Some(local), Some(old)) = (self.backend.as_local(), self.version.get()) {
            let new = local.version()?;
            if new != old {
                // The file view has no git command: it reads its file in `version` itself.
                let replaced = match &*self.git.borrow() {
                    Some(git) => git.borrow_mut().replace_arg(&old.range(), &new.range()),
                    None => true,
                };
                self.version.set(replaced.then_some(new));
            }
        }
        self.reread_threads()
    }

    /// What to give `git diff` for the PR's diff: a local PR's in the commits it was last read in.
    pub fn diff_range(&self) -> String {
        match self.version.get() {
            Some(version) => version.range(),
            None => self.backend.diff_range(),
        }
    }

    /// The checkbox drawn before the file at `path`: blank for a file the backend does not have
    /// in the PR, such as one changed by a commit not pushed.
    pub fn checkbox(&self, path: &str) -> &'static str {
        match self.viewed.borrow().get(path) {
            Some(ViewedState::Viewed) => "[x] ",
            Some(ViewedState::Unviewed) => "[ ] ",
            Some(ViewedState::Dismissed) => "[~] ",
            None => "    ",
        }
    }

    /// The comment `comment_id` in the thread `thread_id`, as last read.
    pub fn comment(&self, thread_id: &str, comment_id: &str) -> Option<Comment> {
        self.threads
            .borrow()
            .iter()
            .find(|thread| thread.id == thread_id)?
            .comments
            .iter()
            .find(|comment| comment.id == comment_id)
            .cloned()
    }

    /// What is drawn after the name of the file at `path`: how many comments its unresolved
    /// review threads have, as `[3 comments]`, or `None` when it has none.
    pub fn comment_count(&self, path: &str) -> Option<String> {
        let count: usize = self
            .threads
            .borrow()
            .iter()
            .filter(|thread| thread.path == path && !thread.resolved)
            .map(|thread| thread.comments.len())
            .sum();
        match count {
            0 => None,
            1 => Some("[1 comment]".to_owned()),
            n => Some(format!("[{n} comments]")),
        }
    }
}

/// The path a file of `diff` is known by in a PR: its new path, or for a deleted file its old
/// one.
pub(crate) fn file_path(diff: &Diff, file_i: usize) -> String {
    let header = &diff.file_diffs[file_i].header;
    let path = match header.status {
        Status::Deleted => &header.old_file,
        _ => &header.new_file,
    };
    path.fmt(&diff.text).into_owned()
}

/// `delta tui review`, which reads and writes a local PR's reviews without starting the TUI.
pub(crate) mod cli {
    use super::{
        Backend, Side,
        github::{GitHub, GitHubBackend},
        local::LocalBackend,
    };
    use crate::{
        Res,
        cli::{ReviewCommand, ReviewSide, ShowFormat},
        error::Error,
    };
    use git2::Repository;
    use std::{
        io::{Read, Write},
        rc::Rc,
    };

    /// Run `command` on the repository `repo`, with `github` for posting to GitHub, reading what it reads from `input` and writing
    /// what it prints to `out`.
    pub(crate) fn run(
        command: &ReviewCommand,
        repo: Repository,
        github: Rc<dyn GitHub>,
        input: &mut impl Read,
        out: &mut impl Write,
    ) -> Res<()> {
        match command {
            ReviewCommand::Sync {
                create_pr,
                head,
                base,
            } => {
                // Sync compares the branch with GitHub itself.
                let pr = local_pr(repo, Rc::clone(&github), head, base)?;
                if !create_pr
                    && github.pr_for_branch(pr.head())?.is_none()
                    && !pr.to_post()?.is_empty()
                {
                    return Err(Error::Gh(format!(
                        "Couldn't post: {} has no GitHub PR. Give --create-pr to create a draft \
                         PR, which everyone who can see the repository can see",
                        pr.head()
                    )));
                }
                let synced = super::post::sync(&pr, github, *create_pr)?;
                writeln!(out, "{}", synced.summary())
                    .map_err(|e| Error::LocalPr(format!("Couldn't print what was synced: {e}")))
            }
            ReviewCommand::Comment {
                path,
                line,
                side,
                agent,
                local,
                draft,
                head,
                base,
            } => {
                let body = read_body(input, "comment")?;
                let pr = with_bucket(
                    pr_by(repo, Rc::clone(&github), head, base, agent)?,
                    *local,
                    *draft,
                );
                let (side, old) = match side {
                    ReviewSide::New => (Side::New, ""),
                    ReviewSide::Old => (Side::Old, " (old side)"),
                };
                if !pr.in_diff(path, (side, *line))? {
                    return Err(Error::LocalPr(format!(
                        "Couldn't comment: {path}:{line}{old} is not in the PR's diff"
                    )));
                }
                pr.comment(path, (side, *line), &body)?;
                Ok(())
            }
            ReviewCommand::Reply {
                thread,
                agent,
                local,
                draft,
                head,
                base,
            } => {
                let body = read_body(input, "reply")?;
                let pr = with_bucket(
                    pr_by(repo, Rc::clone(&github), head, base, agent)?,
                    *local,
                    *draft,
                );
                pr.reply(thread, &body)?;
                Ok(())
            }
            ReviewCommand::Move {
                comment,
                local,
                head,
                base,
                ..
            } => pr_by(repo, Rc::clone(&github), head, base, &None)?.move_comment(comment, *local),
            ReviewCommand::Fetch { head, base } => {
                let pr = pr_by(repo, Rc::clone(&github), head, base, &None)?;
                pr.fetch(&github_backend(&pr, github)?)
            }
            ReviewCommand::Edit {
                comment,
                publish,
                head,
                base,
            } => {
                let body = read_body(input, "comment")?;
                pr_by(repo, Rc::clone(&github), head, base, &None)?.edit(
                    comment,
                    &body,
                    consent(*publish),
                )
            }
            ReviewCommand::Delete {
                comment,
                publish,
                head,
                base,
            } => pr_by(repo, Rc::clone(&github), head, base, &None)?
                .delete(comment, consent(*publish)),
            ReviewCommand::Show {
                format: ShowFormat::Json,
                head,
                base,
                ..
            } => {
                let pr = review_of(Rc::new(repo), Rc::clone(&github), head, base)?;
                let json = serde_json::to_string_pretty(&pr.json()?).expect("JSON");
                writeln!(out, "{json}")
                    .map_err(|e| Error::LocalPr(format!("Couldn't print the review: {e}")))
            }
            ReviewCommand::Listen { head, base } => {
                let repo = Rc::new(repo);
                // The branch's files review, and its PR if it has one, unless --base says which.
                let reviews = match base {
                    Some(_) => vec![review_of(repo, Rc::clone(&github), head, base)?],
                    None => {
                        let files = LocalBackend::for_files(
                            Rc::clone(&repo),
                            Rc::clone(&github),
                            head.clone(),
                        )?;
                        match LocalBackend::new(repo, Rc::clone(&github), head.clone(), None) {
                            Ok(pr) => vec![files, pr],
                            Err(Error::NoPrToReview(_)) => vec![files],
                            Err(e) => return Err(e),
                        }
                    }
                };
                let messages = super::local::wait_for_messages(&reviews)?;
                // Made waiting again if they can't be printed, so that none is lost.
                let printed = messages
                    .iter()
                    .map(|(i, message)| reviews[*i].message_markdown(message))
                    .collect::<Res<String>>()
                    .and_then(|printed| {
                        out.write_all(printed.as_bytes()).map_err(|e| {
                            Error::LocalPr(format!("Couldn't print the messages: {e}"))
                        })
                    });
                if let Err(e) = printed {
                    for (i, review) in reviews.iter().enumerate() {
                        let taken: Vec<_> = messages
                            .iter()
                            .filter(|(at, _)| *at == i)
                            .map(|(_, message)| message.clone())
                            .collect();
                        review.undeliver(&taken)?;
                    }
                    return Err(e);
                }
                Ok(())
            }
            ReviewCommand::Show { head, base, .. } => {
                let pr = review_of(Rc::new(repo), Rc::clone(&github), head, base)?;
                let markdown = pr.markdown()?;
                out.write_all(markdown.as_bytes())
                    .map_err(|e| Error::LocalPr(format!("Couldn't print the review: {e}")))
            }
        }
    }

    /// The GitHub PR of `local`'s branch.
    fn github_backend(local: &LocalBackend, github: Rc<dyn GitHub>) -> Res<GitHubBackend> {
        let pr = github
            .pr_for_branch(local.head())?
            .ok_or_else(|| Error::Gh(format!("{} has no open GitHub PR", local.head())))?;
        Ok(GitHubBackend { github, pr })
    }

    /// The local PR `head` into `base`, whose comments are labelled with `agent`'s name if given.
    /// It isn't compared with GitHub, as opening the PR and sync compare it: what an agent writes
    /// is about the branch as it is here.
    fn pr_by(
        repo: Repository,
        github: Rc<dyn GitHub>,
        head: &Option<String>,
        base: &Option<String>,
        agent: &Option<String>,
    ) -> Res<LocalBackend> {
        let pr = local_pr(repo, github, head, base)?;
        Ok(match agent {
            Some(agent) => pr.with_agent(agent.clone()),
            None => pr,
        })
    }

    /// `pr`, writing local-only comments if `local`, and draft comments if `draft`; by default,
    /// as [`LocalBackend::with_bucket`] says.
    fn with_bucket(pr: LocalBackend, local: bool, draft: bool) -> LocalBackend {
        match (local, draft) {
            (true, _) => pr.with_bucket(true),
            (_, true) => pr.with_bucket(false),
            _ => pr,
        }
    }

    /// The local PR `head` into `base`, or, with `base` the branch itself, the branch's files
    /// review (see [`LocalBackend::for_files`]).
    fn review_of(
        repo: Rc<Repository>,
        github: Rc<dyn GitHub>,
        head: &Option<String>,
        base: &Option<String>,
    ) -> Res<LocalBackend> {
        let files = LocalBackend::for_files(Rc::clone(&repo), Rc::clone(&github), head.clone())?;
        if base.as_deref() == Some(files.head()) {
            return Ok(files);
        }
        LocalBackend::new(repo, github, head.clone(), base.clone())
    }

    /// The local PR `head` into `base` (see [`review_of`]), saying on stderr if it wasn't checked
    /// against GitHub.
    fn local_pr(
        repo: Repository,
        github: Rc<dyn GitHub>,
        head: &Option<String>,
        base: &Option<String>,
    ) -> Res<LocalBackend> {
        let pr = review_of(Rc::new(repo), github, head, base)?;
        // On stderr, which is not what a command prints, such as JSON.
        if let Some(unchecked) = pr.unchecked() {
            eprintln!("{unchecked}");
        }
        Ok(pr)
    }

    /// The consent `--publish` gives.
    fn consent(publish: bool) -> super::remote::Consent {
        if publish {
            super::remote::Consent::Publish
        } else {
            super::remote::Consent::Private
        }
    }

    /// A comment or reply, `what`, read from `input`, without the newline it ends with.
    fn read_body(input: &mut impl Read, what: &str) -> Res<String> {
        let mut body = String::new();
        input
            .read_to_string(&mut body)
            .map_err(|e| Error::LocalPr(format!("Couldn't read the {what}: {e}")))?;
        if body.trim().is_empty() {
            return Err(Error::LocalPr(format!(
                "Nothing to {what}: the {what} on stdin is empty"
            )));
        }
        Ok(body.trim_end().to_owned())
    }
}
