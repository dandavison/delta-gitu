# delta PR review: aligning the local-prs branch with the spec

Nov 24, 2026 · @Dan Davison

For the agent working on `dandavison/delta-gitu`, branch `local-prs` (34 commits ahead of `main` at 677be9b). Read it alongside [delta PR review: sync model spec](https://claude.ai/artifact/Nvo4dvjDLvQuPQhkX87czJ).

The branch already has most of the design: one draft review per local PR, sync into the user's pending review, the lease, live edits of the user's own comments, a state machine with a conformance test, and tests against GitHub. The changes below add five things: a local-only bucket, confirmation for publishing, crash-safe posting, one checked path for every write to GitHub, and the missing invariants.

## What to keep

Keep all of the following and build on it. Together it already covers the spec's testing approach.

- **The state machine** in `src/review/lifecycle.rs`: the explorer that tries every sequence (depth 5, or 7 with `--ignored`), the `Rules` switch-off tests and `drain()`. Step 1 changes it.
- **The conformance test** in `src/tests/lifecycle.rs`, against the multi-user fake GitHub (`Server`).
- **The real-GitHub tests** in `src/tests/real_github.rs` (`cargo test-github`).
- **The store:** `gitu/review/<head>...<base>.json` in the git common directory, with a lock file, atomic replace and versioned migrations.
- **One draft review per local PR,** and sync adding it to the user's pending review, whoever started that review.
- **The lease** (`stale_drafts`, `move_drafts`), including moving a thread whose line has gone onto its file.
- **Sync bringing the branch to the PR's head** with `gh pr checkout`, fast-forward only, and refusing a branch with commits the PR lacks.
- **Edits and deletes of the user's own comments** made on GitHub at once and then fetched, and the rules about first comments.
- **Fetch** replacing the copies wholesale and never touching drafts; thread anchors and `locate`; `orphaned_replies`; and refusing rather than working around.
- **The split between the two traits** (`Backend` for a screen, `GitHub` for remote calls), and `check_pr`.
- **The agent skill** and its command test, and the diagram and its exhaustive-match test.

## Where the branch differs from the spec

Each row is one change. The spec column names the spec section.

| Spec | Branch now | Change |
| --- | --- | --- |
| Local entities: local-only comments | Every local comment is in the draft review, so a conversation with an agent is posted at the next sync unless it's deleted | Add a LocalOnly bucket that sync never posts, with the defaults and moving in the spec's Worked flows |
| Visibility: confirmed publishing | Editing or deleting the user's comment in a submitted review needs no confirmation | Ask y/n in the TUI, and require `--publish` on the command line. Comments in the pending review need neither |
| Visibility: creating a PR | Part of sync: `O s` asks, or `review sync --create-pr` | Its own command, with a TUI key and `review create-pr`. Sync on a branch with no PR refuses and names it |
| Fetch and sync, step 2 | `up_to_github_head` runs whatever state the working tree is in | Refuse while tracked files have uncommitted changes |
| Fetch and sync: crash safety | `mark_posted` runs after each GitHub call, so if the reply is lost, a retry posts the comment twice | Before posting, match against the pending review by path, side, line and body, counting duplicates. Link a match instead of posting it |
| Remote contract: one write path | Calls that write to GitHub are spread through `post.rs` and `local.rs` | One function with an allow-list. It classifies each change as private or publishing and refuses anything else |
| Blocked comments | No local-only comments yet, so no rule for a draft reply in a local-only thread | Sync refuses such a reply, as it refuses one whose thread was deleted |
| Invariants | No checks for 2, 5, 9, 11 or 12 | Add them (see Invariants) |
| Agent skill | Says nothing about local-only comments or about agents running sync | Agents use local-only for conversations and never sync unless asked. Describe posting an agent's review as its own review |

## The plan

Work in this order, keeping the branch's habit of committing failing tests first. Step 1 changes only the model, so the explorer settles the rules before any product code changes. Each later step ends with the conformance test passing.

1. **State machine (`lifecycle.rs`).**
   - Rename `Repo` to `Store { id, owner: Person }`. Each test declares its stores; the standard setup is two owned by Me and one owned by Colleague. Worktrees of one clone share a store, and agents are authors within a store, not stores.
   - Give `LocalComment` a bucket, Draft or LocalOnly. `Write` takes the bucket, and a new `Move { store, comment, to }` moves a comment between buckets.
   - Sync posts only Draft comments, and refuses a draft reply whose thread's first comment is LocalOnly.
   - Split `EditOnGitHub` and `DeleteOnGitHub` into a pending-review version, which is private, and a submitted-review version, which is a Publish action. Add `CreatePR` as a Publish action.
   - Add the invariants below and update the diagram's mapping. Done when the explorer passes at depth 5 and 7, and each new rule has a switch-off test.
2. **One write path.** Add one function that every GitHub write goes through. It takes a `RemoteChange` (start review, add thread, add reply, edit, delete, delete review, create PR), classifies it as private or publishing, requires confirmation for publishing, and refuses anything else. Make the fakes record every write and every git and gh command, and fail the test on anything not allowed.
3. **Crash-safe posting.** In `post`, read the pending review's comments once per sync. Before each `add_thread` or `add_reply`, look for the comment by path, side, line and body, counting duplicates, and record a match with `mark_posted` instead of posting it. Give `Server` a failure after a comment is created but before the reply arrives, and test that a retry doesn't post twice.
4. **The local-only bucket.**
   - Store version 6: `StoredComment` gains a bucket. Migrated comments are Draft. `to_post` skips LocalOnly.
   - CLI: `review comment` and `review reply` take `--local` or `--draft`, with the spec's defaults. Add `review move <comment> --local|--draft`. `review show --format json` includes the bucket.
   - TUI: show local-only comments distinctly, and add a key to move a comment between buckets.
5. **Confirmation for publishing.** In `LocalBackend::whose`, tell pending comments from submitted ones. For a submitted comment, the TUI asks y/n, naming what will become visible, and `review edit` and `review delete` refuse without `--publish`.
6. **Clean working tree.** Before `up_to_github_head`, refuse if tracked files have uncommitted changes, and name them.
7. **PR creation as its own command.** Move `create_pr` and the push out of `sync` into `review create-pr` and a TUI key, keeping the y/n question. Sync on a branch with no PR refuses and names the command.
8. **Docs and screens.** Agent skill: use local-only for conversations (`--local`), never run sync unless asked, and how to post an agent's review as its own review. Regenerate the diagram from the state machine, and update the help text.

## Invariants

The state machine already checks most of the spec's invariants. Five are missing, two of which guard against publishing by mistake. Numbers are the spec's.

| Spec invariant | `Model::invariants` now | What to do |
| --- | --- | --- |
| 1. No loss | "A comment was lost" | Keep, and extend it to local-only comments |
| 2. No unconfirmed publication | Missing | Add: what any other user can see (`visible(u)`) changes only in Publish actions |
| 3. Sync is complete | "Draft review split across GitHub reviews" | Keep, and check that a successful sync leaves no drafts |
| 4. One to one | "Posted twice"; "a posted comment is still a local draft" | Keep, and include the reply-lost failure |
| 5. Hands off | Missing | Add: no action from a store changes a GitHub comment whose author isn't the store's owner |
| 6. Fetch is safe | "Fetching changed a draft review" (`step_invariants`) | Keep, and extend it to local-only comments |
| 7. Constraints hold | "More than one pending review"; "reply posted before its thread" | Keep |
| 8. Convergence | `drain()` | Keep |
| 9. Local-only stays local | Missing | Add: no comment that was local-only when a sync ran was posted by it |
| 10. First comments kept | "Thread lost its first comment" | Keep |
| 11. Anchors at head | Missing | Add: record the PR's head when each comment is posted, and check that it equals the comment's `written_at` |
| 12. Only review data | Missing | Add to the conformance test: fail on any git or gh command other than sync's fast-forward, with a clean tree, and the push in CreatePR |

## Out of scope

If one of these seems needed, stop and ask Dan rather than building it.

- Anything public beyond the two publishing actions: submitting, conversation comments, resolving threads, and editing the PR.
- Local submit and verdicts.
- Comments on uncommitted changes.
- A second backend. The single write path leaves room for one.
- Background fetching or automatic sync.
- Changes to Viewed checkboxes, and TUI changes beyond what the steps need.
