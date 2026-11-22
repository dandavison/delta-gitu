---
name: delta
description: Review code with a person using delta. Write a review of a branch that the person reads and answers in delta's TUI, and read and answer a review that the person has written or replied to. Use when asked to review a branch, or to address or answer review comments, in a repository where the person uses delta.
---

# Reviewing code with a person, using delta

`delta --tui` is a git interface in the terminal. It can show a branch as a local PR: the
branch's diff against its base branch, with review threads under the lines they are on. The person
opens the current branch's local PR with `O l`.

## Concepts

- **Local PR**: a branch and the base branch it is compared with, in this repository. It is
  stored in the repository's git directory, shared by all its worktrees, and never committed. Use
  the commands below to read and write it; don't read or write the files it is stored in.
- **Author**: this repository has one, the person (git's `user.name`). You write as them, with
  your name as a label on your comments.
- **Review**: one author's comments on a PR. A review is in one of these states:
  - **draft**: in this repository only, and can be changed. There is one, and both your comments
    and the person's go into it.
  - **pending**: the person's draft review on GitHub, which only they can see.
  - **submitted**: on GitHub, for everyone to see. Later it may be **dismissed**.
- **Thread**: a first comment on a line of the diff, and the replies to it.
- **Post**: the person sends the draft review to GitHub, your comments included, where it
  becomes their pending review.
- **Fetch**: copy the reviews on the branch's GitHub PR into the local PR. The copies can't be
  changed locally, except the person's own comments, which are changed on GitHub.

How a review goes: your comments go into the draft review, which only this repository has. The
person reads them in `delta --tui`, replies, and edits or deletes what they don't want. When they
post, the whole draft review, your comments included, becomes their pending review on GitHub,
under their name, which they submit there. For the states and how a review moves between them,
see `delta --tui --pr-state-machine-diagram=d2`.

Rules:

- Pass `--agent <your-name>` to `review comment` and `review reply`. Without it, your comments
  look like the person's.
- Don't post a review to GitHub (`delta --tui review post`) unless the person asks you to. Posting
  pushes the branch and can create a GitHub PR.
- Start each comment with 🤖. Posted comments appear on GitHub under the person's name, where
  only the 🤖 shows that an agent wrote them.
- Edit and delete only your own comments, though nothing stops you changing the person's.

The commands act on the current branch's local PR. Its base is the base of the branch's GitHub
PR, or else the repository's default branch on GitHub, as gh gives them, which needs the network.
Use `--head` and `--base` for another branch, or to give the base without asking gh:

```sh
delta --tui review show --head <branch> --base <branch>
```

## 1. Write a review of a branch

1. See which branch and base the review is of, and any threads it already has:

   ```sh
   delta --tui review show
   ```

   The first line is `# Review of <branch> into <base>`.

2. Read the diff, `git diff <base>...<branch>`, and read each changed file in full.

3. Comment on a line of the diff. The comment is read from stdin:

   ```sh
   delta --tui review comment --agent <your-name> --path <path> --line <line> <<'EOF'
   🤖 This returns before the lock is released.
   EOF
   ```

   `--line` is the line's number in the branch's version of the file. To comment on a removed
   line, add `--side old` and use its number in the base branch's version:

   ```sh
   delta --tui review comment --agent <your-name> --path <path> --line <line> --side old <<'EOF'
   🤖 This check is still needed.
   EOF
   ```

   The line must be in the diff, which includes 3 lines of context around each change. Commenting
   on another line is an error.

4. Say what the review concludes overall in your reply to the person. [NOT YET BUILT: a review's
   overall comment.]

5. Run `delta --tui review show` to check what you wrote, then tell the person the review is
   ready, and that `O l` in `delta --tui` shows it.

## 2. Read and answer a review

1. Read the review:

   ```sh
   delta --tui review show
   ```

   This prints each thread under a heading with its file and line, such as `## src/main.rs:42`,
   or `(old side)` for a removed line. Then come its id (`Thread id: ...`), the lines it is on with
   its line marked `>`, and its comments in order. Each comment starts with its author (or the
   agent's name, for one an agent wrote), whether it is in a draft review, and when it was
   written: `**dan** (draft, 2026-11-22 10:15)`.

   Threads follow their lines as the code changes. A thread whose line has gone says so in its
   heading: `## src/main.rs (outdated: was line 42)`.

   The same, as JSON, with the ids of comments and reviews, each comment's `agent` (the name of
   the agent that wrote it, or `null`), and each review's author and state (draft, pending,
   submitted or dismissed):

   ```sh
   delta --tui review show --format json
   ```

2. For each thread, find the person's comments that no later comment of yours answers. Answer each
   one. If a comment asks for a change, make it and commit it, then reply saying what you changed:

   ```sh
   delta --tui review reply <thread-id> --agent <your-name> <<'EOF'
   🤖 Done in the latest commit: the lock is now released before returning.
   EOF
   ```

   If you disagree, reply saying why, and don't make the change.

3. To delete a comment of yours that is still a draft, find its id in
   `delta --tui review show --format json`, and:

   ```sh
   delta --tui review delete <comment-id>
   ```

   A thread's first comment can't be deleted while the thread has replies: delete the replies
   first. Your comments are those whose `agent` is your name.

   To edit a comment of yours that is still a draft, with its new text on stdin:

   ```sh
   delta --tui review edit <comment-id> <<'EOF'
   🤖 This returns before the lock is released, on the error path.
   EOF
   ```

4. To resolve a thread: [NOT YET BUILT: resolving threads.] Leave resolving to the person.

5. When the branch has a GitHub PR, first copy its reviews into the local PR:

   ```sh
   delta --tui review fetch
   ```

   Then `delta --tui review show` prints them with the local ones, and your replies go into the
   draft review, for the person to read before they post it. Fetch again to see changes made on
   GitHub. Don't edit or delete comments in the copies of reviews on GitHub: those that can be
   changed are the person's, and are changed on GitHub.
