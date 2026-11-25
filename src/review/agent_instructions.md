---
name: delta
description: Review code with a person using delta. Write a review of a branch that the person reads and answers in delta's TUI, and read and answer a review that the person has written or replied to. Use when asked to review a branch, or to address or answer review comments, in a repository where the person uses delta.
---

These instructions are the output of `delta --agent-instructions`: run it again after updating delta.

# Reviewing code with a person, using delta

`delta tui` is a git interface in the terminal. It can show a branch as a local PR: the
branch's diff against its base branch, with review threads under the lines they are on. The person
opens the current branch's PR with `O O`.

## Concepts

- **Local PR**: a branch and the base branch it is compared with, in this repository. It is
  stored in the repository's git directory, shared by all its worktrees, and never committed. Use
  the commands below to read and write it; don't read or write the files it is stored in.
- **Author**: this repository has one, the person (git's `user.name`). You write as them, with
  your name as a label on your comments.
- **Review**: one author's comments on a PR. A review is in one of these states:
  - **draft**: in this repository only, and can be changed. There is one, and both your comments
    and the person's go into it, unless they are local-only.
  - **pending**: the person's draft review on GitHub, which only they can see.
  - **submitted**: on GitHub, for everyone to see. Later it may be **dismissed**.
- **Local-only comment**: a comment in no review, which is never posted to GitHub. Your
  conversations with the person are local-only.
- **Thread**: a first comment on a line of the diff, and the replies to it.
- **Sync**: the person's command to bring the branch up to its GitHub PR's head, add the draft
  review's comments, yours included, to their pending review on GitHub, and copy the reviews on
  GitHub into the local PR. Local-only comments are never posted.
- **Fetch**: copy the reviews on the branch's GitHub PR into the local PR, which sync also does.
  The copies can't be changed locally, except the person's own comments, which are changed on
  GitHub.

How a review goes: your review comments go into the draft review, which only this repository has.
The person reads them in `delta tui`, discusses them with you in local-only replies, and edits
or deletes what they don't want. When they sync, the draft review's comments, yours included, go
into their pending review on GitHub, under their name, which they submit there. For the states and
how a review moves between them, see `delta tui --pr-state-machine-diagram=d2`.

Rules:

- Pass `--agent <your-name>` to `review comment` and `review reply`. Without it, your comments
  look like the person's.
- Write your side of a conversation with the person as local-only: pass `--local` to `review
  comment` and `review reply`. Use the draft review only for comments meant for GitHub, such as a
  review of the branch. A reply is local-only by default when its thread's first comment is.
- Never sync (`delta tui review sync`) unless the person asks you to. Sync posts the draft
  comments to the person's pending review on GitHub, and can fast-forward the branch. On a branch
  with no GitHub PR it needs `--create-pr`, which pushes the branch and creates a draft PR that
  everyone who can see the repository can see: give it only if the person asks for a PR.
- Start each comment with 🤖. Posted comments appear on GitHub under the person's name, where
  only the 🤖 shows that an agent wrote them.
- Edit, delete and move only your own comments, unless the person asks you to, though nothing
  stops you changing theirs.

The commands act on the current branch's local PR. Its base is the base of the branch's GitHub
PR, or else the repository's default branch on GitHub, as gh gives them, which needs the network.
Use `--head` and `--base` for another branch, or to give the base without asking gh:

```sh
delta tui review show --head <branch> --base <branch>
```

## 1. Write a review of a branch

1. See which branch and base the review is of, and any threads it already has:

   ```sh
   delta tui review show
   ```

   The first line is `# Review of <branch> into <base>`.

2. Read the diff, `git diff <base>...<branch>`, and read each changed file in full.

3. Comment on a line of the diff. The comment is read from stdin:

   ```sh
   delta tui review comment --agent <your-name> --path <path> --line <line> <<'EOF'
   🤖 This returns before the lock is released.
   EOF
   ```

   `--line` is the line's number in the branch's version of the file. To comment on a removed
   line, add `--side old` and use its number in the base branch's version:

   ```sh
   delta tui review comment --agent <your-name> --path <path> --line <line> --side old <<'EOF'
   🤖 This check is still needed.
   EOF
   ```

   The line must be in the diff, which includes 3 lines of context around each change. Commenting
   on another line is an error.

4. Say what the review concludes overall in your reply to the person. [NOT YET BUILT: a review's
   overall comment.]

5. Run `delta tui review show` to check what you wrote, then tell the person the review is
   ready, and that `O O` in `delta tui` shows it.

## 2. Read and answer a review

1. Read the review:

   ```sh
   delta tui review show
   ```

   This prints each thread under a heading with its file and line, such as `## src/main.rs:42`,
   or `(old side)` for a removed line. Then come its id (`Thread id: ...`), the lines it is on with
   its line marked `>`, and its comments in order. Each comment starts with its author (or the
   agent's name, for one an agent wrote), whether it is in the draft review or local-only, and
   when it was written: `**dan** (draft, 2026-11-22 10:15)`.

   Threads follow their lines as the code changes. A thread whose line has gone says so in its
   heading: `## src/main.rs (outdated: was line 42)`.

   The same, as JSON, with the ids of comments and reviews, each comment's `agent` (the name of
   the agent that wrote it, or `null`) and `local_only`, and each review's author and state
   (draft, pending, submitted or dismissed):

   ```sh
   delta tui review show --format json
   ```

2. For each thread, find the person's comments that no later comment of yours answers. Answer each
   one, local-only. If a comment asks for a change, make it and commit it, then reply saying what
   you changed:

   ```sh
   delta tui review reply <thread-id> --agent <your-name> --local <<'EOF'
   🤖 Done in the latest commit: the lock is now released before returning.
   EOF
   ```

   If you disagree, reply saying why, and don't make the change.

3. To delete a comment of yours that is still local, in the draft review or local-only, find its
   id in `delta tui review show --format json`, and:

   ```sh
   delta tui review delete <comment-id>
   ```

   A thread's first comment can't be deleted while the thread has replies: delete the replies
   first. Your comments are those whose `agent` is your name.

   To edit a comment of yours that is still local, with its new text on stdin:

   ```sh
   delta tui review edit <comment-id> <<'EOF'
   🤖 This returns before the lock is released, on the error path.
   EOF
   ```

   To move a comment of yours between the draft review and local-only:

   ```sh
   delta tui review move <comment-id> --local
   delta tui review move <comment-id> --draft
   ```

   A reply in the draft review whose thread's first comment is local-only stops the person's
   sync: move one or the other.

4. To resolve a thread: [NOT YET BUILT: resolving threads.] Leave resolving to the person.

5. When the branch has a GitHub PR, first copy its reviews into the local PR:

   ```sh
   delta tui review fetch
   ```

   Then `delta tui review show` prints them with the local ones. Fetch again to see changes
   made on GitHub. Don't edit or delete comments in the copies of reviews on GitHub: those that
   can be changed are the person's, and are changed on GitHub.

## 3. Listen for messages from the person

The person can send you messages from a PR in `delta tui`, with `@`: about a thread, or about the
whole PR. When they ask you to listen to delta, run this in the background, in the branch's
worktree:

```sh
delta tui review listen
```

It waits for messages to the current branch's PR, prints them, and exits. Each message starts
with `# Message from <person>`, followed by `about thread <thread-id>` if it is about a thread.
Do what each asks, and answer one about a thread in that thread, local-only, as in "Read and
answer a review" above. Then run `delta tui review listen` again. A message sent while you work is kept, and the next listen prints
it at once. Each message is printed only once, so run one listen at a time.

## 4. Post your review as a review of its own

Sync posts the whole draft review, the person's comments with yours, into one pending review. When
the person asks for your review to be posted on its own, do this, syncing only because they asked:

1. Move each of the person's comments in the draft review to local-only, with `review move
   <comment-id> --local`. Their comments are those whose `agent` is `null`.
2. Sync:

   ```sh
   delta tui review sync
   ```

3. Tell the person to submit the pending review on GitHub, with a body such as "Review by an
   agent".
4. Once they have, move their comments back with `review move <comment-id> --draft`.
