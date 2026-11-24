//! Review a GitHub PR: check it out, see its diff with GitHub's Viewed
//! checkbox on each file, tick them off, open a line on GitHub, and write
//! comments in a box in the diff.

use super::{Action, OpTrait};
use crate::{
    Res,
    app::{App, PromptParams, State},
    error::Error,
    git::{self, diff::Diff},
    item_data::ItemData,
    review::{
        self, Backend, CommentEditor, EditorTarget, OpenPr, Side, ViewedState,
        local::{LocalBackend, NOT_DRAFT},
    },
    screen,
    style::Style,
    term::Term,
    text_area::{Outcome, TextArea},
};
use crossterm::event::KeyEvent;
use std::{rc::Rc, sync::Arc};

const NO_REVIEW: &str = "No PR here: O opens one";
const NOT_ON_GITHUB: &str = "This PR's comments are not on GitHub";

/// Open the current branch's PR: its local PR, linked to its GitHub PR if it has one (see
/// [`LocalBackend::new`]). Nothing is checked out.
pub(crate) struct PrCurrent;
impl OpTrait for PrCurrent {
    fn get_action(&self, _target: &ItemData) -> Option<Action> {
        Some(Rc::new(|app, term| open_current(app, term, None)))
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
                    prompt: "PR number or URL".into(),
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

/// Check out the GitHub PR `selector` refers to, and open it as `O O` opens the current branch's.
/// Uncommitted changes stop it, rather than being carried onto the PR's branch.
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
    open_current(app, term, Some(pr.number))
}

/// Open the current branch's PR, which must be linked to the GitHub PR `expected` if given: its
/// reviews are fetched from GitHub if it is linked, and a PR not checked against GitHub says so.
pub(crate) fn open_current(app: &mut App, term: &mut Term, expected: Option<u64>) -> Res<()> {
    let local =
        LocalBackend::for_current_branch(Rc::clone(&app.state.repo), Rc::clone(&app.state.github))?;
    if let Some(expected) = expected
        && local.linked_number() != Some(expected)
    {
        return Err(Error::Gh(format!(
            "Couldn't open PR #{expected}: the branch checked out for it has {}",
            match local.linked_number() {
                Some(number) => format!("GitHub's PR #{number}"),
                None => "no GitHub PR".to_owned(),
            }
        )));
    }
    confirm_authors(app, term, &local)?;
    local.refresh()?;
    let unchecked = local.unchecked().map(str::to_owned);
    show_pr(app, term, Rc::new(local))?;
    if let Some(unchecked) = unchecked {
        app.display_error(unchecked);
    }
    Ok(())
}

/// Ask the user to go on if the PR's commits are by more than one author, as they would be if
/// its base were wrong.
fn confirm_authors(app: &mut App, term: &mut Term, local: &LocalBackend) -> Res<()> {
    let authors = local.commit_authors()?;
    if authors.len() > 1 {
        app.confirm(
            term,
            format!(
                "The PR's commits are by {} authors: {}. Open it? [y/n]",
                authors.len(),
                authors.join(", ")
            ),
        )?;
    }
    Ok(())
}

/// Post the current branch's local PR to GitHub: push the branch, create a draft PR if it has
/// none, and add every draft comment to the user's pending review there.
pub(crate) struct PrPost;
impl OpTrait for PrPost {
    fn get_action(&self, _target: &ItemData) -> Option<Action> {
        Some(Rc::new(|app, term| {
            // The PR on screen, if it is a local PR's, which may be into another base than the
            // default.
            let on_screen = app.screen().pr.clone();
            let for_current_branch;
            let local = match on_screen.as_ref().and_then(|pr| pr.backend.as_local()) {
                Some(local) => local,
                None => {
                    for_current_branch = LocalBackend::for_current_branch(
                        Rc::clone(&app.state.repo),
                        Rc::clone(&app.state.github),
                    )?;
                    &for_current_branch
                }
            };
            app.confirm(
                term,
                "Push the branch and post its draft comments to GitHub? [y/n]",
            )?;
            let push = review::post::push_command(&app.state.repo, local.head())?;
            app.run_cmd(term, &[], push)?;
            let posted = review::post::post(local, Rc::clone(&app.state.github))?;
            reread_pr(app)?;
            app.update_screens()?;
            app.display_info(posted.summary());
            Ok(())
        }))
    }

    fn display(&self, _state: &State) -> String {
        "post to GitHub".into()
    }
}

/// Open a screen with the diff of the PR `backend` is for, with its checkboxes and threads.
fn show_pr(app: &mut App, term: &mut Term, backend: Rc<dyn Backend>) -> Res<()> {
    let pr = Rc::new(OpenPr::new(backend)?);

    let (threads_pr, repo) = (Rc::clone(&pr), Rc::clone(&app.state.repo));
    let (width, height) = term.size().map_err(Error::Term)?;
    let mut screen = screen::pager::live_decorated(
        Arc::clone(&app.state.config),
        Rc::clone(&app.state.repo),
        app.render_params((width.saturating_sub(review::FILE_PREFIX_WIDTH), height)),
        vec!["git".into(), "diff".into(), pr.backend.diff_range()],
        Rc::new(move |params, items| {
            let config = &params.config;
            let dir = repo.workdir().unwrap_or(repo.path());
            let options = comment_options(&threads_pr, config, dir, &params.features);
            let layout = screen::review::Layout {
                width: params.width(),
                side_by_side: options.side_by_side,
            };
            let error_style = Style::from(&config.style.error_msg);
            screen::review::with_threads(items, &threads_pr, &repo, layout, &options, error_style)
        }),
    )?;
    screen.pr = Some(pr);
    app.state.screens.push(screen);
    Ok(())
}

/// Read the checkboxes and review threads of the PR on screen from its backend
/// again, as they may have changed elsewhere, such as in the browser.
pub(crate) fn reread_pr(app: &mut App) -> Res<()> {
    if let Some(pr) = app.screen().pr.clone() {
        pr.backend.refresh()?;
        *pr.viewed.borrow_mut() = pr.backend.viewed()?;
        pr.reread_threads()?;
        *pr.options.borrow_mut() = None;
    }
    Ok(())
}

/// The options to draw the review's comments with, as delta's config sets them with `features`
/// in force, and whether delta draws side by side. delta settles all its options to answer, so
/// it is asked once for each set of features in force, and again after the review is read
/// again, rather than each time the rows are built.
fn comment_options(
    pr: &OpenPr,
    config: &crate::config::Config,
    dir: &std::path::Path,
    features: &[String],
) -> Rc<screen::review::CommentOptions> {
    let overlay = crate::diff_renderer::feature_overlay(features);
    if let Some((asked_with, options)) = &*pr.options.borrow()
        && *asked_with == overlay
    {
        return Rc::clone(options);
    }
    let values = config.general.diff_renderer.options.get(
        dir,
        overlay.as_deref(),
        &screen::review::OPTION_NAMES,
    );
    let enabled = config.general.diff_renderer.enabled_features.get(dir);
    let options = Rc::new(screen::review::CommentOptions {
        side_by_side: super::editor::is_enabled_in(features, "side-by-side", &enabled),
        ..screen::review::CommentOptions::of(&values, config.general.diff_renderer.color_mode)
    });
    *pr.options.borrow_mut() = Some((overlay, Rc::clone(&options)));
    options
}

pub(crate) struct ToggleViewed;
impl OpTrait for ToggleViewed {
    fn get_action(&self, target: &ItemData) -> Option<Action> {
        let (diff, file_i) = file_of(target)?;
        let path = review::file_path(diff, file_i);
        Some(Rc::new(move |app, _term| {
            let Some(pr) = app.screen().pr.clone() else {
                app.display_error(NO_REVIEW);
                return Ok(());
            };
            let viewed = pr.viewed.borrow().get(&path) != Some(&ViewedState::Viewed);
            pr.backend.set_viewed(&path, viewed)?;
            let state = if viewed {
                ViewedState::Viewed
            } else {
                ViewedState::Unviewed
            };
            pr.viewed.borrow_mut().insert(path.clone(), state);
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
        if let ItemData::ReviewThread { url, .. } | ItemData::ReviewComment { url, .. } = target {
            let url = url.clone();
            return Some(Rc::new(move |app, _term| match &url {
                Some(url) => app.state.github.open(url),
                None => {
                    app.display_error(NOT_ON_GITHUB);
                    Ok(())
                }
            }));
        }
        let (diff, file_i) = file_of(target)?;
        let path = review::file_path(diff, file_i);
        let line = line_of(target);
        Some(Rc::new(move |app, _term| {
            let Some(pr) = app.screen().pr.clone() else {
                app.display_error(NO_REVIEW);
                return Ok(());
            };
            match pr.backend.file_url(&path, line) {
                Some(url) => app.state.github.open(&url),
                None => {
                    app.display_error(NOT_ON_GITHUB);
                    Ok(())
                }
            }
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

/// Write a comment in a box in the diff: on a comment the user can change (theirs in a draft
/// review, or their own on GitHub), an edit of it; on anyone else's, a reply in its thread; on a
/// diff line, a new comment on it. On the box, or anywhere while it holds text, go back to it, rather than lose what was
/// written.
pub(crate) struct WriteComment;
impl OpTrait for WriteComment {
    fn get_action(&self, target: &ItemData) -> Option<Action> {
        let new_target = match target {
            ItemData::ReviewThread { thread_id, .. }
            | ItemData::ReviewComment { thread_id, .. } => Some(EditorTarget::Reply {
                thread_id: thread_id.clone(),
            }),
            ItemData::CommentEditor { .. } => None,
            // A file's row has no line to comment on, but from it, as from any row of a diff,
            // `i` goes back to a box being written.
            _ => {
                let (diff, file_i) = file_of(target)?;
                line_of(target).map(|line| EditorTarget::Line {
                    path: review::file_path(diff, file_i),
                    line,
                })
            }
        };
        let on_comment = comment_of(target);
        Some(Rc::new(move |app, _term| {
            let Some(pr) = app.screen().pr.clone() else {
                app.display_error(NO_REVIEW);
                return Ok(());
            };
            // On a comment the user can change, with no box being written, edit the comment.
            let writing = pr
                .editor
                .borrow()
                .as_ref()
                .is_some_and(|editor| !editor.text.value().is_empty());
            if let Some((thread_id, comment_id)) = &on_comment
                && !writing
                && let Some(comment) = pr.comment(thread_id, comment_id).filter(|c| c.editable)
            {
                let mut text = TextArea::default();
                text.insert_str(&comment.body);
                *pr.editor.borrow_mut() = Some(CommentEditor {
                    target: EditorTarget::Edit {
                        thread_id: thread_id.clone(),
                        comment_id: comment_id.clone(),
                    },
                    text,
                    focused: true,
                    width: 0,
                });
                show_editor(app);
                return Ok(());
            }
            {
                let mut editor = pr.editor.borrow_mut();
                // A box with nothing written in it, elsewhere, gives way to the new one.
                let replaced = match (&*editor, &new_target) {
                    (Some(editor), Some(target)) => {
                        editor.text.value().is_empty() && editor.target != *target
                    }
                    (None, Some(_)) => true,
                    (_, None) => false,
                };
                match (&mut *editor, new_target.clone()) {
                    (_, Some(target)) if replaced => {
                        *editor = Some(CommentEditor {
                            target,
                            text: TextArea::default(),
                            focused: true,
                            width: 0,
                        });
                    }
                    (Some(editor), _) => editor.focused = true,
                    (None, _) => {
                        drop(editor);
                        app.display_error(
                            "Nothing to comment on here: i comments on a diff line, or replies to a thread",
                        );
                        return Ok(());
                    }
                }
            }
            show_editor(app);
            Ok(())
        }))
    }

    fn is_target_op(&self) -> bool {
        true
    }

    fn display(&self, _state: &State) -> String {
        "Write a comment".into()
    }
}

/// Delete a comment in a draft review, once the user confirms.
pub(crate) fn delete_comment(thread_id: String, comment_id: String) -> Action {
    Rc::new(move |app, term| {
        let Some(pr) = app.screen().pr.clone() else {
            app.display_error(NO_REVIEW);
            return Ok(());
        };
        if !pr
            .comment(&thread_id, &comment_id)
            .is_some_and(|c| c.deletable)
        {
            app.display_error(NOT_DRAFT);
            return Ok(());
        }
        app.confirm(term, "Really delete the comment? [y/n]")?;
        let message = pr.backend.delete_comment(&thread_id, &comment_id)?;
        pr.reread_threads()?;
        app.screen_mut().redecorate();
        app.display_info(message);
        Ok(())
    })
}

/// The thread and comment a comment's row is of.
pub(crate) fn comment_of(target: &ItemData) -> Option<(String, String)> {
    match target {
        ItemData::ReviewThread {
            thread_id,
            comment_id,
            ..
        }
        | ItemData::ReviewComment {
            thread_id,
            comment_id,
            ..
        } => Some((thread_id.clone(), comment_id.clone())),
        _ => None,
    }
}

/// Whether keys go to a comment box on the screen.
pub(crate) fn editor_has_focus(app: &App) -> bool {
    app.screen().pr.as_ref().is_some_and(|pr| {
        pr.editor
            .borrow()
            .as_ref()
            .is_some_and(|editor| editor.focused)
    })
}

/// Edit the comment box that has focus as `key` says, or leave it.
pub(crate) fn handle_editor_key(app: &mut App, _term: &mut Term, key: KeyEvent) -> Res<()> {
    let Some(pr) = app.screen().pr.clone() else {
        return Ok(());
    };
    let outcome = {
        let mut editor = pr.editor.borrow_mut();
        let Some(editor) = editor.as_mut() else {
            return Ok(());
        };
        let width = editor.width;
        let outcome = editor.text.handle_key(key, width);
        if outcome == Outcome::Leave {
            editor.focused = false;
        }
        outcome
    };
    match outcome {
        Outcome::Edited | Outcome::Moved | Outcome::Leave => show_editor(app),
        Outcome::Submit => post_comment(app)?,
        Outcome::Ignored => {}
    }
    Ok(())
}

/// Post the comment being written, as a reply to its thread or on its line, and read the threads
/// again to show it, with the box gone. A new comment is not posted while HEAD is not the PR's
/// head on GitHub, which its line is numbered against. What was written is kept if it is not
/// posted.
fn post_comment(app: &mut App) -> Res<()> {
    let Some(pr) = app.screen().pr.clone() else {
        return Ok(());
    };
    let Some((target, body)) = pr
        .editor
        .borrow()
        .as_ref()
        .map(|editor| (editor.target.clone(), editor.text.value().to_owned()))
    else {
        return Ok(());
    };
    if body.trim().is_empty() {
        app.display_error("Nothing to post");
        return Ok(());
    }
    let message = match &target {
        EditorTarget::Reply { thread_id } => pr.backend.reply(thread_id, &body)?,
        EditorTarget::Line { path, line } => {
            if pr.backend.head_mismatch(&app.state.repo) {
                app.display_error(
                    "Cannot comment: HEAD is not the PR's head on GitHub, so the comment's line may be wrong",
                );
                return Ok(());
            }
            pr.backend.comment(path, *line, &body)?
        }
        EditorTarget::Edit {
            thread_id,
            comment_id,
        } => pr.backend.edit_comment(thread_id, comment_id, &body)?,
    };
    *pr.editor.borrow_mut() = None;
    pr.reread_threads()?;
    app.screen_mut().redecorate();
    app.display_info(message);
    Ok(())
}

/// Insert a paste into the comment box that has focus.
pub(crate) fn paste_into_editor(app: &mut App, text: &str) {
    if let Some(pr) = app.screen().pr.clone()
        && let Some(editor) = pr.editor.borrow_mut().as_mut()
    {
        editor.text.insert_str(text);
    }
    show_editor(app);
}

/// Draw the comment box again, with the screen's cursor on the row its text cursor is in while
/// it has focus, so that it stays in view as it grows.
fn show_editor(app: &mut App) {
    let screen = app.screen_mut();
    screen.redecorate();
    let Some(pr) = screen.pr.clone() else {
        return;
    };
    let row = match pr.editor.borrow().as_ref() {
        Some(editor) if editor.focused => editor.text.cursor_at(editor.width).0,
        _ => return,
    };
    screen.select_matching_in_view(
        |data| matches!(data, ItemData::CommentEditor { row: r } if *r == row),
    );
}
