//! What delta requires of the branch here against what GitHub has of it, and what it says when
//! the branch falls short.
//!
//! delta requires the branch here to contain every commit GitHub has of it, and, to post to
//! GitHub, nothing more. What GitHub has of the branch is its PR's head, or else the branch on
//! GitHub.
//!
//! delta never pushes or pulls. When the branch falls short, it says what it requires and how the
//! branch stands, and suggests a command only where git refuses to do harm if the command is not
//! what the user wants: a fetch, a push that isn't forced, or a pull that only fast-forwards.

use std::fmt;

/// Why delta compares the branch with GitHub, which says what it requires.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Purpose {
    /// To review the PR: the branch may have commits GitHub doesn't, as while they are being
    /// written, but must have GitHub's.
    Review,
    /// To post to the PR on GitHub, or create it: the branch must be where GitHub has it.
    Post,
}

/// What GitHub has of the branch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Theirs {
    /// The branch's PR, by its number.
    Pr(u64),
    /// The branch itself, which has no PR.
    Branch,
}

/// Each way the branch here can fall short of what delta requires.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Shortfall {
    /// GitHub's, as `what` says, is at `commit`, which this repository doesn't have, so how the
    /// branch stands can't be told.
    NotHere { what: String, commit: String },
    /// To post: the branch has `commits` that GitHub's doesn't.
    Ahead { commits: usize },
    /// GitHub's has `commits` that the branch doesn't.
    Behind { commits: usize },
    /// Each has commits the other doesn't, as when the branch was rewritten, or both have new
    /// work.
    Diverged { ahead: usize, behind: usize },
    /// To post: GitHub has no branch of this name.
    NotOnGitHub,
    /// To create a PR: the branch has commits of its base, `commits`, that GitHub's base branch
    /// doesn't, which GitHub would show as the PR's.
    BaseNotOnGitHub { commits: Vec<String> },
    /// To create a PR: GitHub's base branch has commits that the base here doesn't.
    BaseBehind,
}

/// A shortfall of the branch `branch`, with what it is said in terms of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct OutOfStep {
    pub shortfall: Shortfall,
    pub purpose: Purpose,
    pub theirs: Theirs,
    pub branch: String,
    /// The branch's base branch, as the PR is into it.
    pub base: String,
    /// The remote that commands push to and fetch from.
    pub remote: String,
}

impl OutOfStep {
    /// What is wrong with the local branch.
    pub fn problem(&self) -> String {
        let (branch, base) = (&self.branch, &self.base);
        let theirs = self.theirs();
        match &self.shortfall {
            Shortfall::NotHere { what, commit } => format!(
                "Your local repository doesn't have commit {}, which {what} is on",
                short(commit)
            ),
            Shortfall::Ahead { commits } => format!(
                "Your local branch {branch} has {} that {theirs} doesn't",
                count(*commits)
            ),
            Shortfall::Behind { commits } => format!(
                "Your local branch {branch} is missing {} that {theirs} has",
                count(*commits)
            ),
            Shortfall::Diverged { ahead, behind } => format!(
                "Your local branch {branch} and {theirs} have diverged: {} only in yours, {} \
                 only in GitHub's",
                count(*ahead),
                count(*behind)
            ),
            Shortfall::NotOnGitHub => format!("Your local branch {branch} is not on GitHub"),
            Shortfall::BaseNotOnGitHub { commits } => format!(
                "Your local branch {branch} has {} from {base} that GitHub's {base} doesn't: {}",
                count(commits.len()),
                commits.join(", ")
            ),
            Shortfall::BaseBehind => {
                format!("Your local {base} is missing commits that GitHub's {base} has")
            }
        }
    }

    /// What delta requires of the local branch, for what is being done.
    pub fn requirement(&self) -> String {
        let base = &self.base;
        match (&self.shortfall, self.purpose) {
            (Shortfall::BaseNotOnGitHub { .. }, _) => format!(
                "To create a PR on GitHub, the commits your branch has from {base} must be on \
                 GitHub's {base}, or GitHub would show them in the PR"
            ),
            (Shortfall::BaseBehind, _) => format!(
                "To create a PR on GitHub, your local {base} must contain every commit of \
                 GitHub's {base}"
            ),
            (_, Purpose::Post) => "To sync a PR to GitHub, your local branch must be on exactly \
                                  the same commit as the remote branch"
                .to_owned(),
            (_, Purpose::Review) => "To review a PR, your local branch must contain every \
                                     commit of the remote branch"
                .to_owned(),
        }
    }

    /// A command that meets the requirement, where one can do no harm if it is not what the user
    /// wants: git refuses a push that isn't a fast-forward, and a pull that only fast-forwards,
    /// rather than overwrite anything. Where the user has to judge what to do, as with a branch
    /// rewritten or a base branch with commits GitHub's doesn't have, there is none.
    pub fn command(&self) -> Option<String> {
        let (branch, remote) = (&self.branch, &self.remote);
        match &self.shortfall {
            Shortfall::NotHere { .. } | Shortfall::BaseBehind => {
                Some(format!("git fetch {remote}"))
            }
            Shortfall::Ahead { .. } => Some(format!("git push {remote} {branch}")),
            Shortfall::Behind { .. } => Some(match self.theirs {
                Theirs::Pr(number) => format!("gh pr checkout {number}"),
                Theirs::Branch => format!("git pull --ff-only {remote} {branch}"),
            }),
            Shortfall::NotOnGitHub => Some(format!("git push -u {remote} {branch}")),
            Shortfall::Diverged { .. } | Shortfall::BaseNotOnGitHub { .. } => None,
        }
    }

    fn theirs(&self) -> String {
        match self.theirs {
            Theirs::Pr(number) => format!("GitHub's PR #{number}"),
            Theirs::Branch => format!("GitHub's {}", self.branch),
        }
    }
}

impl fmt::Display for OutOfStep {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}. {}.", self.problem(), self.requirement())?;
        if let Some(command) = self.command() {
            write!(f, " To get there: {command}")?;
        }
        Ok(())
    }
}

/// `n` commits, in words.
fn count(n: usize) -> String {
    match n {
        1 => "1 commit".to_owned(),
        n => format!("{n} commits"),
    }
}

fn short(oid: &str) -> &str {
    &oid[..oid.len().min(7)]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn out_of_step(shortfall: Shortfall, theirs: Theirs) -> OutOfStep {
        OutOfStep {
            shortfall,
            purpose: Purpose::Post,
            theirs,
            branch: "feature".to_owned(),
            base: "main".to_owned(),
            remote: "origin".to_owned(),
        }
    }

    /// Which shortfalls a command is suggested for: only those where git refuses to do harm.
    #[test]
    fn a_command_is_suggested_only_where_it_can_do_no_harm() {
        let pr = Theirs::Pr(7);
        let cases = [
            (
                Shortfall::NotHere {
                    what: "GitHub's PR #7".to_owned(),
                    commit: "1234567890".to_owned(),
                },
                pr,
                Some("git fetch origin"),
            ),
            (
                Shortfall::Ahead { commits: 1 },
                pr,
                Some("git push origin feature"),
            ),
            (
                Shortfall::Behind { commits: 1 },
                pr,
                Some("gh pr checkout 7"),
            ),
            (
                Shortfall::Behind { commits: 1 },
                Theirs::Branch,
                Some("git pull --ff-only origin feature"),
            ),
            (
                Shortfall::Diverged {
                    ahead: 1,
                    behind: 1,
                },
                pr,
                None,
            ),
            (
                Shortfall::NotOnGitHub,
                Theirs::Branch,
                Some("git push -u origin feature"),
            ),
            (
                Shortfall::BaseNotOnGitHub {
                    commits: vec!["d576095".to_owned()],
                },
                Theirs::Branch,
                None,
            ),
            (
                Shortfall::BaseBehind,
                Theirs::Branch,
                Some("git fetch origin"),
            ),
        ];
        for (shortfall, theirs, command) in cases {
            let out_of_step = out_of_step(shortfall, theirs);
            assert_eq!(out_of_step.command().as_deref(), command, "{out_of_step}");
        }
    }
}
