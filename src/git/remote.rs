use std::str;

use git2::{Branch, Remote, Repository};

use crate::{Res, git};

use super::{Error, Utf8Error};

pub(crate) fn get_branch_upstream<'repo>(branch: &Branch<'repo>) -> Res<Option<Branch<'repo>>> {
    match branch.upstream() {
        Ok(v) => Ok(Some(v)),
        Err(e) if e.class() == git2::ErrorClass::Config => Ok(None),
        Err(e) => Err(Error::GetCurrentBranchUpstream(e)),
    }
}

/// If the branch has an upstream, returns the remote name and branch name in that order.
/// Returns "." as remote if the current branch has no remote upstream.
///
/// Branch references would be used like this (in Magit)
///
/// // Remote branch
/// git … push -v origin feature-branch\:refs/heads/feature-branch
/// git … pull origin refs/heads/feature-branch
/// git … rebase --autostash origin/feature-branch
///
/// // Local branch
/// git … push -v . feature-branch\:refs/heads/main
/// git … pull . refs/heads/main
/// git … rebase --autostash main
pub(crate) fn get_upstream_components(repo: &Repository) -> Res<Option<(String, String)>> {
    // From git config, as git push and pull take it, rather than from the remote-tracking branch,
    // which is gone once the branch is deleted on the remote.
    let head = git::get_head_name(repo)?;
    let config_error = |e: git2::Error| Error::GetCurrentBranchUpstream(e);
    let remote = match repo.branch_upstream_remote(&head) {
        Ok(remote) => remote,
        Err(e) if e.code() == git2::ErrorCode::NotFound => return Ok(None),
        Err(e) => return Err(config_error(e)),
    };
    let merge = repo.branch_upstream_merge(&head).map_err(config_error)?;
    let remote = String::from_utf8_lossy(&remote).into_owned();
    let merge = String::from_utf8_lossy(&merge).into_owned();
    let branch = merge
        .strip_prefix("refs/heads/")
        .unwrap_or(&merge)
        .to_owned();
    Ok(Some((remote, branch)))
}

/// The current branch's upstream as git shows it, such as `origin/main`, or `main` for a local
/// branch.
pub(crate) fn get_upstream_shortname(repo: &Repository) -> Res<Option<String>> {
    Ok(
        get_upstream_components(repo)?.map(|(remote, branch)| match remote.as_str() {
            "." => branch,
            _ => format!("{remote}/{branch}"),
        }),
    )
}

/// Whether the current branch's upstream is a remote branch that is gone: one that its remote
/// had, but no longer has, as the last fetch found.
pub(crate) fn is_upstream_gone(repo: &Repository) -> Res<bool> {
    let Some((remote, branch)) = get_upstream_components(repo)? else {
        return Ok(false);
    };
    if remote == "." {
        return Ok(false);
    }
    match repo.find_reference(&format!("refs/remotes/{remote}/{branch}")) {
        Ok(_) => Ok(false),
        Err(e) if e.code() == git2::ErrorCode::NotFound => Ok(true),
        Err(e) => Err(Error::GetCurrentBranchUpstream(e)),
    }
}

pub(crate) fn get_upstream_remote(repo: &Repository) -> Res<Option<String>> {
    Ok(get_upstream_components(repo)?
        .map(|(remote, _)| remote)
        .filter(|remote| remote != "."))
}

pub(crate) fn get_push_remote(repo: &Repository) -> Res<Option<String>> {
    let push_remote_cfg = head_push_remote_cfg(repo)?;
    let config = repo.config().map_err(Error::ReadGitConfig)?;

    match config.get_entry(&push_remote_cfg) {
        Ok(entry) => Ok(Some(
            String::from_utf8_lossy(entry.value_bytes()).into_owned(),
        )),
        Err(e) if e.class() == git2::ErrorClass::Config => get_default_push_remote(repo),
        Err(e) => Err(Error::ReadGitConfig(e)),
    }
}

pub(crate) fn get_default_push_remote(repo: &Repository) -> Res<Option<String>> {
    let push_default_cfg = "remote.pushDefault";
    let config = repo.config().map_err(Error::ReadGitConfig)?;

    match config.get_entry(push_default_cfg) {
        Ok(entry) => Ok(Some(
            String::from_utf8_lossy(entry.value_bytes()).into_owned(),
        )),
        Err(e) if e.class() == git2::ErrorClass::Config => Ok(None),
        Err(e) => Err(Error::ReadGitConfig(e)),
    }
}

pub(crate) fn set_push_remote(repo: &Repository, remote: Option<&Remote>) -> Res<()> {
    let push_remote_cfg = head_push_remote_cfg(repo)?;
    let mut config = repo.config().map_err(Error::ReadGitConfig)?;
    match remote {
        None => {
            config
                .remove(&push_remote_cfg)
                .map_err(Error::DeleteGitConfig)?;
        }
        Some(remote) => {
            config
                .set_str(
                    &push_remote_cfg,
                    str::from_utf8(remote.name_bytes().ok_or(Error::RemoteHasNoName)?)
                        .map_err(Utf8Error::Str)
                        .map_err(Error::RemoteNameUtf8)?,
                )
                .map_err(Error::SetGitConfig)?;
        }
    }
    Ok(())
}

pub(crate) fn head_push_remote_cfg(repo: &Repository) -> Res<String> {
    let branch = git::get_current_branch_name(repo)?;
    let push_remote_cfg = format!("branch.{branch}.pushRemote");
    Ok(push_remote_cfg)
}
