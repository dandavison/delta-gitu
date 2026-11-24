//! The lifecycle of reviews on a PR, local and on GitHub, as a state machine: what each action in
//! my store of reviews, my store on a second machine, and a colleague's store does, and the
//! invariants that every sequence of actions keeps. A store is a clone's local reviews, shared by
//! its worktrees. Each has one owner, its git `user.name`: an agent writing in my store writes as
//! me, with its name only as a label on its comments, which changes nothing here. It is the definition the product follows (see the lifecycle
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

/// A store of local reviews: a clone's, shared by its worktrees.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Store {
    /// Mine, where my agent writes too, as me.
    Laptop,
    /// Mine, on a second machine.
    Desktop,
    Colleague,
}

impl Store {
    pub const ALL: [Store; 3] = [Store::Laptop, Store::Desktop, Store::Colleague];

    /// The store's owner, whose GitHub login posts from it.
    pub fn owner(self) -> Person {
        match self {
            Store::Laptop | Store::Desktop => Person::Me,
            Store::Colleague => Person::Colleague,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum ReviewState {
    Pending,
    Submitted,
    Dismissed,
}

/// What a user sees on GitHub: the reviews visible to them, and for each of their comments, how
/// many times it has been edited and whether it is deleted.
pub(crate) type SeenOnGitHub = (BTreeMap<Id, GitHubReview>, Vec<(Id, u32, bool)>);

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
    /// The local comment posted as this one, if it was posted from a store.
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
    /// Local comment ids, in the order they were written or moved in.
    pub comments: Vec<Id>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct LocalComment {
    pub thread: Id,
    /// For a comment that starts its thread, the head of the PR its line was found in when it was
    /// written, or since moved to; a reply has no line of its own, nor a thread moved to its file
    /// once its line had gone.
    pub written_at: Option<u32>,
    /// Whether it is local-only: in no review, never posted, such as a conversation with an
    /// agent. Otherwise it is in the store's draft review.
    pub local_only: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct StoreState {
    /// The store's draft review, which has every local comment that is not local-only.
    pub draft: Option<Draft>,
    pub comments: BTreeMap<Id, LocalComment>,
    /// The local copy of GitHub's reviews, as of the last fetch; `None` before the first.
    pub copy: Option<BTreeMap<Id, GitHubReview>>,
    /// How many times each comment in the copy had been edited when it was fetched.
    pub copied_edits: BTreeMap<Id, u32>,
    /// Draft comments a sync was posting when it stopped. GitHub may have added one though its
    /// reply was lost, so it can't be deleted or moved to local-only until a sync posts it, which
    /// finds it on GitHub if it is there. The model's syncs stop before GitHub adds anything, so
    /// here this rule only follows gitu, which can't tell the two apart.
    pub interrupted: BTreeSet<Id>,
}

/// Rules the model found were needed. Each can be turned off, to show which invariant fails
/// without it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Rules {
    /// A thread's first comment can't be deleted while the thread has replies.
    pub keep_first_comment: bool,
    /// Sync posts nothing while a draft thread was written against another head of the PR than
    /// its head: it moves them to its head instead.
    pub lease: bool,
    /// Sync posts nothing while a draft reply's thread starts with a local-only comment.
    pub block_replies_to_local_threads: bool,
    /// Whether an edit or delete of my comment on GitHub publishes is decided from GitHub as it
    /// is now, rather than from the store's copy, which may be from before the review was
    /// submitted.
    pub classify_on_github: bool,
    /// An edit or delete of my comment on GitHub from a copy older than GitHub's comment is
    /// refused, rather than overwriting what was changed there since.
    pub refuse_stale_copies: bool,
}

impl Default for Rules {
    fn default() -> Self {
        Rules {
            keep_first_comment: true,
            lease: true,
            block_replies_to_local_threads: true,
            classify_on_github: true,
            refuse_stale_copies: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Action {
    /// A new thread, or a reply in `thread`: into the store's draft review, or local-only
    /// if `local`.
    Write {
        store: Store,
        thread: Option<Id>,
        local: bool,
    },
    /// Move a local comment into the draft review, or out of it to local-only if `local`.
    Move {
        store: Store,
        comment: Id,
        local: bool,
    },
    /// Delete a local comment, draft or local-only.
    Delete { store: Store, comment: Id },
    /// `O s`: sync. Fetch, then post the store's draft review into its author's pending
    /// review, which is started if they have none, stopping after `stop_after` comments if given,
    /// as when the network fails part way; once all of it is posted, fetch again. A thread whose
    /// line has gone is posted as a comment on its file. With a draft thread written against
    /// another head of the PR than its head now, nothing is posted: the drafts are moved to the
    /// PR's head, for the user to see, and sync again.
    Sync {
        store: Store,
        stop_after: Option<usize>,
    },
    /// `O O`: copy GitHub's reviews, as the store's owner sees them, into the local PR.
    Fetch { store: Store },
    /// On GitHub: submit my pending review.
    Submit { user: Person },
    /// On GitHub: start a thread in the browser, in my pending review.
    BrowserComment { user: Person },
    /// On GitHub: dismiss a submitted review.
    Dismiss { review: Id },
    /// A commit, pushed to the PR on GitHub, removes a thread's line from the diff, and moves the
    /// PR's head.
    ChangeCode { thread: Id },
    /// Edit my own comment in a review on GitHub, from a store whose copy has it, saying to
    /// publish (`y` in the TUI, `--publish` on the command line) if `publish`. The edit is made on
    /// GitHub, and the store fetches again.
    EditOnGitHub {
        store: Store,
        comment: Id,
        publish: bool,
    },
    /// Delete my own reply in a review on GitHub, as [`Action::EditOnGitHub`] edits it. A
    /// thread's first comment can't be deleted this way: anyone may have a draft reply to it in
    /// their store, which no one else can see, and which would be left replying to
    /// nothing.
    DeleteOnGitHub {
        store: Store,
        comment: Id,
        publish: bool,
    },
}

/// How [`Action`]'s display says the user said to publish.
fn publish_words(publish: bool) -> &'static str {
    if publish { ", saying to publish" } else { "" }
}

/// How [`Action`]'s display says a comment is local-only.
fn local_word(local: bool) -> &'static str {
    if local { "local-only " } else { "" }
}

impl Action {
    /// The store the action is taken from, for one taken in a store.
    pub fn store(&self) -> Option<Store> {
        match *self {
            Action::Write { store, .. }
            | Action::Move { store, .. }
            | Action::Delete { store, .. }
            | Action::Sync { store, .. }
            | Action::Fetch { store }
            | Action::EditOnGitHub { store, .. }
            | Action::DeleteOnGitHub { store, .. } => Some(store),
            Action::Submit { .. }
            | Action::BrowserComment { .. }
            | Action::Dismiss { .. }
            | Action::ChangeCode { .. } => None,
        }
    }
}

impl Action {
    /// Whether the user said to publish: `y` in the TUI, or `--publish` on the command line.
    pub fn says_to_publish(&self) -> bool {
        matches!(
            self,
            Action::EditOnGitHub { publish: true, .. }
                | Action::DeleteOnGitHub { publish: true, .. }
        )
    }
}

impl StoreState {
    /// Take `comment` out of the draft review, which goes once it has none.
    fn take_from_draft(&mut self, comment: Id) {
        if let Some(draft) = &mut self.draft {
            draft.comments.retain(|c| *c != comment);
            if draft.comments.is_empty() {
                self.draft = None;
            }
        }
    }
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Action::Write {
                store,
                thread: None,
                local,
            } => write!(f, "write a {}thread in {store:?}", local_word(*local)),
            Action::Write {
                store,
                thread: Some(t),
                local,
            } => write!(f, "{}reply in thread {t} in {store:?}", local_word(*local)),
            Action::Move {
                store,
                comment,
                local: true,
            } => write!(f, "move comment {comment} to local-only in {store:?}"),
            Action::Move {
                store,
                comment,
                local: false,
            } => write!(
                f,
                "move comment {comment} into the draft review in {store:?}"
            ),
            Action::Delete { store, comment } => write!(f, "delete comment {comment} in {store:?}"),
            Action::Sync {
                store,
                stop_after: None,
            } => write!(f, "sync from {store:?}"),
            Action::Sync {
                store,
                stop_after: Some(n),
            } => write!(f, "sync from {store:?}, stopping after {n}"),
            Action::Fetch { store } => write!(f, "fetch in {store:?}"),
            Action::Submit { user } => write!(f, "{user:?} submits on GitHub"),
            Action::BrowserComment { user } => write!(f, "{user:?} comments in the browser"),
            Action::Dismiss { review } => write!(f, "dismiss review {review}"),
            Action::ChangeCode { thread } => write!(f, "change the code under thread {thread}"),
            Action::EditOnGitHub {
                store,
                comment,
                publish,
            } => write!(
                f,
                "edit comment {comment} on GitHub from {store:?}{}",
                publish_words(*publish)
            ),
            Action::DeleteOnGitHub {
                store,
                comment,
                publish,
            } => write!(
                f,
                "delete comment {comment} on GitHub from {store:?}{}",
                publish_words(*publish)
            ),
        }
    }
}

/// The state of every store and of GitHub, with what the invariants need to remember.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Model {
    pub gh_comments: BTreeMap<Id, GitHubComment>,
    pub gh_reviews: BTreeMap<Id, GitHubReview>,
    pub threads: BTreeMap<Id, Thread>,
    pub stores: BTreeMap<Store, StoreState>,
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
    /// The PR's head on GitHub: how many times it has moved, as when its author pushes.
    pub head: u32,
    /// Local-only comments posted by a sync, which must never be.
    pub local_posted: Vec<Id>,
    /// Comments posted whose line was found in another head of the PR than its head when they
    /// were posted.
    pub posted_off_head: Vec<Id>,
    /// My comments on GitHub edited or deleted from a copy older than GitHub's, overwriting what
    /// was changed there since.
    pub overwritten: Vec<Id>,
    next: Id,
}

impl Default for Model {
    fn default() -> Self {
        Model {
            gh_comments: BTreeMap::new(),
            gh_reviews: BTreeMap::new(),
            threads: BTreeMap::new(),
            stores: Store::ALL
                .into_iter()
                .map(|store| (store, StoreState::default()))
                .collect(),
            written: vec![],
            deleted: vec![],
            draft_of: BTreeMap::new(),
            submitted_part_way: vec![],
            orphaned: vec![],
            replies_before_thread: vec![],
            head: 0,
            local_posted: vec![],
            posted_off_head: vec![],
            overwritten: vec![],
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

    /// The threads a store's owner can reply in: those with a local comment there, and
    /// those in its copy of GitHub's reviews.
    pub fn threads_seen_in(&self, store: Store) -> BTreeSet<Id> {
        let r = &self.stores[&store];
        let local = r.comments.values().map(|c| c.thread);
        let copied = r
            .copy
            .iter()
            .flat_map(|copy| copy.values())
            .flat_map(|review| &review.comments)
            .map(|gid| self.gh_comments[gid].thread);
        local.chain(copied).collect()
    }

    /// Copy GitHub's reviews, as the store's owner sees them, into the store.
    fn refresh_copy(&mut self, store: Store) {
        let copy = self.visible(store.owner());
        let edits = copy
            .values()
            .flat_map(|r| &r.comments)
            .map(|g| (*g, self.gh_comments[g].edits))
            .collect();
        let r = self.stores.get_mut(&store).unwrap();
        r.copy = Some(copy);
        r.copied_edits = edits;
    }

    /// The GitHub comments in a store's copy of GitHub's reviews.
    pub fn copied_in(&self, store: Store) -> BTreeSet<Id> {
        self.stores[&store]
            .copy
            .iter()
            .flat_map(|copy| copy.values())
            .flat_map(|review| review.comments.iter().copied())
            .collect()
    }

    /// The draft replies in `store` whose thread's first comment is local-only there.
    pub fn blocked_replies(&self, store: Store) -> Vec<Id> {
        let r = &self.stores[&store];
        r.draft
            .iter()
            .flat_map(|d| &d.comments)
            .copied()
            .filter(|c| match self.threads[&r.comments[c].thread].comments[0] {
                CommentRef::Local(first) => {
                    first != *c && r.comments.get(&first).is_some_and(|f| f.local_only)
                }
                CommentRef::GitHub(_) => false,
            })
            .collect()
    }

    /// Whether others see the comment on GitHub `comment`: whether it is in a review that is not
    /// pending, among `reviews`.
    fn public_in(reviews: &BTreeMap<Id, GitHubReview>, comment: Id) -> bool {
        reviews
            .values()
            .any(|r| r.comments.contains(&comment) && r.state != ReviewState::Pending)
    }

    /// What `user` sees on GitHub: the reviews visible to them, and each of their comments' edits
    /// and whether it is deleted.
    pub fn seen_by(&self, user: Person) -> SeenOnGitHub {
        let reviews = self.visible(user);
        let comments = reviews
            .values()
            .flat_map(|r| &r.comments)
            .map(|g| (*g, self.gh_comments[g].edits, self.gh_comments[g].deleted))
            .collect();
        (reviews, comments)
    }

    /// The model after `action`, or `None` if the rules refuse it.
    pub fn apply(&self, action: &Action, rules: Rules) -> Option<Model> {
        let mut m = self.clone();
        match *action {
            Action::Write {
                store,
                thread,
                local,
            } => {
                if let Some(t) = thread
                    && !self.threads_seen_in(store).contains(&t)
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
                let r = m.stores.get_mut(&store).unwrap();
                let written_at = (!self.threads.contains_key(&thread)).then_some(self.head);
                r.comments.insert(
                    cid,
                    LocalComment {
                        thread,
                        written_at,
                        local_only: local,
                    },
                );
                if !local {
                    let draft = r.draft.get_or_insert(Draft {
                        id: new_draft,
                        comments: vec![],
                    });
                    draft.comments.push(cid);
                    let draft_id = draft.id;
                    m.draft_of.insert(cid, draft_id);
                }
                m.threads
                    .get_mut(&thread)
                    .unwrap()
                    .comments
                    .push(CommentRef::Local(cid));
                m.written.push(cid);
            }
            Action::Delete { store, comment } => {
                let local = self.stores[&store].comments.get(&comment)?;
                if self.stores[&store].interrupted.contains(&comment) {
                    return None;
                }
                let thread = &self.threads[&local.thread].comments;
                let first = thread[0] == CommentRef::Local(comment);
                if rules.keep_first_comment && first && thread.len() > 1 {
                    return None;
                }
                let r = m.stores.get_mut(&store).unwrap();
                let local = r.comments.remove(&comment).unwrap();
                r.take_from_draft(comment);
                let thread = &mut m.threads.get_mut(&local.thread).unwrap().comments;
                if first && thread.len() > 1 {
                    m.orphaned.push(local.thread);
                }
                thread.retain(|c| *c != CommentRef::Local(comment));
                m.deleted.push(comment);
            }
            Action::Move {
                store,
                comment,
                local,
            } => {
                if self.stores[&store].comments.get(&comment)?.local_only == local
                    || self.stores[&store].interrupted.contains(&comment)
                {
                    return None;
                }
                let new_draft = m.new_id();
                let r = m.stores.get_mut(&store).unwrap();
                r.comments.get_mut(&comment).unwrap().local_only = local;
                if local {
                    r.take_from_draft(comment);
                } else {
                    let draft = r.draft.get_or_insert(Draft {
                        id: new_draft,
                        comments: vec![],
                    });
                    draft.comments.push(comment);
                    let draft_id = draft.id;
                    m.draft_of.insert(comment, draft_id);
                }
            }
            Action::Sync { store, stop_after } => {
                let user = store.owner();
                // What is on GitHub comes in first.
                m.refresh_copy(store);
                let r = &self.stores[&store];
                let Some(draft) = r.draft.as_ref().filter(|d| !d.comments.is_empty()) else {
                    return Some(m);
                };
                // A draft reply in a thread whose first comment is local-only can't reach GitHub
                // before its thread: it blocks the sync, which has fetched, and posts nothing.
                if rules.block_replies_to_local_threads && !self.blocked_replies(store).is_empty() {
                    return Some(m);
                }
                // The lease: draft threads written against another head of the PR than its head
                // now are moved to its head, for the user to see where they are, and nothing is
                // posted.
                let stale: Vec<Id> = draft
                    .comments
                    .iter()
                    .copied()
                    .filter(|c| r.comments[c].written_at.is_some_and(|at| at != self.head))
                    .collect();
                if rules.lease && !stale.is_empty() {
                    let r = m.stores.get_mut(&store).unwrap();
                    for c in stale {
                        let comment = r.comments.get_mut(&c).unwrap();
                        // One whose line has gone is now on its file, which has no line to move.
                        comment.written_at =
                            self.threads[&comment.thread].placeable.then_some(self.head);
                    }
                    return Some(m);
                }
                // A thread whose line has gone is posted as a comment on its file.
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
                // Posted in the order they were written, so that each thread's first comment comes
                // before its replies, whichever were moved into the draft review when.
                let mut order = draft.comments.clone();
                order.sort();
                for (posted, cid) in order.into_iter().enumerate() {
                    if stop_after == Some(posted) {
                        m.stores.get_mut(&store).unwrap().interrupted.insert(cid);
                        break;
                    }
                    let gid = m.new_id();
                    let r = m.stores.get_mut(&store).unwrap();
                    r.interrupted.remove(&cid);
                    let local = r.comments.remove(&cid).unwrap();
                    if local.local_only {
                        m.local_posted.push(cid);
                    }
                    if local.written_at.is_some_and(|at| at != self.head) {
                        m.posted_off_head.push(cid);
                    }
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
                let r = m.stores.get_mut(&store).unwrap();
                if r.draft.as_ref().unwrap().comments.is_empty() {
                    r.draft = None;
                    m.refresh_copy(store);
                }
            }
            Action::Fetch { store } => m.refresh_copy(store),
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
                for store in Store::ALL {
                    if store.owner() == user
                        && let Some(d) = &self.stores[&store].draft
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
                m.head += 1;
            }
            Action::EditOnGitHub {
                store,
                comment,
                publish,
            }
            | Action::DeleteOnGitHub {
                store,
                comment,
                publish,
            } => {
                let user = store.owner();
                if !self.copied_in(store).contains(&comment) {
                    return None;
                }
                let c = self.gh_comments.get(&comment)?;
                if c.author != user || c.deleted {
                    return None;
                }
                if self.stores[&store].copied_edits[&comment] != c.edits {
                    if rules.refuse_stale_copies {
                        return None;
                    }
                    m.overwritten.push(comment);
                }
                let public = if rules.classify_on_github {
                    Self::public_in(&self.gh_reviews, comment)
                } else {
                    let copy = self.stores[&store].copy.as_ref().unwrap();
                    Self::public_in(copy, comment)
                };
                if public && !publish {
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
                m.refresh_copy(store);
            }
        }
        Some(m)
    }

    /// The actions to try from this state: some are refused by the rules.
    pub fn actions(&self, max_writes: usize) -> Vec<Action> {
        let mut out = vec![];
        for store in Store::ALL {
            if self.written.len() < max_writes {
                for local in [false, true] {
                    out.push(Action::Write {
                        store,
                        thread: None,
                        local,
                    });
                    for thread in self.threads_seen_in(store) {
                        out.push(Action::Write {
                            store,
                            thread: Some(thread),
                            local,
                        });
                    }
                }
            }
            for (&comment, c) in &self.stores[&store].comments {
                out.push(Action::Delete { store, comment });
                out.push(Action::Move {
                    store,
                    comment,
                    local: !c.local_only,
                });
            }
            out.push(Action::Sync {
                store,
                stop_after: None,
            });
            out.push(Action::Sync {
                store,
                stop_after: Some(1),
            });
            out.push(Action::Fetch { store });
            for comment in self.copied_in(store) {
                for publish in [false, true] {
                    out.push(Action::EditOnGitHub {
                        store,
                        comment,
                        publish,
                    });
                    out.push(Action::DeleteOnGitHub {
                        store,
                        comment,
                        publish,
                    });
                }
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
        for (store, r) in &self.stores {
            let mut in_draft: Vec<Id> = r.draft.iter().flat_map(|d| d.comments.clone()).collect();
            in_draft.sort();
            let not_local: Vec<Id> = r
                .comments
                .iter()
                .filter(|(_, c)| !c.local_only)
                .map(|(id, _)| *id)
                .collect();
            if in_draft != not_local {
                errors.push(format!(
                    "In {store:?}, a comment is neither in exactly one review nor local-only"
                ));
            }
            for (id, review) in r.copy.iter().flatten() {
                if review.state == ReviewState::Pending && review.author != store.owner() {
                    errors.push(format!(
                        "{store:?}'s copy has another user's pending review {id}"
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
        for r in self.stores.values() {
            if r.comments.keys().any(|c| origins.contains(c)) {
                errors.push("A posted comment is still a local draft".to_owned());
            }
        }
        let kept: BTreeSet<Id> = self
            .stores
            .values()
            .flat_map(|r| r.comments.keys().copied())
            .chain(origins.iter().copied())
            .chain(self.deleted.iter().copied())
            .collect();
        if self.written.iter().any(|c| !kept.contains(c)) {
            errors.push("A comment was lost".to_owned());
        }
        for c in &self.local_posted {
            errors.push(format!("Local-only comment {c} was posted"));
        }
        for g in &self.overwritten {
            errors.push(format!(
                "Comment {g} on GitHub was changed from a copy older than GitHub's"
            ));
        }
        for c in &self.posted_off_head {
            errors.push(format!(
                "Comment {c} was posted on a line found in another head of the PR"
            ));
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
        if let Some(store) = action.store() {
            let user = store.owner();
            // No publication: what others can see on GitHub changes only through an action the
            // user said publishes.
            for other in [Person::Me, Person::Colleague] {
                if other != user
                    && !action.says_to_publish()
                    && self.seen_by(other) != after.seen_by(other)
                {
                    errors.push(format!(
                        "{action} changed what {other:?} sees on GitHub without publishing"
                    ));
                }
            }
            // Hands off: nothing anyone else wrote on GitHub is changed.
            for (id, comment) in &self.gh_comments {
                if comment.author != user && after.gh_comments.get(id) != Some(comment) {
                    errors.push(format!(
                        "{action} changed comment {id}, which {:?} wrote on GitHub",
                        comment.author
                    ));
                }
            }
        }
        if let Action::Fetch { store } = action {
            if self.stores[store].draft != after.stores[store].draft {
                errors.push(format!("Fetching changed a draft review in {store:?}"));
            }
            if after.stores[store].copy.as_ref() != Some(&after.visible(store.owner())) {
                errors.push(format!(
                    "After fetching, {store:?}'s copy is not GitHub's reviews as it sees them"
                ));
            }
        }
        errors
    }

    /// Whether I can post my laptop's draft review, without submitting part of it, by syncing,
    /// once any reply it blocks on is made local-only.
    pub fn drain(&self, rules: Rules) -> Vec<String> {
        let mut m = self.clone();
        for _ in 0..8 {
            let r = &m.stores[&Store::Laptop];
            if r.draft.is_none() {
                return vec![];
            }
            // A draft reply in a thread whose first comment is local-only stays local-only too.
            if let Some(&comment) = m.blocked_replies(Store::Laptop).first() {
                let to_local = Action::Move {
                    store: Store::Laptop,
                    comment,
                    local: true,
                };
                match m.apply(&to_local, rules) {
                    Some(next) => m = next,
                    None => return vec!["I can't make a blocked reply local-only".to_owned()],
                }
                continue;
            }
            let sync = Action::Sync {
                store: Store::Laptop,
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

    #[test]
    fn without_the_lease_a_comment_is_posted_on_a_line_found_in_another_head() {
        let rules = Rules {
            lease: false,
            ..Rules::default()
        };
        let (_, error) = violation(rules, 5, 3).expect("a violation");
        assert!(error.contains("in another head"), "{error}");
    }

    #[test]
    fn without_classifying_on_github_a_comment_is_published_without_the_users_say_so() {
        let rules = Rules {
            classify_on_github: false,
            ..Rules::default()
        };
        let (_, error) = violation(rules, 5, 3).expect("a violation");
        assert!(error.contains("without publishing"), "{error}");
    }

    #[test]
    fn without_refusing_stale_copies_a_change_made_on_github_is_overwritten() {
        let rules = Rules {
            refuse_stale_copies: false,
            ..Rules::default()
        };
        let (_, error) = violation(rules, 5, 3).expect("a violation");
        assert!(error.contains("from a copy older than GitHub's"), "{error}");
    }

    #[test]
    fn without_blocking_replies_to_local_threads_a_reply_reaches_github_before_its_thread() {
        let rules = Rules {
            block_replies_to_local_threads: false,
            ..Rules::default()
        };
        let (_, error) = violation(rules, 5, 3).expect("a violation");
        assert!(error.contains("before its thread was on GitHub"), "{error}");
    }
}
