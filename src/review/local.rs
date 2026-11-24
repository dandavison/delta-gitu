//! Local PRs: a branch reviewed against a base branch, with no PR on GitHub. What the review
//! records is kept in a file in the repository's shared git directory, so that every worktree of
//! the repository sees it (see [`Store`]).

use super::{
    ADDED_TO_DRAFT as ADDED, Backend, Comment, PendingReview, Side, Thread, ViewedState,
    github::{GitHub, GitHubBackend, GitHubPr, GitHubReviewState},
    missing_commit,
};
use crate::{Res, error::Error};
use git2::{Oid, Repository};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs, io,
    path::{Path, PathBuf},
    rc::Rc,
    time::{Duration, Instant},
};

/// The [`Backend`] of a local PR: the branch `head` into the branch `base`.
pub(crate) struct LocalBackend {
    repo: Rc<Repository>,
    head: String,
    base: String,
    /// The ref the PR is diffed against, when it is not linked to a GitHub PR: the branch `base`
    /// or its remote's (see [`resolve_base`]).
    base_ref: String,
    /// Who is reviewing: git's `user.name`, the author of every comment written here.
    reviewer: String,
    /// The agent writing as the reviewer, whose name labels the comments it writes.
    agent: Option<String>,
    store: Store,
    /// GitHub, which gives the PR's base, and through which the user's own comments in reviews
    /// copied from GitHub are edited and deleted.
    github: Rc<dyn GitHub>,
    /// The GitHub PR the local PR is linked to, if gh gave one for the branch: its reviews are
    /// fetched on `g r`, its Viewed checkboxes and pages are GitHub's, and its diff is from where
    /// it branched from its base on GitHub.
    linked: Option<GitHubBackend>,
    /// Why the PR was not checked against GitHub, when gh couldn't say what the branch's GitHub
    /// PR is.
    unchecked: Option<String>,
}

impl LocalBackend {
    /// The current branch, as a local PR into the base gh gives (see [`LocalBackend::new`]).
    pub fn for_current_branch(repo: Rc<Repository>, github: Rc<dyn GitHub>) -> Res<Self> {
        Self::new(repo, github, None, None)
    }

    /// The branch `head` (by default, the current branch) as a local PR into the branch `base`,
    /// or, if none is given, the branch's PR as [`find_pr`] finds it: linked to the branch's
    /// GitHub PR if gh gives one, which is then recorded as its link. The PR is diffed against
    /// `base` or its remote's branch (see [`resolve_base`]). A PR linked to a GitHub PR must
    /// have its base and its commits (see [`LocalBackend::check_against`]).
    pub fn new(
        repo: Rc<Repository>,
        github: Rc<dyn GitHub>,
        head: Option<String>,
        base: Option<String>,
    ) -> Res<Self> {
        let not_on_branch = || no_pr("HEAD is not on a branch");
        let head = match head {
            Some(head) => head,
            None => {
                let head_ref = repo.head().map_err(|_| not_on_branch())?;
                if !head_ref.is_branch() {
                    return Err(not_on_branch());
                }
                head_ref.shorthand().ok_or_else(not_on_branch)?.to_owned()
            }
        };
        let reviewer = repo
            .config()
            .and_then(|config| config.get_string("user.name"))
            .map_err(|_| {
                Error::LocalPr(
                    "Set git's user.name: your comments in a local PR are by that name".to_owned(),
                )
            })?;
        let (base, github_pr, unchecked) = match base {
            Some(base) => (base, None, None),
            None => find_pr(&repo, github.as_ref(), &head, &reviewer)?,
        };
        if head == base {
            return Err(no_pr(&format!("{base} is the base branch")));
        }
        let store = Store::new(repo.commondir(), &head, &base, &reviewer);
        let base_ref = resolve_base(&repo, &base)?;
        let pr = Self {
            repo,
            head,
            base,
            base_ref,
            reviewer,
            agent: None,
            store,
            github: Rc::clone(&github),
            linked: None,
            unchecked,
        };
        pr.check_commits()?;
        let Some(github_pr) = github_pr else {
            return Ok(pr);
        };
        let pr = pr.with_linked_pr(GitHubBackend {
            github,
            pr: github_pr.clone(),
        })?;
        pr.record_link(&github_pr)?;
        Ok(pr)
    }

    /// Why the PR was not checked against GitHub, if it wasn't.
    pub fn unchecked(&self) -> Option<&str> {
        self.unchecked.as_deref()
    }

    /// The number of the GitHub PR the local PR is linked to, if it is.
    pub fn linked_number(&self) -> Option<u64> {
        self.linked.as_ref().map(|linked| linked.pr.number)
    }

    /// Record that the local PR is linked to the GitHub PR `github_pr`, so that it stays that
    /// PR's. A local PR already recorded as another GitHub PR's is an error.
    pub fn record_link(&self, github_pr: &GitHubPr) -> Res<()> {
        let link = GitHubLink {
            number: github_pr.number,
            url: github_pr.url.clone(),
        };
        let mut result = Ok(());
        self.store.update(|file| match &file.github_pr {
            Some(recorded) if recorded.number != link.number => {
                result = Err(Error::LocalPr(format!(
                    "Couldn't link {}'s local PR to GitHub's PR #{}: it is linked to #{}",
                    self.head, link.number, recorded.number
                )));
            }
            _ => file.github_pr = Some(link),
        })?;
        result
    }

    /// Refuse a PR with no commits to review, or whose branch is behind its remote's, so that
    /// what is reviewed is the branch as it is.
    fn check_commits(&self) -> Res<()> {
        if self.commit_count()? == 0 {
            return Err(no_pr(&format!(
                "{} has no commits since {}",
                self.head,
                short_ref(&self.base_ref)
            )));
        }
        let local = format!("refs/heads/{}", self.head);
        let upstream = match self.repo.branch_upstream_name(&local) {
            Ok(name) => name.as_str().map(str::to_owned).ok_or_else(|| {
                Error::LocalPr(format!(
                    "Couldn't read {}'s upstream: it is not UTF-8",
                    self.head
                ))
            })?,
            Err(e) if e.code() == git2::ErrorCode::NotFound => return Ok(()),
            Err(e) => {
                return Err(Error::LocalPr(format!(
                    "Couldn't read {}'s upstream: {e}",
                    self.head
                )));
            }
        };
        let Some(upstream_commit) = ref_commit(&self.repo, &upstream)? else {
            return Err(opening(Error::LocalPr(format!(
                "{}'s upstream, {}, is not there. Fetch it, or unset it",
                self.head,
                short_ref(&upstream)
            ))));
        };
        let behind = self.count_commits(upstream_commit, self.head_commit()?)?;
        if behind > 0 {
            let commits = if behind == 1 { "commit" } else { "commits" };
            return Err(opening(Error::LocalPr(format!(
                "{} has {behind} {commits} that {} doesn't. Pull, so that the PR is reviewed as \
                 it is",
                short_ref(&upstream),
                self.head
            ))));
        }
        Ok(())
    }

    /// The distinct authors of the PR's commits, by name, in the order of their first commit
    /// from newest to oldest.
    pub fn commit_authors(&self) -> Res<Vec<String>> {
        let error = |e: git2::Error| Error::LocalPr(format!("Couldn't list the PR's commits: {e}"));
        let mut walk = self.repo.revwalk().map_err(error)?;
        walk.push(self.head_commit()?).map_err(error)?;
        walk.hide(self.merge_base()?).map_err(error)?;
        let mut authors: Vec<String> = vec![];
        for oid in walk {
            let commit = self.repo.find_commit(oid.map_err(error)?).map_err(error)?;
            let author = commit.author();
            let name = author.name().ok_or_else(|| {
                Error::LocalPr(format!(
                    "Couldn't read the author of commit {}: the name is not UTF-8",
                    commit.id()
                ))
            })?;
            if !authors.iter().any(|known| known == name) {
                authors.push(name.to_owned());
            }
        }
        Ok(authors)
    }

    /// How many commits `from` has that `since` doesn't.
    fn count_commits(&self, from: Oid, since: Oid) -> Res<usize> {
        let error = |e: git2::Error| Error::LocalPr(format!("Couldn't count commits: {e}"));
        let mut walk = self.repo.revwalk().map_err(error)?;
        walk.push(from).map_err(error)?;
        walk.hide(since).map_err(error)?;
        walk.try_fold(0, |n, oid| oid.map(|_| n + 1)).map_err(error)
    }

    /// Why the local PR is not the GitHub PR `github_pr`, if it isn't: one whose base is not GitHub's
    /// base, or, while the branch is where GitHub has the PR's head, whose commits are not as
    /// many as GitHub says the PR has.
    pub(crate) fn check_against(&self, github_pr: &GitHubPr) -> Res<()> {
        let in_repository = Oid::from_str(&github_pr.base_oid)
            .ok()
            .is_some_and(|oid| self.repo.find_commit(oid).is_ok());
        if !in_repository {
            return Err(Error::Gh(missing_commit(
                "base",
                &github_pr.base_oid,
                &github_pr.base_ref,
            )));
        }
        // The base as this repository has it, not as the link to GitHub gives it.
        let base = ref_commit(&self.repo, &self.base_ref)?.ok_or_else(|| {
            Error::LocalPr(format!("Couldn't find the PR's base, {}", self.base_ref))
        })?;
        if base.to_string() != github_pr.base_oid {
            return Err(Error::Gh(format!(
                "GitHub's {} is at {}, but {} is at {}. Fetch, so that they agree",
                self.base,
                short(&github_pr.base_oid),
                short_ref(&self.base_ref),
                short(&base.to_string()),
            )));
        }
        let head = self.head_commit()?;
        if head.to_string() != github_pr.head_oid {
            return Ok(());
        }
        let commits = self.commit_count()?;
        if commits != github_pr.commits {
            return Err(Error::Gh(format!(
                "GitHub's PR has {} commits, but {} has {commits} since it branched from {}",
                github_pr.commits,
                self.head,
                short_ref(&self.base_ref),
            )));
        }
        Ok(())
    }

    /// How many commits the PR has: those of its head since it branched from its base.
    pub fn commit_count(&self) -> Res<usize> {
        self.count_commits(self.head_commit()?, self.merge_base()?)
    }

    /// The commit the base is at: on GitHub, for a local PR linked to a GitHub PR, and otherwise
    /// locally.
    fn base_commit(&self) -> Res<Oid> {
        let Some(linked) = &self.linked else {
            return ref_commit(&self.repo, &self.base_ref)?.ok_or_else(|| {
                Error::LocalPr(format!("Couldn't find the PR's base, {}", self.base_ref))
            });
        };
        let pr = &linked.pr;
        Oid::from_str(&pr.base_oid)
            .ok()
            .filter(|oid| self.repo.find_commit(*oid).is_ok())
            .ok_or_else(|| Error::Gh(missing_commit("base", &pr.base_oid, &pr.base_ref)))
    }

    fn branch_commit(&self, branch: &str) -> Res<Oid> {
        self.repo
            .revparse_single(&format!("refs/heads/{branch}"))
            .and_then(|object| object.peel_to_commit())
            .map(|commit| commit.id())
            .map_err(|e| Error::LocalPr(format!("Couldn't read branch {branch}: {e}")))
    }

    /// The commit the PR's head branched from its base at: the commit the old side's lines are
    /// numbered against, as the head's commit is the new side's.
    fn merge_base(&self) -> Res<Oid> {
        let head = self.branch_commit(&self.head)?;
        let base = self.base_commit()?;
        self.repo
            .merge_base(base, head)
            .map_err(|e| Error::LocalPr(format!("Couldn't find where {} branched: {e}", self.head)))
    }

    /// The lines of the file at `path` in `commit`, none if it has no such file.
    fn file_lines(&self, commit: Oid, path: &str) -> Res<Vec<String>> {
        let error = |e: git2::Error| {
            Error::LocalPr(format!("Couldn't read {path} in commit {commit}: {e}"))
        };
        let tree = self
            .repo
            .find_commit(commit)
            .and_then(|commit| commit.tree())
            .map_err(error)?;
        let entry = match tree.get_path(Path::new(path)) {
            Ok(entry) => entry,
            Err(e) if e.code() == git2::ErrorCode::NotFound => return Ok(vec![]),
            Err(e) => return Err(error(e)),
        };
        let blob = self.repo.find_blob(entry.id()).map_err(error)?;
        Ok(String::from_utf8_lossy(blob.content())
            .lines()
            .map(str::to_owned)
            .collect())
    }

    /// Where a comment on `line` of the file at `path` is, with the line and the lines around
    /// it, as they are now.
    fn anchor(&self, path: &str, line: (Side, u32)) -> Res<Anchor> {
        let commit = match line.0 {
            Side::New => self.branch_commit(&self.head)?,
            Side::Old => self.merge_base()?,
        };
        self.anchor_at(commit, path, line)
    }

    /// Where a comment on `line` of the file at `path` is, numbered as in `commit`.
    fn anchor_at(&self, commit: Oid, path: &str, (side, line): (Side, u32)) -> Res<Anchor> {
        let lines = self.file_lines(commit, path)?;
        let i = (line as usize).saturating_sub(1).min(lines.len());
        let end = (i + 1).min(lines.len());
        Ok(Anchor {
            path: path.to_owned(),
            side,
            line,
            commit: commit.to_string(),
            text: lines[i..end].to_vec(),
            before: lines[i.saturating_sub(CONTEXT)..i].to_vec(),
            after: lines[end..(end + CONTEXT).min(lines.len())].to_vec(),
        })
    }

    /// Whether `line` of the file at `path` is in the PR's diff, as `git diff` shows it, with 3
    /// lines of context: only a line in the diff can be commented on, as on GitHub.
    pub fn in_diff(&self, path: &str, (side, line): (Side, u32)) -> Res<bool> {
        let error = |e: git2::Error| Error::LocalPr(format!("Couldn't diff the PR: {e}"));
        let tree = |oid| {
            self.repo
                .find_commit(oid)
                .and_then(|commit| commit.tree())
                .map_err(error)
        };
        let old = tree(self.merge_base()?)?;
        let new = tree(self.branch_commit(&self.head)?)?;
        let mut options = git2::DiffOptions::new();
        options.pathspec(path).disable_pathspec_match(true);
        let diff = self
            .repo
            .diff_tree_to_tree(Some(&old), Some(&new), Some(&mut options))
            .map_err(error)?;
        for delta_i in 0..diff.deltas().len() {
            let Some(patch) = git2::Patch::from_diff(&diff, delta_i).map_err(error)? else {
                continue;
            };
            for hunk_i in 0..patch.num_hunks() {
                for line_i in 0..patch.num_lines_in_hunk(hunk_i).map_err(error)? {
                    let diff_line = patch.line_in_hunk(hunk_i, line_i).map_err(error)?;
                    let n = match side {
                        Side::New => diff_line.new_lineno(),
                        Side::Old => diff_line.old_lineno(),
                    };
                    if n == Some(line) {
                        return Ok(true);
                    }
                }
            }
        }
        Ok(false)
    }

    pub fn repo(&self) -> &Repository {
        &self.repo
    }

    pub fn head(&self) -> &str {
        &self.head
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    /// The commit the PR's head branch is at.
    pub fn head_commit(&self) -> Res<Oid> {
        self.branch_commit(&self.head)
    }

    /// Delete the draft comment `comment_id`, wherever it is.
    pub fn delete(&self, comment_id: &str) -> Res<()> {
        let file = self.store.read()?;
        let thread = file
            .threads
            .iter()
            .find(|t| t.comments.iter().any(|c| c.id == comment_id))
            .ok_or_else(|| Error::LocalPr(format!("There is no comment {comment_id}")))?;
        self.delete_comment(&thread.id, comment_id).map(|_| ())
    }

    /// The draft review, as it is to be posted: refused if it is empty, or if a thread's line has
    /// gone. Its comments are in the order they are to be posted: each thread's first comment
    /// before its replies.
    pub fn to_post(&self) -> Res<ToPost> {
        let file = self.store.read()?;
        let mine = file.reviews.iter().find(|r| r.state == ReviewState::Draft);
        let comments_of = |review: &str| {
            file.threads
                .iter()
                .flat_map(|t| &t.comments)
                .filter(|c| c.review == review)
                .count()
        };
        let Some(mine) = mine.filter(|r| comments_of(&r.id) > 0) else {
            return Err(Error::LocalPr(
                "Nothing to post: you have no draft comments".to_owned(),
            ));
        };
        let lines: HashMap<String, Option<(Side, u32)>> = self
            .threads()?
            .into_iter()
            .map(|t| (t.id, t.line))
            .collect();
        let mut comments = vec![];
        for thread in &file.threads {
            for (i, comment) in thread.comments.iter().enumerate() {
                if comment.review != mine.id {
                    continue;
                }
                let line = lines.get(&thread.id).copied().flatten().ok_or_else(|| {
                    Error::LocalPr(format!(
                        "Couldn't post: the line of a thread on {} has gone. Delete its draft \
                         comments first",
                        thread.anchor.path
                    ))
                })?;
                comments.push(PostComment {
                    id: comment.id.clone(),
                    thread: thread.id.clone(),
                    body: comment.body.clone(),
                    path: thread.anchor.path.clone(),
                    line,
                    starts_thread: i == 0 && thread.github.is_none(),
                    reply_to: thread.github.clone(),
                });
            }
        }
        Ok(ToPost {
            link: mine.github.clone(),
            comments,
        })
    }

    /// Record that posting the draft review uses the pending review `review_id`.
    pub fn set_link(&self, review_id: &str) -> Res<()> {
        self.store.update(|file| {
            for review in &mut file.reviews {
                if review.state == ReviewState::Draft {
                    review.github = Some(review_id.to_owned());
                }
            }
        })
    }

    /// Record that the draft comment `comment_id` has been posted, by removing it, and, if it
    /// started its thread, that the thread's first comment on GitHub is `started`. The comment
    /// comes back as part of the copy of the review on GitHub at the next fetch.
    pub fn mark_posted(&self, comment_id: &str, started: Option<&str>) -> Res<()> {
        self.store.update(|file| {
            for thread in &mut file.threads {
                if thread.comments.iter().any(|c| c.id == comment_id) {
                    thread.comments.retain(|c| c.id != comment_id);
                    if let Some(started) = started {
                        thread.github = Some(started.to_owned());
                    }
                }
            }
            file.threads.retain(|t| !t.comments.is_empty());
            remove_empty_drafts(file);
        })
    }

    /// Copy the reviews of `github`'s PR, as the user sees them, into the local PR, replacing
    /// the copies fetched before. Draft reviews are not changed. A thread new to the local PR is
    /// anchored to its line in the PR's head on GitHub, if that commit is in the repository.
    pub fn fetch(&self, github: &GitHubBackend) -> Res<()> {
        let pr = &github.pr;
        if pr.base_ref != self.base {
            return Err(Error::Gh(format!(
                "Couldn't fetch: GitHub's PR #{} is into {}, not {}",
                pr.number, pr.base_ref, self.base
            )));
        }
        let reviews = github.github.reviews(pr)?;
        let threads = github.github.threads(pr)?;
        // A thread's line is numbered as in the PR's head on GitHub, or where it branched from
        // its base for the old side, which must be in the repository to anchor it.
        let commit = |oid: &str, branch: &str, which: &str| {
            Oid::from_str(oid)
                .ok()
                .filter(|oid| self.repo.find_commit(*oid).is_ok())
                .ok_or_else(|| {
                    Error::Gh(format!(
                        "Couldn't fetch: {}",
                        missing_commit(which, oid, branch)
                    ))
                })
        };
        let mut anchors = HashMap::new();
        for thread in &threads {
            let anchor = match thread.line {
                Some((Side::New, n)) => {
                    let head = commit(&pr.head_oid, &self.head, "head")?;
                    self.anchor_at(head, &thread.path, (Side::New, n))?
                }
                Some((Side::Old, n)) => {
                    let head = commit(&pr.head_oid, &self.head, "head")?;
                    let base = commit(&pr.base_oid, &pr.base_ref, "base")?;
                    let merge_base = self.repo.merge_base(base, head).map_err(|e| {
                        Error::Gh(format!("Couldn't find where the PR branched: {e}"))
                    })?;
                    self.anchor_at(merge_base, &thread.path, (Side::Old, n))?
                }
                // Outdated on GitHub: it has no line to find.
                None => Anchor {
                    path: thread.path.clone(),
                    side: Side::New,
                    line: 0,
                    commit: pr.head_oid.clone(),
                    text: vec![],
                    before: vec![],
                    after: vec![],
                },
            };
            anchors.insert(thread.id.clone(), anchor);
        }
        // A comment in none of the reviews GitHub listed would be lost from the copy.
        let reviewed: HashSet<&str> = reviews
            .iter()
            .flat_map(|review| &review.comments)
            .map(String::as_str)
            .collect();
        let in_no_review: Vec<String> = threads
            .iter()
            .flat_map(|thread| thread.comments.iter().map(move |c| (thread, c)))
            .filter(|(_, c)| !reviewed.contains(c.id.as_str()))
            .map(|(thread, c)| format!("{} on {}", c.id, thread.path))
            .collect();
        if !in_no_review.is_empty() {
            return Err(Error::Gh(format!(
                "Couldn't fetch: GitHub gave comments in no review it listed: {}",
                in_no_review.join(", ")
            )));
        }
        let now = chrono::Local::now().fixed_offset();
        self.store.update(|file| {
            file.reviews.retain(|r| r.state == ReviewState::Draft);
            let drafts: Vec<String> = file.reviews.iter().map(|r| r.id.clone()).collect();
            for thread in &mut file.threads {
                thread.comments.retain(|c| drafts.contains(&c.review));
            }
            let mut review_of = HashMap::new();
            for review in &reviews {
                let id = format!("github-{}", review.id);
                for comment in &review.comments {
                    review_of.insert(comment.clone(), id.clone());
                }
                file.reviews.push(StoredReview {
                    id,
                    author: review.author.clone(),
                    state: match review.state {
                        GitHubReviewState::Pending => ReviewState::Pending,
                        GitHubReviewState::Submitted => ReviewState::Submitted,
                        GitHubReviewState::Dismissed => ReviewState::Dismissed,
                    },
                    github: Some(review.id.clone()),
                    mine: review.mine,
                    head: pr.head_oid.clone(),
                    body: String::new(),
                    created_at: now,
                });
            }
            for thread in &threads {
                let comments: Vec<StoredComment> = thread
                    .comments
                    .iter()
                    .map(|c| StoredComment {
                        id: format!("github-{}", c.id),
                        // Every comment's review was found above.
                        review: review_of[&c.id].clone(),
                        author: c.author.clone(),
                        agent: None,
                        body: c.body.clone(),
                        created_at: c.created_at,
                        github: Some(c.id.clone()),
                        url: c.url.clone(),
                    })
                    .collect();
                let Some(first) = thread.comments.first().map(|c| c.id.clone()) else {
                    continue;
                };
                if comments.is_empty() {
                    continue;
                }
                let local = match file
                    .threads
                    .iter_mut()
                    .find(|t| t.github.as_deref() == Some(first.as_str()))
                {
                    Some(local) => local,
                    None => {
                        file.threads.push(StoredThread {
                            id: new_id(),
                            anchor: anchors.remove(&thread.id).unwrap(),
                            resolved: thread.resolved,
                            github: Some(first),
                            comments: vec![],
                        });
                        file.threads.last_mut().unwrap()
                    }
                };
                local.resolved = thread.resolved;
                let drafts = std::mem::take(&mut local.comments);
                local.comments = comments.into_iter().chain(drafts).collect();
            }
            file.threads.retain(|t| !t.comments.is_empty());
        })
    }

    /// The local PR as JSON, for `delta --tui review show --format json`: its reviews, each with
    /// its author and state (draft, pending, submitted or dismissed), and its threads, each with
    /// its file, side, line now (`null` once it has gone), and comments.
    pub fn json(&self) -> Res<serde_json::Value> {
        let file = self.store.read()?;
        let lines: HashMap<String, Option<(Side, u32)>> = self
            .threads()?
            .into_iter()
            .map(|t| (t.id, t.line))
            .collect();
        let reviews: Vec<serde_json::Value> = file
            .reviews
            .iter()
            .map(|r| {
                serde_json::json!({
                    "id": r.id,
                    "author": r.author,
                    "state": r.state.name(),
                })
            })
            .collect();
        let threads: Vec<serde_json::Value> = file
            .threads
            .iter()
            .map(|t| {
                let line = lines.get(&t.id).copied().flatten();
                serde_json::json!({
                    "id": t.id,
                    "path": t.anchor.path,
                    "side": t.anchor.side,
                    "line": line.map(|(_, n)| n),
                    "resolved": t.resolved,
                    "comments": t.comments.iter().map(|c| serde_json::json!({
                        "id": c.id,
                        "review": c.review,
                        "author": c.author,
                        "agent": c.agent,
                        "body": c.body,
                        "created_at": c.created_at,
                    })).collect::<Vec<_>>(),
                })
            })
            .collect();
        Ok(serde_json::json!({
            "head": self.head,
            "base": self.base,
            "reviews": reviews,
            "threads": threads,
        }))
    }

    /// The same PR, linked to `github`'s PR, as `O O` opens it: refused if it is not that PR
    /// (see [`LocalBackend::check_against`]).
    pub fn with_linked_pr(self, github: GitHubBackend) -> Res<Self> {
        let pr = github.pr.clone();
        let linked = Self {
            linked: Some(github),
            ..self
        };
        linked.check_against(&pr).map_err(opening)?;
        Ok(linked)
    }

    /// The branch's GitHub PR.
    fn github_pr(&self) -> Res<GitHubBackend> {
        if let Some(linked) = &self.linked {
            return Ok(linked.clone());
        }
        let github = Rc::clone(&self.github);
        let pr = github
            .pr_for_branch(&self.head)?
            .ok_or_else(|| Error::Gh(format!("{} has no open GitHub PR", self.head)))?;
        Ok(GitHubBackend { github, pr })
    }

    /// Edit the comment `comment_id`, wherever it is: see [`Backend::edit_comment`].
    pub fn edit(&self, comment_id: &str, body: &str) -> Res<()> {
        let thread = self.thread_of(comment_id)?;
        self.edit_comment(&thread, comment_id, body).map(|_| ())
    }

    fn thread_of(&self, comment_id: &str) -> Res<String> {
        let file = self.store.read()?;
        file.threads
            .iter()
            .find(|t| t.comments.iter().any(|c| c.id == comment_id))
            .map(|t| t.id.clone())
            .ok_or_else(|| Error::LocalPr(format!("There is no comment {comment_id}")))
    }

    /// Where the comment `comment_id` in the thread `thread_id` is: in a draft review, or the
    /// user's own comment in a copy of a review on GitHub, as its GitHub id and whether it starts
    /// its thread; otherwise it can't be changed.
    fn whose(&self, thread_id: &str, comment_id: &str) -> Res<Changeable> {
        let file = self.store.read()?;
        let thread = file
            .threads
            .iter()
            .find(|t| t.id == thread_id)
            .ok_or_else(|| {
                Error::LocalPr("Couldn't change the comment: it has been deleted".to_owned())
            })?;
        let i = thread
            .comments
            .iter()
            .position(|c| c.id == comment_id)
            .ok_or_else(|| {
                Error::LocalPr("Couldn't change the comment: it has been deleted".to_owned())
            })?;
        let comment = &thread.comments[i];
        let review = file.reviews.iter().find(|r| r.id == comment.review);
        match review {
            Some(r) if r.state == ReviewState::Draft => Ok(Changeable::Draft),
            Some(r) if r.mine => Ok(Changeable::OnGitHub {
                id: comment.github.clone().ok_or_else(|| {
                    Error::LocalPr(format!(
                        "Couldn't change the comment: the copy of {} has no GitHub id. Fetch \
                         again",
                        comment.id
                    ))
                })?,
                starts_thread: i == 0,
            }),
            _ => Err(Error::LocalPr(NOT_DRAFT.to_owned())),
        }
    }

    /// The same PR, with the comments written labelled with the name `agent`.
    pub fn with_agent(self, agent: String) -> Self {
        Self {
            agent: Some(agent),
            ..self
        }
    }

    /// Give a comment saying `body`, in the draft review, to `add` to put in a thread. If `add`
    /// puts it in one, the draft review is started if there is none; returns whether it did.
    fn add_to_draft(
        &self,
        body: &str,
        add: impl FnOnce(&mut Vec<StoredThread>, StoredComment) -> bool,
    ) -> Res<bool> {
        let head = self.branch_commit(&self.head)?.to_string();
        let now = chrono::Local::now().fixed_offset();
        let mut added = false;
        self.store.update(|file| {
            let draft = file
                .reviews
                .iter()
                .find(|review| review.state == ReviewState::Draft)
                .map(|review| review.id.clone());
            let review = draft.clone().unwrap_or_else(new_id);
            let comment = StoredComment {
                id: new_id(),
                review: review.clone(),
                author: self.reviewer.clone(),
                agent: self.agent.clone(),
                body: body.to_owned(),
                created_at: now,
                github: None,
                url: None,
            };
            added = add(&mut file.threads, comment);
            if added && draft.is_none() {
                file.reviews.push(StoredReview {
                    id: review,
                    author: self.reviewer.clone(),
                    state: ReviewState::Draft,
                    github: None,
                    mine: false,
                    head,
                    body: String::new(),
                    created_at: now,
                });
            }
        })?;
        Ok(added)
    }

    /// The PR's review threads as markdown, for `delta --tui review show` (see [`markdown`]).
    pub fn markdown(&self) -> Res<String> {
        let file = self.store.read()?;
        let threads = self.threads()?;
        let entries = threads
            .iter()
            .map(|thread| {
                let anchor = &file
                    .threads
                    .iter()
                    .find(|t| t.id == thread.id)
                    .ok_or_else(|| {
                        Error::LocalPr(format!(
                            "Thread {} changed while the review was printed: print it again",
                            thread.id
                        ))
                    })?
                    .anchor;
                Ok(MarkdownThread {
                    thread,
                    old_side: anchor.side == Side::Old,
                    recorded_line: Some(anchor.line),
                    quote: Quote {
                        before: anchor.before.clone(),
                        text: anchor.text.clone(),
                        after: anchor.after.clone(),
                    },
                })
            })
            .collect::<Res<Vec<_>>>()?;
        Ok(markdown(
            &format!("Review of {} into {}", self.head, self.base),
            entries,
        ))
    }

    /// Change the comment `comment_id` in the thread `thread_id` as `change` says, given the
    /// thread and the comment's index in it, if the comment is in a draft review. A thread left
    /// with no comments is deleted.
    fn change_draft_comment(
        &self,
        thread_id: &str,
        comment_id: &str,
        change: impl FnOnce(&mut StoredThread, usize),
    ) -> Res<()> {
        let mut result = Err(Error::LocalPr(
            "Couldn't change the comment: it has been deleted".to_owned(),
        ));
        self.store.update(|file| {
            let drafts: Vec<&str> = file
                .reviews
                .iter()
                .filter(|review| review.state == ReviewState::Draft)
                .map(|review| review.id.as_str())
                .collect();
            let Some(thread) = file.threads.iter_mut().find(|t| t.id == thread_id) else {
                return;
            };
            let Some(i) = thread.comments.iter().position(|c| c.id == comment_id) else {
                return;
            };
            if !drafts.contains(&thread.comments[i].review.as_str()) {
                result = Err(Error::LocalPr(NOT_DRAFT.to_owned()));
                return;
            }
            change(thread, i);
            result = Ok(());
            file.threads.retain(|thread| !thread.comments.is_empty());
            remove_empty_drafts(file);
        })?;
        result
    }

    /// Each file the PR changes, by the path it is known by in the PR, with what it is at the
    /// PR's head: its blob, or "" for a file the PR deletes. A file whose tick no longer matches
    /// what it is has changed since it was ticked.
    fn files(&self) -> Res<HashMap<String, String>> {
        let error = |e: git2::Error| Error::LocalPr(format!("Couldn't diff the PR: {e}"));
        let head = self.branch_commit(&self.head)?;
        let merge_base = self.merge_base()?;
        let tree = |oid| {
            self.repo
                .find_commit(oid)
                .and_then(|commit| commit.tree())
                .map_err(error)
        };
        let (old, new) = (tree(merge_base)?, tree(head)?);
        let diff = self
            .repo
            .diff_tree_to_tree(Some(&old), Some(&new), None)
            .map_err(error)?;
        diff.deltas()
            .map(|delta| {
                let deleted = delta.status() == git2::Delta::Deleted;
                let file = if deleted {
                    delta.old_file()
                } else {
                    delta.new_file()
                };
                let path = file
                    .path()
                    .ok_or_else(|| {
                        Error::LocalPr(format!(
                            "Couldn't diff the PR: a file {} has no path",
                            file.id()
                        ))
                    })?
                    .to_string_lossy()
                    .into_owned();
                let blob = if deleted {
                    String::new()
                } else {
                    file.id().to_string()
                };
                Ok((path, blob))
            })
            .collect()
    }
}

const HAS_REPLIES: &str =
    "Couldn't delete the comment: it starts a thread with replies. Delete the replies first";

/// Where a comment that can be changed is, as [`LocalBackend::whose`] gives it.
enum Changeable {
    Draft,
    OnGitHub { id: String, starts_thread: bool },
}

pub(crate) const NOT_DRAFT: &str = "Only a comment in a draft review can be changed";

/// Lines quoted in markdown: a line, or none, and the lines before and after it.
pub(crate) struct Quote {
    pub before: Vec<String>,
    pub text: Vec<String>,
    pub after: Vec<String>,
}

/// A thread as [`markdown`] prints it.
pub(crate) struct MarkdownThread<'a> {
    pub thread: &'a Thread,
    pub old_side: bool,
    /// The line it was started on, if known, for a thread whose line has gone.
    pub recorded_line: Option<u32>,
    pub quote: Quote,
}

/// `threads` as markdown, under the heading `title`: by file and line, each with its id, the
/// lines it is on (the line marked `>`), and its comments, saying which are drafts.
pub(crate) fn markdown(title: &str, mut threads: Vec<MarkdownThread>) -> String {
    threads.sort_by_key(|entry| {
        let line = entry.thread.line.map(|(side, n)| (n, side == Side::Old));
        (entry.thread.path.clone(), line)
    });
    let mut out = format!("# {title}\n");
    for MarkdownThread {
        thread,
        old_side,
        recorded_line,
        quote,
    } in &threads
    {
        let old = if *old_side { " (old side)" } else { "" };
        let place = match (thread.line, recorded_line) {
            (Some((_, n)), _) => format!("{}:{n}{old}", thread.path),
            (None, Some(n)) => format!("{} (outdated: was line {n}{old})", thread.path),
            (None, None) => format!("{} (outdated)", thread.path),
        };
        let resolved = if thread.resolved { ", resolved" } else { "" };
        out += &format!("\n## {place}\n\nThread id: {}{resolved}\n", thread.id);
        if !quote.text.is_empty() {
            out += "\n````\n";
            for line in &quote.before {
                out += &format!("  {line}\n");
            }
            for line in &quote.text {
                out += &format!("> {line}\n");
            }
            for line in &quote.after {
                out += &format!("  {line}\n");
            }
            out += "````\n";
        }
        for comment in &thread.comments {
            let draft = if comment.draft { "draft, " } else { "" };
            let time = comment.created_at.format("%Y-%m-%d %H:%M");
            out += &format!(
                "\n**{}** ({draft}{time})\n\n{}\n",
                comment.agent.as_ref().unwrap_or(&comment.author),
                comment.body.trim_end()
            );
        }
    }
    out
}

/// The draft review, as [`LocalBackend::to_post`] gives it.
pub(crate) struct ToPost {
    /// The pending review on GitHub that posting it started, if it has.
    pub link: Option<String>,
    pub comments: Vec<PostComment>,
}

pub(crate) struct PostComment {
    pub id: String,
    pub thread: String,
    pub body: String,
    pub path: String,
    pub line: (Side, u32),
    /// Whether posting it starts its thread on GitHub.
    pub starts_thread: bool,
    /// The GitHub id of its thread's first comment, if that is on GitHub.
    pub reply_to: Option<String>,
}

/// Remove the draft reviews with no comments left.
fn remove_empty_drafts(file: &mut PrFile) {
    let threads = &file.threads;
    file.reviews.retain(|review| {
        review.state != ReviewState::Draft
            || threads
                .iter()
                .flat_map(|t| &t.comments)
                .any(|c| c.review == review.id)
    });
}

/// How many lines before and after a comment's line its anchor records.
const CONTEXT: usize = 3;

/// An id for a new review, thread or comment, unique in all likelihood.
fn new_id() -> String {
    use sha2::{Digest, Sha256};
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNT: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    let seed = format!(
        "{nanos} {} {}",
        std::process::id(),
        COUNT.fetch_add(1, Ordering::Relaxed)
    );
    Sha256::digest(seed.as_bytes())[..6]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The line, numbered from 1, that `anchor`'s line is in `lines`, the lines of its file now: of
/// the lines that are its text, the one with the most of its surrounding lines around it, and of
/// those the nearest to where it was. `None` if no line is its text.
fn locate(anchor: &Anchor, lines: &[String]) -> Option<u32> {
    let [text] = anchor.text.as_slice() else {
        return None;
    };
    let surrounding = |i: usize| {
        let before = anchor
            .before
            .iter()
            .rev()
            .enumerate()
            .filter(|(k, line)| i.checked_sub(k + 1).and_then(|j| lines.get(j)) == Some(line))
            .count();
        let after = anchor
            .after
            .iter()
            .enumerate()
            .filter(|(k, line)| lines.get(i + k + 1) == Some(line))
            .count();
        before + after
    };
    let line = (0..lines.len())
        .filter(|&i| lines[i] == *text)
        .max_by_key(|&i| {
            let distance = (i + 1).abs_diff(anchor.line as usize);
            (surrounding(i), std::cmp::Reverse(distance))
        })?;
    u32::try_from(line + 1).ok()
}

/// The base of `head`'s GitHub PR, or else the default branch of the repository gh would open a
/// PR against.
/// A branch name as part of a file name: `%` and `/` escaped.
fn escape_branch(branch: &str) -> String {
    branch.replace('%', "%25").replace('/', "%2F")
}

/// The base of the branch `head`'s PR, the GitHub PR it is linked to, and why it was not checked
/// against GitHub. A branch has at most one PR. If gh gives a GitHub PR for the branch, that is
/// the PR, and its local PR, if it has one, must be into the same base and not linked to another
/// GitHub PR. If gh says GitHub has none, the PR is the branch's local PR, which must not be
/// linked to a GitHub PR, or a new one into the repository's default branch on GitHub. If gh
/// can't say, the PR is the branch's local PR, not checked against GitHub. What else there could
/// be is an error, as not yet handled.
fn find_pr(
    repo: &Repository,
    github: &dyn GitHub,
    head: &str,
    reviewer: &str,
) -> Res<(String, Option<GitHubPr>, Option<String>)> {
    let bases = Store::bases(repo.commondir(), head)?;
    let local = match bases.as_slice() {
        [] => None,
        [base] => {
            let link = Store::new(repo.commondir(), head, base, reviewer)
                .read()?
                .github_pr;
            Some((base.clone(), link))
        }
        [many @ .., last] => {
            return Err(not_handled(&format!(
                "{head} has local PRs into {} and {last}",
                many.join(", ")
            )));
        }
    };
    let github_pr = match github.pr_for_branch(head) {
        Ok(github_pr) => github_pr,
        Err(e) => {
            return match local {
                Some((base, _)) => {
                    Ok((base, None, Some(format!("Not checked against GitHub: {e}"))))
                }
                None => Err(Error::LocalPr(format!(
                    "Couldn't find {head}'s PR: there is none in this repository, and gh couldn't \
                     say what GitHub has: {e}. The review command line can be given the base \
                     with --base"
                ))),
            };
        }
    };
    match (github_pr, local) {
        (Some(pr), None) => Ok((pr.base_ref.clone(), Some(pr), None)),
        (Some(pr), Some((base, link))) => {
            if let Some(link) = link
                && link.number != pr.number
            {
                return Err(not_handled(&format!(
                    "{head}'s local PR is linked to #{}, but GitHub's PR for it is #{}",
                    link.number, pr.number
                )));
            }
            if base != pr.base_ref {
                return Err(not_handled(&format!(
                    "{head}'s local PR is into {base}, but GitHub's PR #{} is into {}",
                    pr.number, pr.base_ref
                )));
            }
            Ok((base, Some(pr), None))
        }
        (None, Some((_, Some(link)))) => Err(not_handled(&format!(
            "{head}'s local PR is linked to #{}, which GitHub no longer has open",
            link.number
        ))),
        (None, Some((base, None))) => Ok((base, None, None)),
        (None, None) => Ok((github.default_branch()?, None, None)),
    }
}

/// An error for a case gitu doesn't handle yet.
fn not_handled(case: &str) -> Error {
    Error::LocalPr(format!("Not handled yet: {case}"))
}

/// The ref a local PR into the branch `base` is diffed against: `refs/heads/<base>`, or its
/// upstream on its remote, whichever has the other's commits, since a local base branch is often
/// behind its remote's. Either alone is it, if the other is not there. When each has commits the
/// other lacks, which the PR is into can't be told, and that is an error, as is neither being
/// there.
fn resolve_base(repo: &Repository, base: &str) -> Res<String> {
    let local = format!("refs/heads/{base}");
    let error =
        |e: git2::Error| Error::LocalPr(format!("Couldn't find the PR's base branch {base}: {e}"));
    let upstream = match repo.branch_upstream_name(&local) {
        Ok(name) => Some(
            name.as_str()
                .ok_or_else(|| {
                    Error::LocalPr(format!("Couldn't read {base}'s upstream: it is not UTF-8"))
                })?
                .to_owned(),
        ),
        Err(e) if e.code() == git2::ErrorCode::NotFound => None,
        Err(e) => return Err(error(e)),
    };
    let upstream = upstream.unwrap_or_else(|| format!("refs/remotes/origin/{base}"));
    match (ref_commit(repo, &local)?, ref_commit(repo, &upstream)?) {
        (None, None) => Err(Error::LocalPr(format!(
            "Couldn't find the PR's base branch {base}: there is no {local} or {upstream}"
        ))),
        (Some(_), None) => Ok(local),
        (None, Some(_)) => Ok(upstream),
        (Some(l), Some(u)) if l == u || repo.graph_descendant_of(u, l).map_err(error)? => {
            Ok(upstream)
        }
        (Some(l), Some(u)) if repo.graph_descendant_of(l, u).map_err(error)? => Ok(local),
        (Some(_), Some(_)) => Err(Error::LocalPr(format!(
            "Couldn't find the PR's base: {base} and {} have diverged, so which the PR is into \
             can't be told. Bring one up to date with the other",
            short_ref(&upstream)
        ))),
    }
}

/// The commit the ref `name` is at, or `None` if there is no such ref.
fn ref_commit(repo: &Repository, name: &str) -> Res<Option<Oid>> {
    let error = |e: git2::Error| Error::LocalPr(format!("Couldn't read {name}: {e}"));
    match repo.find_reference(name) {
        Ok(reference) => Ok(Some(reference.peel_to_commit().map_err(error)?.id())),
        Err(e) if e.code() == git2::ErrorCode::NotFound => Ok(None),
        Err(e) => Err(error(e)),
    }
}

/// `name` without `refs/heads/` or `refs/remotes/`, as git shows a branch.
fn short_ref(name: &str) -> &str {
    name.strip_prefix("refs/heads/")
        .or_else(|| name.strip_prefix("refs/remotes/"))
        .unwrap_or(name)
}

/// The first 7 characters of the commit id `oid`.
fn short(oid: &str) -> &str {
    &oid[..oid.len().min(7)]
}

/// `e`, a reason a PR can't be opened, as an error saying so.
fn opening(e: Error) -> Error {
    Error::Gh(format!("Couldn't open the PR: {e}"))
}

fn no_pr(why: &str) -> Error {
    Error::LocalPr(format!("No PR to review: {why}"))
}

impl Backend for LocalBackend {
    fn diff_range(&self) -> String {
        match &self.linked {
            Some(linked) => format!("{}...refs/heads/{}", linked.pr.base_oid, self.head),
            None => format!("{}...refs/heads/{}", self.base_ref, self.head),
        }
    }

    /// For a local PR linked to a GitHub PR, whether the branch is not at the GitHub PR's head,
    /// whose lines GitHub's comments are numbered against.
    fn head_mismatch(&self, _repo: &Repository) -> bool {
        self.linked.as_ref().is_some_and(|linked| {
            self.head_commit()
                .is_ok_and(|head| head.to_string() != linked.pr.head_oid)
        })
    }

    fn refresh(&self) -> Res<()> {
        match &self.linked {
            Some(linked) => self.fetch(linked),
            None => Ok(()),
        }
    }

    fn as_local(&self) -> Option<&LocalBackend> {
        Some(self)
    }

    fn pending_review(&self) -> Res<Option<PendingReview>> {
        let file = self.store.read()?;
        let Some(pending) = file
            .reviews
            .iter()
            .find(|r| r.state == ReviewState::Pending && r.mine)
        else {
            return Ok(None);
        };
        let comments = file
            .threads
            .iter()
            .flat_map(|t| &t.comments)
            .filter(|c| c.review == pending.id)
            .count();
        let started_by_draft = file.reviews.iter().any(|r| {
            r.state == ReviewState::Draft && r.github.is_some() && r.github == pending.github
        });
        Ok(Some(PendingReview {
            comments,
            started_by_draft,
        }))
    }

    fn viewed(&self) -> Res<HashMap<String, ViewedState>> {
        if let Some(linked) = &self.linked {
            return linked.viewed();
        }
        let file = self.store.read()?;
        let ticked = file.viewed.get(&self.reviewer);
        Ok(self
            .files()?
            .into_iter()
            .map(|(path, blob)| {
                let state = match ticked.and_then(|ticked| ticked.get(&path)) {
                    None => ViewedState::Unviewed,
                    Some(ticked) if *ticked == blob => ViewedState::Viewed,
                    Some(_) => ViewedState::Dismissed,
                };
                (path, state)
            })
            .collect())
    }

    fn set_viewed(&self, path: &str, viewed: bool) -> Res<()> {
        if let Some(linked) = &self.linked {
            return linked.set_viewed(path, viewed);
        }
        let blob = self.files()?.remove(path).ok_or_else(|| {
            Error::LocalPr(format!(
                "Couldn't set the Viewed checkbox: {path} is not a file of the PR"
            ))
        })?;
        self.store.update(|file| {
            let ticked = file.viewed.entry(self.reviewer.clone()).or_default();
            if viewed {
                ticked.insert(path.to_owned(), blob);
            } else {
                ticked.remove(path);
            }
        })
    }

    /// A thread is on the line its anchor records while the commit the line is numbered against
    /// is unchanged, and otherwise on the line it is found at (see [`locate`]).
    fn threads(&self) -> Res<Vec<Thread>> {
        let file = self.store.read()?;
        let draft = |review: &str| {
            file.reviews
                .iter()
                .any(|r| r.id == review && r.state == ReviewState::Draft)
        };
        let mine = |review: &str| file.reviews.iter().any(|r| r.id == review && r.mine);
        let head = self.branch_commit(&self.head)?;
        let merge_base = self.merge_base()?;
        let mut files: HashMap<(Oid, String), Vec<String>> = HashMap::new();
        let mut threads = vec![];
        for thread in &file.threads {
            let anchor = &thread.anchor;
            let commit = match anchor.side {
                Side::New => head,
                Side::Old => merge_base,
            };
            let line = if anchor.text.is_empty() {
                // A thread fetched from GitHub already outdated has no line to find.
                None
            } else if anchor.commit == commit.to_string() {
                Some(anchor.line)
            } else {
                let key = (commit, anchor.path.clone());
                if !files.contains_key(&key) {
                    files.insert(key.clone(), self.file_lines(commit, &anchor.path)?);
                }
                locate(anchor, &files[&key])
            };
            threads.push(Thread {
                id: thread.id.clone(),
                path: thread.anchor.path.clone(),
                line: line.map(|line| (thread.anchor.side, line)),
                resolved: thread.resolved,
                comments: thread
                    .comments
                    .iter()
                    .map(|comment| Comment {
                        id: comment.id.clone(),
                        author: comment.author.clone(),
                        agent: comment.agent.clone(),
                        body: comment.body.clone(),
                        url: comment.url.clone(),
                        created_at: comment.created_at,
                        draft: draft(&comment.review),
                        // In the TUI the user edits their own comments, and replies to an
                        // agent's; they can delete any draft comment.
                        editable: (draft(&comment.review) && comment.agent.is_none())
                            || mine(&comment.review),
                        deletable: draft(&comment.review) || mine(&comment.review),
                    })
                    .collect(),
            });
        }
        Ok(threads)
    }

    fn reply(&self, thread_id: &str, body: &str) -> Res<&'static str> {
        let added = self.add_to_draft(body, |threads, comment| {
            let thread = threads.iter_mut().find(|t| t.id == thread_id);
            thread.map(|thread| thread.comments.push(comment)).is_some()
        })?;
        if !added {
            return Err(Error::LocalPr(format!(
                "Couldn't reply: there is no thread {thread_id}"
            )));
        }
        Ok(ADDED)
    }

    fn comment(&self, path: &str, line: (Side, u32), body: &str) -> Res<&'static str> {
        let anchor = self.anchor(path, line)?;
        self.add_to_draft(body, |threads, comment| {
            threads.push(StoredThread {
                id: new_id(),
                anchor,
                resolved: false,
                github: None,
                comments: vec![comment],
            });
            true
        })?;
        Ok(ADDED)
    }

    /// A draft comment is edited locally; the user's own comment in a review copied from GitHub
    /// is edited on GitHub, and the PR's reviews fetched again.
    fn edit_comment(&self, thread_id: &str, comment_id: &str, body: &str) -> Res<&'static str> {
        if let Changeable::OnGitHub { id, .. } = self.whose(thread_id, comment_id)? {
            let github = self.github_pr()?;
            github.github.edit_comment(&id, body)?;
            self.fetch(&github)?;
            return Ok("Comment updated on GitHub");
        }
        self.change_draft_comment(thread_id, comment_id, |thread, i| {
            thread.comments[i].body = body.to_owned();
        })?;
        Ok("Comment updated")
    }

    /// As [`LocalBackend::edit_comment`] edits. A draft thread's first comment can't be deleted
    /// while the thread has replies, which would be left replying to nothing; on GitHub, a
    /// thread's first comment can't be deleted at all, since anyone may have a draft reply to it.
    fn delete_comment(&self, thread_id: &str, comment_id: &str) -> Res<&'static str> {
        if let Changeable::OnGitHub { id, starts_thread } = self.whose(thread_id, comment_id)? {
            if starts_thread {
                return Err(Error::LocalPr(
                    "Couldn't delete the comment: it starts a thread on GitHub, to which anyone \
                     may have a draft reply. Edit it, or delete it on GitHub"
                        .to_owned(),
                ));
            }
            let github = self.github_pr()?;
            github.github.delete_comment(&id)?;
            self.fetch(&github)?;
            return Ok("Comment deleted on GitHub");
        }
        let mut has_replies = false;
        self.change_draft_comment(thread_id, comment_id, |thread, i| {
            if i == 0 && thread.comments.len() > 1 {
                has_replies = true;
            } else {
                thread.comments.remove(i);
            }
        })?;
        if has_replies {
            return Err(Error::LocalPr(HAS_REPLIES.to_owned()));
        }
        Ok("Comment deleted")
    }

    fn file_url(&self, path: &str, line: Option<(Side, u32)>) -> Option<String> {
        self.linked
            .as_ref()
            .and_then(|linked| linked.file_url(path, line))
    }
}

/// The version of [`PrFile`] this gitu writes. Version 1 had a draft review for each author,
/// an agent's among them; version 2 has one, the reviewer's (see [`merge_drafts`]). Version 3
/// records the GitHub PR the local PR is linked to, which a gitu writing version 2 would drop.
const VERSION: u32 = 3;

/// What is recorded about a local PR, as JSON.
#[derive(Serialize, Deserialize)]
struct PrFile {
    version: u32,
    head: String,
    base: String,
    /// Each reviewer's ticked files: by path, what the file was at the PR's head when ticked, as
    /// [`LocalBackend::files`] gives it.
    #[serde(default)]
    viewed: BTreeMap<String, BTreeMap<String, String>>,
    #[serde(default)]
    reviews: Vec<StoredReview>,
    #[serde(default)]
    threads: Vec<StoredThread>,
    /// The GitHub PR the local PR is linked to, once it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    github_pr: Option<GitHubLink>,
}

/// A GitHub PR, as a local PR's link to it records it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub(crate) struct GitHubLink {
    pub number: u64,
    pub url: String,
}

/// One reviewer's comments on the PR, which are the comments that give its id. There is at most
/// one draft review, the reviewer's, and the rest are copies of reviews on GitHub.
#[derive(Serialize, Deserialize)]
struct StoredReview {
    id: String,
    author: String,
    state: ReviewState,
    /// For a draft, the pending review on GitHub that posting it started; for a copy of a review
    /// on GitHub, its id there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    github: Option<String>,
    /// For a copy of a review on GitHub, whether the user wrote it, and so may edit and delete
    /// its comments there.
    #[serde(default)]
    mine: bool,
    /// The commit the PR's head was at when the review was started.
    head: String,
    /// What the review says overall.
    body: String,
    created_at: chrono::DateTime<chrono::FixedOffset>,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ReviewState {
    /// A local draft review: only this repository has it, and its comments can be changed.
    Draft,
    /// The rest are copies of reviews on GitHub, as last fetched, which only fetching changes.
    /// Pending is the user's own draft review on GitHub, which only they see.
    Pending,
    /// Commented, Approved or Changes requested on GitHub. The first gitu with local PRs called
    /// a review posted to GitHub published; a fetch replaces it.
    #[serde(alias = "published")]
    Submitted,
    Dismissed,
}

impl ReviewState {
    fn name(self) -> &'static str {
        match self {
            ReviewState::Draft => "draft",
            ReviewState::Pending => "pending",
            ReviewState::Submitted => "submitted",
            ReviewState::Dismissed => "dismissed",
        }
    }
}

/// Comments on a line of the PR's diff: the first and the replies. Each comment belongs to a
/// review, which needn't be the first comment's.
#[derive(Serialize, Deserialize)]
struct StoredThread {
    id: String,
    anchor: Anchor,
    resolved: bool,
    /// The GitHub id of the thread's first comment, once it is on GitHub, which is how a fetch
    /// finds the thread, and what a reply posted to it replies to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    github: Option<String>,
    comments: Vec<StoredComment>,
}

#[derive(Serialize, Deserialize)]
struct StoredComment {
    id: String,
    /// The id of the review the comment is in.
    review: String,
    author: String,
    /// The name of the agent that wrote the comment as the reviewer, if one did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    agent: Option<String>,
    body: String,
    created_at: chrono::DateTime<chrono::FixedOffset>,
    /// The comment's id on GitHub, once it has been posted there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    github: Option<String>,
    /// The comment's page on GitHub, for a copy of a comment there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    url: Option<String>,
}

/// Where a thread is: a line of a file, on the old or the new side of the diff, numbered as in
/// `commit`, which was the PR's head (for the new side) or where it branched from its base (for
/// the old side) when the thread was started. The line's text, and the lines around it, are
/// recorded so that the thread can be found again once the lines have moved.
#[derive(Serialize, Deserialize)]
struct Anchor {
    path: String,
    side: Side,
    line: u32,
    commit: String,
    /// The line, or none if the file was shorter than `line`.
    text: Vec<String>,
    /// Up to [`CONTEXT`] lines before the line, and after it.
    before: Vec<String>,
    after: Vec<String>,
}

/// The file a local PR is recorded in: `gitu/review/<head>...<base>.json` in the repository's
/// shared git directory (`git rev-parse --git-common-dir`), which all its worktrees use. Each
/// change is made while holding a lock file beside it, and is written to a temporary file that
/// then replaces it, so that gitus in several worktrees can write to it at once.
struct Store {
    path: PathBuf,
    head: String,
    base: String,
    /// git's `user.name`, whose the draft review is.
    reviewer: String,
}

/// How long to wait for another gitu to finish writing.
const LOCK_TIMEOUT: Duration = Duration::from_secs(5);

impl Store {
    fn new(common_dir: &Path, head: &str, base: &str, reviewer: &str) -> Self {
        // `...` is not in any branch name, and `%` and `/` are escaped so that the name is one
        // file name, from which the branch names can be read back.
        Self {
            path: Self::dir(common_dir).join(format!(
                "{}...{}.json",
                escape_branch(head),
                escape_branch(base)
            )),
            head: head.to_owned(),
            base: base.to_owned(),
            reviewer: reviewer.to_owned(),
        }
    }

    fn dir(common_dir: &Path) -> PathBuf {
        common_dir.join("gitu").join("review")
    }

    /// The bases of the local PRs of the branch `head` recorded in `common_dir`, sorted.
    fn bases(common_dir: &Path, head: &str) -> Res<Vec<String>> {
        let dir = Self::dir(common_dir);
        let error = |e: io::Error| {
            Error::LocalPr(format!(
                "Couldn't list the local PRs in {}: {e}",
                dir.display()
            ))
        };
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(vec![]),
            Err(e) => return Err(error(e)),
        };
        let prefix = format!("{}...", escape_branch(head));
        let mut bases = vec![];
        for entry in entries {
            let name = entry.map_err(error)?.file_name();
            let name = name.to_str().ok_or_else(|| {
                Error::LocalPr(format!(
                    "Couldn't list the local PRs in {}: a file's name is not UTF-8",
                    dir.display()
                ))
            })?;
            if let Some(base) = name
                .strip_prefix(&prefix)
                .and_then(|rest| rest.strip_suffix(".json"))
            {
                bases.push(base.replace("%2F", "/").replace("%25", "%"));
            }
        }
        bases.sort();
        Ok(bases)
    }

    fn read(&self) -> Res<PrFile> {
        let text = match fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Ok(PrFile {
                    version: VERSION,
                    head: self.head.clone(),
                    base: self.base.clone(),
                    viewed: BTreeMap::new(),
                    reviews: vec![],
                    threads: vec![],
                    github_pr: None,
                });
            }
            Err(e) => return Err(self.error("read", e)),
        };
        let mut file: PrFile = serde_json::from_str(&text)
            .map_err(|e| Error::LocalPr(format!("Couldn't read {}: {e}", self.path.display())))?;
        if file.version > VERSION {
            return Err(Error::LocalPr(format!(
                "Couldn't read {}: it was written by a newer gitu",
                self.path.display()
            )));
        }
        if file.version < 2 {
            merge_drafts(&mut file, &self.reviewer);
        }
        let mut drafts = file
            .reviews
            .iter()
            .filter(|r| r.state == ReviewState::Draft);
        if let Some(draft) = drafts.find(|r| r.author != self.reviewer) {
            return Err(Error::LocalPr(format!(
                "Couldn't read {}: its draft review is by {}, but git's user.name is {}",
                self.path.display(),
                draft.author,
                self.reviewer
            )));
        }
        Ok(file)
    }

    /// Change what is recorded as `change` says.
    fn update(&self, change: impl FnOnce(&mut PrFile)) -> Res<()> {
        let dir = self.path.parent().expect("the store is in a directory");
        fs::create_dir_all(dir).map_err(|e| self.error("write", e))?;
        let _lock = Lock::acquire(&self.path.with_extension("json.lock"))?;
        let mut file = self.read()?;
        change(&mut file);
        file.version = VERSION;
        let text = serde_json::to_string_pretty(&file).expect("a PrFile is JSON");
        let temporary = self
            .path
            .with_extension(format!("json.{}.tmp", std::process::id()));
        fs::write(&temporary, text + "\n").map_err(|e| self.error("write", e))?;
        fs::rename(&temporary, &self.path).map_err(|e| self.error("write", e))
    }

    fn error(&self, doing: &str, e: io::Error) -> Error {
        Error::LocalPr(format!("Couldn't {doing} {}: {e}", self.path.display()))
    }
}

/// Make version 1's draft reviews, one for each author, into one, the reviewer's: the other
/// authors' comments move into it, labelled with their names as agents.
fn merge_drafts(file: &mut PrFile, reviewer: &str) {
    let drafts: Vec<&StoredReview> = file
        .reviews
        .iter()
        .filter(|r| r.state == ReviewState::Draft)
        .collect();
    let Some(kept) = drafts
        .iter()
        .find(|r| r.author == reviewer)
        .or(drafts.first())
        .map(|r| r.id.clone())
    else {
        return;
    };
    let merged: Vec<String> = drafts.iter().map(|r| r.id.clone()).collect();
    for comment in file.threads.iter_mut().flat_map(|t| &mut t.comments) {
        if !merged.contains(&comment.review) {
            continue;
        }
        comment.review = kept.clone();
        if comment.author != reviewer {
            comment.agent = Some(std::mem::replace(&mut comment.author, reviewer.to_owned()));
        }
    }
    file.reviews
        .retain(|r| r.id == kept || !merged.contains(&r.id));
    for review in &mut file.reviews {
        if review.id == kept {
            review.author = reviewer.to_owned();
        }
    }
}

/// A lock file, held until dropped.
struct Lock(PathBuf);

impl Lock {
    fn acquire(path: &Path) -> Res<Self> {
        let start = Instant::now();
        loop {
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
            {
                Ok(_) => return Ok(Self(path.to_owned())),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    if start.elapsed() > LOCK_TIMEOUT {
                        return Err(Error::LocalPr(format!(
                            "Couldn't write the review: {} has been there for over {}s. \
                             Remove it if no gitu is writing the review",
                            path.display(),
                            LOCK_TIMEOUT.as_secs()
                        )));
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(e) => {
                    return Err(Error::LocalPr(format!(
                        "Couldn't lock {}: {e}",
                        path.display()
                    )));
                }
            }
        }
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concurrent_writers_do_not_lose_each_others_changes() {
        let dir = temp_dir::TempDir::new().unwrap();
        let writers: Vec<_> = (0..8)
            .map(|writer| {
                let store = Store::new(dir.path(), "feature", "main", "me");
                std::thread::spawn(move || {
                    for i in 0..20 {
                        store
                            .update(|file| {
                                file.viewed
                                    .entry(format!("writer {writer}"))
                                    .or_default()
                                    .insert(format!("file {i}"), String::new());
                            })
                            .unwrap();
                    }
                })
            })
            .collect();
        for writer in writers {
            writer.join().unwrap();
        }

        let file = Store::new(dir.path(), "feature", "main", "me")
            .read()
            .unwrap();
        let ticked: usize = file.viewed.values().map(BTreeMap::len).sum();
        assert_eq!(ticked, 8 * 20);
    }
}
