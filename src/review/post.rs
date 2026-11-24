//! Posting a local PR's draft comments to GitHub, as the user's pending review there, so that a
//! review written locally, by the user or by an agent, can be revised and then shared.

use super::{
    github::{GitHub, GitHubBackend, GitHubPr},
    local::LocalBackend,
};
use crate::{Res, error::Error};
use git2::Repository;
use std::{collections::HashMap, process::Command, rc::Rc};

/// What [`post`] did.
pub(crate) struct Posted {
    pub url: String,
    pub threads: usize,
    pub replies: usize,
}

impl Posted {
    /// What to tell the user.
    pub fn summary(&self) -> String {
        let plural =
            |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
        let mut summary = format!("Posted {}", plural(self.threads, "thread", "threads"));
        if self.replies > 0 {
            summary += &format!(" and {}", plural(self.replies, "reply", "replies"));
        }
        summary
            + &format!(
                " to your pending review on {}. Submit it there to make it visible",
                self.url
            )
    }
}

/// The command that pushes `head`, in `repo`, to its remote (`origin` if it has none), setting it
/// as the branch's upstream.
pub(crate) fn push_command(repo: &Repository, head: &str) -> Res<Command> {
    let key = format!("branch.{head}.remote");
    let error = |e: git2::Error| Error::LocalPr(format!("Couldn't read git's {key}: {e}"));
    let remote = match repo.config().map_err(error)?.get_string(&key) {
        Ok(remote) => remote,
        Err(e) if e.code() == git2::ErrorCode::NotFound => "origin".to_owned(),
        Err(e) => return Err(error(e)),
    };
    let mut cmd = Command::new("git");
    cmd.args(["push", "--set-upstream", &remote, head]);
    cmd.current_dir(repo.workdir().unwrap_or(repo.path()));
    Ok(cmd)
}

/// Post the user's draft review of `local`'s PR, an agent's comments in it included, as their
/// pending review on GitHub, creating a draft PR if the branch has none, and then fetch the PR's
/// reviews. The branch must already be pushed. All of the draft review is posted, or none of it:
/// posting is refused if a thread's line has gone, or if the user has a pending review on GitHub
/// that this draft review did not start. If posting stops part way, as when the
/// network fails, each comment posted is recorded as it is, so that posting again posts the
/// rest into the same pending review. Each thread's first comment is posted before its replies.
pub(crate) fn post(local: &LocalBackend, github: Rc<dyn GitHub>, create_pr: bool) -> Res<Posted> {
    let to_post = local.to_post()?;
    let pr = match github.pr_for_branch(local.head())? {
        Some(pr) => pr,
        None if create_pr => github.create_pr(local.head(), local.base())?,
        None => {
            return Err(Error::Gh(format!(
                "Couldn't post: {} has no GitHub PR, and creating one wasn't asked for",
                local.head()
            )));
        }
    };
    check_pr(local, &pr).map_err(|e| Error::Gh(format!("Couldn't post: {e}")))?;
    local.record_link(&pr)?;
    let review = match (github.pending_review(&pr)?, &to_post.link) {
        (Some(pending), Some(link)) if pending == *link => pending,
        (Some(pending), _) => {
            let comments = github
                .reviews(&pr)?
                .into_iter()
                .find(|review| review.id == pending)
                .map(|review| review.comments.len())
                .ok_or_else(|| {
                    Error::Gh(format!(
                        "Couldn't post: GitHub says you have a pending review {pending}, but \
                         doesn't list it among the PR's reviews"
                    ))
                })?;
            let comments = match comments {
                0 => "no comments".to_owned(),
                1 => "1 comment".to_owned(),
                n => format!("{n} comments"),
            };
            return Err(Error::Gh(format!(
                "Couldn't post: you have a pending review on {}, with {comments}, that this \
                 draft review did not start. Submit or delete it there first",
                pr.url
            )));
        }
        (None, _) => github.start_review(&pr)?,
    };
    local.set_link(&review)?;
    let mut posted = Posted {
        url: pr.url.clone(),
        threads: 0,
        replies: 0,
    };
    let mut started: HashMap<String, String> = HashMap::new();
    for comment in to_post.comments {
        if comment.starts_thread {
            let first = github.add_thread(&review, &comment.path, comment.line, &comment.body)?;
            local.mark_posted(&comment.id, Some(&first))?;
            started.insert(comment.thread, first);
            posted.threads += 1;
        } else {
            let first = started
                .get(&comment.thread)
                .or(comment.reply_to.as_ref())
                .ok_or_else(|| {
                    Error::Gh("Couldn't post a reply: its thread is not on GitHub".to_owned())
                })?;
            github.add_reply(&review, first, &comment.body)?;
            local.mark_posted(&comment.id, None)?;
            posted.replies += 1;
        }
    }
    local.fetch(&GitHubBackend { github, pr })?;
    Ok(posted)
}

/// An error unless `pr` is into `local`'s base and at its head's commit, with its base commit
/// and its commits, so that their lines match.
pub(crate) fn check_pr(local: &LocalBackend, pr: &GitHubPr) -> Res<()> {
    if pr.base_ref != local.base() {
        return Err(Error::Gh(format!(
            "GitHub's PR #{} is into {}, not {}, so its lines would not match",
            pr.number,
            pr.base_ref,
            local.base()
        )));
    }
    let head = local.head_commit()?.to_string();
    if pr.head_oid != head {
        return Err(Error::Gh(format!(
            "GitHub's PR #{} is at {}, which is not at {}'s commit {}. \
             Push the branch, and try again once GitHub has it",
            pr.number,
            short(&pr.head_oid),
            local.head(),
            short(&head)
        )));
    }
    local.check_against(pr)
}

fn short(oid: &str) -> &str {
    &oid[..oid.len().min(7)]
}
