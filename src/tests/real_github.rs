//! gitu against GitHub itself, in the private repository `dandavison/gitu-review-test`, through
//! gh as gitu runs it. `cargo test` leaves them out, as ignored; `cargo test-github` runs them,
//! which needs the network, gh logged in, and SSH access to the repository. Each test pushes branches of its own and opens a PR between
//! them, which is closed, and the branches deleted, when the test ends.

use super::*;
use crate::{
    error::Error,
    review::{
        Side,
        github::{Gh, GitHub},
    },
};
use std::{path::PathBuf, process::Command, rc::Rc};

const REPOSITORY: &str = "dandavison/gitu-review-test";

/// A clone of the repository, with a PR of `head` into `base`, both branches of this test's own.
struct RealPr {
    dir: PathBuf,
    base: String,
    head: String,
    number: u64,
}

impl RealPr {
    /// A PR of a new branch, which adds `lines.txt` with 12 lines, into a new branch at `main`.
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
        run(&dir, &["git", "push", "-q", "-u", "origin", &base]);
        run(&dir, &["git", "checkout", "-q", "-b", &head]);
        let lines: String = (1..=12).map(|n| format!("line {n}\n")).collect();
        commit(&dir, "lines.txt", &lines);
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

    /// What `delta --tui review <args>` prints, with `input` on stdin.
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

    pr.review(&["post"], "").unwrap();

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
    pr.review(&["post"], "").unwrap();
    let thread = pr.json()["threads"][0]["id"].as_str().unwrap().to_owned();
    pr.review(&["reply", &thread], "and more").unwrap();

    pr.review(&["post"], "").unwrap();

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
    let review = github.start_review(&github_pr).unwrap();

    let result = github.add_thread(&review, "lines.txt", (Side::New, 9999), "nowhere");

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

/// A pending review the user started elsewhere, as in the browser, stops posting, which says
/// where it is and what it has.
#[test]
#[ignore = "against GitHub itself: run with cargo test-github"]
fn posting_while_a_pending_review_from_elsewhere_has_comments_is_refused() {
    let pr = RealPr::new(stdext::function_name!());
    let github = pr.github();
    let github_pr = github.pr(Some(&pr.head)).unwrap();
    let review = github.start_review(&github_pr).unwrap();
    github
        .add_thread(&review, "lines.txt", (Side::New, 5), "from the browser")
        .unwrap();
    pr.review(
        &["comment", "--path", "lines.txt", "--line", "3"],
        "on three",
    )
    .unwrap();

    let error = pr.review(&["post"], "").unwrap_err().to_string();

    assert!(
        error.contains(&format!("github.com/{REPOSITORY}/pull/{}", pr.number)),
        "{error}"
    );
    assert!(error.contains("with 1 comment"), "{error}");
    assert_eq!(pr.pending_comments(), ["from the browser"]);
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
    pr.review(&["post"], "").unwrap();
    let thread = pr.json()["threads"][0]["id"].as_str().unwrap().to_owned();
    pr.review(&["reply", &thread], "and more").unwrap();
    pr.review(&["post"], "").unwrap();
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

/// A file's Viewed checkbox is ticked and unticked on GitHub.
#[test]
#[ignore = "against GitHub itself: run with cargo test-github"]
fn a_files_viewed_checkbox_is_set_on_github() {
    let pr = RealPr::new(stdext::function_name!());
    let github = pr.github();
    let github_pr = github.pr(Some(&pr.head)).unwrap();
    let viewed = |github_pr| github.viewed(github_pr).unwrap()["lines.txt"];

    github.set_viewed(&github_pr, "lines.txt", true).unwrap();
    assert_eq!(viewed(&github_pr), crate::review::ViewedState::Viewed);

    github.set_viewed(&github_pr, "lines.txt", false).unwrap();
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
        .review(&[&["post"][..], &base].concat(), "")
        .unwrap_err()
        .to_string();
    assert!(error.contains("--create-pr"), "{error}");
    let found = gh(
        &pr.dir,
        &["pr", "list", "--head", &head, "--json", "number"],
    );
    assert_eq!(found.trim(), "[]");

    pr.review(&[&["post", "--create-pr"][..], &base].concat(), "")
        .unwrap();
    let number = gh(
        &pr.dir,
        &["pr", "view", &head, "--json", "number", "--jq", ".number"],
    );
    gh(&pr.dir, &["pr", "close", number.trim(), "--delete-branch"]);
}
