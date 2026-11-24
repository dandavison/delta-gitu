//! The lifecycle of reviews on a PR, local and on GitHub, as a state machine: what each action in
//! my repository, my repository on a second machine, and a colleague's repository does, and the
//! invariants that every sequence of actions keeps. Each repository has one author, its git
//! `user.name`: an agent writing in my repository writes as me, with its name only as a label on
//! its comments, which changes nothing here. It is the definition the product follows (see the lifecycle
//! proposal on dandavison/delta-maintenance#20): its tests explore every sequence of actions up
//! to a bound, and `tests::lifecycle` checks that gitu, driven through the same actions, ends in
//! the same states.
//!
//! Comments are identified by number, and a comment's text is its number, so that a state of
//! gitu can be compared with a state of the model.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

pub(crate) type Id = u32;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Person {
    Me,
    Colleague,
}

/// A repository, and so a local store of reviews.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Repo {
    /// Mine, where my agent writes too, as me.
    Laptop,
    /// Mine, on a second machine.
    Desktop,
    Colleague,
}

impl Repo {
    pub const ALL: [Repo; 3] = [Repo::Laptop, Repo::Desktop, Repo::Colleague];

    /// The repository's author, whose GitHub login posts from it.
    pub fn github_user(self) -> Person {
        match self {
            Repo::Laptop | Repo::Desktop => Person::Me,
            Repo::Colleague => Person::Colleague,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum ReviewState {
    Pending,
    Submitted,
    Dismissed,
}

/// A comment in a thread: a local draft comment, or a comment on GitHub.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum CommentRef {
    Local(Id),
    GitHub(Id),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct GitHubComment {
    pub author: Person,
    pub thread: Id,
    /// The local comment posted as this one, if it was posted from a repository.
    pub origin: Option<Id>,
    /// How many times its author has edited it.
    pub edits: u32,
    /// Whether its author has deleted it. It is kept, as a copy fetched before may still have
    /// it.
    pub deleted: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct GitHubReview {
    pub author: Person,
    pub state: ReviewState,
    /// GitHub comment ids, in order.
    pub comments: Vec<Id>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Thread {
    pub comments: Vec<CommentRef>,
    /// Whether the thread's line is still in the diff.
    pub placeable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Draft {
    pub id: Id,
    /// Local comment ids, in order.
    pub comments: Vec<Id>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct LocalComment {
    pub thread: Id,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct RepoState {
    /// The repository's draft review, which has every local comment.
    pub draft: Option<Draft>,
    pub comments: BTreeMap<Id, LocalComment>,
    /// The local copy of GitHub's reviews, as of the last fetch; `None` before the first.
    pub copy: Option<BTreeMap<Id, GitHubReview>>,
    /// How many times each comment in the copy had been edited when it was fetched.
    pub copied_edits: BTreeMap<Id, u32>,
}

/// Rules the model found were needed. Each can be turned off, to show which invariant fails
/// without it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Rules {
    /// A thread's first comment can't be deleted while the thread has replies.
    pub keep_first_comment: bool,
}

impl Default for Rules {
    fn default() -> Self {
        Rules {
            keep_first_comment: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Action {
    /// A new thread, or a reply in `thread`, into the repository's draft review.
    Write { repo: Repo, thread: Option<Id> },
    /// Delete a local draft comment.
    Delete { repo: Repo, comment: Id },
    /// `O s`: sync. Post the repository's draft review into its author's pending review, which
    /// is started if they have none, stopping after `stop_after` comments if given, as when the
    /// network fails part way; once all of it is posted, fetch. With no draft comments, it only
    /// fetches.
    Sync {
        repo: Repo,
        stop_after: Option<usize>,
    },
    /// `O O`: copy GitHub's reviews, as the repository's user sees them, into the local PR.
    Fetch { repo: Repo },
    /// On GitHub: submit my pending review.
    Submit { user: Person },
    /// On GitHub: start a thread in the browser, in my pending review.
    BrowserComment { user: Person },
    /// On GitHub: dismiss a submitted review.
    Dismiss { review: Id },
    /// A commit removes a thread's line from the diff.
    ChangeCode { thread: Id },
    /// Edit my own comment in a review on GitHub, from a repository whose copy has it. The edit
    /// is made on GitHub, and the repository fetches again.
    EditOnGitHub { repo: Repo, comment: Id },
    /// Delete my own reply in a review on GitHub, as [`Action::EditOnGitHub`] edits it. A
    /// thread's first comment can't be deleted this way: anyone may have a draft reply to it in
    /// their repository, which no one else can see, and which would be left replying to
    /// nothing.
    DeleteOnGitHub { repo: Repo, comment: Id },
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Action::Write { repo, thread: None } => write!(f, "write a thread in {repo:?}"),
            Action::Write {
                repo,
                thread: Some(t),
            } => write!(f, "reply in thread {t} in {repo:?}"),
            Action::Delete { repo, comment } => write!(f, "delete comment {comment} in {repo:?}"),
            Action::Sync {
                repo,
                stop_after: None,
            } => write!(f, "sync from {repo:?}"),
            Action::Sync {
                repo,
                stop_after: Some(n),
            } => write!(f, "sync from {repo:?}, stopping after {n}"),
            Action::Fetch { repo } => write!(f, "fetch in {repo:?}"),
            Action::Submit { user } => write!(f, "{user:?} submits on GitHub"),
            Action::BrowserComment { user } => write!(f, "{user:?} comments in the browser"),
            Action::Dismiss { review } => write!(f, "dismiss review {review}"),
            Action::ChangeCode { thread } => write!(f, "change the code under thread {thread}"),
            Action::EditOnGitHub { repo, comment } => {
                write!(f, "edit comment {comment} on GitHub from {repo:?}")
            }
            Action::DeleteOnGitHub { repo, comment } => {
                write!(f, "delete comment {comment} on GitHub from {repo:?}")
            }
        }
    }
}

/// The state of every repository and of GitHub, with what the invariants need to remember.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Model {
    pub gh_comments: BTreeMap<Id, GitHubComment>,
    pub gh_reviews: BTreeMap<Id, GitHubReview>,
    pub threads: BTreeMap<Id, Thread>,
    pub repos: BTreeMap<Repo, RepoState>,
    /// Every local comment written, and those deleted on purpose.
    pub written: Vec<Id>,
    pub deleted: Vec<Id>,
    /// The draft review each local comment was last in.
    pub draft_of: BTreeMap<Id, Id>,
    /// Draft reviews whose pending review was submitted on GitHub before posting finished.
    pub submitted_part_way: Vec<Id>,
    /// Threads whose first comment was deleted while they had replies.
    pub orphaned: Vec<Id>,
    /// Replies posted before their thread was on GitHub, which GitHub refuses.
    pub replies_before_thread: Vec<Id>,
    next: Id,
}

impl Default for Model {
    fn default() -> Self {
        Model {
            gh_comments: BTreeMap::new(),
            gh_reviews: BTreeMap::new(),
            threads: BTreeMap::new(),
            repos: Repo::ALL
                .into_iter()
                .map(|repo| (repo, RepoState::default()))
                .collect(),
            written: vec![],
            deleted: vec![],
            draft_of: BTreeMap::new(),
            submitted_part_way: vec![],
            orphaned: vec![],
            replies_before_thread: vec![],
            next: 1,
        }
    }
}

impl Model {
    fn new_id(&mut self) -> Id {
        self.next += 1;
        self.next - 1
    }

    pub fn pending_of(&self, user: Person) -> Option<Id> {
        self.gh_reviews
            .iter()
            .find(|(_, r)| r.author == user && r.state == ReviewState::Pending)
            .map(|(id, _)| *id)
    }

    /// GitHub's reviews as `user` sees them: everyone's submitted and dismissed reviews, and
    /// their own pending one.
    pub fn visible(&self, user: Person) -> BTreeMap<Id, GitHubReview> {
        self.gh_reviews
            .iter()
            .filter(|(_, r)| r.state != ReviewState::Pending || r.author == user)
            .map(|(id, r)| (*id, r.clone()))
            .collect()
    }

    /// The threads a repository's author can reply in: those with a local comment there, and
    /// those in its copy of GitHub's reviews.
    pub fn threads_seen_in(&self, repo: Repo) -> BTreeSet<Id> {
        let r = &self.repos[&repo];
        let local = r.comments.values().map(|c| c.thread);
        let copied = r
            .copy
            .iter()
            .flat_map(|copy| copy.values())
            .flat_map(|review| &review.comments)
            .map(|gid| self.gh_comments[gid].thread);
        local.chain(copied).collect()
    }

    /// Copy GitHub's reviews, as the repository's user sees them, into the repository.
    fn refresh_copy(&mut self, repo: Repo) {
        let copy = self.visible(repo.github_user());
        let edits = copy
            .values()
            .flat_map(|r| &r.comments)
            .map(|g| (*g, self.gh_comments[g].edits))
            .collect();
        let r = self.repos.get_mut(&repo).unwrap();
        r.copy = Some(copy);
        r.copied_edits = edits;
    }

    /// The GitHub comments in a repository's copy of GitHub's reviews.
    pub fn copied_in(&self, repo: Repo) -> BTreeSet<Id> {
        self.repos[&repo]
            .copy
            .iter()
            .flat_map(|copy| copy.values())
            .flat_map(|review| review.comments.iter().copied())
            .collect()
    }

    /// The model after `action`, or `None` if the rules refuse it.
    pub fn apply(&self, action: &Action, rules: Rules) -> Option<Model> {
        let mut m = self.clone();
        match *action {
            Action::Write { repo, thread } => {
                if let Some(t) = thread
                    && !self.threads_seen_in(repo).contains(&t)
                {
                    return None;
                }
                let thread = match thread {
                    Some(t) => t,
                    None => {
                        let t = m.new_id();
                        m.threads.insert(
                            t,
                            Thread {
                                comments: vec![],
                                placeable: true,
                            },
                        );
                        t
                    }
                };
                let cid = m.new_id();
                let new_draft = m.new_id();
                let r = m.repos.get_mut(&repo).unwrap();
                r.comments.insert(cid, LocalComment { thread });
                let draft = r.draft.get_or_insert(Draft {
                    id: new_draft,
                    comments: vec![],
                });
                draft.comments.push(cid);
                let draft_id = draft.id;
                m.draft_of.insert(cid, draft_id);
                m.threads
                    .get_mut(&thread)
                    .unwrap()
                    .comments
                    .push(CommentRef::Local(cid));
                m.written.push(cid);
            }
            Action::Delete { repo, comment } => {
                let local = self.repos[&repo].comments.get(&comment)?;
                let thread = &self.threads[&local.thread].comments;
                let first = thread[0] == CommentRef::Local(comment);
                if rules.keep_first_comment && first && thread.len() > 1 {
                    return None;
                }
                let r = m.repos.get_mut(&repo).unwrap();
                let local = r.comments.remove(&comment).unwrap();
                let draft = r.draft.as_mut().unwrap();
                draft.comments.retain(|c| *c != comment);
                if draft.comments.is_empty() {
                    r.draft = None;
                }
                let thread = &mut m.threads.get_mut(&local.thread).unwrap().comments;
                if first && thread.len() > 1 {
                    m.orphaned.push(local.thread);
                }
                thread.retain(|c| *c != CommentRef::Local(comment));
                m.deleted.push(comment);
            }
            Action::Sync { repo, stop_after } => {
                let user = repo.github_user();
                let r = &self.repos[&repo];
                let Some(draft) = r.draft.as_ref().filter(|d| !d.comments.is_empty()) else {
                    m.refresh_copy(repo);
                    return Some(m);
                };
                if draft
                    .comments
                    .iter()
                    .any(|c| !self.threads[&r.comments[c].thread].placeable)
                {
                    return None;
                }
                let pending = match self.pending_of(user) {
                    Some(p) => p,
                    None => {
                        let p = m.new_id();
                        m.gh_reviews.insert(
                            p,
                            GitHubReview {
                                author: user,
                                state: ReviewState::Pending,
                                comments: vec![],
                            },
                        );
                        p
                    }
                };
                let order = draft.comments.clone();
                for (posted, cid) in order.into_iter().enumerate() {
                    if stop_after == Some(posted) {
                        break;
                    }
                    let gid = m.new_id();
                    let r = m.repos.get_mut(&repo).unwrap();
                    let local = r.comments.remove(&cid).unwrap();
                    let draft = r.draft.as_mut().unwrap();
                    draft.comments.retain(|c| *c != cid);
                    let thread = &mut m.threads.get_mut(&local.thread).unwrap().comments;
                    let first = thread[0];
                    if first != CommentRef::Local(cid) && !matches!(first, CommentRef::GitHub(_)) {
                        m.replies_before_thread.push(cid);
                    }
                    let i = thread
                        .iter()
                        .position(|c| *c == CommentRef::Local(cid))
                        .unwrap();
                    thread[i] = CommentRef::GitHub(gid);
                    m.gh_comments.insert(
                        gid,
                        GitHubComment {
                            author: user,
                            thread: local.thread,
                            origin: Some(cid),
                            edits: 0,
                            deleted: false,
                        },
                    );
                    m.gh_reviews.get_mut(&pending).unwrap().comments.push(gid);
                }
                let r = m.repos.get_mut(&repo).unwrap();
                if r.draft.as_ref().unwrap().comments.is_empty() {
                    r.draft = None;
                    m.refresh_copy(repo);
                }
            }
            Action::Fetch { repo } => m.refresh_copy(repo),
            Action::Submit { user } => {
                let p = self.pending_of(user)?;
                m.gh_reviews.get_mut(&p).unwrap().state = ReviewState::Submitted;
                // A draft review of which some is in the pending review, and some is not yet
                // posted, is submitted part way.
                let posted_from = |draft: &Draft| {
                    self.gh_reviews[&p].comments.iter().any(|g| {
                        self.gh_comments[g]
                            .origin
                            .is_some_and(|origin| self.draft_of.get(&origin) == Some(&draft.id))
                    })
                };
                for repo in Repo::ALL {
                    if repo.github_user() == user
                        && let Some(d) = &self.repos[&repo].draft
                        && !d.comments.is_empty()
                        && posted_from(d)
                    {
                        m.submitted_part_way.push(d.id);
                    }
                }
            }
            Action::BrowserComment { user } => {
                let p = match self.pending_of(user) {
                    Some(p) => p,
                    None => {
                        let p = m.new_id();
                        m.gh_reviews.insert(
                            p,
                            GitHubReview {
                                author: user,
                                state: ReviewState::Pending,
                                comments: vec![],
                            },
                        );
                        p
                    }
                };
                let thread = m.new_id();
                let gid = m.new_id();
                m.threads.insert(
                    thread,
                    Thread {
                        comments: vec![CommentRef::GitHub(gid)],
                        placeable: true,
                    },
                );
                m.gh_comments.insert(
                    gid,
                    GitHubComment {
                        author: user,
                        thread,
                        origin: None,
                        edits: 0,
                        deleted: false,
                    },
                );
                m.gh_reviews.get_mut(&p).unwrap().comments.push(gid);
            }
            Action::Dismiss { review } => {
                if self.gh_reviews.get(&review)?.state != ReviewState::Submitted {
                    return None;
                }
                m.gh_reviews.get_mut(&review).unwrap().state = ReviewState::Dismissed;
            }
            Action::ChangeCode { thread } => {
                if !self.threads.get(&thread)?.placeable {
                    return None;
                }
                m.threads.get_mut(&thread).unwrap().placeable = false;
            }
            Action::EditOnGitHub { repo, comment } | Action::DeleteOnGitHub { repo, comment } => {
                let user = repo.github_user();
                if !self.copied_in(repo).contains(&comment) {
                    return None;
                }
                let c = self.gh_comments.get(&comment)?;
                if c.author != user || c.deleted {
                    return None;
                }
                if let Action::EditOnGitHub { .. } = action {
                    m.gh_comments.get_mut(&comment).unwrap().edits += 1;
                } else {
                    if self.threads[&c.thread].comments[0] == CommentRef::GitHub(comment) {
                        return None;
                    }
                    m.gh_comments.get_mut(&comment).unwrap().deleted = true;
                    for review in m.gh_reviews.values_mut() {
                        review.comments.retain(|g| *g != comment);
                    }
                    m.threads
                        .get_mut(&c.thread)
                        .unwrap()
                        .comments
                        .retain(|r| *r != CommentRef::GitHub(comment));
                    m.deleted.extend(c.origin);
                }
                m.refresh_copy(repo);
            }
        }
        Some(m)
    }

    /// The actions to try from this state: some are refused by the rules.
    pub fn actions(&self, max_writes: usize) -> Vec<Action> {
        let mut out = vec![];
        for repo in Repo::ALL {
            if self.written.len() < max_writes {
                out.push(Action::Write { repo, thread: None });
                for thread in self.threads_seen_in(repo) {
                    out.push(Action::Write {
                        repo,
                        thread: Some(thread),
                    });
                }
            }
            for &comment in self.repos[&repo].comments.keys() {
                out.push(Action::Delete { repo, comment });
            }
            out.push(Action::Sync {
                repo,
                stop_after: None,
            });
            out.push(Action::Sync {
                repo,
                stop_after: Some(1),
            });
            out.push(Action::Fetch { repo });
            for comment in self.copied_in(repo) {
                out.push(Action::EditOnGitHub { repo, comment });
                out.push(Action::DeleteOnGitHub { repo, comment });
            }
        }
        for user in [Person::Me, Person::Colleague] {
            out.push(Action::Submit { user });
        }
        if !self.gh_comments.values().any(|c| c.origin.is_none()) {
            out.push(Action::BrowserComment { user: Person::Me });
        }
        for &review in self.gh_reviews.keys() {
            out.push(Action::Dismiss { review });
        }
        for &thread in self.threads.keys() {
            out.push(Action::ChangeCode { thread });
        }
        out
    }

    /// The invariants every state keeps, as the ones violated.
    pub fn invariants(&self) -> Vec<String> {
        let mut errors = vec![];
        for (repo, r) in &self.repos {
            let mut in_draft: Vec<Id> = r.draft.iter().flat_map(|d| d.comments.clone()).collect();
            in_draft.sort();
            if in_draft != r.comments.keys().copied().collect::<Vec<_>>() {
                errors.push(format!(
                    "In {repo:?}, a comment is not in exactly one review"
                ));
            }
            for (id, review) in r.copy.iter().flatten() {
                if review.state == ReviewState::Pending && review.author != repo.github_user() {
                    errors.push(format!(
                        "{repo:?}'s copy has another user's pending review {id}"
                    ));
                }
            }
        }
        let mut in_reviews: Vec<Id> = self
            .gh_reviews
            .values()
            .flat_map(|r| r.comments.clone())
            .collect();
        in_reviews.sort();
        let existing: Vec<Id> = self
            .gh_comments
            .iter()
            .filter(|(_, c)| !c.deleted)
            .map(|(id, _)| *id)
            .collect();
        if in_reviews != existing {
            errors.push("On GitHub, a comment is not in exactly one review".to_owned());
        }
        for review in self.gh_reviews.values() {
            if review
                .comments
                .iter()
                .any(|c| self.gh_comments[c].author != review.author)
            {
                errors.push("On GitHub, a comment's author is not its review's author".to_owned());
            }
        }
        for user in [Person::Me, Person::Colleague] {
            let pending = self
                .gh_reviews
                .values()
                .filter(|r| r.author == user && r.state == ReviewState::Pending)
                .count();
            if pending > 1 {
                errors.push(format!("{user:?} has more than one pending review"));
            }
        }
        let origins: Vec<Id> = self
            .gh_comments
            .values()
            .filter(|c| !c.deleted)
            .filter_map(|c| c.origin)
            .collect();
        if origins.len() != origins.iter().collect::<BTreeSet<_>>().len() {
            errors.push("A local comment was posted twice".to_owned());
        }
        for r in self.repos.values() {
            if r.comments.keys().any(|c| origins.contains(c)) {
                errors.push("A posted comment is still a local draft".to_owned());
            }
        }
        let kept: BTreeSet<Id> = self
            .repos
            .values()
            .flat_map(|r| r.comments.keys().copied())
            .chain(origins.iter().copied())
            .chain(self.deleted.iter().copied())
            .collect();
        if self.written.iter().any(|c| !kept.contains(c)) {
            errors.push("A comment was lost".to_owned());
        }
        for c in &self.replies_before_thread {
            errors.push(format!(
                "Reply {c} was posted before its thread was on GitHub"
            ));
        }
        for t in &self.orphaned {
            errors.push(format!(
                "Thread {t} lost its first comment while it had replies"
            ));
        }
        let mut by_draft: BTreeMap<Id, BTreeSet<Id>> = BTreeMap::new();
        for (rid, review) in &self.gh_reviews {
            for gid in &review.comments {
                if let Some(origin) = self.gh_comments[gid].origin {
                    by_draft
                        .entry(self.draft_of[&origin])
                        .or_default()
                        .insert(*rid);
                }
            }
        }
        for (draft, rids) in by_draft {
            if rids.len() > 1 && !self.submitted_part_way.contains(&draft) {
                errors.push(format!(
                    "Draft review {draft} was split across GitHub reviews {rids:?}"
                ));
            }
        }
        errors
    }

    /// The invariants of a step from `self` to `after` by `action`, as the ones violated.
    pub fn step_invariants(&self, action: &Action, after: &Model) -> Vec<String> {
        let mut errors = vec![];
        for (id, review) in &self.gh_reviews {
            if let Some(later) = after.gh_reviews.get(id)
                && later.state < review.state
            {
                errors.push(format!("Review {id} went back from {:?}", review.state));
            }
        }
        if let Action::Fetch { repo } = action {
            if self.repos[repo].draft != after.repos[repo].draft {
                errors.push(format!("Fetching changed a draft review in {repo:?}"));
            }
            if after.repos[repo].copy.as_ref() != Some(&after.visible(repo.github_user())) {
                errors.push(format!(
                    "After fetching, {repo:?}'s copy is not GitHub's reviews as it sees them"
                ));
            }
        }
        errors
    }

    /// Whether I can post my laptop's draft review, without submitting part of it: delete the
    /// comments whose lines have gone, and sync.
    pub fn drain(&self, rules: Rules) -> Vec<String> {
        let mut m = self.clone();
        for _ in 0..8 {
            let r = &m.repos[&Repo::Laptop];
            if r.draft.is_none() {
                return vec![];
            }
            // Comments whose lines have gone are deleted: each thread's last local comment first,
            // since a first comment can't be deleted while the thread has replies.
            let doomed = r
                .comments
                .values()
                .find(|c| !m.threads[&c.thread].placeable);
            if let Some(doomed) = doomed {
                let last = m.threads[&doomed.thread]
                    .comments
                    .iter()
                    .rev()
                    .find_map(|c| match c {
                        CommentRef::Local(id) if r.comments.contains_key(id) => Some(*id),
                        _ => None,
                    })
                    .expect("the thread has a local comment");
                let delete = Action::Delete {
                    repo: Repo::Laptop,
                    comment: last,
                };
                match m.apply(&delete, rules) {
                    Some(next) => m = next,
                    None => {
                        return vec![
                            "I can't delete the comments whose lines have gone, so I can't post"
                                .to_owned(),
                        ];
                    }
                }
                continue;
            }
            let sync = Action::Sync {
                repo: Repo::Laptop,
                stop_after: None,
            };
            let Some(next) = m.apply(&sync, rules) else {
                return vec!["I can't sync my draft review".to_owned()];
            };
            let errors = next.invariants();
            if !errors.is_empty() {
                return errors
                    .into_iter()
                    .map(|e| format!("Posting everything: {e}"))
                    .collect();
            }
            m = next;
        }
        vec!["Posting everything does not finish".to_owned()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashSet, VecDeque};

    /// The first sequence of actions, of at most `depth`, that ends in a state that violates an
    /// invariant, with what it violates.
    fn violation(rules: Rules, depth: usize, max_writes: usize) -> Option<(Vec<Action>, String)> {
        let start = Model::default();
        let mut seen = HashSet::from([start.clone()]);
        let mut queue = VecDeque::from([(start, vec![])]);
        while let Some((m, path)) = queue.pop_front() {
            for action in m.actions(max_writes) {
                let Some(next) = m.apply(&action, rules) else {
                    continue;
                };
                if !seen.insert(next.clone()) {
                    continue;
                }
                let mut path: Vec<Action> = path.clone();
                path.push(action.clone());
                let mut errors = m.step_invariants(&action, &next);
                errors.extend(next.invariants());
                if errors.is_empty() {
                    errors = next.drain(rules);
                }
                if let Some(error) = errors.into_iter().next() {
                    return Some((path, error));
                }
                if path.len() < depth {
                    queue.push_back((next, path));
                }
            }
        }
        None
    }

    fn describe((path, error): (Vec<Action>, String)) -> String {
        let steps: Vec<String> = path.iter().map(|a| format!("  - {a}")).collect();
        format!("{error}\n{}", steps.join("\n"))
    }

    #[test]
    fn every_sequence_of_actions_keeps_the_invariants() {
        if let Some(found) = violation(Rules::default(), 5, 3) {
            panic!("{}", describe(found));
        }
    }

    /// Slower: run with `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn every_longer_sequence_of_actions_keeps_the_invariants() {
        if let Some(found) = violation(Rules::default(), 7, 3) {
            panic!("{}", describe(found));
        }
    }

    #[test]
    fn without_keeping_first_comments_a_thread_loses_its_first_comment() {
        let rules = Rules {
            keep_first_comment: false,
            ..Rules::default()
        };
        let (_, error) = violation(rules, 5, 3).expect("a violation");
        assert!(error.contains("lost its first comment"), "{error}");
    }
}
