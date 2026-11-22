//! Pull requests on GitHub, read and written with the `gh` command, which holds
//! the user's GitHub login.

use crate::{Res, error::Error};
use serde::Deserialize;
use std::{cell::RefCell, collections::HashMap, path::PathBuf, process::Command};

/// A pull request, as GitHub describes it.
#[derive(Clone, Debug)]
pub(crate) struct Pr {
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
}

/// The "Viewed" checkbox GitHub shows on each file of a PR. Each reviewer has
/// their own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Viewed {
    Viewed,
    Unviewed,
    /// Marked viewed, but the file has changed since.
    Dismissed,
}

/// A PR checked out for review, with the checkbox state of each of its files,
/// by path.
pub(crate) struct Review {
    pub pr: Pr,
    pub viewed: RefCell<HashMap<String, Viewed>>,
}

pub(crate) trait GitHub {
    /// The PR `selector` (its number or URL) refers to, or with none, the
    /// current branch's.
    fn pr(&self, selector: Option<&str>) -> Res<Pr>;
    /// The command that checks the PR out.
    fn checkout(&self, pr: &Pr) -> Command;
    /// The command that fetches the PR's base branch, for when its commit is
    /// not in the repository.
    fn fetch_base(&self, pr: &Pr) -> Res<Command>;
    /// The checkbox state of each file of the PR, by path.
    fn viewed(&self, pr: &Pr) -> Res<HashMap<String, Viewed>>;
    /// Tick or untick the checkbox of the file at `path`.
    fn set_viewed(&self, pr: &Pr, path: &str, viewed: bool) -> Res<()>;
    /// Open `url` in the browser.
    fn open(&self, url: &str) -> Res<()>;
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

const MARK_VIEWED: &str = "mutation($id: ID!, $path: String!) {
  markFileAsViewed(input: {pullRequestId: $id, path: $path}) { clientMutationId }
}";

const UNMARK_VIEWED: &str = "mutation($id: ID!, $path: String!) {
  unmarkFileAsViewed(input: {pullRequestId: $id, path: $path}) { clientMutationId }
}";

impl Gh {
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
    fn pr(&self, selector: Option<&str>) -> Res<Pr> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct View {
            id: String,
            number: u64,
            url: String,
            base_ref_name: String,
            base_ref_oid: String,
        }

        let mut args = vec!["pr", "view"];
        args.extend(selector);
        args.extend(["--json", "id,number,url,baseRefName,baseRefOid"]);
        let view: View = serde_json::from_str(&self.run(&args)?)
            .map_err(|e| Error::Gh(format!("Couldn't read gh's description of the PR: {e}")))?;
        Ok(Pr {
            id: view.id,
            number: view.number,
            url: view.url,
            base_ref: view.base_ref_name,
            base_oid: view.base_ref_oid,
        })
    }

    fn checkout(&self, pr: &Pr) -> Command {
        let mut cmd = Command::new("gh");
        cmd.args(["pr", "checkout", &pr.number.to_string()]);
        cmd
    }

    fn fetch_base(&self, pr: &Pr) -> Res<Command> {
        let slug =
            repo_of(&pr.url).ok_or_else(|| Error::Gh(format!("Not a PR URL: {}", pr.url)))?;
        let repo = git2::Repository::open(&self.workdir).map_err(Error::OpenRepo)?;
        let names = repo.remotes().map_err(Error::GetRemote)?;
        let remotes = names.iter().flatten().filter_map(|name| {
            let remote = repo.find_remote(name).ok()?;
            Some((name.to_owned(), remote.url()?.to_owned()))
        });
        let remote = remote_for(&slug, remotes).ok_or(Error::NoRemoteForPr(slug))?;
        let mut cmd = Command::new("git");
        cmd.args(["fetch", &remote, &pr.base_ref]);
        Ok(cmd)
    }

    fn viewed(&self, pr: &Pr) -> Res<HashMap<String, Viewed>> {
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
        Ok(parse_viewed(&output))
    }

    fn set_viewed(&self, pr: &Pr, path: &str, viewed: bool) -> Res<()> {
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

/// The repository a PR's page is in, as `{owner}/{repo}`.
fn repo_of(pr_url: &str) -> Option<String> {
    let path = pr_url.strip_prefix("https://")?.split_once('/')?.1;
    let mut parts = path.split('/');
    let (owner, repo) = (parts.next()?, parts.next()?);
    Some(format!("{owner}/{repo}"))
}

/// The name of the remote, of `remotes` given as `(name, url)`, whose URL is
/// of the repository `slug` (`{owner}/{repo}`), over https or ssh.
fn remote_for(slug: &str, remotes: impl IntoIterator<Item = (String, String)>) -> Option<String> {
    remotes.into_iter().find_map(|(name, url)| {
        let url = url.strip_suffix('/').unwrap_or(&url);
        let url = url.strip_suffix(".git").unwrap_or(url);
        let path = url.rsplit_once(':').map_or(url, |(_, path)| path);
        let ends_with_slug = path == slug
            || path
                .strip_suffix(slug)
                .is_some_and(|before| before.ends_with('/'));
        ends_with_slug.then_some(name)
    })
}

/// The lines `path<TAB>state` that [`Gh::viewed`] has gh print. A line it
/// cannot read is left out.
fn parse_viewed(output: &str) -> HashMap<String, Viewed> {
    output
        .lines()
        .filter_map(|line| {
            let (path, state) = line.rsplit_once('\t')?;
            let viewed = match state {
                "VIEWED" => Viewed::Viewed,
                "UNVIEWED" => Viewed::Unviewed,
                "DISMISSED" => Viewed::Dismissed,
                _ => return None,
            };
            Some((path.to_owned(), viewed))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_of_reads_owner_and_repo() {
        assert_eq!(
            repo_of("https://github.com/o/r/pull/7").as_deref(),
            Some("o/r")
        );
    }

    #[test]
    fn remote_for_matches_https_and_ssh_urls() {
        let remotes = |url: &str| [("origin".to_owned(), url.to_owned())];
        for url in [
            "https://github.com/o/r.git",
            "https://github.com/o/r",
            "git@github.com:o/r.git",
            "ssh://git@github.com/o/r.git",
        ] {
            assert_eq!(
                remote_for("o/r", remotes(url)).as_deref(),
                Some("origin"),
                "{url}"
            );
        }
        assert_eq!(
            remote_for("o/r", remotes("https://github.com/xo/r.git")),
            None
        );
    }

    #[test]
    fn parse_viewed_reads_each_state() {
        let viewed = parse_viewed("a.txt\tVIEWED\nb c.txt\tUNVIEWED\nd.txt\tDISMISSED\n");
        assert_eq!(
            viewed,
            HashMap::from([
                ("a.txt".to_owned(), Viewed::Viewed),
                ("b c.txt".to_owned(), Viewed::Unviewed),
                ("d.txt".to_owned(), Viewed::Dismissed),
            ])
        );
    }
}
