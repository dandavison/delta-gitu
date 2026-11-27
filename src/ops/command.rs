//! Shell commands and views the user defines in `general.commands` and
//! `general.views` and binds to keys.

use crate::{
    Res,
    app::{App, PromptParams},
    calling_process::GitCommand,
    error::Error,
    item_data::{ItemData, Ref},
    prompt::HistoryKind,
    screen,
    term::Term,
};
use std::{rc::Rc, sync::Arc};

/// Run the command `name`, with what it asks for filled in from the item under
/// the cursor, and `{input}` from a prompt, as a fetch is run: in the
/// repository's root, while gitu stays usable, with the width and features gitu
/// draws with in its environment. Its output is not shown, unless with
/// `--debug`; one that fails says so. A command asking for what is not under
/// the cursor is not run, nor one whose prompt is cancelled or left empty.
pub(crate) fn run(app: &mut App, term: &mut Term, name: &str) -> Res<()> {
    let Some(template) = app.state.config.general.commands.get(name) else {
        app.display_error(format!("{name}: there is no command of that name"));
        return Ok(());
    };
    let mut template = template.clone();
    if template.contains(COMMAND) {
        let Some((line, _)) = shown_command(app) else {
            app.display_error(no_command(name));
            return Ok(());
        };
        // Already as a shell takes it.
        template = template.replace(COMMAND, &line);
    }
    let Some(values) = values_or_say_why(app, term, name, &template)? else {
        return Ok(());
    };
    let command = filled_in(&template, &values, |value| {
        shell_words::quote(value).into_owned()
    });

    // A delta it runs draws as gitu draws the screen.
    let params = app.render_params(term.size().map_err(Error::Term)?);
    let env = crate::diff_renderer::drawing_env(&params);
    app.run_shell_async(term, &command, env)
}

/// Open the view `name` on top of the screen in view, with what it asks for
/// filled in from the item under the cursor, and `{input}` from a prompt, as
/// one word. A view asking for what is not under the cursor, or that is not a
/// `git diff`, `git show` or `git log`, is not opened, nor one whose prompt is
/// cancelled or left empty.
pub(crate) fn open_view(app: &mut App, term: &mut Term, name: &str) -> Res<()> {
    let Some(template) = app.state.config.general.views.get(name) else {
        app.display_error(format!("{name}: there is no view of that name"));
        return Ok(());
    };
    let Ok(words) = shell_words::split(template) else {
        app.display_error(format!("{name}: {}", Error::EditedCommandQuotes));
        return Ok(());
    };
    // The command shown is several words, so it is a word of its own here, and
    // stands for those words.
    if words
        .iter()
        .any(|word| word != COMMAND && word.contains(COMMAND))
    {
        app.display_error(format!("{name}: {COMMAND} must be a word of its own"));
        return Ok(());
    }
    let mut prefix = String::new();
    let mut spliced = Vec::new();
    for word in words {
        if word != COMMAND {
            spliced.push(word);
            continue;
        }
        let Some((line, shown_prefix)) = shown_command(app) else {
            app.display_error(no_command(name));
            return Ok(());
        };
        // Its paths are relative to where it was run.
        prefix = shown_prefix;
        spliced.extend(shell_words::split(&line).map_err(|_| Error::EditedCommandQuotes)?);
    }

    let Some(values) = values_or_say_why(app, term, name, &spliced.join(" "))? else {
        return Ok(());
    };
    // Word by word, so that a value with spaces stays one word.
    let argv: Vec<String> = spliced
        .iter()
        .map(|word| filled_in(word, &values, |value| value.to_owned()))
        .collect();
    if GitCommand::of(argv.clone()).is_none() {
        app.display_error(format!(
            "{name}: a view is a git diff, git show or git log command"
        ));
        return Ok(());
    }

    let params = app.render_params(term.size().map_err(Error::Term)?);
    let screen = screen::pager::live_in(
        Arc::clone(&app.state.config),
        Rc::clone(&app.state.repo),
        params,
        argv,
        prefix,
    )?;
    app.state.screens.push(screen);
    Ok(())
}

/// The placeholder for the git command the screen is showing.
const COMMAND: &str = "{command}";

/// The placeholder for what is typed at the prompt the command's key opens.
const INPUT: &str = "{input}";

/// The git command the screen in view is showing, as `:` shows it, and where it
/// was run (see [`GitCommand::prefix`]). `None` for a screen that is not a git
/// command's output, such as the status view.
fn shown_command(app: &App) -> Option<(String, String)> {
    let git = app.screen().git_command.as_ref()?.borrow();
    Some((
        git.line(app.state.context.as_deref()),
        git.prefix().to_owned(),
    ))
}

fn no_command(name: &str) -> String {
    format!("{name}: this screen shows no git command, for {COMMAND}")
}

/// What to say when the command or view `name` asks for `placeholder` and the
/// cursor is on nothing that has one.
fn nothing_for(app: &App, name: &str, placeholder: &str) -> String {
    let message = format!("{name}: nothing under the cursor for {placeholder}");
    match app.screen().unmarked_commits_hint() {
        Some(hint) if placeholder == "{commit}" => format!("{message}: {hint}"),
        _ => message,
    }
}

/// The value of each placeholder `template` uses: from `target`, the item under
/// the cursor, and then `{input}` typed at a prompt labelled with `name`.
/// `None`, having said why, when `target` has no value for one, and `None` when
/// the prompt is cancelled or left empty. The prompt opens only once the values
/// from `target` are known, so that nothing typed is wasted.
fn values_or_say_why(
    app: &mut App,
    term: &mut Term,
    name: &str,
    template: &str,
) -> Res<Option<Vec<(&'static str, String)>>> {
    let target = &app.screen().get_selected_item().data;
    let mut values = vec![];
    for (placeholder, value) in [
        ("{commit}", commit_of(target)),
        ("{file}", file_of(target)),
        ("{branch}", branch_of(target)),
    ] {
        if !template.contains(placeholder) {
            continue;
        }
        let Some(value) = value else {
            app.display_error(nothing_for(app, name, placeholder));
            return Ok(None);
        };
        values.push((placeholder, value));
    }
    if template.contains(INPUT) {
        let input = match app.prompt(
            term,
            &PromptParams {
                prompt: name.to_owned().into(),
                history: Some(HistoryKind::Input(name.to_owned())),
                ..Default::default()
            },
        ) {
            Ok(input) => input,
            Err(Error::PromptAborted) => return Ok(None),
            Err(e) => return Err(e),
        };
        if input.is_empty() {
            return Ok(None);
        }
        values.push((INPUT, input));
    }
    Ok(Some(values))
}

/// `template` with each placeholder in `values` replaced by `written` of its
/// value, in one pass, so that a value is never itself filled in.
fn filled_in(
    template: &str,
    values: &[(&'static str, String)],
    written: impl Fn(&str) -> String,
) -> String {
    let mut filled = String::new();
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        filled.push_str(&rest[..start]);
        rest = &rest[start..];
        match values
            .iter()
            .find(|(placeholder, _)| rest.starts_with(placeholder))
        {
            Some((placeholder, value)) => {
                filled.push_str(&written(value));
                rest = &rest[placeholder.len()..];
            }
            None => {
                filled.push('{');
                rest = &rest[1..];
            }
        }
    }
    filled.push_str(rest);
    filled
}

fn commit_of(target: &ItemData) -> Option<String> {
    match target {
        ItemData::Commit { oid, .. } => Some(oid.clone()),
        ItemData::BlameHeader { commit_hash, .. } | ItemData::BlameCodeLine { commit_hash, .. } => {
            Some(commit_hash.clone())
        }
        ItemData::Delta { commit, .. } => commit.clone(),
        ItemData::Hunk { diff, .. } | ItemData::HunkLine { diff, .. } => diff.commit.clone(),
        _ => None,
    }
}

/// The file a row is of. A deleted file is named as it was.
fn file_of(target: &ItemData) -> Option<String> {
    let (diff, file_i) = match target {
        ItemData::Untracked(path) => return Some(path.to_string_lossy().into_owned()),
        ItemData::Delta { diff, file_i, .. }
        | ItemData::Hunk { diff, file_i, .. }
        | ItemData::HunkLine { diff, file_i, .. } => (diff, *file_i),
        _ => return None,
    };
    let header = &diff.file_diffs[file_i].header;
    let path = match header.status {
        crate::gitu_diff::Status::Deleted => &header.old_file,
        _ => &header.new_file,
    };
    Some(path.fmt(&diff.text).into_owned())
}

/// The branch a row is, or the first one at the commit it is.
fn branch_of(target: &ItemData) -> Option<String> {
    match target {
        ItemData::Reference {
            kind: Ref::Head(branch),
            ..
        } => Some(branch.clone()),
        ItemData::Commit {
            associated_references,
            ..
        } => associated_references
            .iter()
            .find_map(|reference| match reference {
                Ref::Head(branch) => Some(branch.clone()),
                _ => None,
            }),
        _ => None,
    }
}
