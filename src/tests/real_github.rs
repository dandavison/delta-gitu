//! gitu against GitHub itself, in the private repository `dandavison/delta-test`, through
//! gh as gitu runs it. `cargo test` leaves them out, as ignored; `cargo test-github` runs them,
//! which needs the network, gh logged in, and SSH access to the repository. Each test pushes branches of its own and opens a PR between
//! them, which is closed, and the branches deleted, when the test ends.

use super::*;
use crate::review::remote::WriteToken;
use crate::{
    error::Error,
    review::{
        Place, Side,
        github::{Gh, GitHub},
    },
};
use std::{path::PathBuf, process::Command, rc::Rc};

const REPOSITORY: &str = "dandavison/delta-test";

/// A clone of the repository, with a PR of `head` into `base`, both branches of this test's own.
struct RealPr {
    dir: PathBuf,
    base: String,
    head: String,
    number: u64,
}

impl RealPr {
    /// A PR of a new branch, which changes line 5 of `lines.txt` to `line five`, into a new branch
    /// that adds `lines.txt`, of 12 lines `line 1` to `line 12`, to `main`. The PR's diff has
    /// lines 2 to 8 of each side.
    fn new(test_name: &str) -> Self {
        helpers::set_env_vars();
        let name = test_name.rsplit("::").next().unwrap();
        let dir = std::fs::canonicalize(".")
            .unwrap()
            .join("testfiles/real-github")
            .join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.parent().unwrap()).unwrap();
        run(
            dir.parent().unwrap(),
            &[
                "git",
                "clone",
                "-q",
                &format!("git@github.com:{REPOSITORY}.git"),
                name,
            ],
        );
        run(&dir, &["git", "config", "user.name", "CI"]);
        run(&dir, &["git", "config", "user.email", "ci@example.com"]);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = format!("gitu-test/{name}-{nanos}/base");
        let head = format!("gitu-test/{name}-{nanos}/head");
        run(&dir, &["git", "checkout", "-q", "-b", &base, "origin/main"]);
        let lines: String = (1..=12).map(|n| format!("line {n}\n")).collect();
        commit(&dir, "lines.txt", &lines);
        // Commits are made at a fixed time, so without a message of its own the PR's head would be
        // the same commit in every run, and GitHub allows only 100 PRs with one head commit.
        let message = format!("Add lines.txt for {name}-{nanos}");
        run(&dir, &["git", "commit", "-q", "--amend", "-m", &message]);
        run(&dir, &["git", "push", "-q", "-u", "origin", &base]);
        run(&dir, &["git", "checkout", "-q", "-b", &head]);
        commit(&dir, "lines.txt", &lines.replace("line 5\n", "line five\n"));
        run(&dir, &["git", "push", "-q", "-u", "origin", &head]);
        gh(
            &dir,
            &[
                "pr", "create", "--draft", "--title", name, "--body", "", "--head", &head,
                "--base", &base,
            ],
        );
        let number = gh(
            &dir,
            &["pr", "view", &head, "--json", "number", "--jq", ".number"],
        )
        .trim()
        .parse()
        .unwrap();
        RealPr {
            dir,
            base,
            head,
            number,
        }
    }

    fn github(&self) -> Rc<Gh> {
        Rc::new(Gh {
            workdir: self.dir.clone(),
        })
    }

    /// What `delta tui review <args>` prints, with `input` on stdin.
    fn review(&self, args: &[&str], input: &str) -> Result<String, Error> {
        use clap::Parser;
        let args = crate::cli::Args::try_parse_from(["gitu", "review"].iter().chain(args))
            .unwrap_or_else(|e| panic!("{e}"));
        let Some(crate::cli::Commands::Review { command }) = args.command else {
            unreachable!()
        };
        let mut out = vec![];
        crate::review::cli::run(
            &command,
            git2::Repository::open(&self.dir).unwrap(),
            self.github(),
            &mut input.as_bytes(),
            &mut out,
        )?;
        Ok(String::from_utf8(out).unwrap())
    }

    fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.review(&["show", "--format", "json"], "").unwrap()).unwrap()
    }

    /// The bodies of the comments in the user's pending review on GitHub, as GitHub has them.
    fn pending_comments(&self) -> Vec<String> {
        let reviews = gh(
            &self.dir,
            &[
                "api",
                &format!("repos/{REPOSITORY}/pulls/{}/reviews", self.number),
                "--jq",
                ".[] | select(.state == \"PENDING\") | .id",
            ],
        );
        let Some(review) = reviews.lines().next() else {
            return vec![];
        };
        gh(
            &self.dir,
            &[
                "api",
                &format!(
                    "repos/{REPOSITORY}/pulls/{}/reviews/{review}/comments",
                    self.number
                ),
                "--jq",
                ".[].body",
            ],
        )
        .lines()
        .map(str::to_owned)
        .collect()
    }
}

impl Drop for RealPr {
    fn drop(&mut self) {
        let closed = Command::new("gh")
            .args(["pr", "close", &self.number.to_string(), "--delete-branch"])
            .current_dir(&self.dir)
            .output();
        let deleted = Command::new("git")
            .args(["push", "-q", "origin", "--delete", &self.base])
            .current_dir(&self.dir)
            .output();
        for (what, result) in [("close the PR", closed), ("delete the base", deleted)] {
            match result {
                Ok(output) if output.status.success() => {}
                Ok(output) => eprintln!(
                    "Couldn't {what} of {}: {}",
                    self.head,
                    String::from_utf8_lossy(&output.stderr)
                ),
                Err(e) => eprintln!("Couldn't {what} of {}: {e}", self.head),
            }
        }
    }
}

impl RealPr {
    /// GitHub's reply to the GraphQL `query`, with the string variables `strings` and the typed
    /// variables `typed`, which must succeed.
    fn graphql(
        &self,
        query: &str,
        strings: &[(&str, &str)],
        typed: &[(&str, &str)],
    ) -> serde_json::Value {
        let mut args = vec![
            "api".to_owned(),
            "graphql".to_owned(),
            "-f".to_owned(),
            format!("query={query}"),
        ];
        for (name, value) in strings {
            args.extend(["-f".to_owned(), format!("{name}={value}")]);
        }
        for (name, value) in typed {
            args.extend(["-F".to_owned(), format!("{name}={value}")]);
        }
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let reply: serde_json::Value = serde_json::from_str(&gh(&self.dir, &args)).unwrap();
        assert!(reply.get("errors").is_none(), "{reply}");
        reply
    }

    /// The PR's id on GitHub.
    fn node_id(&self) -> String {
        gh(
            &self.dir,
            &["pr", "view", &self.head, "--json", "id", "--jq", ".id"],
        )
        .trim()
        .to_owned()
    }

    /// A review submitted on GitHub, as in the browser, with one thread, whose `place` is the
    /// fields of `addPullRequestReviewThread`'s input that say where it is, as GraphQL writes
    /// them, such as `line: 3, side: RIGHT`. Returns the thread's id on GitHub.
    fn submitted_thread(&self, place: &str, body: &str) -> String {
        let review = self.graphql(
            "mutation($id: ID!) { addPullRequestReview(input: {pullRequestId: $id}) { pullRequestReview { id } } }",
            &[("id", &self.node_id())],
            &[],
        )["data"]["addPullRequestReview"]["pullRequestReview"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let thread = self.graphql(
            &format!(
                "mutation($review: ID!, $body: String!) {{ addPullRequestReviewThread(input: {{pullRequestReviewId: $review, path: \"lines.txt\", body: $body, {place}}}) {{ thread {{ id }} }} }}"
            ),
            &[("review", &review), ("body", body)],
            &[],
        )["data"]["addPullRequestReviewThread"]["thread"]["id"]
            .as_str()
            .unwrap_or_else(|| panic!("GitHub added no thread at {place}"))
            .to_owned();
        self.graphql(
            "mutation($review: ID!) { submitPullRequestReview(input: {pullRequestReviewId: $review, event: COMMENT}) { pullRequestReview { id } } }",
            &[("review", &review)],
            &[],
        );
        thread
    }

    /// Submit my pending review on GitHub, as I would in the browser.
    fn submit_pending_review(&self) {
        let review = self.graphql(
            "query($id: ID!) { node(id: $id) { ... on PullRequest { reviews(states: PENDING, first: 1) { nodes { id } } } } }",
            &[("id", &self.node_id())],
            &[],
        )["data"]["node"]["reviews"]["nodes"][0]["id"]
            .as_str()
            .expect("a pending review")
            .to_owned();
        self.graphql(
            "mutation($review: ID!) { submitPullRequestReview(input: {pullRequestReviewId: $review, event: COMMENT}) { pullRequestReview { id } } }",
            &[("review", &review)],
            &[],
        );
    }

    /// Edit the comment whose local id is `id` on GitHub, as I would in the browser.
    fn edit_on_github(&self, id: &str, body: &str) {
        let comment = id.strip_prefix("github-").expect("a comment from GitHub");
        self.graphql(
            "mutation($comment: ID!, $body: String!) { updatePullRequestReviewComment(input: {pullRequestReviewCommentId: $comment, body: $body}) { pullRequestReviewComment { id } } }",
            &[("comment", comment), ("body", body)],
            &[],
        );
    }

    /// The PR's review threads, as GitHub has them: for each, where it is, and its comments'
    /// bodies.
    fn github_threads(&self) -> Vec<serde_json::Value> {
        self.graphql(
            "query($id: ID!) { node(id: $id) { ... on PullRequest { reviewThreads(first: 100) { nodes { subjectType path line startLine diffSide isOutdated isResolved comments(first: 100) { nodes { body } } } } } } }",
            &[("id", &self.node_id())],
            &[],
        )["data"]["node"]["reviewThreads"]["nodes"]
            .as_array()
            .unwrap()
            .clone()
    }

    /// Delete, on GitHub, the thread `thread`, by deleting its one comment.
    fn delete_thread(&self, thread: &str) {
        let comment = self.graphql(
            "query($id: ID!) { node(id: $id) { ... on PullRequestReviewThread { comments(first: 1) { nodes { id } } } } }",
            &[("id", thread)],
            &[],
        )["data"]["node"]["comments"]["nodes"][0]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        self.graphql(
            "mutation($id: ID!) { deletePullRequestReviewComment(input: {id: $id}) { clientMutationId } }",
            &[("id", &comment)],
            &[],
        );
    }

    /// The commit the PR's head branch is at on GitHub.
    fn remote_head(&self) -> String {
        run(
            &self.dir,
            &[
                "git",
                "ls-remote",
                "origin",
                &format!("refs/heads/{}", self.head),
            ],
        )
        .split_whitespace()
        .next()
        .unwrap()
        .to_owned()
    }

    /// The PR's author pushes a commit to its branch that makes the file `path` `contents`, which
    /// the clone's branch, behind, doesn't have. Returns once GitHub shows the commit as the PR's
    /// head.
    fn author_pushes(&self, path: &str, contents: &str) {
        commit(&self.dir, path, contents);
        run(&self.dir, &["git", "push", "-q", "origin", &self.head]);
        let pushed = run(&self.dir, &["git", "rev-parse", "HEAD"])
            .trim()
            .to_owned();
        run(&self.dir, &["git", "reset", "-q", "--hard", "HEAD~1"]);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        loop {
            let view = gh(
                &self.dir,
                &[
                    "pr",
                    "view",
                    &self.head,
                    "--json",
                    "headRefOid",
                    "--jq",
                    ".headRefOid",
                ],
            );
            if view.trim() == pushed {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "GitHub never had {pushed}"
            );
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    }

    /// The one thread of the local PR, as `review show --format json` gives it.
    fn only_local_thread(&self) -> serde_json::Value {
        let json = self.json();
        let threads = json["threads"].as_array().unwrap();
        assert_eq!(threads.len(), 1, "{json}");
        threads[0].clone()
    }

    /// The id, in the local PR, of its one thread.
    fn only_local_thread_id(&self) -> String {
        self.only_local_thread()["id"].as_str().unwrap().to_owned()
    }
}

/// What `gh <args>` prints, run in `dir`, which must succeed.
fn gh(dir: &std::path::Path, args: &[&str]) -> String {
    let output = Command::new("gh")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "gh {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

/// The comment's thread is added to the user's pending review on GitHub, and the local copy of
/// the review, fetched after posting, has it.
#[test]
#[ignore = "against GitHub itself: run with cargo test-github"]
fn posting_a_comment_adds_it_to_the_pending_review() {
    let pr = RealPr::new(stdext::function_name!());
    pr.review(
        &["comment", "--path", "lines.txt", "--line", "3"],
        "on three",
    )
    .unwrap();

    pr.review(&["sync"], "").unwrap();

    assert_eq!(pr.pending_comments(), ["on three"]);
    let json = pr.json();
    let pending = json["reviews"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["state"] == "pending")
        .unwrap_or_else(|| panic!("{json}"));
    let comment = &json["threads"][0]["comments"][0];
    assert_eq!(comment["body"], "on three", "{json}");
    assert_eq!(comment["review"], pending["id"], "{json}");
}

/// A reply to a thread already on GitHub is added to the same pending review.
#[test]
#[ignore = "against GitHub itself: run with cargo test-github"]
fn posting_a_reply_to_a_posted_thread_adds_it() {
    let pr = RealPr::new(stdext::function_name!());
    pr.review(
        &["comment", "--path", "lines.txt", "--line", "3"],
        "on three",
    )
    .unwrap();
    pr.review(&["sync"], "").unwrap();
    let thread = pr.json()["threads"][0]["id"].as_str().unwrap().to_owned();
    pr.review(&["reply", &thread], "and more").unwrap();

    pr.review(&["sync"], "").unwrap();

    let mut comments = pr.pending_comments();
    comments.sort();
    assert_eq!(comments, ["and more", "on three"]);
}

/// GitHub adds no thread on a line it can't place in the PR's diff, and says so only by
/// replying with no thread: gitu makes that an error.
#[test]
#[ignore = "against GitHub itself: run with cargo test-github"]
fn a_thread_github_does_not_add_is_an_error() {
    let pr = RealPr::new(stdext::function_name!());
    let github = pr.github();
    let github_pr = github.pr(Some(&pr.head)).unwrap();
    let review = github
        .start_review(WriteToken::for_test(), &github_pr)
        .unwrap();

    let result = github.add_thread(
        WriteToken::for_test(),
        &review,
        "lines.txt",
        Place::Line(Side::New, 9999),
        "nowhere",
    );

    let error = result.unwrap_err().to_string();
    assert!(error.contains("GitHub did nothing"), "{error}");
}

/// A PR whose base branch has moved on since the PR was last updated is still the PR, and opens.
#[test]
#[ignore = "against GitHub itself: run with cargo test-github"]
fn a_pr_whose_base_branch_has_moved_on_opens() {
    let pr = RealPr::new(stdext::function_name!());
    run(&pr.dir, &["git", "checkout", "-q", &pr.base]);
    commit(&pr.dir, "elsewhere.txt", "elsewhere\n");
    run(&pr.dir, &["git", "push", "-q", "origin", &pr.base]);
    run(&pr.dir, &["git", "checkout", "-q", &pr.head]);

    let markdown = pr.review(&["show"], "").unwrap();

    assert!(
        markdown.starts_with(&format!("# Review of {} into {}\n", pr.head, pr.base)),
        "{markdown}"
    );
}

/// The draft comments are added to the user's pending review, though they started it elsewhere,
/// as in the browser.
#[test]
#[ignore = "against GitHub itself: run with cargo test-github"]
fn syncing_adds_to_a_pending_review_started_elsewhere() {
    let pr = RealPr::new(stdext::function_name!());
    let github = pr.github();
    let github_pr = github.pr(Some(&pr.head)).unwrap();
    let review = github
        .start_review(WriteToken::for_test(), &github_pr)
        .unwrap();
    github
        .add_thread(
            WriteToken::for_test(),
            &review,
            "lines.txt",
            Place::Line(Side::New, 5),
            "from the browser",
        )
        .unwrap();
    pr.review(
        &["comment", "--path", "lines.txt", "--line", "3"],
        "on three",
    )
    .unwrap();

    pr.review(&["sync"], "").unwrap();

    let mut comments = pr.pending_comments();
    comments.sort();
    assert_eq!(comments, ["from the browser", "on three"]);
}

/// With no draft comments, sync copies GitHub's reviews into the local PR, such as the user's
/// pending review started in the browser.
#[test]
#[ignore = "against GitHub itself: run with cargo test-github"]
fn syncing_with_no_draft_comments_fetches() {
    let pr = RealPr::new(stdext::function_name!());
    let github = pr.github();
    let github_pr = github.pr(Some(&pr.head)).unwrap();
    let review = github
        .start_review(WriteToken::for_test(), &github_pr)
        .unwrap();
    github
        .add_thread(
            WriteToken::for_test(),
            &review,
            "lines.txt",
            Place::Line(Side::New, 5),
            "from the browser",
        )
        .unwrap();

    pr.review(&["sync"], "").unwrap();

    let json = pr.json();
    let comment = &json["threads"][0]["comments"][0];
    assert_eq!(comment["body"], "from the browser", "{json}");
    let pending = json["reviews"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["state"] == "pending")
        .unwrap_or_else(|| panic!("{json}"));
    assert_eq!(comment["review"], pending["id"], "{json}");
}

/// The user's own comment on GitHub is edited, and deleted, on GitHub, from its local copy.
#[test]
#[ignore = "against GitHub itself: run with cargo test-github"]
fn my_comment_on_github_is_edited_and_deleted_there() {
    let pr = RealPr::new(stdext::function_name!());
    pr.review(
        &["comment", "--path", "lines.txt", "--line", "3"],
        "on three",
    )
    .unwrap();
    pr.review(&["sync"], "").unwrap();
    let thread = pr.json()["threads"][0]["id"].as_str().unwrap().to_owned();
    pr.review(&["reply", &thread], "and more").unwrap();
    pr.review(&["sync"], "").unwrap();
    let reply = pr.json()["threads"][0]["comments"][1]["id"]
        .as_str()
        .unwrap()
        .to_owned();

    pr.review(&["edit", &reply], "and more, edited").unwrap();
    let mut comments = pr.pending_comments();
    comments.sort();
    assert_eq!(comments, ["and more, edited", "on three"]);

    let reply = pr.json()["threads"][0]["comments"][1]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    pr.review(&["delete", &reply], "").unwrap();
    assert_eq!(pr.pending_comments(), ["on three"]);
}

/// My comment in my pending review is public once I submit the review on GitHub, though delta
/// hasn't fetched since: editing it without --publish is refused, and it is unchanged there.
#[test]
#[ignore = "against GitHub itself: run with cargo test-github"]
fn editing_my_comment_whose_review_was_submitted_since_the_fetch_needs_publish() {
    let pr = RealPr::new(stdext::function_name!());
    pr.review(
        &["comment", "--path", "lines.txt", "--line", "3"],
        "on three",
    )
    .unwrap();
    pr.review(&["sync"], "").unwrap();
    let comment = pr.only_local_thread()["comments"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    pr.submit_pending_review();

    let error = pr
        .review(&["edit", &comment], "on three, edited")
        .unwrap_err()
        .to_string();

    assert!(error.contains("--publish"), "{error}");
    assert_eq!(
        pr.github_threads()[0]["comments"]["nodes"][0]["body"],
        "on three"
    );
}

/// An edit made on GitHub since delta last fetched is not overwritten from delta's older copy.
#[test]
#[ignore = "against GitHub itself: run with cargo test-github"]
fn editing_my_comment_changed_on_github_since_the_fetch_is_refused() {
    let pr = RealPr::new(stdext::function_name!());
    pr.review(
        &["comment", "--path", "lines.txt", "--line", "3"],
        "on three",
    )
    .unwrap();
    pr.review(&["sync"], "").unwrap();
    let comment = pr.only_local_thread()["comments"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    pr.edit_on_github(&comment, "on three, edited on GitHub");

    let error = pr
        .review(&["edit", &comment], "on three, edited in delta")
        .unwrap_err()
        .to_string();

    assert!(error.contains("changed on GitHub"), "{error}");
    assert_eq!(pr.pending_comments(), ["on three, edited on GitHub"]);
}

/// A reaction to a comment on GitHub is fetched with who reacted.
#[test]
#[ignore = "against GitHub itself: run with cargo test-github"]
fn a_reaction_on_github_is_fetched_with_who_reacted() {
    let pr = RealPr::new(stdext::function_name!());
    let thread = pr.submitted_thread("line: 4, side: RIGHT", "on four");
    let comment = pr.graphql(
        "query($id: ID!) { node(id: $id) { ... on PullRequestReviewThread { comments(first: 1) { nodes { id } } } } }",
        &[("id", &thread)],
        &[],
    )["data"]["node"]["comments"]["nodes"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    pr.graphql(
        "mutation($id: ID!) { addReaction(input: {subjectId: $id, content: HOORAY}) { reaction { content } } }",
        &[("id", &comment)],
        &[],
    );
    let me = gh(&pr.dir, &["api", "user", "--jq", ".login"])
        .trim()
        .to_owned();

    pr.review(&["fetch"], "").unwrap();

    assert_eq!(
        pr.only_local_thread()["comments"][0]["reactions"],
        serde_json::json!([{"emoji": "🎉", "count": 1, "people": [me]}])
    );
}

/// A file's Viewed checkbox is ticked and unticked on GitHub.
#[test]
#[ignore = "against GitHub itself: run with cargo test-github"]
fn a_files_viewed_checkbox_is_set_on_github() {
    let pr = RealPr::new(stdext::function_name!());
    let github = pr.github();
    let github_pr = github.pr(Some(&pr.head)).unwrap();
    let viewed = |github_pr| github.viewed(github_pr).unwrap()["lines.txt"];

    github
        .set_viewed(WriteToken::for_test(), &github_pr, "lines.txt", true)
        .unwrap();
    assert_eq!(viewed(&github_pr), crate::review::ViewedState::Viewed);

    github
        .set_viewed(WriteToken::for_test(), &github_pr, "lines.txt", false)
        .unwrap();
    assert_eq!(viewed(&github_pr), crate::review::ViewedState::Unviewed);
}

/// Posting from a branch with no PR on GitHub creates one only when told to: creating a PR makes
/// the branch visible to everyone who can see the repository.
#[test]
#[ignore = "against GitHub itself: run with cargo test-github"]
fn posting_from_a_branch_with_no_pr_creates_one_only_with_create_pr() {
    let pr = RealPr::new(stdext::function_name!());
    run(
        &pr.dir,
        &["git", "checkout", "-q", "-b", &format!("{}-2", pr.head)],
    );
    commit(&pr.dir, "more.txt", "more\n");
    run(
        &pr.dir,
        &[
            "git",
            "branch",
            "--set-upstream-to",
            &format!("origin/{}", pr.base),
        ],
    );
    let head = format!("{}-2", pr.head);
    let base = ["--base", pr.base.as_str()];
    pr.review(
        &[&["comment", "--path", "more.txt", "--line", "1"][..], &base].concat(),
        "on more",
    )
    .unwrap();

    let error = pr
        .review(&[&["sync"][..], &base].concat(), "")
        .unwrap_err()
        .to_string();
    assert!(error.contains("--create-pr"), "{error}");
    let found = gh(
        &pr.dir,
        &["pr", "list", "--head", &head, "--json", "number"],
    );
    assert_eq!(found.trim(), "[]");

    pr.review(&[&["sync", "--create-pr"][..], &base].concat(), "")
        .unwrap();
    let number = gh(
        &pr.dir,
        &["pr", "view", &head, "--json", "number", "--jq", ".number"],
    );
    gh(&pr.dir, &["pr", "close", number.trim(), "--delete-branch"]);
}

// Round trips: what is written here comes back, once synced, as it was written, and what is
// written on GitHub comes back here as it is there, and takes replies.

/// A thread written here comes back from GitHub on the same line, as the same local thread.
#[test]
#[ignore = "against GitHub itself: run with cargo test-github"]
fn a_thread_synced_comes_back_on_its_line_as_the_same_thread() {
    let pr = RealPr::new(stdext::function_name!());
    pr.review(
        &["comment", "--path", "lines.txt", "--line", "3"],
        "on three",
    )
    .unwrap();
    let id = pr.only_local_thread_id();

    pr.review(&["sync"], "").unwrap();

    let thread = pr.only_local_thread();
    assert_eq!(thread["id"], id.as_str(), "{thread}");
    assert_eq!(thread["path"], "lines.txt", "{thread}");
    assert_eq!(thread["side"], "new", "{thread}");
    assert_eq!(thread["line"], 3, "{thread}");
    assert_eq!(thread["subject"], "line", "{thread}");
    assert_eq!(thread["comments"][0]["body"], "on three", "{thread}");
    let github = pr.github_threads();
    assert_eq!(github[0]["line"], 3, "{github:?}");
    assert_eq!(github[0]["diffSide"], "RIGHT", "{github:?}");
}

/// A thread on a removed line goes to GitHub on the old side, and comes back there.
#[test]
#[ignore = "against GitHub itself: run with cargo test-github"]
fn a_thread_on_a_removed_line_comes_back_on_the_old_side() {
    let pr = RealPr::new(stdext::function_name!());
    pr.review(
        &[
            "comment",
            "--path",
            "lines.txt",
            "--line",
            "5",
            "--side",
            "old",
        ],
        "was five",
    )
    .unwrap();

    pr.review(&["sync"], "").unwrap();

    let github = pr.github_threads();
    assert_eq!(github[0]["line"], 5, "{github:?}");
    assert_eq!(github[0]["diffSide"], "LEFT", "{github:?}");
    let thread = pr.only_local_thread();
    assert_eq!(thread["side"], "old", "{thread}");
    assert_eq!(thread["line"], 5, "{thread}");
}

/// An agent's comment reaches GitHub as it was written, under the user's name: its agent's name
/// is a label kept only here.
#[test]
#[ignore = "against GitHub itself: run with cargo test-github"]
fn an_agents_comment_reaches_github_as_written() {
    let pr = RealPr::new(stdext::function_name!());
    pr.review(
        &[
            "comment",
            "--path",
            "lines.txt",
            "--line",
            "3",
            "--agent",
            "claude",
        ],
        "🤖 on three",
    )
    .unwrap();

    pr.review(&["sync"], "").unwrap();

    assert_eq!(pr.pending_comments(), ["🤖 on three"]);
    let comment = &pr.only_local_thread()["comments"][0];
    assert_eq!(comment["body"], "🤖 on three", "{comment}");
    assert_eq!(comment["agent"], serde_json::Value::Null, "{comment}");
}

/// A thread written on GitHub shows here on its line and side, and a reply written here goes
/// into it on GitHub.
#[test]
#[ignore = "against GitHub itself: run with cargo test-github"]
fn a_thread_from_github_shows_on_its_line_and_takes_a_reply() {
    let pr = RealPr::new(stdext::function_name!());
    pr.submitted_thread("line: 4, side: RIGHT", "on four");

    pr.review(&["sync"], "").unwrap();
    let thread = pr.only_local_thread();
    assert_eq!(thread["line"], 4, "{thread}");
    assert_eq!(thread["side"], "new", "{thread}");
    assert_eq!(thread["subject"], "line", "{thread}");
    let id = thread["id"].as_str().unwrap().to_owned();
    pr.review(&["reply", &id], "and more").unwrap();
    pr.review(&["sync"], "").unwrap();

    let github = pr.github_threads();
    assert_eq!(github.len(), 1, "{github:?}");
    let bodies: Vec<&str> = github[0]["comments"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["body"].as_str().unwrap())
        .collect();
    assert_eq!(bodies, ["on four", "and more"]);
    let thread = pr.only_local_thread();
    assert_eq!(thread["id"], id.as_str(), "{thread}");
    assert_eq!(thread["comments"].as_array().unwrap().len(), 2, "{thread}");
}

/// A thread resolved on GitHub is resolved here.
#[test]
#[ignore = "against GitHub itself: run with cargo test-github"]
fn a_thread_resolved_on_github_is_resolved_here() {
    let pr = RealPr::new(stdext::function_name!());
    let thread = pr.submitted_thread("line: 4, side: RIGHT", "on four");
    pr.graphql(
        "mutation($id: ID!) { resolveReviewThread(input: {threadId: $id}) { thread { id } } }",
        &[("id", &thread)],
        &[],
    );

    pr.review(&["sync"], "").unwrap();

    assert_eq!(pr.only_local_thread()["resolved"], true);
}

/// A thread whose line GitHub has as outdated, once the line changed, is outdated here: on no
/// line, and not a file comment.
#[test]
#[ignore = "against GitHub itself: run with cargo test-github"]
fn a_thread_outdated_on_github_is_outdated_here() {
    let pr = RealPr::new(stdext::function_name!());
    pr.submitted_thread("line: 5, side: RIGHT", "on five");
    let lines = std::fs::read_to_string(pr.dir.join("lines.txt")).unwrap();
    commit(
        &pr.dir,
        "lines.txt",
        &lines.replace("line five\n", "line FIVE\n"),
    );
    run(&pr.dir, &["git", "push", "-q", "origin", &pr.head]);
    // GitHub works out what is outdated after a push, not at once.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while pr.github_threads()[0]["isOutdated"] != true {
        assert!(
            std::time::Instant::now() < deadline,
            "GitHub never had the thread outdated"
        );
        std::thread::sleep(std::time::Duration::from_secs(2));
    }

    pr.review(&["sync"], "").unwrap();

    let thread = pr.only_local_thread();
    assert_eq!(thread["line"], serde_json::Value::Null, "{thread}");
    assert_eq!(thread["subject"], "line", "{thread}");
}

/// A comment on a whole file on GitHub shows here under the file, as a file comment, and takes a
/// reply written here.
#[test]
#[ignore = "against GitHub itself: run with cargo test-github"]
fn a_file_comment_from_github_shows_as_one_and_takes_a_reply() {
    let pr = RealPr::new(stdext::function_name!());
    pr.submitted_thread("subjectType: FILE", "on the file");

    pr.review(&["sync"], "").unwrap();
    let thread = pr.only_local_thread();
    assert_eq!(thread["subject"], "file", "{thread}");
    assert_eq!(thread["line"], serde_json::Value::Null, "{thread}");
    let markdown = pr.review(&["show"], "").unwrap();
    assert!(markdown.contains("\n## lines.txt (file)\n"), "{markdown}");
    let id = thread["id"].as_str().unwrap().to_owned();
    pr.review(&["reply", &id], "and more").unwrap();
    pr.review(&["sync"], "").unwrap();

    let github = pr.github_threads();
    assert_eq!(github[0]["subjectType"], "FILE", "{github:?}");
    let bodies: Vec<&str> = github[0]["comments"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["body"].as_str().unwrap())
        .collect();
    assert_eq!(bodies, ["on the file", "and more"]);
}

/// A comment on several lines on GitHub shows here at its last line, and the local PR records
/// its first.
#[test]
#[ignore = "against GitHub itself: run with cargo test-github"]
fn a_comment_on_several_lines_shows_at_its_last_with_its_first_recorded() {
    let pr = RealPr::new(stdext::function_name!());
    pr.submitted_thread(
        "startLine: 3, startSide: RIGHT, line: 6, side: RIGHT",
        "on three to six",
    );

    pr.review(&["sync"], "").unwrap();

    let thread = pr.only_local_thread();
    assert_eq!(thread["line"], 6, "{thread}");
    assert_eq!(thread["start_line"], 3, "{thread}");
    let markdown = pr.review(&["show"], "").unwrap();
    assert!(markdown.contains("\n## lines.txt:3-6\n"), "{markdown}");
}

/// A draft thread whose line has gone since it was written, as when the PR's author has changed
/// it, is posted as a comment on its file, saying which line it was on, and comes back as one.
#[test]
#[ignore = "against GitHub itself: run with cargo test-github"]
fn a_draft_thread_whose_line_has_gone_is_posted_as_a_file_comment() {
    let pr = RealPr::new(stdext::function_name!());
    pr.review(
        &["comment", "--path", "lines.txt", "--line", "5"],
        "on five",
    )
    .unwrap();
    // The author changes line 5 on GitHub.
    let lines = std::fs::read_to_string(pr.dir.join("lines.txt")).unwrap();
    pr.author_pushes("lines.txt", &lines.replace("line five\n", "line FIVE\n"));

    // The PR changed since the draft was written: nothing is posted, and the draft, whose line
    // has gone, is now on its file, for the user to see.
    let error = pr.review(&["sync"], "").unwrap_err().to_string();
    assert!(error.contains("changed since"), "{error}");
    assert!(pr.pending_comments().is_empty());
    assert_eq!(pr.only_local_thread()["subject"], "file");

    pr.review(&["sync"], "").unwrap();

    let github = pr.github_threads();
    assert_eq!(github.len(), 1, "{github:?}");
    assert_eq!(github[0]["subjectType"], "FILE", "{github:?}");
    assert_eq!(
        github[0]["comments"]["nodes"][0]["body"], "On line 5: `line five`\n\non five",
        "{github:?}"
    );
    assert_eq!(pr.only_local_thread()["subject"], "file");
}

/// The lease: a draft written before the PR changed on GitHub is not posted at the next sync,
/// which moves it with its line, for the user to see where it now is; the sync after posts it
/// there.
#[test]
#[ignore = "against GitHub itself: run with cargo test-github"]
fn a_draft_written_before_the_pr_changed_is_moved_and_posted_only_at_the_next_sync() {
    let pr = RealPr::new(stdext::function_name!());
    pr.review(
        &["comment", "--path", "lines.txt", "--line", "3"],
        "on three",
    )
    .unwrap();
    // The author adds a line above it on GitHub.
    let lines = std::fs::read_to_string(pr.dir.join("lines.txt")).unwrap();
    pr.author_pushes("lines.txt", &format!("line 0\n{lines}"));

    let error = pr.review(&["sync"], "").unwrap_err().to_string();

    assert!(error.contains("changed since"), "{error}");
    assert!(pr.pending_comments().is_empty());
    assert_eq!(pr.only_local_thread()["line"], 4);

    pr.review(&["sync"], "").unwrap();

    let github = pr.github_threads();
    assert_eq!(github[0]["line"], 4, "{github:?}");
    assert_eq!(
        github[0]["comments"]["nodes"][0]["body"], "on three",
        "{github:?}"
    );
}

// Sync deals with comments, never with the PR's code: it doesn't push.

/// A branch with commits GitHub's PR doesn't have is not pushed by sync, which posts nothing and
/// says to push them first.
#[test]
#[ignore = "against GitHub itself: run with cargo test-github"]
fn syncing_with_commits_github_doesnt_have_refuses_and_pushes_nothing() {
    let pr = RealPr::new(stdext::function_name!());
    let before = pr.remote_head();
    pr.review(
        &["comment", "--path", "lines.txt", "--line", "3"],
        "on three",
    )
    .unwrap();
    commit(&pr.dir, "more.txt", "more\n");

    let error = pr.review(&["sync"], "").unwrap_err().to_string();

    assert!(error.contains("push them first"), "{error}");
    assert_eq!(pr.remote_head(), before);
    assert!(pr.pending_comments().is_empty());
}

/// A branch behind GitHub's PR, as when its author has pushed since, is brought up to it by sync.
/// Untracked files don't stop it.
#[test]
#[ignore = "against GitHub itself: run with cargo test-github"]
fn syncing_fast_forwards_a_branch_behind_githubs_head() {
    let pr = RealPr::new(stdext::function_name!());
    pr.author_pushes("more.txt", "more\n");
    std::fs::write(pr.dir.join("untracked.txt"), "untracked\n").unwrap();

    pr.review(&["sync"], "").unwrap();

    assert_eq!(
        run(&pr.dir, &["git", "rev-parse", "HEAD"]).trim(),
        pr.remote_head()
    );
}

/// Right after the PR's author pushes, the PR opens: GitHub's head for it and how many commits it
/// has are read as of one moment, so that they agree.
#[test]
#[ignore = "against GitHub itself: run with cargo test-github"]
fn the_pr_opens_right_after_its_author_pushes() {
    let pr = RealPr::new(stdext::function_name!());
    commit(&pr.dir, "more.txt", "more\n");
    run(&pr.dir, &["git", "push", "-q", "origin", &pr.head]);
    run(&pr.dir, &["git", "reset", "-q", "--hard", "HEAD~1"]);

    for _ in 0..5 {
        pr.review(&["show"], "").unwrap();
    }
}

/// A branch behind GitHub's PR is not brought up to it while tracked files have uncommitted
/// changes, which the incoming commits would be mixed into: sync says which files they are, and
/// leaves the branch and the changes as they were.
#[test]
#[ignore = "against GitHub itself: run with cargo test-github"]
fn syncing_a_branch_behind_githubs_head_with_uncommitted_changes_refuses() {
    let pr = RealPr::new(stdext::function_name!());
    pr.author_pushes("more.txt", "more\n");
    let before = run(&pr.dir, &["git", "rev-parse", "HEAD"]);
    std::fs::write(pr.dir.join("lines.txt"), "uncommitted\n").unwrap();

    let error = pr.review(&["sync"], "").unwrap_err().to_string();

    assert!(error.contains("uncommitted changes"), "{error}");
    assert!(error.contains("lines.txt"), "{error}");
    assert_eq!(run(&pr.dir, &["git", "rev-parse", "HEAD"]), before);
    assert_eq!(
        std::fs::read_to_string(pr.dir.join("lines.txt")).unwrap(),
        "uncommitted\n"
    );
    // So that closing the PR can switch branches.
    run(&pr.dir, &["git", "checkout", "--", "lines.txt"]);
}

/// A draft reply whose thread was deleted on GitHub can't be posted: sync posts nothing, and says
/// which reply it is. The reply is kept, in its thread, which says it was deleted on GitHub.
#[test]
#[ignore = "against GitHub itself: run with cargo test-github"]
fn a_draft_reply_whose_thread_was_deleted_on_github_is_refused_and_kept() {
    let pr = RealPr::new(stdext::function_name!());
    let thread = pr.submitted_thread("line: 4, side: RIGHT", "on four");
    pr.review(&["sync"], "").unwrap();
    let local = pr.only_local_thread_id();
    pr.review(&["reply", &local], "and more").unwrap();
    pr.review(
        &["comment", "--path", "lines.txt", "--line", "3"],
        "on three",
    )
    .unwrap();
    pr.delete_thread(&thread);

    let error = pr.review(&["sync"], "").unwrap_err().to_string();

    assert!(error.contains("deleted on GitHub"), "{error}");
    assert!(error.contains("and more"), "{error}");
    assert!(pr.pending_comments().is_empty());
    let json = pr.json();
    let orphan = json["threads"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == local.as_str())
        .unwrap_or_else(|| panic!("{json}"));
    assert_eq!(orphan["deleted_on_github"], true, "{json}");
    assert_eq!(orphan["comments"][0]["body"], "and more", "{json}");
}

/// My comment in a submitted review is public: the review command line changes it on GitHub only
/// when given --publish.
#[test]
#[ignore = "against GitHub itself: run with cargo test-github"]
fn editing_my_comment_in_a_submitted_review_needs_publish() {
    let pr = RealPr::new(stdext::function_name!());
    pr.submitted_thread("line: 4, side: RIGHT", "on four");
    pr.review(&["sync"], "").unwrap();
    let comment = pr.only_local_thread()["comments"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let error = pr
        .review(&["edit", &comment], "on four, edited")
        .unwrap_err()
        .to_string();
    assert!(error.contains("--publish"), "{error}");
    let github = pr.github_threads();
    assert_eq!(
        github[0]["comments"]["nodes"][0]["body"], "on four",
        "{github:?}"
    );

    pr.review(&["edit", &comment, "--publish"], "on four, edited")
        .unwrap();
    let github = pr.github_threads();
    assert_eq!(
        github[0]["comments"]["nodes"][0]["body"], "on four, edited",
        "{github:?}"
    );
}
