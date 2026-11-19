use crate::{Res, error::Error};
use git2::Repository;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

pub struct FileWatcher {
    pending_updates: Arc<AtomicBool>,
}

impl FileWatcher {
    pub fn new(repo: &Repository) -> Res<Self> {
        let pending_updates = Arc::new(AtomicBool::new(false));
        let pending_updates_w = pending_updates.clone();
        let git_dir = repo.path().to_path_buf();
        let workdir = repo.workdir().expect("Bare repos unhandled").to_path_buf();

        std::thread::spawn(move || {
            if let Err(e) = watch(&git_dir, &workdir, pending_updates_w) {
                log::error!("File watcher error: {:?}", e)
            }
        });

        Ok(Self { pending_updates })
    }

    pub fn pending_updates(&self) -> bool {
        self.pending_updates.swap(false, Ordering::Relaxed)
    }
}

fn watch(git_dir: &Path, workdir: &Path, pending_updates_w: Arc<AtomicBool>) -> Res<()> {
    let repo = Repository::open(git_dir).map_err(Error::OpenRepo)?;
    repo.set_workdir(workdir, false).map_err(Error::OpenRepo)?;
    let canonical = |path: &Path| path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let workdir = canonical(workdir);
    let git_dirs = GitDirs {
        git_dir: canonical(repo.path()),
        common_dir: canonical(repo.commondir()),
    };
    // A linked worktree's git dir and refs live outside its working directory.
    let common_dir_outside_workdir =
        (!git_dirs.common_dir.starts_with(&workdir)).then(|| git_dirs.common_dir.clone());

    let mut watcher = notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
        if let Ok(event) = res {
            if !is_changed(&event) {
                return;
            }

            for path in event.paths {
                let relevant = match git_dirs.is_relevant(&path) {
                    Some(relevant) => relevant,
                    None => !repo.status_should_ignore(&path).unwrap_or(false),
                };
                if relevant {
                    log::info!("File changed: {:?} ({:?})", path, event.kind);
                    pending_updates_w.store(true, Ordering::Relaxed);
                    break;
                }
            }
        }
    })
    .map_err(Error::FileWatcher)?;

    watcher
        .watch(&workdir, RecursiveMode::Recursive)
        .map_err(Error::FileWatcher)?;
    if let Some(common_dir) = common_dir_outside_workdir {
        watcher
            .watch(&common_dir, RecursiveMode::Recursive)
            .map_err(Error::FileWatcher)?;
    }

    log::info!(
        "File watcher started (kind: {:?})",
        RecommendedWatcher::kind()
    );

    std::mem::forget(watcher);
    Ok(())
}

/// For a linked worktree, `git_dir` is `<common_dir>/worktrees/<name>`.
/// Otherwise the two are the same directory.
struct GitDirs {
    git_dir: PathBuf,
    common_dir: PathBuf,
}

impl GitDirs {
    /// Whether a change to `path` can affect what gitu shows, or `None` if
    /// `path` is not in the repository's git directories.
    fn is_relevant(&self, path: &Path) -> Option<bool> {
        let rel = path.strip_prefix(&self.common_dir).ok()?;

        // Lock files are transient; the write they guard shows up as a change
        // to the locked file itself.
        if path.extension().is_some_and(|ext| ext == "lock") {
            return Some(false);
        }

        if self.git_dir != self.common_dir && path.starts_with(&self.git_dir) {
            return Some(true);
        }

        // New objects are always followed by a change to a ref or the index.
        // Other worktrees' HEAD and index do not affect this one.
        Some(!(rel.starts_with("objects") || rel.starts_with("worktrees")))
    }
}

fn is_changed(event: &Event) -> bool {
    matches!(
        event.kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    )
}
