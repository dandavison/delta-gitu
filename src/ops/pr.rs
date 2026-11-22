//! Review a GitHub PR: check it out, see its diff with GitHub's Viewed
//! checkbox on each file, tick them off, and open a line on GitHub to comment
//! on it.

use super::{Action, OpTrait};
use crate::{
    Res,
    app::{App, PromptParams, State},
    error::Error,
    git::{self, diff::Diff},
    github::{self, Review, Side, ViewedState},
    item_data::ItemData,
    screen,
    style::Style,
    term::Term,
};
use git2::Oid;
use std::{cell::RefCell, rc::Rc, sync::Arc};

const NO_REVIEW: &str = "No PR here: O opens one";

pub(crate) struct PrCurrent;
impl OpTrait for PrCurrent {
    fn get_action(&self, _target: &ItemData) -> Option<Action> {
        Some(Rc::new(|app, term| open_pr(app, term, None)))
    }

    fn display(&self, _state: &State) -> String {
        "current".into()
    }
}

pub(crate) struct PrOther;
impl OpTrait for PrOther {
    fn get_action(&self, _target: &ItemData) -> Option<Action> {
        Some(Rc::new(|app, term| {
            let selector = app.prompt(
                term,
                &PromptParams {
                    prompt: "PR number or URL",
                    ..Default::default()
                },
            )?;
            open_pr(app, term, Some(&selector))
        }))
    }

    fn display(&self, _state: &State) -> String {
        "other".into()
    }
}

/// Check out the PR `selector` refers to, or with none the current branch's,
/// and open its diff as GitHub shows it: from where the PR branched from its
/// base. Uncommitted changes stop it, rather than being carried onto the PR's
/// branch.
fn open_pr(app: &mut App, term: &mut Term, selector: Option<&str>) -> Res<()> {
    let workdir = app.state.repo.workdir().ok_or(Error::NoRepoWorkdir)?;
    let uncommitted = git::status(workdir)?
        .files
        .iter()
        .any(|file| file.status_code != ['?', '?']);
    if uncommitted {
        return Err(Error::PrCheckoutDirty);
    }

    let github = Rc::clone(&app.state.github);
    let pr = github.pr(selector)?;
    app.run_cmd(term, &[], github.checkout(&pr))?;
    let has_base =
        Oid::from_str(&pr.base_oid).is_ok_and(|oid| app.state.repo.find_commit(oid).is_ok());
    if !has_base {
        app.run_cmd(term, &[], github.fetch_base(&pr)?)?;
    }
    let review = Rc::new(Review {
        viewed: RefCell::new(github.viewed(&pr)?),
        threads: RefCell::new(github.threads(&pr)?),
        options: RefCell::new(None),
        pr,
    });

    let (threads_review, repo) = (Rc::clone(&review), Rc::clone(&app.state.repo));
    let config = Arc::clone(&app.state.config);
    let (width, height) = term.size().map_err(Error::Term)?;
    let mut screen = screen::pager::live_decorated(
        Arc::clone(&app.state.config),
        Rc::clone(&app.state.repo),
        app.render_params((width.saturating_sub(review.reserved_columns()), height)),
        vec![
            "git".into(),
            "diff".into(),
            format!("{}...HEAD", review.pr.base_oid),
        ],
        Rc::new(move |params, items| {
            let dir = repo.workdir().unwrap_or(repo.path());
            let enabled = config.general.diff_renderer.enabled_features.get(dir);
            let layout = screen::review::Layout {
                width: params.width(),
                side_by_side: super::editor::is_enabled_in(
                    &params.features,
                    "side-by-side",
                    &enabled,
                ),
            };
            let options = comment_options(&threads_review, &config, dir, &params.features);
            let error_style = Style::from(&config.style.error_msg);
            screen::review::with_threads(
                items,
                &threads_review,
                &repo,
                layout,
                &options,
                error_style,
            )
        }),
    )?;
    screen.review = Some(review);
    app.state.screens.push(screen);
    Ok(())
}

/// Read the checkboxes and review threads of the PR on screen from GitHub
/// again, as they may have changed in the browser.
pub(crate) fn reread_review(app: &mut App) -> Res<()> {
    if let Some(review) = app.screen().review.clone() {
        *review.viewed.borrow_mut() = app.state.github.viewed(&review.pr)?;
        *review.threads.borrow_mut() = app.state.github.threads(&review.pr)?;
        *review.options.borrow_mut() = None;
    }
    Ok(())
}

/// The options to draw the review's comments with, as delta's config sets them with `features`
/// in force. delta is asked once for each set of features in force, and again after the review
/// is read again.
fn comment_options(
    review: &Review,
    config: &crate::config::Config,
    dir: &std::path::Path,
    features: &[String],
) -> Rc<screen::review::CommentOptions> {
    let features = crate::diff_renderer::feature_overlay(features);
    if let Some((asked_with, options)) = &*review.options.borrow()
        && *asked_with == features
    {
        return Rc::clone(options);
    }
    let values = config.general.diff_renderer.options.get(
        dir,
        features.as_deref(),
        &screen::review::OPTION_NAMES,
    );
    let options = Rc::new(screen::review::CommentOptions::of(&values));
    *review.options.borrow_mut() = Some((features, Rc::clone(&options)));
    options
}

pub(crate) struct ToggleViewed;
impl OpTrait for ToggleViewed {
    fn get_action(&self, target: &ItemData) -> Option<Action> {
        let (diff, file_i) = file_of(target)?;
        let path = github::file_path(diff, file_i);
        Some(Rc::new(move |app, _term| {
            let Some(review) = app.screen().review.clone() else {
                app.display_error(NO_REVIEW);
                return Ok(());
            };
            let viewed = review.viewed.borrow().get(&path) != Some(&ViewedState::Viewed);
            app.state.github.set_viewed(&review.pr, &path, viewed)?;
            let state = if viewed {
                ViewedState::Viewed
            } else {
                ViewedState::Unviewed
            };
            review.viewed.borrow_mut().insert(path.clone(), state);
            Ok(())
        }))
    }

    fn is_target_op(&self) -> bool {
        true
    }

    fn display(&self, _state: &State) -> String {
        "Toggle viewed".into()
    }
}

pub(crate) struct ShowOnGithub;
impl OpTrait for ShowOnGithub {
    fn get_action(&self, target: &ItemData) -> Option<Action> {
        if let ItemData::ReviewThread { url } | ItemData::ReviewComment { url } = target {
            let url = url.clone();
            return Some(Rc::new(move |app, _term| app.state.github.open(&url)));
        }
        let (diff, file_i) = file_of(target)?;
        let path = github::file_path(diff, file_i);
        let line = line_of(target);
        Some(Rc::new(move |app, _term| {
            let Some(review) = app.screen().review.clone() else {
                app.display_error(NO_REVIEW);
                return Ok(());
            };
            app.state
                .github
                .open(&github::file_url(&review.pr, &path, line))
        }))
    }

    fn is_target_op(&self) -> bool {
        true
    }

    fn display(&self, _state: &State) -> String {
        "Show on GitHub".into()
    }
}

/// The diff and file a file, hunk or line is in.
fn file_of(target: &ItemData) -> Option<(&Rc<Diff>, usize)> {
    match target {
        ItemData::Delta { diff, file_i, .. }
        | ItemData::Hunk { diff, file_i, .. }
        | ItemData::HunkLine { diff, file_i, .. } => Some((diff, *file_i)),
        _ => None,
    }
}

/// The line of a hunk or line to open on GitHub: a hunk's first change, or the
/// line itself.
fn line_of(target: &ItemData) -> Option<(Side, u32)> {
    let (diff, file_i, hunk_i, line_i) = match target {
        ItemData::Hunk {
            diff,
            file_i,
            hunk_i,
        } => {
            let line_i = diff
                .hunk_content(*file_i, *hunk_i)
                .lines()
                .position(|line| line.starts_with('+') || line.starts_with('-'))?;
            (diff, *file_i, *hunk_i, line_i)
        }
        ItemData::HunkLine {
            diff,
            file_i,
            hunk_i,
            line_i,
            ..
        } => (diff, *file_i, *hunk_i, *line_i),
        _ => return None,
    };
    let removed = diff
        .hunk_content(file_i, hunk_i)
        .lines()
        .nth(line_i)
        .is_some_and(|line| line.starts_with('-'));
    Some(if removed {
        (
            Side::Old,
            diff.hunk_line_old_file_num(file_i, hunk_i, line_i),
        )
    } else {
        (
            Side::New,
            diff.hunk_line_new_file_num(file_i, hunk_i, line_i),
        )
    })
}
