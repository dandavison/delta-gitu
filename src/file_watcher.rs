use crate::{Res, error::Error};
use git2::Repository;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::{
    path::Path,
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
    let mut watcher = notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
        if let Ok(event) = res {
            if !is_changed(&event) {
                return;
            }

            for path in event.paths {
                if !repo.status_should_ignore(&path).unwrap_or(false) {
                    log::info!("File changed: {:?} ({:?})", path, event.kind);
                    pending_updates_w.store(true, Ordering::Relaxed);
                    break;
                }
            }
        }
    })
    .map_err(Error::FileWatcher)?;

    watcher
        .watch(workdir, RecursiveMode::Recursive)
        .map_err(Error::FileWatcher)?;

    log::info!(
        "File watcher started (kind: {:?})",
        RecommendedWatcher::kind()
    );

    std::mem::forget(watcher);
    Ok(())
}

fn is_changed(event: &Event) -> bool {
    matches!(
        event.kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    )
}
