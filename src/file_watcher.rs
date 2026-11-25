use crate::{Res, error::Error};
use git2::Repository;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

pub struct FileWatcher {
    pending_updates: Arc<AtomicBool>,
    /// Whether the watcher has started watching, which it does on a thread of its own some time
    /// after [`FileWatcher::new`] returns: on macOS, registering with fseventsd has taken over
    /// half a second. A change made before then is not seen. Only tests read it, to wait for it.
    #[cfg_attr(not(test), allow(dead_code))]
    watching: Arc<AtomicBool>,
    /// What went wrong in the watcher's thread, for the user to be told: until they are, the
    /// screens may not show changes made elsewhere.
    errors: Arc<Mutex<Vec<String>>>,
}

impl FileWatcher {
    pub fn new(repo: &Repository) -> Res<Self> {
        let pending_updates = Arc::new(AtomicBool::new(false));
        let pending_updates_w = pending_updates.clone();
        let errors = Arc::new(Mutex::new(vec![]));
        let errors_w = Arc::clone(&errors);
        let watching = Arc::new(AtomicBool::new(false));
        let watching_w = Arc::clone(&watching);
        let git_dir = repo.path().to_path_buf();
        let workdir = repo.workdir().expect("Bare repos unhandled").to_path_buf();

        std::thread::spawn(move || {
            match watch(&git_dir, &workdir, pending_updates_w, Arc::clone(&errors_w)) {
                Ok(()) => watching_w.store(true, Ordering::Relaxed),
                Err(e) => report(&errors_w, format!("File watcher stopped: {e}")),
            }
        });

        Ok(Self {
            pending_updates,
            watching,
            errors,
        })
    }

    pub fn pending_updates(&self) -> bool {
        self.pending_updates.swap(false, Ordering::Relaxed)
    }

    /// Whether the watcher has started watching.
    #[cfg(test)]
    pub fn is_watching(&self) -> bool {
        self.watching.load(Ordering::Relaxed)
    }

    /// What has gone wrong since last asked.
    pub fn take_errors(&self) -> Vec<String> {
        std::mem::take(
            &mut *self
                .errors
                .lock()
                .expect("no watcher thread panics holding it"),
        )
    }
}

fn report(errors: &Mutex<Vec<String>>, error: String) {
    log::error!("{error}");
    errors
        .lock()
        .expect("no watcher thread panics holding it")
        .push(error);
}

fn watch(
    git_dir: &Path,
    workdir: &Path,
    pending_updates_w: Arc<AtomicBool>,
    errors: Arc<Mutex<Vec<String>>>,
) -> Res<()> {
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
        let event = match res {
            Ok(event) => event,
            Err(e) => {
                report(&errors, format!("File watcher error: {e}"));
                return;
            }
        };
        if !is_changed(&event) {
            return;
        }

        for path in event.paths {
            let relevant = match git_dirs.is_relevant(&path) {
                Some(relevant) => relevant,
                None => match repo.status_should_ignore(&path) {
                    Ok(ignored) => !ignored,
                    Err(e) => {
                        report(
                            &errors,
                            format!(
                                "File watcher couldn't tell whether git ignores {}: {e}",
                                path.display()
                            ),
                        );
                        true
                    }
                },
            };
            if relevant {
                log::info!("File changed: {:?} ({:?})", path, event.kind);
                pending_updates_w.store(true, Ordering::Relaxed);
                break;
            }
        }
    })
    .map_err(Error::FileWatcher)?;

    // Adding both paths before committing starts one FSEvents stream. Calling
    // `watch` for each would start a stream and then restart it, and each
    // start registers the stream with fseventsd, which has taken over 100ms.
    let mut paths = watcher.paths_mut();
    paths
        .add(&workdir, RecursiveMode::Recursive)
        .map_err(Error::FileWatcher)?;
    if let Some(common_dir) = common_dir_outside_workdir {
        paths
            .add(&common_dir, RecursiveMode::Recursive)
            .map_err(Error::FileWatcher)?;
    }
    paths.commit().map_err(Error::FileWatcher)?;

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
