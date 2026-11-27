//! Syncing a local PR with GitHub: its draft comments are posted into the user's pending review
//! there, so that a review written locally, by the user or by an agent, can be revised and then
//! shared, and GitHub's reviews are copied into the local PR.

use super::{
    Backend, Place, Side, Subject,
    github::{GitHub, GitHubBackend, GitHubPr},
    local::{LocalBackend, Purpose},
    remote::{self, Change, Consent},
};
use crate::{Res, error::Error};
use std::{
    collections::{HashMap, HashSet},
    rc::Rc,
};

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

/// Refuse to post while a draft reply is in a thread whose first comment is local-only, since a
/// reply can't reach GitHub before its thread.
fn refuse_blocked_replies(local: &LocalBackend) -> Res<()> {
    let blocked = local.blocked_replies()?;
    if blocked.is_empty() {
        return Ok(());
    }
    Err(Error::Gh(format!(
        "Couldn't post: these draft replies are in threads whose first comment is local-only: \
         \"{}\". Move each thread's first comment to the draft review, or the reply to local-only",
        blocked.join("\", \"")
    )))
}

/// Refuse to post while a draft thread is on a line outside GitHub's diff of the PR, where GitHub
/// can't place it, as a thread can come to be when the code changes.
fn refuse_unplaceable(local: &LocalBackend, to_post: &[super::local::PostComment]) -> Res<()> {
    let mut unplaceable = vec![];
    for comment in to_post {
        if let Some(Place::Line(side, line)) = comment.starts
            && !local.in_diff(&comment.path, (side, line))?
        {
            unplaceable.push(format!("{}:{line}", comment.path));
        }
    }
    if unplaceable.is_empty() {
        return Ok(());
    }
    Err(Error::Gh(format!(
        "Posted nothing: these draft threads are on lines outside GitHub's diff of the PR, which \
         has 3 lines of context, so GitHub can't place them: {}. Make each local-only with p, or \
         delete it, then sync again",
        unplaceable.join(", ")
    )))
}

/// Sync `local`'s PR with GitHub. Its draft comments, an agent's included, are posted into the
/// user's pending review on GitHub, which is started if they have none, and then the PR's reviews
/// are fetched; with no draft comments, they are only fetched.
///
/// Sync never pushes or pulls: the branch must be where GitHub has it, and otherwise sync says
/// which command makes them agree (see [`LocalBackend::compare_with_github`]). Draft threads
/// written before the PR changed are not posted: they are moved with their lines, for the user to
/// check, and posted at the next sync. A branch with no PR on GitHub gets a draft PR only if
/// `create_pr`.
/// Local-only comments are never posted. Nothing is posted while a draft reply's thread has been
/// deleted on GitHub, or starts with a local-only comment. If posting stops part way, as when the
/// network fails, each comment posted is recorded as it is, so that syncing again posts the rest
/// into the same pending review, the only one the user can have; one GitHub added but whose reply
/// was lost is found among the user's comments, even once they have submitted it, and not posted
/// again. Each thread's first comment is posted before its replies.
pub(crate) fn sync(local: &LocalBackend, github: Rc<dyn GitHub>, create_pr: bool) -> Res<Synced> {
    let Some(pr) = github.pr_for_branch(local.head())? else {
        let to_post = local.to_post()?;
        if to_post.is_empty() {
            return Err(Error::Gh(format!(
                "Nothing to sync: there are no draft comments, and {} has no PR on GitHub",
                local.head()
            )));
        }
        refuse_blocked_replies(local)?;
        refuse_unplaceable(local, &to_post)?;
        if !create_pr {
            return Err(Error::Gh(format!(
                "Couldn't post: {} has no GitHub PR, and creating one wasn't asked for",
                local.head()
            )));
        }
        local
            .compare_with_github(None, Purpose::Post)
            .map_err(couldnt_sync)?;
        // The user asked for it, as create_pr says.
        let change = Change::CreatePr {
            head: local.head(),
            base: local.base(),
        };
        let pr = remote::write(github.as_ref(), change, Consent::Publish)?.pr()?;
        return post(local, github, pr, to_post);
    };
    local
        .compare_with_github(Some(&pr), Purpose::Post)
        .map_err(couldnt_sync)?;
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
    refuse_blocked_replies(local)?;
    // The lease: drafts written before the PR changed are moved with their lines, for the user
    // to see where they are, and nothing is posted.
    let stale = local.stale_drafts()?;
    if !stale.is_empty() {
        local.move_drafts(&stale)?;
        // Where each now is, as the PR's screen shows it.
        let places: Vec<String> = local
            .threads()?
            .into_iter()
            .filter(|thread| stale.contains(&thread.id))
            .map(|thread| match thread.line {
                Some((Side::New, line)) => format!("{}:{line}", thread.path),
                Some((Side::Old, line)) => format!("{}:{line} (old side)", thread.path),
                None => format!("{} (on the file, as its line has gone)", thread.path),
            })
            .collect();
        let (drafts, moved, each, them) = match stale.len() {
            1 => (
                "1 of your draft threads was".to_owned(),
                "It has moved with its line",
                "it is",
                "it",
            ),
            n => (
                format!("{n} of your draft threads were"),
                "They have moved with their lines",
                "each is",
                "them",
            ),
        };
        return Err(Error::Gh(format!(
            "Posted nothing: GitHub's PR #{} has changed since {drafts} written. {moved}, to {}. \
             If {each} on the line you meant, sync again to post {them} there",
            pr.number,
            places.join(", ")
        )));
    }
    let to_post = local.to_post()?;
    if to_post.is_empty() {
        return Ok(Synced::Fetched { url: pr.url });
    }
    refuse_unplaceable(local, &to_post)?;
    post(local, github, pr, to_post)
}

/// Post `to_post` into the user's pending review on `pr`, which is started if they have none and
/// a comment is to be added, and then fetch the PR's reviews.
fn post(
    local: &LocalBackend,
    github: Rc<dyn GitHub>,
    pr: GitHubPr,
    to_post: Vec<super::local::PostComment>,
) -> Res<Synced> {
    local.record_link(&pr)?;
    // Started only when a comment is to be added to it, as every one may be on GitHub already.
    let mut review = github.pending_review(&pr)?;
    // What an earlier attempt that didn't finish may have added, though gitu never heard back.
    let mut added = if to_post.iter().any(|c| c.posting_since.is_some()) {
        my_comments(github.as_ref(), &pr)?
    } else {
        vec![]
    };
    let (mut threads, mut replies) = (0, 0);
    let mut started: HashMap<String, String> = HashMap::new();
    for comment in to_post {
        let found = |at: &At, added: &mut Vec<Added>| {
            comment
                .posting_since
                .and_then(|since| take_added(added, at, &comment.body, since))
        };
        if let Some(place) = comment.starts {
            let at = At::Thread {
                path: comment.path.clone(),
                place,
            };
            let first = match found(&at, &mut added) {
                Some(added) => added,
                None => {
                    local.mark_posting(&comment.id)?;
                    let change = Change::AddThread {
                        review: &pending_review(github.as_ref(), &pr, &mut review)?,
                        path: &comment.path,
                        place,
                        body: &comment.body,
                    };
                    remote::write(github.as_ref(), change, Consent::Private)?.id()?
                }
            };
            local.mark_posted(&comment.id, Some(&first))?;
            started.insert(comment.thread.clone(), first);
            threads += 1;
        } else {
            let first = started
                .get(&comment.thread)
                .or(comment.reply_to.as_ref())
                .ok_or_else(|| {
                    Error::Gh("Couldn't post a reply: its thread is not on GitHub".to_owned())
                })?
                .clone();
            let at = At::Reply { to: first.clone() };
            if found(&at, &mut added).is_none() {
                local.mark_posting(&comment.id)?;
                let change = Change::AddReply {
                    review: &pending_review(github.as_ref(), &pr, &mut review)?,
                    to: &first,
                    body: &comment.body,
                };
                remote::write(github.as_ref(), change, Consent::Private)?;
            }
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

/// How much earlier than the attempt that posted it GitHub may say a comment was made, as this
/// machine's clock and GitHub's differ.
const CLOCK_SKEW: chrono::TimeDelta = chrono::TimeDelta::minutes(5);

/// The id of the user's pending review on `pr`: `review`, or one started if they have none.
fn pending_review(github: &dyn GitHub, pr: &GitHubPr, review: &mut Option<String>) -> Res<String> {
    if let Some(id) = review {
        return Ok(id.clone());
    }
    let id = remote::write(github, Change::StartReview { pr }, Consent::Private)?.id()?;
    *review = Some(id.clone());
    Ok(id)
}

/// A comment of the user's on GitHub, as an attempt to post a draft comment that didn't finish
/// may have added it.
struct Added {
    id: String,
    at: At,
    body: String,
    created_at: chrono::DateTime<chrono::FixedOffset>,
}

/// Where a comment is on GitHub: starting a thread at a place on a file, or replying to a
/// thread's first comment.
#[derive(PartialEq)]
enum At {
    Thread { path: String, place: Place },
    Reply { to: String },
}

/// The user's comments on `pr`, in their pending review or in one they have submitted since an
/// attempt added them.
fn my_comments(github: &dyn GitHub, pr: &GitHubPr) -> Res<Vec<Added>> {
    let mine: HashSet<String> = github
        .reviews(pr)?
        .into_iter()
        .filter(|review| review.mine)
        .flat_map(|review| review.comments)
        .collect();
    let mut added = vec![];
    for thread in github.threads(pr)? {
        let place = match (thread.subject, thread.line) {
            (Subject::File, _) => Some(Place::File),
            (Subject::Line, Some((side, line))) => Some(Place::Line(side, line)),
            // Outdated: not one just added, which is on a line of the PR's head.
            (Subject::Line, None) => None,
        };
        let Some(first) = thread.comments.first().map(|c| c.id.clone()) else {
            continue;
        };
        for (i, comment) in thread.comments.into_iter().enumerate() {
            if !mine.contains(&comment.id) {
                continue;
            }
            let at = match (i, place) {
                (0, Some(place)) => At::Thread {
                    path: thread.path.clone(),
                    place,
                },
                (0, None) => continue,
                _ => At::Reply { to: first.clone() },
            };
            added.push(Added {
                id: comment.id,
                at,
                body: comment.body,
                created_at: comment.created_at,
            });
        }
    }
    Ok(added)
}

/// The id of the comment in `added` that an attempt begun at `since` added at `at` saying
/// `body`, if there is one, taken from `added` so that each is found once.
fn take_added(
    added: &mut Vec<Added>,
    at: &At,
    body: &str,
    since: chrono::DateTime<chrono::FixedOffset>,
) -> Option<String> {
    let i = added.iter().position(|a| {
        a.at == *at && a.body.trim_end() == body.trim_end() && a.created_at >= since - CLOCK_SKEW
    })?;
    Some(added.remove(i).id)
}

fn couldnt_sync(e: Error) -> Error {
    Error::Gh(format!("Couldn't sync: {e}"))
}
