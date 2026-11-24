# delta PR review: aligning the local-prs branch with the spec

Nov 24, 2026 · @Dan Davison

For the agent working on `dandavison/delta-gitu`, branch `local-prs` (34 commits ahead of `main` at 677be9b). Read it alongside [delta PR review: sync model spec](https://claude.ai/artifact/Nvo4dvjDLvQuPQhkX87czJ).

The branch already follows the spec's testing method: an executable state machine, a conformance test, and tests against GitHub. Two gaps remain. Every local comment goes into one draft review, and sync posts it whole, so a conversation with an agent can't stay local and an agent's review can't be sent on its own. And sync and edits can make things public. The plan below closes both, and keeps the state machine, the conformance test and most of the GitHub code.

## What to keep

Keep all of the following and build on it. Together it already covers the spec's testing approach.

- **The state machine** in `src/review/lifecycle.rs`: the `Model`, `Action` and invariants, the explorer that tries every sequence (depth 5, or 7 with `--ignored`), the `Rules` switch-off tests and `drain()`. This is the spec's executable model. Change it first (step 1).
- **The conformance test** in `src/tests/lifecycle.rs`. It drives gitu through `delta --tui review` against a multi-user fake GitHub (`Server`) that can fail part way through a post.
- **The real-GitHub tests** in `src/tests/real_github.rs` (`cargo test-github`). These are the spec's real-remote traces.
- **The store:** `gitu/review/<head>...<base>.json` in the git common directory, with a lock file, atomic replace and versioned migrations.
- **Thread anchors** that record the line's text and context, and the line-following in `locate`, for display.
- **Fetch** in `LocalBackend::fetch`. It replaces the copies wholesale and never touches drafts, which is the spec's "pull is safe".
- **Refusing rather than working around:** errors for unknown review states, missing commits, threads with more than 100 comments, and GitHub replies that did nothing. Also `orphaned_replies`, which is the spec's "thread gone" blocked reason.
- **The split between the two traits:** `Backend` serves a screen, and `GitHub` makes the remote calls. `check_pr` and the base checks on opening a PR also stay.
- **The agent skill** (`agent_instructions.md`) with its test that every command it names exists, and the diagram with its exhaustive-match test.

## Where the branch differs from the spec

Each row is one change. The spec column names the spec section.

| Spec | Branch now | Change |
| --- | --- | --- |
| Local entities: several Open reviews, comments in no review | One draft review per repository (`RepoState.draft`; one `Draft` in `StoredReview`). Every local comment is in it. An agent writes as the repository's author, with `--agent` as a label | Several Open reviews per PR, each with an author: Me or Agent(name). A code comment is in one review or none. Comments in no review are never sent |
| Reviews: Send(review) | `post::sync` posts the whole draft review, adding to any pending review | `review send <review>` posts one Open review, and is refused while the user has a pending review. Adding to the pending review is its own explicit action |
| Reviews: Unsend | Missing | Add it: delete the pending review and recreate its comments locally as an Open review |
| Visibility: no publication | `LocalBackend::whose` returns `OnGitHub` for any review marked `mine`. So `review edit`, `review delete`, `i` and `K` change submitted, public comments | Edit or delete on GitHub only for comments in the user's pending review. Refuse the rest with the spec's message |
| Goals and principles: no PR creation | `sync` with `create_pr` pushes and runs `gh pr create`; `O s` asks first | Remove both from sync. The user runs `gh pr create`, and the next pull links the PR |
| Goals and principles: no branch changes | `up_to_github_head` runs `gh pr checkout` | Remove it. Pull leaves the branch alone. If the PR's head commit isn't in the repository, pull says to fetch it, as \`missing\_commit\` already does for a thread's commit |
| Blocked comments: anchors at the PR's head, re-anchored explicitly | Sync runs `stale_drafts` and `move_drafts`. These move drafts, and turn a thread whose line has gone into a file comment | Send refuses and lists the stale comments. A new `review reanchor` reuses `move_drafts` to move them. A thread whose line has gone stays blocked until the user moves it to its file with an explicit flag |
| Sync: crash safety | `mark_posted` runs after each GitHub call. A crash in between means the retry posts the comment twice | A marker in each body holds the local id. Before each create, look for that marker in the pending review |
| Authorship: attribution | The agent label is lost once posted: fetched comments get `agent: None` | The marker also carries the author. Fetch restores the label on the user's own comments |
| Sync: pull and send | `review sync` fetches then posts; `review fetch` also exists | `review pull` (automatic in the TUI on opening and on `g r`) and `review send <review>`. Drop `sync` |
| Remote contract | Calls that write to GitHub are spread through `post.rs` and `local.rs` | Route every remote write through one function. It asks the backend to classify the change and refuses anything not private |

## The plan

Work in this order, keeping the branch's habit of committing failing tests first. Step 1 changes only the model, so the explorer settles the rules before any product code changes. Each later step ends with the conformance test passing.

1. **Model (`lifecycle.rs`).**
   - Rename `Repo` to `Store`: a local store with an owner, `Store { id, owner: Person }`. Make stores data that each test declares, not fixed values. The tests declare two stores owned by Me and one owned by Colleague, the smallest setup that covers two clones under one GitHub identity. Worktrees of one clone share a store, and an agent is an author within a store, not a store.
   - In each store, give each comment an author: Me or Agent(name). Replace `RepoState.draft` with `reviews: BTreeMap<Id, OpenReview { author }>`, and give `LocalComment` a `review: Option<Id>`.
   - Actions: `Write { repo, author, thread, review }`, where review `None` means no review; `Move { repo, comment, to }`; `Send { repo, review, stop_after }`; `AddToPending { repo, comment }`; `Unsend { repo }`; `Pull { repo }` (renamed from `Fetch`); `Reanchor { repo, comment }`. Remove `Sync`. Restrict `EditOnGitHub` and `DeleteOnGitHub` to comments in the user's pending review.
   - Send is refused while the user has a pending review, or while any comment in the review is blocked: written at another head, its thread neither on GitHub nor started in this review, or its thread deleted.
   - Add the invariants in the next section, and update the diagram's mapping, which the compiler will force.
   - Done when the explorer passes at depth 5 and 7, and each new rule has a switch-off test.
2. **Store version 6 and local commands.**
   - `StoredComment.review` becomes optional, and the store holds several Open reviews, each with an author and agent label. Migrating from version 5: the existing draft becomes the user's Open review, agent-labelled comments included. Don't guess which comments to split out.
   - CLI: `review comment` and `review reply` take `--agent NAME`, and either `--review ID` or `--no-review`. Defaults follow the spec's "Default destinations": an agent writes to its own Open review, which is created if needed. Add `review move <comment> (--to ID | --none)`. `review show --format json` lists each review with its author and state.
   - TUI: show which review each draft is in, and add a key to move a comment. Keep this minimal.
3. **No publication.**
   - In `LocalBackend::whose`, return `OnGitHub` only when the review is the user's and Pending.
   - Remove `create_pr`, the push and `up_to_github_head` from sync, along with `--create-pr` and the question `O s` asks.
   - Add one function that every remote write goes through. It takes a `RemoteChange` (start review, add thread, add reply, edit, delete, delete review) and refuses any change the backend doesn't classify as private.
   - Make both fakes, `Server` and the fake in the PR tests, record each write along with the state of the review it touched. Add a test that fails if any write touched a review that wasn't pending.
4. **Pull, send and unsend.**
   - Replace `post::sync` with `pull`, which fetches and records the link, and `send(review)`. Send pulls first, refuses as in step 1, starts the pending review, and posts each thread before its replies. It then deletes the local Open review, whose comments come back in the copy, and pulls.
   - While a send is under way, record `sending: <pending review id>` on the Open review. That way a retry after a failure part way continues the send instead of refusing because of the pending review it started.
   - `review move <comment> --to pending` adds the comment to the pending review on GitHub straight away, with the same blocked checks, then pulls.
   - Before editing or deleting a comment in the pending review, re-read it from GitHub and refuse if it differs from the copy.
   - Unsend: create the Open review locally from the copy first, then delete the pending review, then pull. If it crashes part way, both copies exist; neither is ever lost.
5. **Marker.**
   - End every body sent with `<!-- delta id=<local id> author=<me or agent:name> -->`. Fetch strips the marker and restores the agent label, but trusts a marker only on comments the user wrote.
   - Before each create, look for the marker in a freshly pulled copy of the pending review. If it's there, link the comment instead of creating it again.
   - Give `Server` a way to fail after GitHub creates a comment but before gitu records it. Then test that the retry doesn't post a duplicate. Add a real-GitHub test that the marker survives a round trip.
6. **Explicit re-anchoring.**
   - Take the lease's moving out of sync. Send and add-to-pending refuse, listing each stale comment and saying to run `review reanchor`.
   - `review reanchor [<thread>...]` reuses `move_drafts` and prints each thread's old and new place. A thread whose line has gone stays blocked unless it is named with `--to-file`. Only then does `to_post` add its "On line N" prefix.
   - Reanchoring needs the local branch at the PR's head. If the branch is behind, refuse and say to update it first, e.g. with `gh pr checkout`.
7. **Docs and screens.** Rewrite `agent_instructions.md` for Open reviews, comments in no review, pull, send, unsend and reanchor. Agents write to their own review and never send unless asked. Regenerate the diagram from the new actions, and update the help text and `O s`.

## Invariants

The state machine already checks most of the spec's invariants. Five are missing, two of which guard against publishing too early. Numbers are the spec's.

| Spec invariant | `Model::invariants` now | What to do |
| --- | --- | --- |
| 1. No loss | "A comment was lost" | Keep. Extend it to Open reviews, comments in no review, and Unsend |
| 2. No publication | Missing | Add: no action taken from a repository changes what any other user can see on GitHub (`visible(u)` for each other user u) |
| 3. Send is exact | "Draft review split across GitHub reviews" | Replace: once a Send completes, the origins of the pending review's comments are exactly the sent review's comments |
| 4. One to one | "Posted twice"; "a posted comment is still a local draft" | Keep |
| 5. Hands off | Missing | Add: no action from a repository changes a GitHub comment whose author isn't that repository's user |
| 6. Pull is safe | "Fetching changed a draft review" (`step_invariants`) | Keep, under the name Pull |
| 7. Constraints hold | "More than one pending review"; "reply posted before its thread" | Keep |
| 8. Convergence | `drain()` | Keep. Make drain send each Open review in turn, submitting between sends |
| 10. First comments kept | "Thread lost its first comment" | Keep |
| 9. Local-only stays local | Missing | Record the review each comment was in when it was posted. Check that none was in no review |
| 11. Anchors at head | Missing | Record the PR's head when each comment is posted. Check that it equals the comment's `written_at` |
| 12. Only review data | Missing | Add to the conformance test: the fakes record every git or gh command gitu runs, and the test fails on any push, checkout or PR creation |

## Out of scope

If one of these seems needed, stop and ask Dan rather than building it.

- Anything public: submitting, conversation comments, resolving threads, and creating or editing the PR.
- Submitting locally, and verdicts.
- Comments on uncommitted changes.
- A second backend. Keeping GitHub's classification behind the single write function (step 3) is enough to leave room for one.
- Automatic send, and any background posting.
- Changes to Viewed checkboxes, and TUI changes beyond what steps 2 to 7 need.
