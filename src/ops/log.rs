use super::{Action, OpTrait, selected_rev};
use crate::{
    Res,
    app::{App, PromptParams, State},
    error::Error,
    item_data::{ItemData, Rev},
    menu::arg::{Arg, any_regex, positive_number},
    screen,
    term::Term,
};
use git2::Oid;
use regex::Regex;
use std::{rc::Rc, sync::Arc};

pub(crate) fn init_args() -> Vec<Arg> {
    vec![
        Arg::new_arg(
            "-n",
            "Limit number of commits",
            Some(|| 256),
            positive_number,
        ),
        Arg::new_arg("--grep", "Search messages", None, any_regex),
        // Arg::new_str("-S", "Search occurrences"), // TODO: Implement search
    ]
}

/// The current branch's log, or, on a PR's screen, the PR's: its commits, on top of its screen.
pub(crate) struct LogCurrent;
impl OpTrait for LogCurrent {
    fn get_action(&self, _target: &ItemData) -> Option<Action> {
        Some(Rc::new(|app: &mut App, _term: &mut Term| {
            let pr = app.screen().pr.clone();
            match pr {
                Some(pr) => {
                    let local = pr.backend.as_local().ok_or_else(|| {
                        Error::LocalPr(
                            "Couldn't show the PR's log: the PR on screen is not a local PR"
                                .to_owned(),
                        )
                    })?;
                    let range = local.log_range()?;
                    push_log_screen(app, vec![range])
                }
                None => goto_log_screen(app, None),
            }
        }))
    }

    fn display(&self, _state: &State) -> String {
        "current".into()
    }
}

pub(crate) struct LogOther;
impl OpTrait for LogOther {
    fn get_action(&self, _target: &ItemData) -> Option<Action> {
        Some(Rc::new(move |app: &mut App, term: &mut Term| {
            let rev = app.prompt(
                term,
                &PromptParams {
                    prompt: "Log rev".into(),
                    create_default_value: Box::new(|app| {
                        selected_rev(app)
                            .as_ref()
                            .map(Rev::shorthand)
                            .map(String::from)
                    }),
                    ..Default::default()
                },
            )?;

            log_other(app, term, &rev)?;
            Ok(())
        }))
    }

    fn display(&self, _state: &State) -> String {
        "other".into()
    }
}

fn log_other(app: &mut App, _term: &mut Term, result: &str) -> Res<()> {
    let oid_result = match app.state.repo.revparse_single(result) {
        Ok(rev) => Ok(rev.id()),
        Err(err) => Err(Error::FindGitRev(err)),
    };

    let oid = oid_result?;

    goto_log_screen(app, Some(oid))
}

fn goto_log_screen(app: &mut App, rev: Option<Oid>) -> Res<()> {
    app.state.screens.drain(1..);
    push_log_screen(app, rev.map(|oid| oid.to_string()).into_iter().collect())
}

/// Open the log of `revs` on top of the screen in view, as the log menu's arguments limit it.
fn push_log_screen(app: &mut App, revs: Vec<String>) -> Res<()> {
    let params = app.render_params(app.state.screens.last().unwrap().size);
    let limit = *app
        .state
        .pending_menu
        .as_ref()
        .and_then(|m| m.args.get("-n"))
        .and_then(|arg| arg.value_as::<u32>())
        .unwrap_or(&u32::MAX);

    let msg_regex_menu = app
        .state
        .pending_menu
        .as_ref()
        .and_then(|m| m.args.get("--grep"));

    let msg_regex = msg_regex_menu.and_then(|arg| arg.value_as::<Regex>().cloned());

    app.state.screens.push(screen::pager::log(
        Arc::clone(&app.state.config),
        Rc::clone(&app.state.repo),
        params,
        limit as usize,
        revs,
        msg_regex.as_ref(),
    )?);
    Ok(())
}
