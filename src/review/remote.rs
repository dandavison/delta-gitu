//! Every change gitu makes on GitHub goes through [`write`], which knows which changes others can
//! see. A change only the user can see, such as to their pending review, is made. One that others
//! can see is made only if the user said to publish it. Whether a change to a comment is one others
//! can see is decided from the comment as GitHub has it when the change is made, not from a copy
//! fetched before.

use super::{
    Place,
    github::{CommentNow, GitHub, GitHubPr},
};
use crate::{Res, error::Error};

/// A change on GitHub.
pub(crate) enum Change<'a> {
    /// Start the user's pending review on `pr`.
    StartReview { pr: &'a GitHubPr },
    /// Add a thread to the user's pending review `review`.
    AddThread {
        review: &'a str,
        path: &'a str,
        place: Place,
        body: &'a str,
    },
    /// Add a reply to the first comment `to`, in the user's pending review `review`.
    AddReply {
        review: &'a str,
        to: &'a str,
        body: &'a str,
    },
    /// Edit the user's own comment to say `body`. It said `was` when last fetched, and must still:
    /// otherwise it was changed on GitHub since, and the edit would overwrite that.
    EditComment {
        comment: &'a str,
        body: &'a str,
        was: &'a str,
    },
    /// Delete the user's own comment, which said `was` when last fetched, as
    /// [`Change::EditComment`] edits one.
    DeleteComment { comment: &'a str, was: &'a str },
    /// Tick or untick a file's Viewed checkbox, which only the user sees.
    SetViewed {
        pr: &'a GitHubPr,
        path: &'a str,
        viewed: bool,
    },
    /// Create a draft PR of `head` into `base`, which everyone who can see the repository sees.
    CreatePr { head: &'a str, base: &'a str },
}

impl Change<'_> {
    /// Whether the change is one others can see, asking GitHub about a comment to be changed,
    /// which must be the user's, and as it was when last fetched.
    fn publishes(&self, github: &dyn GitHub) -> Res<bool> {
        match self {
            Change::EditComment { comment, was, .. } | Change::DeleteComment { comment, was } => {
                let now = self.as_fetched(github, comment, was)?;
                Ok(now.public)
            }
            Change::CreatePr { .. } => Ok(true),
            Change::StartReview { .. }
            | Change::AddThread { .. }
            | Change::AddReply { .. }
            | Change::SetViewed { .. } => Ok(false),
        }
    }

    /// The comment `comment` as GitHub has it, if it is the user's and says `was`, as when last
    /// fetched.
    fn as_fetched(&self, github: &dyn GitHub, comment: &str, was: &str) -> Res<CommentNow> {
        let refuse = |why: &str| Err(Error::Gh(format!("Couldn't {}: {why}", self.what())));
        let Some(now) = github.comment_now(comment)? else {
            return refuse(
                "it was deleted on GitHub since delta last fetched. Fetch (g r, or review fetch) \
                 to see the PR's reviews as they are",
            );
        };
        if !now.mine {
            return refuse("someone else wrote it");
        }
        if now.body != was {
            return refuse(
                "it was changed on GitHub since delta last fetched. Fetch (g r, or review fetch), \
                 check it, then change it again",
            );
        }
        Ok(now)
    }

    fn what(&self) -> String {
        match self {
            Change::StartReview { .. } => "start your pending review".to_owned(),
            Change::AddThread { path, .. } => format!("add a thread on {path}"),
            Change::AddReply { .. } => "add a reply".to_owned(),
            Change::EditComment { comment, .. } => format!("edit comment {comment}"),
            Change::DeleteComment { comment, .. } => format!("delete comment {comment}"),
            Change::SetViewed { path, .. } => format!("set {path}'s Viewed checkbox"),
            Change::CreatePr { head, .. } => format!("create a PR for {head}"),
        }
    }
}

/// Whether the user said to publish: to make changes others can see.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Consent {
    Private,
    Publish,
}

/// What a [`Change`] made: the id GitHub gave a review or comment, or the PR created.
pub(crate) enum Made {
    Id(String),
    Pr(GitHubPr),
    Nothing,
}

impl Made {
    /// The id made, which a change that adds something makes.
    pub fn id(self) -> Res<String> {
        match self {
            Made::Id(id) => Ok(id),
            _ => Err(Error::Gh("GitHub gave no id for what was added".to_owned())),
        }
    }

    /// The PR made, which [`Change::CreatePr`] makes.
    pub fn pr(self) -> Res<GitHubPr> {
        match self {
            Made::Pr(pr) => Ok(pr),
            _ => Err(Error::Gh(
                "GitHub gave no PR for the one created".to_owned(),
            )),
        }
    }
}

/// Make `change` on GitHub, unless others would see it and the user hasn't said to publish it.
pub(crate) fn write(github: &dyn GitHub, change: Change, consent: Consent) -> Res<Made> {
    if change.publishes(github)? && consent != Consent::Publish {
        return Err(Error::Gh(format!(
            "Couldn't {}: others would see it on GitHub, so it needs your say-so: y when delta \
             asks, or --publish. If delta didn't ask, its review was submitted since delta last \
             fetched: fetch first (g r, or review fetch)",
            change.what()
        )));
    }
    Ok(match change {
        Change::StartReview { pr } => Made::Id(github.start_review(pr)?),
        Change::AddThread {
            review,
            path,
            place,
            body,
        } => Made::Id(github.add_thread(review, path, place, body)?),
        Change::AddReply { review, to, body } => Made::Id(github.add_reply(review, to, body)?),
        Change::EditComment { comment, body, .. } => {
            github.edit_comment(comment, body)?;
            Made::Nothing
        }
        Change::DeleteComment { comment, .. } => {
            github.delete_comment(comment)?;
            Made::Nothing
        }
        Change::SetViewed { pr, path, viewed } => {
            github.set_viewed(pr, path, viewed)?;
            Made::Nothing
        }
        Change::CreatePr { head, base } => Made::Pr(github.create_pr(head, base)?),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::helpers::github::NoGitHub;

    /// A change others would see is refused before GitHub is asked to make it. Whether a change
    /// to a comment is one is GitHub's to say, which `tests::local_pr` tests with a fake GitHub.
    #[test]
    fn a_change_others_would_see_needs_consent() {
        let change = Change::CreatePr {
            head: "h",
            base: "b",
        };
        let error = write(&NoGitHub, change, Consent::Private)
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("needs your say-so"), "{error}");
    }

    /// A change only the user sees needs no consent, so GitHub is asked.
    #[test]
    fn a_private_change_goes_to_github() {
        let change = Change::AddReply {
            review: "r",
            to: "c",
            body: "b",
        };
        let error = write(&NoGitHub, change, Consent::Private)
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("This test has no GitHub"), "{error}");
    }
}
