# delta PR review: sync model spec

Nov 24, 2026 · @Dan Davison

## Goals and principles

delta keeps one local store of PR review data, shared by the user and their agents, and syncs it with a remote when a PR is linked. Each kind of remote is reached through a backend, and GitHub is the first. v1 aims for semantics that are easy to predict and impossible to misuse. Anything outside that is refused with a clear error rather than half-supported.

**Core flows v1 must support**

1. An agent writes code on a branch. The user reviews it with the agent locally, then opens a GitHub PR with review comments explaining the code.
2. An agent helps the user review a colleague's GitHub PR. The user posts the agent's comments with their own, or as a separate review.
3. Any mix of the above, with the user also working directly on GitHub at the same time.

**Principles**

- **Bulletproof over broad.** An unsupported action fails with a message naming the action and the way to get it done.
- **Each comment has one owner.** Until it is posted, a comment lives only in the local store. Once posted, it lives on the remote, and the local store holds a copy.
- **Sync is predictable.** It fetches, brings the branch to the PR's head, and posts the draft comments. It never pushes, and it posts nothing if the PR has changed since a draft was written.
- **Nothing becomes visible to others unless the user confirms it.** Fetch and sync change only what the user alone can see. Each publishing action needs a confirmation.
- **Nothing is lost by accident.** Only explicit actions delete anything, and sync never deletes local work.
- **Other people's content is read-only.** delta never edits or deletes anything on the remote that someone else wrote.

## Local entities

Local entities follow GitHub's shape, which other review systems share closely, with three additions: stores, agent authors, and two buckets for comments not yet on the remote.

```
Store   { owner }                 -- one per clone; its worktrees share it
Author  = Me | Agent(name)        -- Me is the store's owner

PR      { base, head, title, body, author, link? }
Review  { pr, author, body, state, link }   -- always a copy of a remote review
  state = Pending                 -- the owner's; only they can see it
        | Submitted(verdict) | Dismissed
Thread  { pr, anchor: (commit, path, side, lines), subject: Line | File, resolved }
Comment { pr, author, body, place, link? }
  place  = Conversation
         | Code { thread, bucket }
  bucket = Draft                  -- posted at the next sync
         | LocalOnly              -- never leaves the store
         | Remote(review)         -- a copy of a comment in a remote review
```

- **Draft comments** go into the owner's pending review at the next sync, whoever wrote them. An agent's draft is posted under the owner's identity on the remote. Together they are the store's draft review.
- **Local-only comments** never leave the store. Conversations between the user and agents live here.
- **Moving** a comment between Draft and LocalOnly is an explicit action.
- **Once posted,** a comment belongs to the remote, and the store keeps only the copy that fetch gives it. Reviews exist locally only as such copies.
- **A thread is linked** when its first comment is on the remote.
- **A local conversation comment** stays local in v1, because GitHub would publish it at once.
- **Anchors are commits.** Comments on uncommitted changes are not supported in v1.

## Replicated state

The whole system is a set of stores, the remote R, and each store's Link. Every behaviour in this spec is a function of them.

| Part | What it is |
| --- | --- |
| L, per store | Draft and local-only comments, and copies of remote data |
| R | The remote, as its backend shows each user |
| Link, per store | A one-to-one map between local ids and remote ids |

**Two kinds of local data.** Draft and local-only comments are owned by the store: they exist nowhere else, and change only through local actions. Everything linked is a copy of remote data. Fetch replaces the copies, and nothing else writes to them.

**Changing a posted comment** (editing or deleting one of the owner's comments on the remote) is a change on the remote, made at once. delta first checks that its copy is current, refusing if the comment changed on the remote since the last fetch, and fetches afterwards. A comment in the pending review changes privately. A comment in a submitted review changes publicly, so it needs confirmation (see Visibility). Without a connection these changes fail at once; nothing is queued.

## Fetch and sync

There are two verbs, and neither merges: a store and the remote never hold two versions of the same comment.

**Fetch** replaces every copy in L with R as the owner sees it, links remote entities it has not seen, and never changes a draft or local-only comment. It runs when a linked PR is opened, on `g r`, and at the start and end of a sync. There is no background polling.

**Sync** is always an explicit user action. It runs these steps, stopping at the first refusal:

1. Fetch. If the PR is not on the remote, refuse and name the command that creates it.
2. Bring the branch to the PR's head, fast-forward only. Refuse if the branch has commits the PR lacks, or if tracked files have uncommitted changes.
3. The lease: if any draft thread was written against a different head from the PR's head now, move each one to where its line is now (or onto its file if the line has gone), post nothing, and tell the user to check them and sync again.
4. Refuse if any draft comment is blocked, listing each with its reason.
5. Post every draft comment into the owner's pending review, starting one if there is none, with each thread's first comment before its replies.
6. Fetch.

**Crash safety.** Before posting each comment, sync looks for it among the comments already in the pending review, matching path, side, line and body and counting duplicates. A match is linked instead of posted again. The pending review is private to the owner, so a match is reliable.

## Visibility

Without the user's confirmation, delta never makes anything visible to anyone but the user. Most things become public only through the user's own action on the remote, such as GitHub's Submit review button.

The backend classifies every outgoing change (GitHub's rules are under GitHub backend):

- **Private:** visible only to the user, such as posting drafts into their pending review, or editing its comments.
- **Publishing:** visible to others, and supported with confirmation, such as editing the user's own submitted comment.
- **Unsupported:** visible to others and not supported in v1, such as submitting a review. Refused at once.

| Verb | Moves data | Who can see the result | When |
| --- | --- | --- | --- |
| Fetch | remote → local | nobody new | on opening, `g r`, and in sync |
| Sync | drafts → the user's pending review | only the user | explicit |
| Publishing action | local → public on the remote | everyone | explicit and confirmed |

**Confirmation.** In the TUI, a publishing action asks y/n, naming what will become visible. On the command line it needs `--publish`; without it, the command refuses and says why. The flag makes intent explicit, so an agent tidying drafts can't change a public comment by mistake. It is not a security boundary.

**Unsupported changes are refused when attempted, not queued.** With the GitHub backend, for example, resolving a thread fails with: "Resolving is visible to others, and delta v1 doesn't do it. Resolve it on GitHub."

v1 has two publishing actions: editing or deleting the user's own comment in a submitted review, and creating a PR. More can be added the same way, each named, confirmed and classified by the backend.

## Reviews

A store has one draft review per local PR: its draft comments. Remote reviews appear locally only as copies. Sync adds the drafts to the owner's pending review, whoever started it, whether that was delta or the GitHub UI. GitHub allows one pending review per user per PR, so there is never a choice of which.

&#91;embedded content: where a code comment can be, and what moves it\]

Solid arrows are delta actions. Dashed arrows happen on GitHub and reach delta at the next fetch. A pending review discarded on GitHub disappears at the next fetch, and the store's drafts are not affected.

**Posting an agent's review as its own review.** Move your own drafts to local-only, sync, and submit the pending review on GitHub with a body such as "Review by an agent". Then move your drafts back.

**Editing and deleting.** Draft and local-only comments change locally. The owner's comments on the remote change on the remote at once: privately in the pending review, and as a confirmed publishing action in a submitted review (see Replicated state).

**First comments.** A thread's first comment can't be deleted while the thread has replies. A first comment on the remote can't be deleted from delta at all; edit it instead.

**Changes made on the remote, seen at the next fetch:**

- *Submitted:* the copy becomes Submitted.
- *Discarded:* the pending review is gone from the copy. The store's drafts are not affected.

## Authorship

Every author in a store maps to the store owner's identity on the remote (on GitHub, their login). The store keeps the true author of draft and local-only comments. Once a comment is posted, its copy shows the owner, and the agent's name is not kept.

**Permissions.** Anyone writing in a store can edit or delete its draft and local-only comments. On the remote, only the owner's own comments can be edited or deleted. Other people's content is read-only: the user can reply to it, and can resolve only local-only threads. Agents act with the owner's permissions.

## Blocked comments

A draft comment is blocked when the remote could not accept it as it stands. Sync posts nothing while any draft is blocked, and lists each with its reason. A blocked comment keeps its content.

| Reason | Typical cause | What happens |
| --- | --- | --- |
| Anchor not at the PR's current head | Written before the PR changed | Sync moves it to where its line is now (the lease). The user checks it and syncs again |
| Line gone at the PR's head | The code under it was changed or removed | Sync moves it onto its file, starting with the line it was on. The user checks it and syncs again |
| Thread's first comment is local-only | A draft reply in a local-only thread | The user moves the first comment to Draft, or the reply to LocalOnly |
| Thread gone on the remote | Its first comment was deleted there | The user deletes the reply, or writes it as a new comment |
| Backend reason | Anything else the backend reports | As the backend's message says |

The lease moves comments locally and posts nothing in the same sync, so the user always sees where a comment will land before it is posted. Agents never run sync unless the user asks, so that second sync is always the user's own check.

## Remote contract

A backend connects the core to one kind of remote. Backends declare; the core enforces. Every guarantee in this spec holds for any backend that meets this contract, and a backend that breaks it has a bug.

| Question | The backend answers | GitHub's answer |
| --- | --- | --- |
| Classify a change | Private, Publishing or Unsupported | See Classification under GitHub backend |
| Constraints on sync | Refusal reasons, or where drafts go | One pending review per user per PR, so sync adds to it |
| Blocked comments | Reasons a comment can't be posted | Anchor not at the PR's head; thread's first comment missing or deleted |
| Identity | The remote user a store's authors map to, and what that user may edit | The owner's login; only their own comments |
| Retry after a crash | How sync finds what an interrupted sync posted | Match the pending review's comments by path, side, line and body |
| Fetch | R, as visible to the owner | Includes the owner's pending review, not others' |
| Verdicts | A backend-defined value, opaque to the core | Commented, approved, changes requested |

The core makes a Publishing change only after confirmation, and never makes an Unsupported one. Every remote write goes through one function that asks the backend to classify it, so this rule is enforced in one place. RemoteOp in Events is backend-specific. GitLab draft notes and Gerrit draft comments both fit the same model of drafts published together.

## GitHub backend

GitHub is the only backend in v1. On GitHub, a review is a batch of code comments published together, and a thread is a conversation about one place in the code. They are independent: a thread's comments usually belong to different reviews.

**Data model**

- **Conversation comments** sit on the PR as a whole and are never in a review. They have no draft state: creating one publishes it.
- **Code comments** are always in exactly one review, and in exactly one thread. "Add single comment" creates a review that is submitted at once and contains that one comment.
- **Threads** carry the anchor (commit, path, side, line range) and a resolved flag. A thread's first comment is its root.
- **Reviews** have an author, a body, and a state: pending, or submitted with a verdict.
- **Pending reviews:** each user has at most one per PR, and only that user can see it. A pending review can be deleted. A submitted review cannot.

**Classification**

- *Private:* starting the owner's pending review, posting to it, editing or deleting its comments, and deleting it.
- *Publishing:* editing or deleting the owner's comments in a submitted review, and creating a PR.
- *Unsupported:* everything else others would see. That covers submitting a review, conversation comments, resolving or unresolving a thread, and editing the PR.

**After a push,** GitHub can briefly report the old head together with the new commit count. Any check that compares the two must allow for this.

**Anchors.** GitHub places a pending review's comments against the PR's head, whatever commit is given when the review is started. That is why a comment can be posted only when it is anchored at the PR's head (see Blocked comments).

## Events

Every history is a sequence of these events applied to the stores and the remote. The same list is the alphabet for property tests.

```
Event =
  | Local(store, author, LocalOp)
  | Remote(remote_user, RemoteOp)
  | Fetch(store)
  | Sync(store)
  | Publish(store, PublishOp)       -- only after confirmation
  | Crash                           -- interrupts a sync between remote calls

LocalOp =
  | Comment(path, line, side, body, Draft | LocalOnly)
  | Reply(thread, body, Draft | LocalOnly)
  | Edit(comment) | Delete(comment) -- draft or local-only
  | Move(comment, Draft | LocalOnly)
  | EditPending(comment) | DeletePending(comment)   -- made on the remote at once
  | ResolveThread(local-only thread)
  | Commit

PublishOp =
  | EditSubmitted(comment) | DeleteSubmitted(comment)   -- the owner's own
  | CreatePR

RemoteOp =                          -- backend-specific; GitHub's:
  | SingleComment                   -- a review submitted with one comment
  | ConversationComment
  | StartPendingReview | AddToPending
  | EditComment | DeleteComment
  | SubmitReview(verdict) | DiscardPendingReview | DismissReview
  | ResolveThread | UnresolveThread
  | PushHead | ForcePushHead
  | EditPR
```

A Publish event happens only after confirmation (see Visibility).

## Invariants

These must hold after every event, for any sequence of events. They are the promises made to users, and each one is a test assertion.

1. **No loss.** Every body an author has saved can still be reached, in a store or on the remote, unless someone deleted it with an explicit action.
2. **No unconfirmed publication.** What other users can see changes only through Publish events.
3. **Sync is complete.** After a successful sync, every draft comment is in the owner's pending review, and the store has no drafts left.
4. **One to one.** Link is a bijection. No comment is posted twice, even after a Crash at any point in a sync.
5. **Hands off.** Nothing authored on the remote by someone else is ever edited or deleted by delta.
6. **Fetch is safe.** Fetch never changes a draft or local-only comment.
7. **Constraints hold.** No sync breaks a backend constraint. With GitHub, no user ever has more than one pending review per PR.
8. **Convergence.** If edits stop, a sync posts every draft that isn't blocked, and leaves every copy equal to R.
9. **Local-only stays local.** No comment that was local-only when a sync ran was posted by it.
10. **First comments kept.** No thread loses its first comment while it has replies.
11. **Anchors at head.** Every comment posted was anchored at the PR's head when it was posted.
12. **Only review data.** delta changes a branch only by fast-forwarding it to the PR's head in a sync, with a clean working tree, and pushes only in CreatePR.

## Not supported in v1

Each of these fails at once with a message that names what the user can do instead. The list is for the GitHub backend.

| Action | Message points to |
| --- | --- |
| Submitting a review | Submit it on GitHub |
| Publishing a conversation comment | Post it on GitHub; it stays local in delta |
| Resolving or unresolving a GitHub thread | Do it on GitHub |
| Editing the PR's title or body | Edit it on GitHub |
| A publishing action without confirmation | Answer y, or pass `--publish` |
| Syncing with no PR on the remote | The command that creates one |
| Syncing with commits the PR lacks | Push them first |
| Syncing with uncommitted changes to tracked files | Commit or stash them first |
| Syncing with blocked comments | The list of blocked comments and their reasons |
| Changing a posted comment without a connection | Try again when online |
| Deleting a thread's first comment while it has replies | Delete the replies first |
| Deleting a first comment on GitHub | Edit it instead |
| Editing someone else's content | Reply instead |
| Commenting on uncommitted changes | Commit first |
| Local submit or verdicts | Deferred to v2 |

## Worked flows

All three flows use the GitHub backend.

**Default destinations.** Where a new comment lands is a UI default, not part of the semantics. The user can always move it.

- A new thread, from anyone, starts as Draft.
- A reply is LocalOnly if its thread's first comment is LocalOnly, and Draft otherwise.
- `--local` and `--draft` on the command line, and a TUI key, move a comment either way.

### A. Agent wrote the code; you open a PR with explanations

1. The agent commits on `feat`. delta has an unlinked local PR P, with base `main` and head `feat`.
2. You and the agent discuss the code in threads you mark local-only, or that the agent starts with `--local`.
3. The agent revises the code. Threads on old commits show as outdated, and you resolve them locally.
4. You write draft comments explaining the code, or have the agent draft them and edit them yourself.
5. You create the PR with delta's create-PR command, which asks first and pushes the branch.
6. You sync. The drafts go into a new pending review.
7. You check the pending review on GitHub and submit it.

The explanations are public, and the discussion with the agent never left the machine.

### B. An agent helps you review a colleague's PR

1. You open Alice's PR. Fetch copies its reviews, and a sync brings your branch to the PR's head.
2. The agent reviews: its comments are drafts, labelled with its name.
3. You discuss its comments with it in local-only replies.
4. Then either:
   - post its review on its own: move your drafts to local-only, sync, submit on GitHub with a body such as "Review by an agent", then move your drafts back; or
   - edit or delete its drafts as you like, add your own, sync, and submit one review.
5. Alice replies. A fetch brings her replies in, and you answer them as drafts and sync.

With the first option, the agent's review and yours appear on GitHub as two separate reviews.

### C. Mixed work on GitHub and in delta

| Step | What delta does | Visible to Alice |
| --- | --- | --- |
| You post single comments on GitHub | Fetch copies them as submitted | Yes, you posted them |
| You start a pending review on GitHub | Fetch copies it as your pending review | No |
| You add comments in delta | Drafts | No |
| An agent reviews | Drafts labelled with its name | No |
| Alice replies to your single comments | Fetch copies her replies, read-only | They're hers |
| You discuss with the agent | Local-only replies | No |
| You sync by accident | The drafts join your pending review | No |
| You tidy them up | Each edit or delete is made in the pending review at once | No |
| You fix a typo in an earlier single comment | Asks y/n, then edits it on GitHub | Yes, once you confirm |
| You submit on GitHub | Fetch marks the review submitted | Yes, from now |

## Testing

The model is executable Rust, and the tests generate event sequences against it. There is no separate formal spec to keep in step.

- **Model.** The state machine in `lifecycle.rs`: stores, the remote, actions matching Events, and one check per invariant.
- **Fake remotes.** One in-memory fake per backend, implementing the same trait as the real backend. The fake GitHub encodes GitHub's rules: one pending review per user, pending reviews private, conversation comments public at once, submitted reviews undeletable. It can fail part way through a sync, including after creating a comment but before replying. The fakes record every git and gh command delta runs.
- **Property tests.** Random sequences of events, with two stores owned by one user and one owned by another, and agents writing in the first. All invariants are checked after every step, and Crash is injected between every pair of remote calls in a sync.
- **Scenario tests.** Flows A, B and C, as named tests.
- **Real-remote traces.** Scripted runs against a sandbox repo pin down the behaviour each fake must copy. Each trace becomes a regression test.

Refusals are part of the model: a test can assert that an event is refused with a given reason and that the state is unchanged.

## Open questions

**GitHub behaviour to confirm with traces**

- [ ] What happens to comments in a pending review whose anchor commit is force-pushed away?
- [ ] Can a pending review's body be updated through the API before submit?

**Design decisions**

- [ ] Should agent comments carry a visible tag on GitHub, since the agent's name is not kept once posted?
- [ ] What happens when a linked remote PR's base or head differs from the local PR's?
- [ ] Should anchors on uncommitted changes be supported for agent loops?
- [ ] Which second backend should test the Remote contract: GitLab, or a git-refs remote for sharing review data without a forge?

**Settled:** L lives in `gitu/review/<head>...<base>.json` in the git common directory, shared by all worktrees, as the `local-prs` branch has it.

**Deferred to v2:** local submit and verdicts, more publishing actions, background fetching.
