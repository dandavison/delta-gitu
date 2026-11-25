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
    prompt::HistoryKind,
    review::{
        self, Backend, CommentEditor, EditorTarget, OpenPr, Side, ViewedState,
        local::{LocalBackend, NOT_DRAFT},
        remote::Consent,
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
const PR_NOT_ON_GITHUB: &str = "This PR is not on GitHub";

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
    local.check_not_behind()?;
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

/// Sync the current branch's PR with GitHub (see [`review::post::sync`]): add every draft comment
/// to the user's pending review there, and fetch. With no PR on GitHub, it asks before creating
/// one, which pushes the branch.
pub(crate) struct PrSync;
impl OpTrait for PrSync {
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
            // Creating a PR pushes the branch, and makes it visible to everyone who can see the
            // repository.
            let create_pr = app.state.github.pr_for_branch(local.head())?.is_none()
                && !local.to_post()?.is_empty();
            if create_pr {
                app.confirm(term, "No PR on GitHub: create one? [y/n]")?;
            }
            let github = Rc::clone(&app.state.github);
            let synced = review::post::sync(local, github, create_pr, &mut |cmd| {
                app.run_cmd(term, &[], cmd)
            });
            // What sync did is shown, as drafts it moved, even when it posted nothing.
            reread_pr(app)?;
            app.update_screens()?;
            app.display_info(synced?.summary());
            Ok(())
        }))
    }

    fn display(&self, _state: &State) -> String {
        "sync with GitHub".into()
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
        vec!["git".into(), "diff".into(), pr.diff_range()],
        Rc::new(move |params, items| {
            let config = &params.config;
            let dir = repo.workdir().unwrap_or(repo.path());
            let options = comment_options(&threads_pr, config, dir, &params.features);
            let layout = screen::review::Layout {
                width: params.width(),
            };
            let error_style = Style::from(&config.style.error_msg);
            let summary_style = Style::from(&config.style.pr_summary);
            screen::review::with_threads(
                items,
                &threads_pr,
                &repo,
                layout,
                &options,
                summary_style,
                error_style,
            )
        }),
    )?;
    *pr.git.borrow_mut() = screen.git_command.clone();
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
        pr.repin()?;
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
    let options = Rc::new(screen::review::CommentOptions::of(
        &values,
        config.general.diff_renderer.color_mode,
    ));
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

/// Move a local comment to local-only, which sync never posts, or back to the draft review.
pub(crate) struct ToggleLocalOnly;
impl OpTrait for ToggleLocalOnly {
    fn get_action(&self, target: &ItemData) -> Option<Action> {
        let (thread_id, comment_id) = comment_of(target)?;
        Some(Rc::new(move |app, _term| {
            let Some(pr) = app.screen().pr.clone() else {
                app.display_error(NO_REVIEW);
                return Ok(());
            };
            let Some(local) = pr.backend.as_local() else {
                app.display_error("Only a local PR's comments can be local-only");
                return Ok(());
            };
            let Some(comment) = pr.comment(&thread_id, &comment_id) else {
                app.display_error("The comment has been deleted");
                return Ok(());
            };
            if !comment.draft && !comment.local_only {
                app.display_error("Only a local comment can be moved: this one is on GitHub");
                return Ok(());
            }
            let to_local = !comment.local_only;
            local.move_comment(&comment_id, to_local)?;
            pr.reread_threads()?;
            app.screen_mut().redecorate();
            app.display_info(if to_local {
                "Moved the comment to local-only: sync won't post it"
            } else {
                "Moved the comment to your draft review: sync will post it"
            });
            Ok(())
        }))
    }

    fn is_target_op(&self) -> bool {
        true
    }

    fn display(&self, _state: &State) -> String {
        "Toggle local-only".into()
    }
}

/// Leave a message for an agent running `delta tui review listen`, about the thread under the
/// cursor, or else the whole PR.
pub(crate) struct MessageAgent;
impl OpTrait for MessageAgent {
    fn get_action(&self, target: &ItemData) -> Option<Action> {
        let thread = comment_of(target).map(|(thread_id, _)| thread_id);
        // On the rows `i` comments from: a comment, the PR's summary, or a diff.
        if thread.is_none() && !matches!(target, ItemData::PrSummary) && file_of(target).is_none() {
            return None;
        }
        Some(Rc::new(move |app, term| {
            let Some(pr) = app.screen().pr.clone() else {
                app.display_error(NO_REVIEW);
                return Ok(());
            };
            let Some(local) = pr.backend.as_local() else {
                app.display_error("Only a local PR can message an agent");
                return Ok(());
            };
            let Some(version) = pr.version.get() else {
                app.display_error(
                    "Couldn't send: the diff is not the PR's, as its command was changed with :",
                );
                return Ok(());
            };
            let default = match thread {
                Some(_) => "Please respond to this thread",
                None => "Please respond to the comments",
            };
            let body = match app.prompt(
                term,
                &PromptParams {
                    prompt: "Send message".into(),
                    create_default_value: Box::new(move |_| Some(default.to_owned())),
                    history: Some(HistoryKind::AgentMessage),
                    ..Default::default()
                },
            ) {
                Ok(body) => body,
                Err(Error::PromptAborted) => return Ok(()),
                Err(e) => return Err(e),
            };
            local.send_message(&body, thread.as_deref(), &version)?;
            app.display_info("Message sent");
            Ok(())
        }))
    }

    fn is_target_op(&self) -> bool {
        true
    }

    fn display(&self, _state: &State) -> String {
        "Send message".into()
    }
}

pub(crate) struct ShowOnGithub;
impl OpTrait for ShowOnGithub {
    fn get_action(&self, target: &ItemData) -> Option<Action> {
        if let ItemData::PrSummary = target {
            return Some(Rc::new(|app, _term| {
                let Some(pr) = app.screen().pr.clone() else {
                    app.display_error(NO_REVIEW);
                    return Ok(());
                };
                match pr.backend.url() {
                    Some(url) => app.state.github.open(&url),
                    None => {
                        app.display_error(PR_NOT_ON_GITHUB);
                        Ok(())
                    }
                }
            }));
        }
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
        "Open in GitHub".into()
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
            line_indices,
            ..
        } => {
            // A side-by-side row of a removed line and an added line is the added line's.
            let content = diff.hunk_content(*file_i, *hunk_i);
            let added = line_indices.iter().copied().find(|&i| {
                content
                    .lines()
                    .nth(i)
                    .is_some_and(|line| line.starts_with('+'))
            });
            (diff, *file_i, *hunk_i, added.unwrap_or(*line_i))
        }
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
            // The PR's summary has no line to comment on, but from it `i` goes back to a box
            // being written, as from a file's row.
            ItemData::PrSummary => None,
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

/// What to ask before changing a comment others can see.
const PUBLIC_EDIT: &str = "This comment is public on GitHub: change it there? [y/n]";
const PUBLIC_DELETE: &str = "This comment is public on GitHub: delete it there? [y/n]";

/// Delete a comment, once the user confirms: one in a submitted review, which others can see, as
/// such.
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
        let public = pr
            .comment(&thread_id, &comment_id)
            .is_some_and(|c| c.submitted);
        let consent = if public {
            app.confirm(term, PUBLIC_DELETE)?;
            Consent::Publish
        } else {
            app.confirm(term, "Really delete the comment? [y/n]")?;
            Consent::Private
        };
        let message = pr
            .backend
            .delete_comment(&thread_id, &comment_id, consent)?;
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

/// Edit the comment box that has focus as `key` says, or leave it: a box with nothing written in
/// it goes when left.
pub(crate) fn handle_editor_key(app: &mut App, term: &mut Term, key: KeyEvent) -> Res<()> {
    let Some(pr) = app.screen().pr.clone() else {
        return Ok(());
    };
    let outcome = {
        let mut slot = pr.editor.borrow_mut();
        let Some(editor) = slot.as_mut() else {
            return Ok(());
        };
        let width = editor.width;
        let outcome = editor.text.handle_key(key, width);
        if outcome == Outcome::Leave {
            if editor.text.value().is_empty() {
                *slot = None;
            } else {
                editor.focused = false;
            }
        }
        outcome
    };
    match outcome {
        Outcome::Edited | Outcome::Moved | Outcome::Leave => show_editor(app),
        Outcome::Submit => post_comment(app, term)?,
        Outcome::Ignored => {}
    }
    Ok(())
}

/// Post the comment being written, as a reply to its thread or on its line, and read the threads
/// again to show it, with the box gone. A new comment is not posted while HEAD is not the PR's
/// head on GitHub, which its line is numbered against. What was written is kept if it is not
/// posted.
fn post_comment(app: &mut App, term: &mut Term) -> Res<()> {
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
        } => {
            // A comment in a submitted review is public: changing it is asked first.
            let consent = if pr
                .comment(thread_id, comment_id)
                .is_some_and(|c| c.submitted)
            {
                app.confirm(term, PUBLIC_EDIT)?;
                Consent::Publish
            } else {
                Consent::Private
            };
            pr.backend
                .edit_comment(thread_id, comment_id, &body, consent)?
        }
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
