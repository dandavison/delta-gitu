//! Syncing a local PR with GitHub: its draft comments are posted into the user's pending review
//! there, so that a review written locally, by the user or by an agent, can be revised and then
//! shared, and GitHub's reviews are copied into the local PR.

use super::{
    github::{GitHub, GitHubBackend, GitHubPr},
    local::LocalBackend,
};
use crate::{Res, error::Error};
use git2::Repository;
use std::{collections::HashMap, process::Command, rc::Rc};

/// What [`sync`] did.
pub(crate) enum Synced {
    /// It posted draft comments into the user's pending review on the PR at `url`, and fetched.
    Posted {
        url: String,
        threads: usize,
        replies: usize,
    },
    /// There were no draft comments, so it only fetched from the PR at `url`.
    Fetched { url: String },
}

impl Synced {
    /// What to tell the user.
    pub fn summary(&self) -> String {
        let plural =
            |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
        match self {
            Synced::Posted {
                url,
                threads,
                replies,
            } => {
                let mut summary = format!("Posted {}", plural(*threads, "thread", "threads"));
                if *replies > 0 {
                    summary += &format!(" and {}", plural(*replies, "reply", "replies"));
                }
                summary
                    + &format!(
                        " to your pending review on {url}. Submit it there to make it visible"
                    )
            }
            Synced::Fetched { url } => {
                format!("No draft comments to post: fetched the PR's reviews from {url}")
            }
        }
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

/// Sync `local`'s PR with GitHub, running commands such as `git push` with `run`. Its draft
/// comments, an agent's included, are posted into the user's pending review on GitHub, which is
/// started if they have none, and then the PR's reviews are fetched; with no draft comments, they
/// are only fetched.
///
/// Sync never pushes to a PR on GitHub: a branch behind GitHub's head is brought up to it, and one
/// with commits the PR doesn't have is refused. A branch with no PR on GitHub is pushed, and gets
/// a draft PR, only if `create_pr`. Nothing is posted while a draft reply's thread has been deleted
/// on GitHub. If posting stops part way, as when the network fails, each comment posted is
/// recorded as it is, so that syncing again posts the rest into the same pending review, the only
/// one the user can have. Each thread's first comment is posted before its replies.
pub(crate) fn sync(
    local: &LocalBackend,
    github: Rc<dyn GitHub>,
    create_pr: bool,
    run: &mut dyn FnMut(Command) -> Res<()>,
) -> Res<Synced> {
    let Some(pr) = github.pr_for_branch(local.head())? else {
        let to_post = local.to_post()?;
        if to_post.is_empty() {
            return Err(Error::Gh(format!(
                "Nothing to sync: there are no draft comments, and {} has no PR on GitHub",
                local.head()
            )));
        }
        if !create_pr {
            return Err(Error::Gh(format!(
                "Couldn't post: {} has no GitHub PR, and creating one wasn't asked for",
                local.head()
            )));
        }
        run(push_command(local.repo(), local.head())?)?;
        let pr = github.create_pr(local.head(), local.base())?;
        let pr = once_github_has_head(local, github.as_ref(), pr)?;
        return post(local, github, pr, to_post);
    };
    up_to_github_head(local, github.as_ref(), &pr, run)?;
    local.record_link(&pr)?;
    local.fetch(&GitHubBackend {
        github: Rc::clone(&github),
        pr: pr.clone(),
    })?;
    let orphans = local.orphaned_replies()?;
    if !orphans.is_empty() {
        return Err(Error::Gh(format!(
            "Couldn't post: the threads of these draft replies were deleted on GitHub: \"{}\". \
             Delete them, or write them as new comments",
            orphans.join("\", \"")
        )));
    }
    let to_post = local.to_post()?;
    if to_post.is_empty() {
        return Ok(Synced::Fetched { url: pr.url });
    }
    post(local, github, pr, to_post)
}

/// Post `to_post` into the user's pending review on `pr`, which is started if they have none,
/// and then fetch the PR's reviews.
fn post(
    local: &LocalBackend,
    github: Rc<dyn GitHub>,
    pr: GitHubPr,
    to_post: Vec<super::local::PostComment>,
) -> Res<Synced> {
    check_pr(local, &pr).map_err(|e| Error::Gh(format!("Couldn't post: {e}")))?;
    local.record_link(&pr)?;
    let review = match github.pending_review(&pr)? {
        Some(pending) => pending,
        None => github.start_review(&pr)?,
    };
    let (mut threads, mut replies) = (0, 0);
    let mut started: HashMap<String, String> = HashMap::new();
    for comment in to_post {
        if let Some(place) = comment.starts {
            let first = github.add_thread(&review, &comment.path, place, &comment.body)?;
            local.mark_posted(&comment.id, Some(&first))?;
            started.insert(comment.thread, first);
            threads += 1;
        } else {
            let first = started
                .get(&comment.thread)
                .or(comment.reply_to.as_ref())
                .ok_or_else(|| {
                    Error::Gh("Couldn't post a reply: its thread is not on GitHub".to_owned())
                })?;
            github.add_reply(&review, first, &comment.body)?;
            local.mark_posted(&comment.id, None)?;
            replies += 1;
        }
    }
    let url = pr.url.clone();
    local.fetch(&GitHubBackend { github, pr })?;
    Ok(Synced::Posted {
        url,
        threads,
        replies,
    })
}

/// Bring `local`'s branch up to `pr`'s head on GitHub, with `gh pr checkout`, which only fast-
/// forwards it. Never the other way: a branch with commits the PR doesn't have is refused.
fn up_to_github_head(
    local: &LocalBackend,
    github: &dyn GitHub,
    pr: &GitHubPr,
    run: &mut dyn FnMut(Command) -> Res<()>,
) -> Res<()> {
    if local.head_commit()?.to_string() == pr.head_oid {
        return Ok(());
    }
    if local.is_older_commit_of_head(&pr.head_oid)? {
        return Err(Error::Gh(format!(
            "Couldn't sync: {} has commits GitHub's PR #{} doesn't. Sync never pushes: push them \
             first, then sync",
            local.head(),
            pr.number
        )));
    }
    run(github.checkout(pr))?;
    let head = local.head_commit()?.to_string();
    if head != pr.head_oid {
        return Err(Error::Gh(format!(
            "Couldn't sync: {} is at {}, not at GitHub's PR #{}'s head, {}: it has commits the PR \
             doesn't, or the PR's branch was rewritten",
            local.head(),
            &head[..7],
            pr.number,
            &pr.head_oid[..pr.head_oid.len().min(7)]
        )));
    }
    Ok(())
}

/// How long GitHub is given to show a pushed commit as its PR's head.
const PUSH_SEEN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// `pr` as GitHub has it once it shows the branch's commit, just pushed, as the PR's head, which
/// it does not at once: while GitHub has an older commit of the branch, it is asked again, for up
/// to [`PUSH_SEEN_TIMEOUT`]. A PR at any other commit is as it is, for [`check_pr`] to refuse.
fn once_github_has_head(local: &LocalBackend, github: &dyn GitHub, pr: GitHubPr) -> Res<GitHubPr> {
    let deadline = std::time::Instant::now() + PUSH_SEEN_TIMEOUT;
    let mut pr = pr;
    while local.is_older_commit_of_head(&pr.head_oid)? && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_secs(1));
        pr = github.pr_for_branch(local.head())?.ok_or_else(|| {
            Error::Gh(format!(
                "Couldn't post: GitHub's PR for {} has gone while it was posted to",
                local.head()
            ))
        })?;
    }
    Ok(pr)
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
