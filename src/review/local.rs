//! Local PRs: a branch reviewed against a base branch, linked to the branch's PR on GitHub if it
//! has one. What the review records is kept in a file in the repository's shared git directory,
//! so that every worktree of the repository sees it (see [`Store`]).

use super::{
    ADDED_TO_DRAFT as ADDED, Backend, Comment, PendingReview, Place, Reaction, Side, Subject,
    Thread, ViewedState, Working,
    github::{GitHub, GitHubBackend, GitHubPr, GitHubReviewState},
    missing_commit,
    remote::{self, Change, Consent},
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
    /// Whether comments written are local-only (true) or in the draft review (false), when
    /// said; by default, a new thread is in the draft review, and a reply is local-only if its
    /// thread's first comment is.
    local: Option<bool>,
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
            local: None,
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

    /// The range of the PR's commits, as `git log` takes it: from where it branched from its base
    /// to its head.
    pub fn log_range(&self) -> Res<String> {
        Ok(format!("{}..{}", self.merge_base()?, self.head_commit()?))
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

    /// Refuse a PR with no commits to review.
    fn check_commits(&self) -> Res<()> {
        if self.commit_count()? == 0 {
            return Err(no_pr(&format!(
                "{} has no commits since {}",
                self.head,
                short_ref(&self.base_ref)
            )));
        }
        Ok(())
    }

    /// Refuse a PR whose branch is behind its remote's, so that what is reviewed is the branch
    /// as it is. A branch whose upstream is gone, as git says of one deleted on the remote, has
    /// nothing to be behind. Sync, which brings the branch up to GitHub's head itself, doesn't
    /// ask this.
    pub fn check_not_behind(&self) -> Res<()> {
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
            return Ok(());
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

    /// Why the local PR is not the GitHub PR `github_pr`, if it isn't: one that doesn't branch
    /// from its base where GitHub's does, or, while the branch is where GitHub has the PR's head,
    /// whose commits are not as many as GitHub says the PR has. GitHub's base for a PR is where
    /// the base branch was when the PR was last updated, which the base branch may since have
    /// moved on from, so the base here need only have it.
    pub(crate) fn check_against(&self, github_pr: &GitHubPr) -> Res<()> {
        let githubs_base = Oid::from_str(&github_pr.base_oid)
            .ok()
            .filter(|oid| self.repo.find_commit(*oid).is_ok())
            .ok_or_else(|| {
                Error::Gh(missing_commit(
                    "base",
                    &github_pr.base_oid,
                    &github_pr.base_ref,
                ))
            })?;
        // The base as this repository has it, not as the link to GitHub gives it.
        let base = ref_commit(&self.repo, &self.base_ref)?.ok_or_else(|| {
            Error::LocalPr(format!("Couldn't find the PR's base, {}", self.base_ref))
        })?;
        let error = |e: git2::Error| Error::LocalPr(format!("Couldn't compare with GitHub: {e}"));
        let has_githubs = base == githubs_base
            || self
                .repo
                .graph_descendant_of(base, githubs_base)
                .map_err(error)?;
        if !has_githubs {
            return Err(Error::Gh(format!(
                "GitHub's {} is at {}, which {} doesn't have. Fetch, so that it does",
                self.base,
                short(&github_pr.base_oid),
                short_ref(&self.base_ref),
            )));
        }
        let head = self.head_commit()?;
        let here = self.repo.merge_base(base, head).map_err(error)?;
        let on_github = self.repo.merge_base(githubs_base, head).map_err(error)?;
        if here != on_github {
            return Err(Error::Gh(format!(
                "The PR branched from {} at {} here, but at {} on GitHub, so its diff here \
                 would not be GitHub's",
                short_ref(&self.base_ref),
                short(&here.to_string()),
                short(&on_github.to_string()),
            )));
        }
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

    /// Whether `oid` is a commit of the PR's head branch before its head: one the head has, and
    /// is not.
    pub fn is_older_commit_of_head(&self, oid: &str) -> Res<bool> {
        let head = self.head_commit()?;
        let Some(oid) = Oid::from_str(oid)
            .ok()
            .filter(|oid| self.repo.find_commit(*oid).is_ok())
        else {
            return Ok(false);
        };
        Ok(oid != head
            && self
                .repo
                .graph_descendant_of(head, oid)
                .map_err(|e| Error::LocalPr(format!("Couldn't compare commits: {e}")))?)
    }

    /// The draft threads not yet on GitHub whose lines were found in another commit than the PR's
    /// head (or, for the old side, where it branched from its base) now: those written, or last
    /// moved, before the PR changed. Posting them would put them where the user hasn't seen them.
    pub fn stale_drafts(&self) -> Res<Vec<String>> {
        let file = self.store.read()?;
        let head = self.head_commit()?.to_string();
        let merge_base = self.merge_base()?.to_string();
        let drafts: HashSet<&str> = file
            .reviews
            .iter()
            .filter(|r| r.state == ReviewState::Draft)
            .map(|r| r.id.as_str())
            .collect();
        Ok(file
            .threads
            .iter()
            .filter(|t| t.github.is_none() && t.subject == Subject::Line)
            .filter(|t| {
                t.comments
                    .iter()
                    .any(|c| c.review.as_deref().is_some_and(|r| drafts.contains(r)))
            })
            .filter(|t| {
                let now = match t.anchor.side {
                    Side::New => &head,
                    Side::Old => &merge_base,
                };
                t.anchor.commit != *now
            })
            .map(|t| t.id.clone())
            .collect())
    }

    /// Move the draft threads `ids` to where their lines are now, in the PR's head (or where it
    /// branched from its base), as the screen shows them; one whose line has gone goes on its
    /// file. Returns how many went on their files.
    pub fn move_drafts(&self, ids: &[String]) -> Res<usize> {
        let head = self.head_commit()?;
        let merge_base = self.merge_base()?;
        let mut moved: HashMap<String, Option<Anchor>> = HashMap::new();
        for thread in self.threads()? {
            if !ids.contains(&thread.id) {
                continue;
            }
            let anchor = match thread.line {
                Some((side, line)) => {
                    let commit = match side {
                        Side::New => head,
                        Side::Old => merge_base,
                    };
                    Some(Anchor {
                        start_line: thread.start_line,
                        ..self.anchor_at(commit, &thread.path, (side, line))?
                    })
                }
                None => None,
            };
            moved.insert(thread.id, anchor);
        }
        let to_file = moved.values().filter(|anchor| anchor.is_none()).count();
        self.store.update(|file| {
            for thread in &mut file.threads {
                match moved.remove(&thread.id) {
                    Some(Some(anchor)) => thread.anchor = anchor,
                    // Its anchor stays, to say which line it was on.
                    Some(None) => thread.subject = Subject::File,
                    None => {}
                }
            }
        })?;
        Ok(to_file)
    }

    /// The draft replies, by their text, in threads that have been deleted on GitHub, which can't
    /// be posted.
    pub fn orphaned_replies(&self) -> Res<Vec<String>> {
        Ok(self
            .threads()?
            .into_iter()
            .filter(|t| t.deleted_on_github)
            .flat_map(|t| t.comments.into_iter().filter(|c| c.draft).map(|c| c.body))
            .collect())
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
            start_line: None,
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

    /// Delete the comment `comment_id`, wherever it is: see [`Backend::delete_comment`].
    pub fn delete(&self, comment_id: &str, consent: Consent) -> Res<()> {
        let file = self.store.read()?;
        let thread = file
            .threads
            .iter()
            .find(|t| t.comments.iter().any(|c| c.id == comment_id))
            .ok_or_else(|| Error::LocalPr(format!("There is no comment {comment_id}")))?;
        self.delete_comment(&thread.id, comment_id, consent)
            .map(|_| ())
    }

    /// The draft review's comments, as they are to be posted, in that order: each thread's
    /// first comment before its replies. None if there is no draft review. A thread whose line
    /// has gone starts on its file, saying which line it was on.
    pub fn to_post(&self) -> Res<Vec<PostComment>> {
        let file = self.store.read()?;
        let Some(mine) = file.reviews.iter().find(|r| r.state == ReviewState::Draft) else {
            return Ok(vec![]);
        };
        let lines: HashMap<String, Option<(Side, u32)>> = self
            .threads()?
            .into_iter()
            .map(|t| (t.id, t.line))
            .collect();
        let mut comments = vec![];
        for thread in &file.threads {
            for (i, comment) in thread.comments.iter().enumerate() {
                if comment.review.as_deref() != Some(mine.id.as_str()) {
                    continue;
                }
                // A thread whose line has gone, or on a whole file, starts on its file. One
                // whose line has gone says which it was, so that it means what it did.
                let line = lines.get(&thread.id).copied().flatten();
                let place = match (thread.subject, line) {
                    (Subject::Line, Some((side, line))) => Place::Line(side, line),
                    _ => Place::File,
                };
                let starts = (i == 0 && thread.github.is_none()).then_some(place);
                // A thread whose line has gone, now or when sync moved it to its file, still has
                // the line it was on in its anchor; one fetched on a whole file has none.
                let gone = starts == Some(Place::File) && thread.anchor.line > 0;
                let body = if gone {
                    let anchor = &thread.anchor;
                    let side = if anchor.side == Side::Old {
                        "old line"
                    } else {
                        "line"
                    };
                    match anchor.text.first() {
                        Some(text) => {
                            format!("On {side} {}: `{text}`\n\n{}", anchor.line, comment.body)
                        }
                        None => format!("On {side} {}:\n\n{}", anchor.line, comment.body),
                    }
                } else {
                    comment.body.clone()
                };
                comments.push(PostComment {
                    id: comment.id.clone(),
                    thread: thread.id.clone(),
                    body,
                    path: thread.anchor.path.clone(),
                    starts,
                    reply_to: thread.github.clone(),
                    posting_since: comment.posting_since,
                });
            }
        }
        Ok(comments)
    }

    /// Record that posting the draft comment `comment_id` is beginning, unless an earlier attempt
    /// already began it, so that a later attempt can look for it on GitHub.
    pub fn mark_posting(&self, comment_id: &str) -> Res<()> {
        let now = chrono::Local::now().fixed_offset();
        self.store.update(|file| {
            for comment in file.threads.iter_mut().flat_map(|t| &mut t.comments) {
                if comment.id == comment_id && comment.posting_since.is_none() {
                    comment.posting_since = Some(now);
                }
            }
        })
    }

    /// Record that the draft comment `comment_id` has been posted, by removing it, and, if it
    /// started its thread, that the thread's first comment on GitHub is `started`. The comment
    /// comes back as part of the copy of the review on GitHub at the next fetch, in the same
    /// thread: a copy of that thread made apart from it, by a fetch before it was known to be
    /// posted, goes.
    pub fn mark_posted(&self, comment_id: &str, started: Option<&str>) -> Res<()> {
        self.store.update(|file| {
            let mut posted_in = None;
            for thread in &mut file.threads {
                if thread.comments.iter().any(|c| c.id == comment_id) {
                    thread.comments.retain(|c| c.id != comment_id);
                    if let Some(started) = started {
                        thread.github = Some(started.to_owned());
                    }
                    posted_in = Some(thread.id.clone());
                }
            }
            if let (Some(started), Some(posted_in)) = (started, posted_in) {
                let drafts = draft_ids(file);
                file.threads.retain(|t| {
                    t.id == posted_in
                        || t.github.as_deref() != Some(started)
                        || t.comments.iter().any(|c| is_local(&drafts, c))
                });
            }
            // A thread on GitHub stays, with no comments until the fetch after posting brings
            // them back, so that it is the same thread.
            file.threads
                .retain(|t| !t.comments.is_empty() || t.github.is_some());
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
                // On a whole file: it has no line.
                _ if thread.subject == Subject::File => Anchor {
                    path: thread.path.clone(),
                    side: Side::New,
                    line: 0,
                    start_line: None,
                    commit: pr.head_oid.clone(),
                    text: vec![],
                    before: vec![],
                    after: vec![],
                },
                Some((Side::New, n)) => {
                    let head = commit(&pr.head_oid, &self.head, "head")?;
                    Anchor {
                        start_line: thread.start_line,
                        ..self.anchor_at(head, &thread.path, (Side::New, n))?
                    }
                }
                Some((Side::Old, n)) => {
                    let head = commit(&pr.head_oid, &self.head, "head")?;
                    let base = commit(&pr.base_oid, &pr.base_ref, "base")?;
                    let merge_base = self.repo.merge_base(base, head).map_err(|e| {
                        Error::Gh(format!("Couldn't find where the PR branched: {e}"))
                    })?;
                    Anchor {
                        start_line: thread.start_line,
                        ..self.anchor_at(merge_base, &thread.path, (Side::Old, n))?
                    }
                }
                // Outdated on GitHub: it has no line to find.
                None => Anchor {
                    path: thread.path.clone(),
                    side: Side::New,
                    line: 0,
                    start_line: None,
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
            let drafts = draft_ids(file);
            for thread in &mut file.threads {
                thread.comments.retain(|c| is_local(&drafts, c));
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
                        review: Some(review_of[&c.id].clone()),
                        author: c.author.clone(),
                        agent: None,
                        body: c.body.clone(),
                        created_at: c.created_at,
                        github: Some(c.id.clone()),
                        url: c.url.clone(),
                        posting_since: None,
                        reactions: c.reactions.clone(),
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
                            subject: thread.subject,
                            resolved: thread.resolved,
                            github: Some(first),
                            comments: vec![],
                        });
                        file.threads.last_mut().unwrap()
                    }
                };
                // As GitHub has it, as when a thread whose line had gone was posted on its file.
                local.subject = thread.subject;
                local.resolved = thread.resolved;
                let unposted = std::mem::take(&mut local.comments);
                local.comments = comments.into_iter().chain(unposted).collect();
            }
            file.threads.retain(|t| !t.comments.is_empty());
        })
    }

    /// The local PR as JSON, for `delta tui review show --format json`: its reviews, each with
    /// its author and state (draft, pending, submitted or dismissed), and its threads, each with
    /// its file, side, line now (`null` once it has gone), and comments.
    pub fn json(&self) -> Res<serde_json::Value> {
        let file = self.store.read()?;
        // Each thread's line, and first line if it is on several.
        type Place = (Option<(Side, u32)>, Option<u32>);
        let threads = self.threads()?;
        let deleted: HashSet<&str> = threads
            .iter()
            .filter(|t| t.deleted_on_github)
            .map(|t| t.id.as_str())
            .collect();
        let places: HashMap<String, Place> = threads
            .iter()
            .map(|t| (t.id.clone(), (t.line, t.start_line)))
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
                let (line, start_line) = places.get(&t.id).copied().unwrap_or_default();
                serde_json::json!({
                    "id": t.id,
                    "path": t.anchor.path,
                    "subject": t.subject,
                    "side": t.anchor.side,
                    "line": line.map(|(_, n)| n),
                    "start_line": start_line,
                    "deleted_on_github": deleted.contains(t.id.as_str()),
                    "resolved": t.resolved,
                    "comments": t.comments.iter().map(|c| serde_json::json!({
                        "id": c.id,
                        "review": c.review,
                        "local_only": c.review.is_none(),
                        "author": c.author,
                        "agent": c.agent,
                        "body": c.body,
                        "created_at": c.created_at,
                        "reactions": c.reactions.iter().map(|r| serde_json::json!({
                            "emoji": r.emoji,
                            "count": r.count,
                            "people": r.people.iter().map(|p| &p.login).collect::<Vec<_>>(),
                        })).collect::<Vec<_>>(),
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
    pub fn edit(&self, comment_id: &str, body: &str, consent: Consent) -> Res<()> {
        let thread = self.thread_of(comment_id)?;
        self.edit_comment(&thread, comment_id, body, consent)
            .map(|_| ())
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
        let Some(review_id) = &comment.review else {
            // Local-only.
            return Ok(Changeable::Draft);
        };
        let review = file.reviews.iter().find(|r| r.id == *review_id);
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
                was: comment.body.clone(),
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

    /// The same PR, with the comments written local-only if `local`, and otherwise in the draft
    /// review.
    pub fn with_bucket(self, local: bool) -> Self {
        Self {
            local: Some(local),
            ..self
        }
    }

    /// Move a local comment to local-only if `local`, and otherwise to the draft review,
    /// starting the draft review if there is none.
    pub fn move_comment(&self, comment_id: &str, local: bool) -> Res<()> {
        let head = self.branch_commit(&self.head)?.to_string();
        let now = chrono::Local::now().fixed_offset();
        let mut result = Err(Error::LocalPr(format!(
            "Couldn't move the comment: there is no comment {comment_id}"
        )));
        self.store.update(|file| {
            let drafts = draft_ids(file);
            let draft = drafts.iter().next().cloned();
            let Some(comment) = file
                .threads
                .iter_mut()
                .flat_map(|t| &mut t.comments)
                .find(|c| c.id == comment_id)
            else {
                return;
            };
            if !is_local(&drafts, comment) {
                result = Err(Error::LocalPr(
                    "Couldn't move the comment: only a local comment can be moved, and this one \
                     is on GitHub"
                        .to_owned(),
                ));
                return;
            }
            if local && comment.posting_since.is_some() {
                result = Err(Error::LocalPr(STOPPED_POSTING.to_owned()));
                return;
            }
            result = Ok(());
            if local {
                comment.review = None;
                remove_empty_drafts(file);
                return;
            }
            if comment.review.is_some() {
                return;
            }
            let review = draft.unwrap_or_else(new_id);
            comment.review = Some(review.clone());
            if !drafts.contains(&review) {
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
        result
    }

    /// The draft replies, by their text, in threads whose first comment is local-only, which
    /// can't be posted before their thread.
    pub fn blocked_replies(&self) -> Res<Vec<String>> {
        let file = self.store.read()?;
        let drafts = draft_ids(&file);
        Ok(file
            .threads
            .iter()
            .filter(|t| t.comments.first().is_some_and(|c| c.review.is_none()))
            .flat_map(|t| &t.comments)
            .filter(|c| c.review.as_ref().is_some_and(|r| drafts.contains(r)))
            .map(|c| c.body.clone())
            .collect())
    }

    /// Give a comment saying `body`, local-only if `local` and otherwise in the draft review, to
    /// `add` to put in a thread. If `add` puts one in the draft review, the draft review is
    /// started if there is none; returns whether `add` put it in a thread.
    fn add_comment(
        &self,
        body: &str,
        local: bool,
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
                review: (!local).then(|| review.clone()),
                author: self.reviewer.clone(),
                agent: self.agent.clone(),
                body: body.to_owned(),
                created_at: now,
                github: None,
                url: None,
                posting_since: None,
                reactions: vec![],
            };
            added = add(&mut file.threads, comment);
            if added && !local && draft.is_none() {
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

    /// The PR's review threads as markdown, for `delta tui review show` (see [`markdown`]).
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

    /// Leave `body` for an agent, about the thread `thread`, or else the whole PR, for `delta tui
    /// review listen` to print.
    pub fn send_message(&self, body: &str, thread: Option<&str>) -> Res<()> {
        if let Some(thread) = thread
            && !self.store.read()?.threads.iter().any(|t| t.id == thread)
        {
            return Err(Error::LocalPr(format!(
                "Couldn't send the message: the PR has no thread {thread}"
            )));
        }
        let now = chrono::Local::now().fixed_offset();
        self.store.update(|file| {
            file.messages.push(StoredMessage {
                id: new_id(),
                body: body.to_owned(),
                thread: thread.map(str::to_owned),
                created_at: now,
                delivered_at: None,
                done_at: None,
            });
        })
    }

    /// The messages to agents that no `listen` has printed, marked printed, once there are any.
    /// While there are none, it waits, watching the directory the PR is recorded in.
    pub fn wait_for_messages(&self) -> Res<Vec<Message>> {
        let dir = self
            .store
            .path
            .parent()
            .expect("the store is in a directory");
        fs::create_dir_all(dir).map_err(|e| self.store.error("watch", e))?;
        // FSEvents gives the real paths of what changed, which a path through a symlink, such as
        // macOS's /var, is not.
        let dir = dir
            .canonicalize()
            .map_err(|e| self.store.error("watch", e))?;
        let watch_error = |e: notify::Error| {
            Error::LocalPr(format!(
                "Couldn't watch {} for messages: {e}",
                dir.display()
            ))
        };
        use notify::Watcher;
        let (sender, events) = std::sync::mpsc::channel();
        let mut watcher = notify::recommended_watcher(sender).map_err(watch_error)?;
        // Before looking, so that a message sent after looking wakes it.
        watcher
            .watch(&dir, notify::RecursiveMode::NonRecursive)
            .map_err(watch_error)?;
        loop {
            let messages = self.take_messages()?;
            if !messages.is_empty() {
                return Ok(messages);
            }
            match events.recv() {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => return Err(watch_error(e)),
                Err(_) => {
                    return Err(Error::LocalPr(format!(
                        "Couldn't watch {} for messages: the watcher stopped",
                        dir.display()
                    )));
                }
            }
        }
    }

    /// The messages to agents that no `listen` has printed, marked printed. Those printed before
    /// are marked done, as the agent listening again has done what they asked.
    fn take_messages(&self) -> Res<Vec<Message>> {
        // Looked for first without writing, which would wake every listener.
        if self
            .store
            .read()?
            .messages
            .iter()
            .all(|m| m.delivered_at.is_some() && m.done_at.is_some())
        {
            return Ok(vec![]);
        }
        let now = chrono::Local::now().fixed_offset();
        let mut taken = vec![];
        self.store.update(|file| {
            for message in &mut file.messages {
                if message.delivered_at.is_some() && message.done_at.is_none() {
                    message.done_at = Some(now);
                } else if message.delivered_at.is_none() {
                    message.delivered_at = Some(now);
                    taken.push(Message {
                        from: self.reviewer.clone(),
                        head: self.head.clone(),
                        base: self.base.clone(),
                        body: message.body.clone(),
                        thread: message.thread.clone(),
                    });
                }
            }
        })?;
        Ok(taken)
    }

    /// Change the comment `comment_id` in the thread `thread_id` as `change` says, given the
    /// thread and the comment's index in it, if the comment is local: in the draft review, or
    /// local-only. A thread left with no comments is deleted.
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
            let drafts = draft_ids(file);
            let Some(thread) = file.threads.iter_mut().find(|t| t.id == thread_id) else {
                return;
            };
            let Some(i) = thread.comments.iter().position(|c| c.id == comment_id) else {
                return;
            };
            if !is_local(&drafts, &thread.comments[i]) {
                result = Err(Error::LocalPr(NOT_DRAFT.to_owned()));
                return;
            }
            if thread.comments[i].posting_since.is_some() {
                result = Err(Error::LocalPr(STOPPED_POSTING.to_owned()));
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
    /// The user's own comment on GitHub, which said `was` when last fetched.
    OnGitHub {
        id: String,
        starts_thread: bool,
        was: String,
    },
}

pub(crate) const NOT_DRAFT: &str = "Only a local comment, or your own on GitHub, can be changed";

/// Why a comment a sync was posting when it stopped can't be changed: it may be in the pending
/// review on GitHub already, where the next sync finds it by its text and place.
const STOPPED_POSTING: &str = "Couldn't change the comment: a sync stopped while posting it, so it \
                               may be in your pending review on GitHub already. To change it, \
                               sync again, which finds it there, and then change it";

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
/// lines it is on (the line marked `>`), and its comments, saying which are drafts and which
/// local-only.
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
            _ if thread.subject == Subject::File => format!("{} (file)", thread.path),
            (Some((_, n)), _) => match thread.start_line {
                Some(start) => format!("{}:{start}-{n}{old}", thread.path),
                None => format!("{}:{n}{old}", thread.path),
            },
            (None, Some(n)) => format!("{} (outdated: was line {n}{old})", thread.path),
            (None, None) => format!("{} (outdated)", thread.path),
        };
        let resolved = match (thread.resolved, thread.deleted_on_github) {
            (_, true) => ", deleted on GitHub",
            (true, false) => ", resolved",
            (false, false) => "",
        };
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
            let bucket = match (comment.draft, comment.local_only) {
                (true, _) => "draft, ",
                (_, true) => "local-only, ",
                _ => "",
            };
            let time = comment.created_at.format("%Y-%m-%d %H:%M");
            out += &format!(
                "\n**{}** ({bucket}{time})\n\n{}\n",
                comment.agent.as_ref().unwrap_or(&comment.author),
                comment.body.trim_end()
            );
        }
    }
    out
}

/// The draft review, as [`LocalBackend::to_post`] gives it.
pub(crate) struct PostComment {
    pub id: String,
    pub thread: String,
    pub body: String,
    pub path: String,
    /// Where its thread goes on GitHub, if posting it starts the thread there.
    pub starts: Option<Place>,
    /// The GitHub id of its thread's first comment, if that is on GitHub.
    pub reply_to: Option<String>,
    /// When an earlier attempt to post it began, if one did and didn't finish.
    pub posting_since: Option<chrono::DateTime<chrono::FixedOffset>>,
}

/// Remove the draft reviews with no comments left.
fn remove_empty_drafts(file: &mut PrFile) {
    let threads = &file.threads;
    file.reviews.retain(|review| {
        review.state != ReviewState::Draft
            || threads
                .iter()
                .flat_map(|t| &t.comments)
                .any(|c| c.review.as_deref() == Some(review.id.as_str()))
    });
}

/// The ids of the draft reviews in `file`.
fn draft_ids(file: &PrFile) -> HashSet<String> {
    file.reviews
        .iter()
        .filter(|r| r.state == ReviewState::Draft)
        .map(|r| r.id.clone())
        .collect()
}

/// Whether `comment` is local: in a draft review, one of `drafts`, or local-only.
fn is_local(drafts: &HashSet<String>, comment: &StoredComment) -> bool {
    comment
        .review
        .as_ref()
        .is_none_or(|review| drafts.contains(review))
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
                "{head} has local PRs into {} and {last}{}",
                many.join(", "),
                drafts_kept(head, &bases)
            )));
        }
    };
    let kept = drafts_kept(head, &bases);
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
                    "{head}'s local PR is linked to #{}, but GitHub's PR for it is #{}{kept}",
                    link.number, pr.number
                )));
            }
            if base != pr.base_ref {
                return Err(not_handled(&format!(
                    "{head}'s local PR is into {base}, but GitHub's PR #{} is into {}{kept}",
                    pr.number, pr.base_ref
                )));
            }
            Ok((base, Some(pr), None))
        }
        (None, Some((_, Some(link)))) => Err(not_handled(&format!(
            "{head}'s local PR is linked to #{}, which GitHub no longer has open{kept}",
            link.number
        ))),
        (None, Some((base, None))) => Ok((base, None, None)),
        (None, None) => Ok((github.default_branch()?, None, None)),
    }
}

/// The user's pending review on GitHub, as the copy of it in `file` has it.
fn pending_review(file: &PrFile) -> Option<PendingReview> {
    let pending = file
        .reviews
        .iter()
        .find(|r| r.state == ReviewState::Pending && r.mine)?;
    let comments = file
        .threads
        .iter()
        .flat_map(|t| &t.comments)
        .filter(|c| c.review.as_deref() == Some(pending.id.as_str()))
        .count();
    Some(PendingReview { comments })
}

/// A local PR, as the status view shows it.
pub(crate) struct PrSummary {
    /// The base it is diffed against, as git shows a branch.
    pub base: String,
    pub link: Option<GitHubLink>,
    pub commits: usize,
    pub drafts: usize,
    pub local_only: usize,
    pub pending: Option<PendingReview>,
}

/// The branch `head`'s local PR, as recorded in this repository, without asking GitHub; `None`
/// if it has none. A branch with more than one is an error, as not yet handled.
pub(crate) fn summary(repo: &Repository, head: &str) -> Res<Option<PrSummary>> {
    let base = match Store::bases(repo.commondir(), head)?.as_slice() {
        [] => return Ok(None),
        [base] => base.clone(),
        [many @ .., last] => {
            return Err(not_handled(&format!(
                "{head} has local PRs into {} and {last}",
                many.join(", ")
            )));
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
    let file = Store::new(repo.commondir(), head, &base, &reviewer).read()?;
    summarize(repo, &file, &resolve_base(repo, &base)?, head).map(Some)
}

/// The local PR recorded in `file`, of the branch `head` into the base at `base_ref`.
fn summarize(repo: &Repository, file: &PrFile, base_ref: &str, head: &str) -> Res<PrSummary> {
    let missing = |name: &str| Error::LocalPr(format!("Couldn't find {name}"));
    let base_commit = ref_commit(repo, base_ref)?.ok_or_else(|| missing(base_ref))?;
    let head_ref = format!("refs/heads/{head}");
    let head_commit = ref_commit(repo, &head_ref)?.ok_or_else(|| missing(&head_ref))?;
    let error = |e: git2::Error| Error::LocalPr(format!("Couldn't read {head}'s PR: {e}"));
    let merge_base = repo.merge_base(base_commit, head_commit).map_err(error)?;
    let mut walk = repo.revwalk().map_err(error)?;
    walk.push(head_commit).map_err(error)?;
    walk.hide(merge_base).map_err(error)?;
    let commits = walk
        .try_fold(0, |n, oid| oid.map(|_| n + 1))
        .map_err(error)?;
    let drafts = file
        .reviews
        .iter()
        .filter(|r| r.state == ReviewState::Draft)
        .map(|draft| {
            file.threads
                .iter()
                .flat_map(|t| &t.comments)
                .filter(|c| c.review.as_deref() == Some(draft.id.as_str()))
                .count()
        })
        .sum();
    let local_only = file
        .threads
        .iter()
        .flat_map(|t| &t.comments)
        .filter(|c| c.review.is_none())
        .count();
    Ok(PrSummary {
        base: short_ref(base_ref).to_owned(),
        pending: pending_review(file),
        link: file.github_pr.clone(),
        commits,
        drafts,
        local_only,
    })
}

impl PrSummary {
    /// The PR in one line, as the status view and the PR's screen show it: its base, its GitHub
    /// PR or that it is not on GitHub, and how many commits and draft comments it has, with the
    /// user's pending review on GitHub if they have one.
    pub fn line(&self) -> String {
        let count = |n: usize, noun: &str| match n {
            1 => format!("1 {noun}"),
            n => format!("{n} {noun}s"),
        };
        let mut parts = vec![format!("PR into {}", self.base)];
        parts.push(match &self.link {
            Some(link) => format!("#{}", link.number),
            None => "not on GitHub".to_owned(),
        });
        parts.push(count(self.commits, "commit"));
        if self.drafts > 0 {
            parts.push(count(self.drafts, "draft comment"));
        }
        if self.local_only > 0 {
            parts.push(count(self.local_only, "local-only comment"));
        }
        if let Some(pending) = self.pending {
            let comments = match pending.comments {
                0 => "no comments".to_owned(),
                n => count(n, "comment"),
            };
            parts.push(format!("pending review on GitHub with {comments}"));
        }
        parts.join(" · ")
    }
}

/// What to add to an error about the branch `head`'s local PRs, into `bases`: that their drafts
/// are kept, and how to see them.
fn drafts_kept(head: &str, bases: &[String]) -> String {
    let commands: Vec<String> = bases
        .iter()
        .map(|base| format!("delta tui review show --head {head} --base {base}"))
        .collect();
    match commands.as_slice() {
        [] => String::new(),
        [one] => format!(". Its drafts are kept: see them with {one}"),
        many => format!(
            ". Their drafts are kept: see them with {}",
            many.join(", or ")
        ),
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
        Ok(pending_review(&self.store.read()?))
    }

    fn working(&self) -> Res<Vec<Working>> {
        Ok(self
            .store
            .read()?
            .messages
            .into_iter()
            .filter(|message| message.done_at.is_none())
            .filter_map(|message| {
                Some(Working {
                    thread: message.thread,
                    since: message.delivered_at?,
                })
            })
            .collect())
    }

    fn summary(&self) -> Res<Option<String>> {
        let summary = summarize(&self.repo, &self.store.read()?, &self.base_ref, &self.head)?;
        Ok(Some(summary.line()))
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
        let review_of = |review: &Option<String>| {
            review
                .as_ref()
                .and_then(|id| file.reviews.iter().find(|r| r.id == *id))
        };
        // A local-only comment is in no review, and is local as a draft is.
        let local = |review: &Option<String>| {
            review.is_none() || review_of(review).is_some_and(|r| r.state == ReviewState::Draft)
        };
        let draft = |review: &Option<String>| {
            review_of(review).is_some_and(|r| r.state == ReviewState::Draft)
        };
        let mine = |review: &Option<String>| review_of(review).is_some_and(|r| r.mine);
        let submitted = |review: &Option<String>| {
            review_of(review)
                .is_some_and(|r| matches!(r.state, ReviewState::Submitted | ReviewState::Dismissed))
        };
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
            let line = if thread.subject == Subject::File || anchor.text.is_empty() {
                // A thread on a whole file has no line, nor one fetched from GitHub already
                // outdated.
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
            // A thread on several lines keeps as many, ending where its line now is.
            let start_line = line.and_then(|line| {
                let span = anchor.line.checked_sub(anchor.start_line?)?;
                line.checked_sub(span)
            });
            threads.push(Thread {
                id: thread.id.clone(),
                path: thread.anchor.path.clone(),
                line: line.map(|line| (thread.anchor.side, line)),
                start_line,
                subject: thread.subject,
                resolved: thread.resolved,
                // On GitHub, and with none of its comments copied from there.
                deleted_on_github: thread.github.is_some()
                    && !thread.comments.is_empty()
                    && thread.comments.iter().all(|c| local(&c.review)),
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
                        local_only: comment.review.is_none(),
                        reactions: comment.reactions.clone(),
                        // In the TUI the user edits their own comments, and replies to an
                        // agent's; they can delete any local comment.
                        editable: (local(&comment.review) && comment.agent.is_none())
                            || mine(&comment.review),
                        deletable: local(&comment.review) || mine(&comment.review),
                        pending: review_of(&comment.review)
                            .is_some_and(|r| r.state == ReviewState::Pending),
                        submitted: submitted(&comment.review),
                    })
                    .collect(),
            });
        }
        Ok(threads)
    }

    /// A reply is local-only if its thread's first comment is, unless [`LocalBackend::with_bucket`]
    /// says otherwise.
    fn reply(&self, thread_id: &str, body: &str) -> Res<&'static str> {
        let local = match self.local {
            Some(local) => local,
            None => self
                .store
                .read()?
                .threads
                .iter()
                .find(|t| t.id == thread_id)
                .and_then(|t| t.comments.first())
                .is_some_and(|c| c.review.is_none()),
        };
        let added = self.add_comment(body, local, |threads, comment| {
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
        self.add_comment(body, self.local.unwrap_or(false), |threads, comment| {
            threads.push(StoredThread {
                id: new_id(),
                anchor,
                subject: Subject::Line,
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
    fn edit_comment(
        &self,
        thread_id: &str,
        comment_id: &str,
        body: &str,
        consent: Consent,
    ) -> Res<&'static str> {
        if let Changeable::OnGitHub { id, was, .. } = self.whose(thread_id, comment_id)? {
            let github = self.github_pr()?;
            let change = Change::EditComment {
                comment: &id,
                body,
                was: &was,
            };
            remote::write(github.github.as_ref(), change, consent)?;
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
    fn delete_comment(
        &self,
        thread_id: &str,
        comment_id: &str,
        consent: Consent,
    ) -> Res<&'static str> {
        if let Changeable::OnGitHub {
            id,
            starts_thread,
            was,
        } = self.whose(thread_id, comment_id)?
        {
            if starts_thread {
                return Err(Error::LocalPr(
                    "Couldn't delete the comment: it starts a thread on GitHub, to which anyone \
                     may have a draft reply. Edit it, or delete it on GitHub"
                        .to_owned(),
                ));
            }
            let github = self.github_pr()?;
            let change = Change::DeleteComment {
                comment: &id,
                was: &was,
            };
            remote::write(github.github.as_ref(), change, consent)?;
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

    fn url(&self) -> Option<String> {
        self.linked.as_ref().and_then(|linked| linked.url())
    }
}

/// A message from the reviewer to an agent.
pub(crate) struct Message {
    pub from: String,
    /// The PR's branch and base branch.
    pub head: String,
    pub base: String,
    pub body: String,
    /// The thread the message is about, if it is about one rather than the whole PR.
    pub thread: Option<String>,
}

impl Message {
    /// The message as markdown, for `delta tui review listen`.
    pub fn markdown(&self) -> String {
        let about = match &self.thread {
            Some(thread) => format!(", about thread {thread}"),
            None => String::new(),
        };
        format!(
            "# Message from {} on {} into {}{about}\n\n{}\n",
            self.from, self.head, self.base, self.body
        )
    }
}

/// The version of [`PrFile`] this gitu writes. Version 1 had a draft review for each author,
/// an agent's among them; version 2 has one, the reviewer's (see [`merge_drafts`]). Version 3
/// records the GitHub PR the local PR is linked to, which a gitu writing version 2 would drop.
/// Before version 4, a draft review recorded the pending review on GitHub that posting it
/// started, which posting no longer needs. Version 5 records a thread's subject and the first of
/// its lines, which a gitu writing version 4 would drop. Version 6 records when posting a draft
/// comment began, which a gitu writing version 5 would drop. Version 7 has local-only comments,
/// in no review, which a gitu writing version 6 can't read. Version 8 records messages to agents,
/// which a gitu writing version 7 would drop.
const VERSION: u32 = 8;

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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    messages: Vec<StoredMessage>,
}

/// A message from the reviewer to an agent, which `delta tui review listen` prints.
#[derive(Serialize, Deserialize)]
struct StoredMessage {
    id: String,
    body: String,
    /// The thread the message is about, if it is about one rather than the whole PR.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    thread: Option<String>,
    created_at: chrono::DateTime<chrono::FixedOffset>,
    /// When `listen` printed it to an agent; none until then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    delivered_at: Option<chrono::DateTime<chrono::FixedOffset>>,
    /// When the agent listened for the next message, having done what this one asked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    done_at: Option<chrono::DateTime<chrono::FixedOffset>>,
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
    /// For a copy of a review on GitHub, its id there. A draft review has none.
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
    /// What the thread is on: its anchor's line, or its whole file.
    #[serde(default)]
    subject: Subject,
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
    /// The id of the review the comment is in: none for a local-only comment, which sync never
    /// posts, such as a conversation with an agent.
    review: Option<String>,
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
    /// For a draft comment, when an attempt to post it began that may not have finished: GitHub
    /// may have added it though gitu never heard back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    posting_since: Option<chrono::DateTime<chrono::FixedOffset>>,
    /// For a copy of a comment on GitHub, the reactions to it there.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    reactions: Vec<Reaction>,
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
    /// For a thread on several lines, the first: `line` is the last.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    start_line: Option<u32>,
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
                    messages: vec![],
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
        if file.version < 4 {
            for review in &mut file.reviews {
                if review.state == ReviewState::Draft {
                    review.github = None;
                }
            }
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
        if !comment.review.as_ref().is_some_and(|r| merged.contains(r)) {
            continue;
        }
        comment.review = Some(kept.clone());
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
