//! Reviewing a PR: its diff, with a Viewed checkbox on each file, review threads under the lines
//! they are on, and a box in the diff to write comments in. Where the PR and its threads come
//! from, and where comments go, is the PR's [`Backend`].

/// Instructions for an AI agent on reviewing code with a person, which
/// `delta --tui --agent-instructions` prints.
pub(crate) const AGENT_INSTRUCTIONS: &str = include_str!("agent_instructions.md");

pub(crate) mod diagram;
pub(crate) mod github;
#[cfg(test)]
pub(crate) mod lifecycle;
pub(crate) mod local;
pub(crate) mod post;

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
    fn comment(&self, path: &str, line: (Side, u32), body: &str) -> Res<&'static str>;
    /// Change what the comment `comment_id`, in the thread `thread_id`, says to `body`,
    /// returning what to tell the user. Only a comment in a draft review can be changed.
    fn edit_comment(&self, thread_id: &str, comment_id: &str, body: &str) -> Res<&'static str>;
    /// Delete the comment `comment_id`, in the thread `thread_id`, and the thread with it if it
    /// was the thread's only comment, returning what to tell the user. Only a comment in a draft
    /// review can be deleted.
    fn delete_comment(&self, thread_id: &str, comment_id: &str) -> Res<&'static str>;
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
    /// The PR in one line, as the top of its screen shows it, if the backend can say.
    fn summary(&self) -> Res<Option<String>> {
        Ok(None)
    }
    /// The page showing the file at `path` in the PR's diff, at `line` when given, or `None`
    /// when the backend has no pages.
    fn file_url(&self, path: &str, line: Option<(Side, u32)>) -> Option<String>;
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
    /// Whether the comment is in a submitted review, which everyone who can see the PR sees.
    pub submitted: bool,
    /// Whether `i` on it edits it, rather than replying: the user's own comment, in a draft
    /// review or in a review on GitHub, when the PR's backend can change that, and not written by
    /// an agent.
    pub editable: bool,
    /// Whether the user can delete it: any comment in a draft review, or their own comment in a
    /// review on GitHub, when the PR's backend can change that.
    pub deletable: bool,
}

/// A PR opened for review on a screen, with the checkbox state of each of its files, by path, and
/// its review threads.
/// The user's pending review on GitHub: only they can see its comments until they submit it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PendingReview {
    pub comments: usize,
}

pub(crate) struct OpenPr {
    pub backend: Rc<dyn Backend>,
    pub viewed: RefCell<HashMap<String, ViewedState>>,
    pub threads: RefCell<Vec<Thread>>,
    pub pending_review: RefCell<Option<PendingReview>>,
    /// The PR in one line (see [`Backend::summary`]).
    pub summary: RefCell<Option<String>>,
    /// The options comments were last drawn with, and the features in force they were asked
    /// with, as `DELTA_FEATURES` gives them; `None` until they are first asked for, and once the
    /// PR is read again, so that a change to delta's config counts.
    pub options: RefCell<Option<(Option<String>, Rc<CommentOptions>)>>,
    /// The comment being written, if one is.
    pub editor: RefCell<Option<CommentEditor>>,
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
        Ok(Self {
            viewed: RefCell::new(backend.viewed()?),
            threads: RefCell::new(backend.threads()?),
            pending_review: RefCell::new(backend.pending_review()?),
            summary: RefCell::new(backend.summary()?),
            options: RefCell::new(None),
            editor: RefCell::new(None),
            backend,
        })
    }

    /// Read the threads, the user's pending review on GitHub, and the summary of the PR from
    /// the backend again.
    pub fn reread_threads(&self) -> Res<()> {
        *self.threads.borrow_mut() = self.backend.threads()?;
        *self.pending_review.borrow_mut() = self.backend.pending_review()?;
        *self.summary.borrow_mut() = self.backend.summary()?;
        Ok(())
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

/// `delta --tui review`, which reads and writes a local PR's reviews without starting the TUI.
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
                let pr = pr_by(repo, Rc::clone(&github), head, base, &None)?;
                // Only fetching, there is nothing to push.
                if pr.to_post()?.is_empty() {
                    let synced = super::post::sync(&pr, github, *create_pr)?;
                    return writeln!(out, "{}", synced.summary()).map_err(|e| {
                        Error::LocalPr(format!("Couldn't print what was synced: {e}"))
                    });
                }
                if !create_pr && github.pr_for_branch(pr.head())?.is_none() {
                    return Err(Error::Gh(format!(
                        "Couldn't post: {} has no GitHub PR. Give --create-pr to push it and \
                         create a draft PR, which everyone who can see the repository can see",
                        pr.head()
                    )));
                }
                let output = super::post::push_command(pr.repo(), pr.head())?
                    .output()
                    .map_err(|e| Error::LocalPr(format!("Couldn't run git push: {e}")))?;
                if !output.status.success() {
                    return Err(Error::LocalPr(format!(
                        "Couldn't push {}: {}",
                        pr.head(),
                        String::from_utf8_lossy(&output.stderr).trim_end()
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
                head,
                base,
            } => {
                let body = read_body(input, "comment")?;
                let pr = pr_by(repo, Rc::clone(&github), head, base, agent)?;
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
                head,
                base,
            } => {
                let body = read_body(input, "reply")?;
                let pr = pr_by(repo, Rc::clone(&github), head, base, agent)?;
                pr.reply(thread, &body)?;
                Ok(())
            }
            ReviewCommand::Fetch { head, base } => {
                let pr = pr_by(repo, Rc::clone(&github), head, base, &None)?;
                pr.fetch(&github_backend(&pr, github)?)
            }
            ReviewCommand::Edit {
                comment,
                head,
                base,
            } => {
                let body = read_body(input, "comment")?;
                pr_by(repo, Rc::clone(&github), head, base, &None)?.edit(comment, &body)
            }
            ReviewCommand::Delete {
                comment,
                head,
                base,
            } => pr_by(repo, Rc::clone(&github), head, base, &None)?.delete(comment),
            ReviewCommand::Show {
                format: ShowFormat::Json,
                head,
                base,
                ..
            } => {
                let pr = LocalBackend::new(
                    Rc::new(repo),
                    Rc::clone(&github),
                    head.clone(),
                    base.clone(),
                )?;
                let json = serde_json::to_string_pretty(&pr.json()?).expect("JSON");
                writeln!(out, "{json}")
                    .map_err(|e| Error::LocalPr(format!("Couldn't print the review: {e}")))
            }
            ReviewCommand::Show { head, base, .. } => {
                let pr = LocalBackend::new(
                    Rc::new(repo),
                    Rc::clone(&github),
                    head.clone(),
                    base.clone(),
                )?;
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
    fn pr_by(
        repo: Repository,
        github: Rc<dyn GitHub>,
        head: &Option<String>,
        base: &Option<String>,
        agent: &Option<String>,
    ) -> Res<LocalBackend> {
        let pr = LocalBackend::new(Rc::new(repo), github, head.clone(), base.clone())?;
        // On stderr, which is not what a command prints, such as JSON.
        if let Some(unchecked) = pr.unchecked() {
            eprintln!("{unchecked}");
        }
        Ok(match agent {
            Some(agent) => pr.with_agent(agent.clone()),
            None => pr,
        })
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
