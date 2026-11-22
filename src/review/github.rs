//! PRs on GitHub, read and written with the `gh` command, which holds the user's GitHub login.
//! Comments written in gitu go into the user's pending review, which GitHub shows only to them
//! until they submit it on GitHub.

use super::{ADDED_TO_DRAFT, Backend, Comment, Side, Thread, ViewedState};
use crate::{Res, error::Error};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{collections::HashMap, path::PathBuf, process::Command, rc::Rc};

/// A PR, as GitHub describes it.
#[derive(Clone, Debug)]
pub(crate) struct GitHubPr {
    /// The id GitHub's GraphQL API knows the PR by.
    pub id: String,
    pub number: u64,
    /// The PR's page, `https://github.com/{owner}/{repo}/pull/{number}`.
    pub url: String,
    /// The branch the PR is to be merged into.
    pub base_ref: String,
    /// The commit `base_ref` is at on GitHub. The PR's diff is taken from where
    /// the PR branched from it, as GitHub takes it.
    pub base_oid: String,
    /// The commit the PR's branch is at on GitHub, which the lines of its
    /// review threads are numbered against.
    pub head_oid: String,
}

/// The URL of the file at `path` in the PR's diff on GitHub, at `line` when
/// given.
fn file_url(pr: &GitHubPr, path: &str, line: Option<(Side, u32)>) -> String {
    let hash: String = Sha256::digest(path.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let line = match line {
        Some((Side::Old, n)) => format!("L{n}"),
        Some((Side::New, n)) => format!("R{n}"),
        None => String::new(),
    };
    format!("{}/files#diff-{hash}{line}", pr.url)
}

/// The [`Backend`] of a PR on GitHub.
#[derive(Clone)]
pub(crate) struct GitHubBackend {
    pub github: Rc<dyn GitHub>,
    pub pr: GitHubPr,
}

impl GitHubBackend {
    /// The id of the user's pending review of the PR, started if they have none.
    pub fn pending_review(&self) -> Res<String> {
        match self.github.pending_review(&self.pr)? {
            Some(review) => Ok(review),
            None => self.github.start_review(&self.pr),
        }
    }
}

impl Backend for GitHubBackend {
    /// The PR is checked out, and its diff taken to HEAD.
    fn diff_range(&self) -> String {
        format!("{}...HEAD", self.pr.base_oid)
    }

    fn head_mismatch(&self, repo: &git2::Repository) -> bool {
        let head = repo.head().ok().and_then(|head| head.target());
        head.is_none_or(|oid| oid.to_string() != self.pr.head_oid)
    }

    fn viewed(&self) -> Res<HashMap<String, ViewedState>> {
        self.github.viewed(&self.pr)
    }

    fn set_viewed(&self, path: &str, viewed: bool) -> Res<()> {
        self.github.set_viewed(&self.pr, path, viewed)
    }

    fn threads(&self) -> Res<Vec<Thread>> {
        self.github.threads(&self.pr)
    }

    fn reply(&self, thread_id: &str, body: &str) -> Res<&'static str> {
        let threads = self.github.threads(&self.pr)?;
        let first = threads
            .iter()
            .find(|thread| thread.id == thread_id)
            .and_then(|thread| thread.comments.first())
            .ok_or_else(|| Error::Gh("Couldn't reply: the thread is not on GitHub".to_owned()))?;
        let review = self.pending_review()?;
        self.github.add_reply(&review, &first.id, body)?;
        Ok(ADDED_TO_DRAFT)
    }

    fn comment(&self, path: &str, line: (Side, u32), body: &str) -> Res<&'static str> {
        let review = self.pending_review()?;
        self.github.add_thread(&review, path, line, body)?;
        Ok(ADDED_TO_DRAFT)
    }

    fn edit_comment(&self, _thread_id: &str, comment_id: &str, body: &str) -> Res<&'static str> {
        self.github.edit_comment(comment_id, body)?;
        Ok("Comment updated")
    }

    fn delete_comment(&self, _thread_id: &str, comment_id: &str) -> Res<&'static str> {
        self.github.delete_comment(comment_id)?;
        Ok("Comment deleted")
    }

    fn file_url(&self, path: &str, line: Option<(Side, u32)>) -> Option<String> {
        Some(file_url(&self.pr, path, line))
    }
}

pub(crate) trait GitHub {
    /// The PR `selector` (its number or URL) refers to, or with none, the
    /// current branch's.
    fn pr(&self, selector: Option<&str>) -> Res<GitHubPr>;
    /// The command that checks the PR out.
    fn checkout(&self, pr: &GitHubPr) -> Command;
    /// The checkbox state of each file of the PR, by path.
    fn viewed(&self, pr: &GitHubPr) -> Res<HashMap<String, ViewedState>>;
    /// The PR's review threads.
    fn threads(&self, pr: &GitHubPr) -> Res<Vec<Thread>>;
    /// The PR's reviews as the user sees them: everyone's submitted and dismissed reviews, and
    /// the user's pending review, each with the ids of its comments, which are those of
    /// [`GitHub::threads`].
    fn reviews(&self, pr: &GitHubPr) -> Res<Vec<GitHubReview>>;
    /// The id of the user's pending review of the PR: their draft review, which only they see.
    /// GitHub lets each user have one at a time.
    fn pending_review(&self, pr: &GitHubPr) -> Res<Option<String>>;
    /// Start a pending review of the PR, returning its id.
    fn start_review(&self, pr: &GitHubPr) -> Res<String>;
    /// Add `body` to the pending review `review_id` as a new thread on `line` of the file at
    /// `path`, numbered as in the PR's head commit on GitHub for the new side and its base for
    /// the old, returning the id of the thread's comment.
    fn add_thread(&self, review_id: &str, path: &str, line: (Side, u32), body: &str)
    -> Res<String>;
    /// The open PR whose head is the branch `head`, if there is one.
    fn pr_for_branch(&self, head: &str) -> Res<Option<GitHubPr>>;
    /// The default branch of the repository a PR would be opened against, as `gh pr create`
    /// chooses it.
    fn default_branch(&self) -> Res<String>;
    /// Create a draft PR of the branch `head`, already pushed, into the branch `base`.
    fn create_pr(&self, head: &str, base: &str) -> Res<GitHubPr>;
    /// Add `body` to the pending review `review_id` as a reply to the comment `comment_id`,
    /// which is the first comment of the thread it replies in, returning the reply's id.
    fn add_reply(&self, review_id: &str, comment_id: &str, body: &str) -> Res<String>;
    /// Change what the review comment `comment_id` says to `body`.
    fn edit_comment(&self, comment_id: &str, body: &str) -> Res<()>;
    /// Delete the review comment `comment_id`.
    fn delete_comment(&self, comment_id: &str) -> Res<()>;
    /// Tick or untick the checkbox of the file at `path`.
    fn set_viewed(&self, pr: &GitHubPr, path: &str, viewed: bool) -> Res<()>;
    /// Open `url` in the browser.
    fn open(&self, url: &str) -> Res<()>;
}

/// A review on GitHub, as [`GitHub::reviews`] gives it.
#[derive(Clone, Debug)]
pub(crate) struct GitHubReview {
    pub id: String,
    /// The author's login.
    pub author: String,
    pub state: GitHubReviewState,
    pub comments: Vec<String>,
    /// Whether the user wrote it, and so may edit and delete its comments.
    pub mine: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GitHubReviewState {
    /// Visible only to its author.
    Pending,
    /// Commented, Approved, or Changes requested.
    Submitted,
    Dismissed,
}

/// [`GitHub`] through the `gh` command, run in `workdir`.
pub(crate) struct Gh {
    pub workdir: PathBuf,
}

const VIEWED_QUERY: &str = "query($id: ID!, $endCursor: String) {
  node(id: $id) {
    ... on PullRequest {
      files(first: 100, after: $endCursor) {
        pageInfo { hasNextPage endCursor }
        nodes { path viewerViewedState }
      }
    }
  }
}";

const THREADS_QUERY: &str = "query($id: ID!, $endCursor: String) {
  node(id: $id) {
    ... on PullRequest {
      reviewThreads(first: 100, after: $endCursor) {
        pageInfo { hasNextPage endCursor }
        nodes {
          id isResolved path line diffSide
          comments(first: 100) { nodes { id author { login } body url createdAt state } }
        }
      }
    }
  }
}";

const REVIEWS_QUERY: &str = "query($id: ID!, $endCursor: String) {
  node(id: $id) {
    ... on PullRequest {
      reviews(first: 100, after: $endCursor) {
        pageInfo { hasNextPage endCursor }
        nodes { id state viewerDidAuthor author { login } comments(first: 100) { nodes { id } } }
      }
    }
  }
}";

const PENDING_REVIEW_QUERY: &str = "query($id: ID!) {
  node(id: $id) {
    ... on PullRequest {
      reviews(first: 100, states: [PENDING]) { nodes { id viewerDidAuthor } }
    }
  }
}";

const START_REVIEW: &str = "mutation($id: ID!) {
  addPullRequestReview(input: {pullRequestId: $id}) { pullRequestReview { id } }
}";

const ADD_THREAD: &str =
    "mutation($review: ID!, $path: String!, $line: Int!, $side: DiffSide!, $body: String!) {
  addPullRequestReviewThread(input: {
    pullRequestReviewId: $review, path: $path, line: $line, side: $side, body: $body
  }) { thread { comments(first: 1) { nodes { id } } } }
}";

const ADD_REPLY: &str = "mutation($review: ID!, $comment: ID!, $body: String!) {
  addPullRequestReviewComment(input: {
    pullRequestReviewId: $review, inReplyTo: $comment, body: $body
  }) { comment { id } }
}";

const EDIT_COMMENT: &str = "mutation($id: ID!, $body: String!) {
  updatePullRequestReviewComment(input: {pullRequestReviewCommentId: $id, body: $body}) {
    pullRequestReviewComment { id }
  }
}";

const DELETE_COMMENT: &str = "mutation($id: ID!) {
  deletePullRequestReviewComment(input: {id: $id}) { clientMutationId }
}";

const MARK_VIEWED: &str = "mutation($id: ID!, $path: String!) {
  markFileAsViewed(input: {pullRequestId: $id, path: $path}) { clientMutationId }
}";

const UNMARK_VIEWED: &str = "mutation($id: ID!, $path: String!) {
  unmarkFileAsViewed(input: {pullRequestId: $id, path: $path}) { clientMutationId }
}";

impl Gh {
    /// Run the GraphQL `query` with the string variables `strings` and the typed variables
    /// `typed` (such as numbers), printing what `jq`, if given, picks out of the result.
    fn graphql(
        &self,
        query: &str,
        strings: &[(&str, &str)],
        typed: &[(&str, &str)],
        jq: Option<&str>,
    ) -> Res<String> {
        let mut args = vec!["api".to_owned(), "graphql".to_owned()];
        args.extend(["-f".to_owned(), format!("query={query}")]);
        for (name, value) in strings {
            args.extend(["-f".to_owned(), format!("{name}={value}")]);
        }
        for (name, value) in typed {
            args.extend(["-F".to_owned(), format!("{name}={value}")]);
        }
        if let Some(jq) = jq {
            args.extend(["--jq".to_owned(), jq.to_owned()]);
        }
        self.run(&args.iter().map(String::as_str).collect::<Vec<_>>())
    }

    fn run(&self, args: &[&str]) -> Res<String> {
        let output = Command::new("gh")
            .args(args)
            .current_dir(&self.workdir)
            .output()
            .map_err(|e| Error::Gh(format!("Couldn't run gh: {e}")))?;
        if !output.status.success() {
            return Err(Error::Gh(
                String::from_utf8_lossy(&output.stderr)
                    .trim_end()
                    .to_owned(),
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

impl GitHub for Gh {
    fn pr(&self, selector: Option<&str>) -> Res<GitHubPr> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct View {
            id: String,
            number: u64,
            url: String,
            base_ref_name: String,
            base_ref_oid: String,
            head_ref_oid: String,
        }

        let mut args = vec!["pr", "view"];
        args.extend(selector);
        args.extend(["--json", "id,number,url,baseRefName,baseRefOid,headRefOid"]);
        let view: View = serde_json::from_str(&self.run(&args)?)
            .map_err(|e| Error::Gh(format!("Couldn't read gh's description of the PR: {e}")))?;
        Ok(GitHubPr {
            id: view.id,
            number: view.number,
            url: view.url,
            base_ref: view.base_ref_name,
            base_oid: view.base_ref_oid,
            head_oid: view.head_ref_oid,
        })
    }

    fn checkout(&self, pr: &GitHubPr) -> Command {
        let mut cmd = Command::new("gh");
        cmd.args(["pr", "checkout", &pr.number.to_string()]);
        cmd
    }

    fn viewed(&self, pr: &GitHubPr) -> Res<HashMap<String, ViewedState>> {
        let output = self.run(&[
            "api",
            "graphql",
            "--paginate",
            "-f",
            &format!("query={VIEWED_QUERY}"),
            "-f",
            &format!("id={}", pr.id),
            "--jq",
            ".data.node.files.nodes[] | [.path, .viewerViewedState] | @tsv",
        ])?;
        parse_viewed(&output).map_err(Error::Gh)
    }

    fn threads(&self, pr: &GitHubPr) -> Res<Vec<Thread>> {
        let output = self.run(&[
            "api",
            "graphql",
            "--paginate",
            "-f",
            &format!("query={THREADS_QUERY}"),
            "-f",
            &format!("id={}", pr.id),
        ])?;
        parse_threads(&output)
            .map_err(|e| Error::Gh(format!("Couldn't read gh's review threads: {e}")))
    }

    fn reviews(&self, pr: &GitHubPr) -> Res<Vec<GitHubReview>> {
        let output = self.run(&[
            "api",
            "graphql",
            "--paginate",
            "-f",
            &format!("query={REVIEWS_QUERY}"),
            "-f",
            &format!("id={}", pr.id),
        ])?;
        parse_reviews(&output).map_err(|e| Error::Gh(format!("Couldn't read gh's reviews: {e}")))
    }

    fn pending_review(&self, pr: &GitHubPr) -> Res<Option<String>> {
        let output = self.graphql(
            PENDING_REVIEW_QUERY,
            &[("id", &pr.id)],
            &[],
            Some(".data.node.reviews.nodes[] | select(.viewerDidAuthor) | .id"),
        )?;
        Ok(output.lines().next().map(str::to_owned))
    }

    fn start_review(&self, pr: &GitHubPr) -> Res<String> {
        let output = self.graphql(
            START_REVIEW,
            &[("id", &pr.id)],
            &[],
            Some(".data.addPullRequestReview.pullRequestReview.id"),
        )?;
        Ok(output.trim().to_owned())
    }

    fn add_thread(
        &self,
        review_id: &str,
        path: &str,
        line: (Side, u32),
        body: &str,
    ) -> Res<String> {
        let side = match line.0 {
            Side::Old => "LEFT",
            Side::New => "RIGHT",
        };
        let output = self.graphql(
            ADD_THREAD,
            &[
                ("review", review_id),
                ("path", path),
                ("side", side),
                ("body", body),
            ],
            &[("line", &line.1.to_string())],
            Some(".data.addPullRequestReviewThread.thread.comments.nodes[0].id"),
        )?;
        Ok(output.trim().to_owned())
    }

    fn default_branch(&self) -> Res<String> {
        let output = self.run(&[
            "repo",
            "view",
            "--json",
            "defaultBranchRef",
            "--jq",
            ".defaultBranchRef.name",
        ])?;
        Ok(output.trim().to_owned())
    }

    /// gh's own PR for the branch, as `gh pr view <branch>` finds it, which handles forks.
    fn pr_for_branch(&self, head: &str) -> Res<Option<GitHubPr>> {
        match self.pr(Some(head)) {
            Ok(pr) => Ok(Some(pr)),
            Err(Error::Gh(message)) if message.contains("no pull requests found") => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn create_pr(&self, head: &str, base: &str) -> Res<GitHubPr> {
        self.run(&[
            "pr", "create", "--draft", "--fill", "--head", head, "--base", base,
        ])?;
        self.pr_for_branch(head)?
            .ok_or_else(|| Error::Gh(format!("Couldn't find the PR gh created for {head}")))
    }

    fn add_reply(&self, review_id: &str, comment_id: &str, body: &str) -> Res<String> {
        let output = self.graphql(
            ADD_REPLY,
            &[
                ("review", review_id),
                ("comment", comment_id),
                ("body", body),
            ],
            &[],
            Some(".data.addPullRequestReviewComment.comment.id"),
        )?;
        Ok(output.trim().to_owned())
    }

    fn edit_comment(&self, comment_id: &str, body: &str) -> Res<()> {
        self.graphql(
            EDIT_COMMENT,
            &[("id", comment_id), ("body", body)],
            &[],
            None,
        )?;
        Ok(())
    }

    fn delete_comment(&self, comment_id: &str) -> Res<()> {
        self.graphql(DELETE_COMMENT, &[("id", comment_id)], &[], None)?;
        Ok(())
    }

    fn set_viewed(&self, pr: &GitHubPr, path: &str, viewed: bool) -> Res<()> {
        let mutation = if viewed { MARK_VIEWED } else { UNMARK_VIEWED };
        self.run(&[
            "api",
            "graphql",
            "-f",
            &format!("query={mutation}"),
            "-f",
            &format!("id={}", pr.id),
            "-f",
            &format!("path={path}"),
        ])?;
        Ok(())
    }

    fn open(&self, url: &str) -> Res<()> {
        let mut cmd = if cfg!(target_os = "macos") {
            Command::new("open")
        } else if cfg!(windows) {
            let mut cmd = Command::new("cmd");
            cmd.args(["/C", "start", ""]);
            cmd
        } else {
            Command::new("xdg-open")
        };
        let status = cmd
            .arg(url)
            .status()
            .map_err(|e| Error::Gh(format!("Couldn't open {url}: {e}")))?;
        if !status.success() {
            return Err(Error::Gh(format!("Couldn't open {url}")));
        }
        Ok(())
    }
}

/// The pages of review threads [`Gh::threads`] has gh print: a JSON document
/// a page.
fn parse_threads(output: &str) -> serde_json::Result<Vec<Thread>> {
    #[derive(Deserialize)]
    struct Page {
        data: Data,
    }
    #[derive(Deserialize)]
    struct Data {
        node: Node,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Node {
        review_threads: Nodes<ThreadNode>,
    }
    #[derive(Deserialize)]
    struct Nodes<T> {
        nodes: Vec<T>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct ThreadNode {
        id: String,
        is_resolved: bool,
        path: String,
        line: Option<u32>,
        diff_side: String,
        comments: Nodes<CommentNode>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct CommentNode {
        id: String,
        author: Option<Author>,
        body: String,
        url: String,
        created_at: chrono::DateTime<chrono::FixedOffset>,
        /// PENDING for a comment in the user's pending review.
        state: Option<String>,
    }
    #[derive(Deserialize)]
    struct Author {
        login: String,
    }

    let mut threads = vec![];
    for page in serde_json::Deserializer::from_str(output).into_iter::<Page>() {
        threads.extend(
            page?
                .data
                .node
                .review_threads
                .nodes
                .into_iter()
                .map(|node| {
                    let side = if node.diff_side == "LEFT" {
                        Side::Old
                    } else {
                        Side::New
                    };
                    Thread {
                        id: node.id,
                        path: node.path,
                        line: node.line.map(|line| (side, line)),
                        resolved: node.is_resolved,
                        comments: node
                            .comments
                            .nodes
                            .into_iter()
                            .map(|comment| Comment {
                                id: comment.id,
                                author: comment
                                    .author
                                    .map_or_else(|| "ghost".to_owned(), |author| author.login),
                                body: comment.body,
                                url: Some(comment.url),
                                created_at: comment.created_at,
                                draft: comment.state.as_deref() == Some("PENDING"),
                                editable: comment.state.as_deref() == Some("PENDING"),
                                deletable: comment.state.as_deref() == Some("PENDING"),
                                agent: None,
                            })
                            .collect(),
                    }
                }),
        );
    }
    Ok(threads)
}

/// The pages of reviews [`Gh::reviews`] has gh print: a JSON document a page.
fn parse_reviews(output: &str) -> serde_json::Result<Vec<GitHubReview>> {
    #[derive(Deserialize)]
    struct Page {
        data: Data,
    }
    #[derive(Deserialize)]
    struct Data {
        node: Node,
    }
    #[derive(Deserialize)]
    struct Node {
        reviews: Nodes<ReviewNode>,
    }
    #[derive(Deserialize)]
    struct Nodes<T> {
        nodes: Vec<T>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct ReviewNode {
        id: String,
        state: String,
        viewer_did_author: bool,
        author: Option<Author>,
        comments: Nodes<Id>,
    }
    #[derive(Deserialize)]
    struct Author {
        login: String,
    }
    #[derive(Deserialize)]
    struct Id {
        id: String,
    }

    let mut reviews = vec![];
    for page in serde_json::Deserializer::from_str(output).into_iter::<Page>() {
        for node in page?.data.node.reviews.nodes {
            let state = match node.state.as_str() {
                "PENDING" => GitHubReviewState::Pending,
                "COMMENTED" | "APPROVED" | "CHANGES_REQUESTED" => GitHubReviewState::Submitted,
                "DISMISSED" => GitHubReviewState::Dismissed,
                state => {
                    return Err(serde::de::Error::custom(format!(
                        "review {} is in a state gitu doesn't know: {state}",
                        node.id
                    )));
                }
            };
            reviews.push(GitHubReview {
                id: node.id,
                author: node
                    .author
                    .map_or_else(|| "ghost".to_owned(), |author| author.login),
                state,
                comments: node.comments.nodes.into_iter().map(|c| c.id).collect(),
                mine: node.viewer_did_author,
            });
        }
    }
    Ok(reviews)
}

/// The lines `path<TAB>state` that [`Gh::viewed`] has gh print. A line it can't read is an
/// error.
fn parse_viewed(output: &str) -> Result<HashMap<String, ViewedState>, String> {
    output
        .lines()
        .map(|line| {
            let unknown = || format!("Couldn't read gh's Viewed checkbox: {line:?}");
            let (path, state) = line.rsplit_once('\t').ok_or_else(unknown)?;
            let viewed = match state {
                "VIEWED" => ViewedState::Viewed,
                "UNVIEWED" => ViewedState::Unviewed,
                "DISMISSED" => ViewedState::Dismissed,
                _ => return Err(unknown()),
            };
            Ok((path.to_owned(), viewed))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_threads_reads_every_page() {
        let page = |id: &str, line: &str, side: &str| {
            format!(
                r#"{{"data":{{"node":{{"reviewThreads":{{"pageInfo":{{"hasNextPage":false,"endCursor":null}},"nodes":[{{"id":"{id}","isResolved":false,"path":"a.txt","line":{line},"diffSide":"{side}","comments":{{"nodes":[{{"id":"c1","author":null,"body":"hm","url":"u","createdAt":"2026-11-20T10:00:00Z"}}]}}}}]}}}}}}}}"#
            )
        };
        let output = format!("{}{}", page("t1", "3", "LEFT"), page("t2", "null", "RIGHT"));

        let threads = parse_threads(&output).unwrap();

        assert_eq!(threads.len(), 2);
        assert_eq!(threads[0].line, Some((Side::Old, 3)));
        assert_eq!(threads[0].comments[0].author, "ghost");
        assert_eq!(threads[1].line, None);
    }

    #[test]
    fn parse_threads_reads_a_pending_comment_as_a_draft() {
        let output = r#"{"data":{"node":{"reviewThreads":{"pageInfo":{"hasNextPage":false,"endCursor":null},"nodes":[{"id":"t1","isResolved":false,"path":"a.txt","line":3,"diffSide":"RIGHT","comments":{"nodes":[{"id":"c1","author":{"login":"a"},"body":"x","url":"u","createdAt":"2026-11-20T10:00:00Z","state":"SUBMITTED"},{"id":"c2","author":{"login":"me"},"body":"y","url":"u","createdAt":"2026-11-20T10:00:00Z","state":"PENDING"}]}}]}}}}"#;

        let threads = parse_threads(output).unwrap();

        let drafts: Vec<bool> = threads[0].comments.iter().map(|c| c.draft).collect();
        assert_eq!(drafts, [false, true]);
    }

    #[test]
    fn parse_reviews_reads_each_state_and_every_page() {
        let page = |id: &str, state: &str| {
            format!(
                r#"{{"data":{{"node":{{"reviews":{{"pageInfo":{{"hasNextPage":false,"endCursor":null}},"nodes":[{{"id":"{id}","state":"{state}","viewerDidAuthor":true,"author":{{"login":"a"}},"comments":{{"nodes":[{{"id":"c-{id}"}}]}}}}]}}}}}}}}"#
            )
        };
        let output = [
            page("r1", "PENDING"),
            page("r2", "APPROVED"),
            page("r3", "DISMISSED"),
        ]
        .concat();

        let reviews = parse_reviews(&output).unwrap();

        let states: Vec<_> = reviews.iter().map(|r| r.state).collect();
        assert_eq!(
            states,
            [
                GitHubReviewState::Pending,
                GitHubReviewState::Submitted,
                GitHubReviewState::Dismissed
            ]
        );
        assert_eq!(reviews[1].comments, ["c-r2"]);
    }

    #[test]
    fn parse_reviews_refuses_a_state_it_does_not_know() {
        let output = r#"{"data":{"node":{"reviews":{"pageInfo":{"hasNextPage":false,"endCursor":null},"nodes":[{"id":"r1","state":"PONDERING","viewerDidAuthor":true,"author":{"login":"a"},"comments":{"nodes":[]}}]}}}}"#;

        assert!(parse_reviews(output).is_err());
    }

    #[test]
    fn parse_viewed_refuses_a_line_it_can_not_read() {
        assert!(parse_viewed("a.txt\tVIEWED\nb.txt\tPONDERING\n").is_err());
        assert!(parse_viewed("no tab\n").is_err());
    }

    #[test]
    fn parse_viewed_reads_each_state() {
        let viewed = parse_viewed("a.txt\tVIEWED\nb c.txt\tUNVIEWED\nd.txt\tDISMISSED\n").unwrap();
        assert_eq!(
            viewed,
            HashMap::from([
                ("a.txt".to_owned(), ViewedState::Viewed),
                ("b c.txt".to_owned(), ViewedState::Unviewed),
                ("d.txt".to_owned(), ViewedState::Dismissed),
            ])
        );
    }
}
