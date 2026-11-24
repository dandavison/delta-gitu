# delta PR review: sync model spec

Nov 24, 2026 · @Dan Davison

## Goals and principles

delta keeps one local store of PR review data, shared by the user and their agents, and syncs it with a remote when a PR is linked. Each kind of remote is reached through a backend, and GitHub is the first. v1 aims for semantics that are easy to predict and impossible to misuse. Anything outside that is refused with a clear error rather than half-supported.

**Core flows v1 must support**

1. An agent writes code on a branch. The user reviews it with the agent locally, then opens a GitHub PR with review comments explaining the code.
2. An agent helps the user review a colleague's GitHub PR. The agent's comments are either adopted into the user's review or sent as a separate agent review.
3. Any mix of the above, with the user also working directly on GitHub at the same time.

**Principles**

- **Bulletproof over broad.** An unsupported action fails with a message naming the action and the way to get it done.
- **Each item has one owner.** Until it is sent, an item lives only in the local store. Once sent, it lives on the remote, and the local store holds a copy.
- **Sync is dumb.** Pull replaces local copies of remote data, and Send creates remote copies of local reviews. Neither merges, splits, moves or re-anchors anything. Every transformation is an explicit action.
- **Sync touches only review data.** Pull and Send never push, create a PR, check out, or move a branch.
- **Visibility only rises on request.** Pull sends nothing out. Send changes only what the user alone can see. Publish (not in v1) is the only verb that makes anything visible to others.
- **Nothing is lost by accident.** Local deletions are tombstones, and only the user's own explicit actions delete anything.
- **Other people's content is read-only.** delta never edits or deletes anything on the remote that someone else wrote.

## Local entities

Local entities follow GitHub's shape, which other review systems share closely, with three additions: agent authors, local-only reviews, and code comments that are in no review.

```
Author  = Me | Agent(name)

PR      { base, head, title, body, author, link? }
Review  { pr, author, body, state, link? }
  state = Open                -- local only
        | Sent                -- linked to my pending review on the remote
        | Submitted(verdict)  -- linked to a submitted remote review;
                              -- verdict is backend-defined
Thread  { pr, anchor: (commit, path, side, lines), resolved }
Comment { pr, author, body, place, link? }
  place = Conversation
        | Code { thread, review: Option<Review> }
```

- **A code comment in no review is local-only.** It can never be sent. This is where conversations between the user and agents live.
- **A code comment in a review travels with that review** when the review is sent.
- **A thread is linked when its root comment is linked.** It has no link of its own.
- **A local conversation comment** stays local in v1, because on GitHub it would be public at once.
- **Local reviews never reach Submitted locally in v1.** Submitted reviews come only from the remote, via pull. Verdicts and local submit are deferred.
- **Anchors are commits.** Comments on uncommitted changes are not supported in v1.

## Replicated state

The whole system is the tuple (L, R, Link). Every behaviour in this spec is a function of it.

| Part | What it is |
| --- | --- |
| L | The local store: Open reviews, comments in no review, tombstones, and copies of remote data |
| R | The remote, as its backend shows it to the user |
| Link | A one-to-one map between local ids and remote ids |

**Two kinds of local data.** Open reviews and comments in no review are owned locally: they exist nowhere else, and change only through local actions. Everything linked is a copy of remote data. Pull replaces the copies, and nothing else writes to them.

**Changing a sent item** means adding, editing, deleting or moving a comment in the user's pending review, or editing its body. It is a change on the remote, made at once if the backend classifies it as private. delta first checks that its copy is current, refusing if the item changed on the remote since the last pull, and pulls afterwards. Without a connection these changes fail at once; nothing is queued. To work on a sent review offline, unsend it first.

**Tombstones.** Deleting a local item sets `deleted` and keeps its content, so it can be restored. Copies have no tombstones: an item deleted on the remote is gone from the copy at the next pull.

## Sync: pull and send

Sync is two verbs. Neither merges: the local store and the remote never hold two versions of the same item.

**Pull** fetches R and replaces every copy in L with it. It also:

- creates and links a local copy of each remote entity it has not seen;
- pulls the user's own pending review, if one exists, as a Sent review authored by Me;
- never changes an Open review or a comment in no review.

Pull is automatic: on opening a review and periodically.

**Send(review)** turns an Open review into the user's pending review on the remote (see Reviews). It always pulls first, and is always an explicit user action.

**Crash safety.** Before each remote create, send records its intent in L. On retry, send uses the backend's idempotent-create mechanism (Remote contract) to find anything already created, and links it instead of creating a second copy.

## Visibility

In v1, delta never makes anything visible to anyone but the user. The only way anything becomes public is the user's own action on the remote, such as GitHub's Submit review button.

The backend classifies every outgoing change (GitHub's rules are under GitHub backend):

- **Private:** visible only to the user, such as any change to their pending review.
- **Public:** visible to anyone else.
- **Unsupported:** the backend can't make it at all.

| Verb | Moves data | Who can see the result | v1 |
| --- | --- | --- | --- |
| Pull | remote → local | nobody new | automatic |
| Send | local → user's pending review | only the user | explicit |
| Publish | local → public on the remote | everyone | not supported |

**Public changes are refused when attempted, not queued.** With the GitHub backend, for example, editing a published comment locally fails with: "This comment is public on GitHub; delta v1 doesn't publish. Edit it on GitHub." Refusing at once avoids a growing queue of changes that can never be sent.

Publish is kept in the model so v2 can add it without redesign. When added, it must name each change it publishes.

## Reviews

A review is linked to the remote only when it is sent. Any local review can be sent, including an agent's; on the remote it appears as the user's. With GitHub, at most one review per PR can be Sent at a time, because GitHub allows each user one pending review.

&#91;embedded content: review states and what moves a review between them\]

Shown with the GitHub backend. Solid arrows are delta actions. Dashed arrows happen on GitHub and reach delta at the next pull. A pending review discarded on GitHub disappears at the next pull.

**Send(r)** requires all of the following, and otherwise refuses with the reason:

1. r's PR is linked.
2. r is Open.
3. Sending r breaks no backend constraint. With GitHub, the user has no pending review on the PR; move comments into that review instead.
4. No comment in r is blocked.

It then creates the pending review and posts each thread's first comment before its replies. **After a successful send, the user's pending review on the remote holds exactly r's comments.** From then on r is a Sent review: a copy, changed only on the remote (see Replicated state).

**Unsend(r)** recreates r's comments locally as an Open review, then deletes the pending review on the remote, then pulls. If it stops part way, both exist; neither is lost. It needs a connection, so unsend before working offline on a sent review.

**Moving comments.** A comment can move freely between Open reviews and "no review". Moving one into the Sent review adds it on the remote at once, unless it is blocked. Moving one out deletes it on the remote and puts it in the chosen Open review, or in no review. Moving one out of a Submitted review is refused, because it would delete a public comment.

**Deleting a review** is allowed only when it is Open. Its comments are tombstoned with it. A Sent review must be unsent first.

**First comments.** A thread's first comment can't be deleted while the thread has replies. A first comment on the remote can't be deleted from delta at all; edit it instead.

**Changes made on the remote, seen at the next pull:**

- *Submitted:* r becomes Submitted.
- *Discarded:* r is gone from the copy, like anything deleted on the remote. To keep a pending review's comments, unsend it instead of discarding it.

## Authorship

Every local author maps to the user's identity on the remote (on GitHub, their login). The local store keeps the true author permanently.

**Attribution.** Every item delta creates carries its local id and true author, in a form the backend chooses (GitHub: a hidden marker, under GitHub backend). This keeps agent attribution through a round trip and supports crash recovery.

**Permissions.** A local edit or delete is allowed only when both hold:

- the entity's author maps to the user, and
- the entity is unlinked, or is in the user's Sent review.

Other people's content is read-only. The user can reply to it, and can resolve a linked thread only if the backend classifies that as private, which GitHub never does. Agents act with the user's local permissions.

## Blocked comments

A comment in a review is blocked when the remote could not accept it as it stands. A blocked comment keeps its content and is shown with its reason. Send refuses the whole review until the user fixes or removes the comment.

| Reason | Typical cause | What the user does |
| --- | --- | --- |
| Anchor not at the PR's current head | Written before the PR changed, or on commits not pushed | Bring the branch to the PR's head, then run reanchor |
| Line gone at the PR's head | The code under the comment was changed or removed | Move the comment onto its file, or drop it |
| Thread root not on the remote and not in this review | A reply in a thread whose root is still local | Send the root's review first, or move the root in |
| Thread gone on the remote | Its root was deleted there | Start a new thread, or drop the comment |
| Backend reason | Anything else the backend reports | What the backend's message says |

Re-anchoring is always explicit. `reanchor` moves each comment to where its line is at the PR's current head, and shows where each one went. A comment whose line has gone stays blocked until the user moves it onto its file, which is explicit too.

## Remote contract

A backend connects the core to one kind of remote. Backends declare; the core enforces. Every guarantee in this spec holds for any backend that meets this contract, and a backend that breaks it has a bug.

| Question | The backend answers | GitHub's answer |
| --- | --- | --- |
| Classify a change | Private, Public or Unsupported | Changes to the user's pending review are private; everything else is public |
| Constraints on Send | Refusal reasons | At most one pending review per user per PR |
| Blocked comments | Reasons a comment can't be sent | Anchor commit not in the PR; thread root missing or deleted |
| Identity | The remote user that local authors map to, and what the user may edit | The user's login; only their own content |
| Idempotent create | How a retry finds what an interrupted Send created | A hidden marker holding the local id |
| Fetch | R, as visible to the user | Includes the user's pending review, not others' |
| Verdicts | A backend-defined value, opaque to the core | Commented, approved, changes requested |

The core never sends a Public change except through Publish, which v1 doesn't have, and never sends an Unsupported one. RemoteOp in Events is backend-specific. A review is anything the backend can hold as a batch of drafts published together; GitLab draft notes and Gerrit draft comments both fit.

## GitHub backend

GitHub is the only backend in v1. On GitHub, a review is a batch of code comments published together, and a thread is a conversation about one place in the code. They are independent: a thread's comments usually belong to different reviews.

**Data model**

- **Conversation comments** sit on the PR as a whole and are never in a review. They have no draft state: creating one publishes it.
- **Code comments** are always in exactly one review, and in exactly one thread. "Add single comment" creates a review that is submitted at once and contains that one comment.
- **Threads** carry the anchor (commit, path, side, line range) and a resolved flag. A thread's first comment is its root.
- **Reviews** have an author, a body, and a state: pending, or submitted with a verdict.
- **Pending reviews:** each user has at most one per PR, and only that user can see it. A pending review can be deleted. A submitted review cannot.

**Classification**

- *Private:* creating, editing or deleting the comments or body of the user's pending review; creating or deleting that pending review.
- *Public:* everything else. That covers conversation comments, edits and deletes of published comments, resolving or unresolving a thread, submitting a review, and creating or editing the PR.

**Marker.** Every body delta sends ends with a hidden HTML comment, e.g. `<!-- delta id=c_8f2a author=agent:reviewer -->`. It gives crash recovery and keeps agent attribution through a round trip. Pull strips it for display. delta trusts a marker only on content whose GitHub author is the user, so nobody else can forge one.

**Anchors.** GitHub places a pending review's comments against the PR's head, whatever commit is given when the review is started. That is why a comment can be sent only when it is anchored at the PR's head (see Blocked comments).

## Events

Every history is a sequence of these events applied to (L, R, Link). The same list is the alphabet for property tests.

```
Event =
  | Local(author, LocalOp)
  | Remote(remote_user, RemoteOp)
  | Pull
  | Send(review)
  | Unsend(review)
  | Crash                  -- interrupts a Send between remote calls

LocalOp =
  | CreateComment(place, anchor?, body)
  | EditComment | DeleteComment | RestoreComment
  | MoveComment(comment, Option<review>)
  | CreateReview | EditReviewBody | DeleteReview
  | ResolveThread(local-only thread)
  | Reanchor(comment, anchor)
  | Commit | Rebase       -- on the local branch
  | LinkPR(remote_pr)

RemoteOp =                -- backend-specific; GitHub's:
  | SingleComment         -- a review submitted with one comment
  | ConversationComment
  | StartPendingReview | AddToPending
  | EditComment | DeleteComment
  | SubmitReview(verdict) | DiscardPendingReview | DismissReview
  | ResolveThread | UnresolveThread
  | PushHead | ForcePushHead
  | EditPR
```

Publish is reserved for v2 and is not an event in v1.

## Invariants

These must hold after every event, for any sequence of events. They are the promises made to users, and each one is a test assertion.

1. **No loss.** Every body a local author has saved can still be reached, in L (live or as a tombstone) or on the remote, unless the user deleted it.
2. **No publication.** delta never makes a remote call that its backend classifies as Public.
3. **Send is exact.** Straight after a successful Send(r), the user's pending review on the remote holds exactly r's comments, with their bodies (ignoring markers) and anchors.
4. **One to one.** Link is a bijection. No local entity has two remote copies, even after a Crash at any point in a Send.
5. **Hands off.** Nothing authored on the remote by someone else is ever edited or deleted by delta.
6. **Pull is safe.** Pull never changes an Open review or a comment in no review.
7. **Constraints hold.** Send never breaks a backend constraint; it refuses instead. With GitHub, the user never has more than one pending review per PR.
8. **Convergence.** If edits stop, a pull leaves every copy in L equal to R, and every Open review with no blocked comments can be sent.
9. **Local-only stays local.** A comment in no review never reaches the remote.
10. **First comments kept.** No thread loses its first comment while it has replies.
11. **Anchors at head.** Every comment sent was anchored at the PR's head when it was sent.
12. **Only review data.** No action of delta's changes a branch, the working tree, or what is pushed.

## Not supported in v1

Each of these fails at once with a message that names what the user can do instead. The list is for the GitHub backend.

| Action | Message points to |
| --- | --- |
| Submitting a review | Submit it on GitHub |
| Publishing a conversation comment | Post it on GitHub; it stays local in delta |
| Editing or deleting a published comment | Edit it on GitHub |
| Resolving or unresolving a GitHub thread | Do it on GitHub |
| Creating or editing the PR | Use `gh pr create` or GitHub, then LinkPR |
| Pushing, checking out or moving a branch | Do it with git or gh |
| Sending while you have a pending review | Move comments into it, or submit, discard or unsend it first |
| Sending a review with blocked comments | The list of blocked comments and their reasons |
| Changing a sent review without a connection | Unsend it before going offline |
| Moving a comment out of a Submitted review | It is public; edit or delete it on GitHub |
| Deleting a Sent review | Unsend it first |
| Deleting a thread's first comment while it has replies | Delete the replies first |
| Deleting a first comment on GitHub | Edit it instead |
| Editing someone else's content | Reply instead |
| Commenting on uncommitted changes | Commit first |
| Local submit or verdicts | Deferred to v2 |
| Automatic send | Send is always explicit |

## Worked flows

All three flows use the GitHub backend.

**Default destinations.** Where a new comment lands is a UI default, not part of the semantics. The user can always move it.

- An agent asked to review creates its own Open review, and its comments go there.
- The user's comments go to their selected review, or to no review if none is selected.
- Replies in local-only threads go to no review.
- On a linked PR, the user's replies in GitHub threads go to their Open review.

### A. Agent wrote the code; you open a PR with explanations

1. The agent commits on `feat`. delta has an unlinked local PR P, with base `main` and head `feat`.
2. You and the agent discuss the code in threads. Those comments are in no review, so they are local-only.
3. The agent revises the code. Threads on old commits show as outdated, and you resolve them locally.
4. You create review E and write comments explaining the code. You can also move in, or ask the agent to draft, comments anchored on the final commits.
5. You push, run `gh pr create`, then LinkPR. Pull links P to the GitHub PR.
6. Send(E). Any comment not anchored at the PR's head blocks the send and is listed.
7. You check the pending review on GitHub and submit it. The next pull marks E Submitted.

The explanations are public, and the discussion with the agent never left the machine.

### B. An agent helps you review a colleague's PR

1. Pull brings in Alice's PR, linked, with its existing reviews as Submitted.
2. The agent reviews: its Open review A has a body and comments.
3. You discuss its comments with it. Your replies are in no review.
4. Then either:
   - send A as an agent review: edit it, Send(A), and submit on GitHub; or
   - adopt its comments: move the ones you want into your review M, edit them, Send(M), and submit.
5. Alice replies. Pull brings her replies in, and you answer them in a new review.

You can do both in step 4, but one after the other: submit one before sending the next.

### C. Mixed work on GitHub and in delta

| Step | What delta does | Visible to Alice |
| --- | --- | --- |
| You post single comments on GitHub | Pull links them as Submitted; read-only in delta | Yes, you posted them |
| You start a pending review on GitHub | Pull links it as Sent review M | No |
| You add comments in delta | They go into your Open review N | No |
| An agent reviews | Agent's Open review A | No |
| Alice replies to your single comments | Pull adds her replies, read-only | They're hers |
| You discuss with the agent | Comments in no review | No |
| You try to send N by accident | Refused: you already have pending review M | No |
| You move N's comments, and some of A's, into M | Each is added to M on GitHub at once | No |
| You tidy M's comments | Each edit or delete is made on GitHub at once | No |
| You submit on GitHub | Pull marks M Submitted | Yes, from now |

## Testing

The model is executable Rust, and the tests generate event sequences against it. There is no separate formal spec to keep in step.

- **Model.** `World { local, remote, links }`, the `Event` enum above, `fn step(&mut World, Event) -> Result<(), Refusal>`, and one function per invariant.
- **Fake remotes.** One in-memory fake per backend, implementing the same trait as the real backend. The fake GitHub encodes GitHub's rules: one pending review per user, pending reviews private, conversation comments public at once, submitted reviews undeletable. Each fake is the written-down spec of its remote's behaviour.
- **Property tests.** proptest generates random sequences of Local, Remote, Pull, Send, Unsend and Crash events, with several remote users and agents. The same invariant suite runs against every backend's fake after every step. Crash is injected between every pair of remote calls in a Send.
- **Scenario tests.** Flows A, B and C below, as named tests.
- **Real-remote traces.** Scripted runs against a sandbox repo pin down edge cases each fake must copy. Each trace becomes a regression test for that fake.

Refusals are part of the model: a test can assert that an event is refused with a given reason and that the state is unchanged.

## Open questions

**GitHub behaviour to confirm with traces**

- [ ] What happens to comments in a pending review whose anchor commit is force-pushed away?
- [ ] What happens to a pending reply when its thread's root is deleted?
- [ ] Can a pending review's body be updated through the API before submit?
- [ ] Does GitHub keep hidden HTML comments in bodies exactly as sent?

**Design decisions**

- [ ] Should agent attribution also be visible to colleagues, e.g. a short line under each agent comment?
- [ ] What happens when a linked remote PR's base or head differs from the local PR's?
- [ ] Should anchors on uncommitted changes be supported for agent loops?
- [ ] Which second backend should test the Remote contract: GitLab, or a git-refs remote for sharing review data without a forge?

**Settled:** L lives in `gitu/review/<head>...<base>.json` in the git common directory, shared by all worktrees, as the `local-prs` branch has it.

**Deferred to v2:** publish, local submit and verdicts, creating PRs from delta, automatic send, changing sent reviews offline.
