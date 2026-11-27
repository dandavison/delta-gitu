pub mod app;
mod bindings;
mod calling_process;
mod capped_input;
pub mod cli;
mod cmd_log;
pub mod config;
mod config_edit;
mod config_git;
mod config_migrate;
mod diff_renderer;
mod difftastic;
pub mod error;
mod file_watcher;
mod git;
mod gitu_diff;
mod item_data;
mod items;
mod key_parser;
mod menu;
mod ops;
mod picker;
mod prompt;
mod rebase_todo;
mod review;
mod screen;
mod search;
mod show_config;
mod stream;
mod style;
pub mod term;
#[cfg(test)]
mod tests;
mod text_area;
mod text_input;
mod ui;

use bindings::Bindings;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventState, KeyModifiers};
use error::Error;
use git2::Repository;
use items::Item;
use ops::Action;
use std::{
    env,
    io::{self, IsTerminal},
    path::Path,
    rc::Rc,
    sync::Arc,
    time::Duration,
};
use term::Term;

use crate::calling_process::GitCommand;
use crate::capped_input::Capped;
use crate::config::Config;
use crate::screen::pager::Paged;

pub const LOG_FILE_NAME: &str = "delta-tui.log";

//                                An overview of Gitu's ui and terminology:
//
//                Screen (src/screen/*)
//                  │
//                  ▼
//                 ┌──────────────────────────────────────────────────────────────────┐
//        Item───┬─► Branch: master                                                   │
//        Item   └─► Your branch is up to date with 'origin/master'.                  │
//        ...      │                                                                  │
//                 │ Untracked files                                                  │
//                 │ src/tests/rebase.rs                                              │
//                 │                                                                  │
//                 │ Unstaged changes (4)                                             │
//                 │▌modified   src/keybinds.rs…                                      │
//                 │ modified   src/ops/mod.rs…                                       │
//                 │ modified   src/ops/rebase.rs…                                    │
//                 │ modified   src/tests/mod.rs…                                     │
//                 │                                                                  │
//                 │ Stashes                                                          │
//                 │ stash@0 On master: scroll                                        │
//                 │ stash@1 WIP on fix/run-cmd-error-on-bad-exit: 098d14a feat: prom…│
//                 │                                                                  │
//                 │ Recent commits                                                   │
// Ops (src/ops/*) ├──────────────────────────────────────────────────────────────────┤
//       │         │Help                        Submenu      modified   src/keybinds.r│
//       └─────┬───►g Refresh                   h Help       ret Show                 │
//             └───►tab Toggle section          b Branch     K Discard                │
//                 │k p ↑ Move up               c Commit     s Stage                  │
//                 │j n ↓ Move down             f Fetch      u Unstage                │
// Submenu ───────►│C-k C-p C-↑ Move up line    l Log                                 │
//                 │C-j C-n C-↓ Move down line  F Pull                                │
//                 │C-u Half page up            P Push                                │
//                 │C-d Half page down          r Rebase                              │
//                 │y Show refs                 X Reset                               │
//                 │                            z Stash                               │
//                 └──────────────────────────────────────────────────────────────────┘

pub type Res<T> = Result<T, Error>;

/// Run gitu, returning the status to exit with.
pub fn run(config: Arc<Config>, args: &cli::Args, term: &mut Term) -> Res<i32> {
    // Before anything that reads the TUI's settings, which refuses an old [delta-tui] section.
    if args.migrate_old_config {
        config_migrate::print()?;
        return Ok(0);
    }
    let reviewing = args.agent_instructions
        || args.pr_state_machine_diagram.is_some()
        || matches!(args.command, Some(cli::Commands::Review { .. }));
    // `experimental` is set in git config, which `config` has not read yet.
    if reviewing {
        let dir = match open_repo_from_env() {
            Ok(repo) => repo.workdir().unwrap_or(repo.path()).to_owned(),
            Err(_) => std::env::current_dir().map_err(Error::Term)?,
        };
        if !config.resolved(&dir, None)?.general.experimental {
            return Err(Error::LocalPr(config::NOT_EXPERIMENTAL.to_owned()));
        }
    }
    if args.agent_instructions {
        print!("{}", review::AGENT_INSTRUCTIONS);
        return Ok(0);
    }
    if let Some(format) = args.show_config {
        let dir = std::env::current_dir().map_err(Error::Term)?;
        let settings = show_config::settings(&config, &dir)?;
        match format {
            cli::ConfigFormat::Text => print!(
                "{}",
                show_config::text(&settings, io::stdout().is_terminal())
            ),
            cli::ConfigFormat::Json => println!(
                "{}",
                serde_json::to_string_pretty(&show_config::json(&settings)).expect("JSON")
            ),
        }
        return Ok(0);
    }
    if let Some(cli::DiagramFormat::D2) = args.pr_state_machine_diagram {
        print!("{}", review::diagram::d2());
        return Ok(0);
    }
    if let Some(cli::Commands::Review { command }) = &args.command {
        review::cli::run(
            command,
            open_repo_from_env()?,
            Rc::new(review::github::Gh {
                workdir: std::env::current_dir().map_err(Error::Term)?,
            }),
            &mut io::stdin(),
            &mut io::stdout(),
        )?;
        return Ok(0);
    }
    needs_terminal(io::stdout().is_terminal())?;
    // Before anything slower, and before reading the input: the git process on
    // the other end of a pipe exits as soon as its output fits in the pipe, and
    // cannot be found once it has. Reading to EOF is always too late — EOF is
    // git closing the pipe.
    let paging = pages(args, io::stdin().is_terminal());
    let git_argv = paging.then(calling_process::paging_for).flatten();
    log::debug!("Paging the output of {git_argv:?}");

    log::debug!("Opening repo");
    let repo = open_repo_from_env()?;
    let config = Arc::new(config.resolved(repo.workdir().unwrap_or(repo.path()), None)?);

    let piped = paging
        .then(|| read_piped_input(git_argv, repo.workdir(), config.general.max_input_bytes))
        .transpose()?;

    let mut app = app::App::create(
        Rc::new(repo),
        term.size().map_err(Error::Term)?,
        args,
        Arc::clone(&config),
        true,
        piped,
    )?;

    if args.quit_if_one_screen && paging && app.print_if_one_screen(term)? {
        return Ok(app.state.exit_code);
    }

    if args.print {
        return run_app(&mut app, args, term);
    }

    term.setup_term(&config).map_err(Error::Term)?;
    let result = run_app(&mut app, args, term);
    term.reset_term(&config).map_err(Error::Term)?;
    result
}

/// gitu draws on its output, which must be a terminal.
pub(crate) fn needs_terminal(stdout_is_terminal: bool) -> Res<()> {
    if stdout_is_terminal {
        Ok(())
    } else {
        Err(Error::NotATerminal)
    }
}

/// Whether gitu browses a patch on stdin, as git's pager, rather than opening
/// the status view or a subcommand's screen.
pub(crate) fn pages(args: &cli::Args, stdin_is_terminal: bool) -> bool {
    args.command.is_none() && !stdin_is_terminal
}

fn run_app(app: &mut app::App, args: &cli::Args, term: &mut Term) -> Res<i32> {
    if let Some(keys_string) = &args.keys {
        let ("", keys) = key_parser::parse_keys(keys_string).expect("Couldn't parse keys") else {
            panic!("Couldn't parse keys");
        };

        replay_keys(app, term, keys_to_events(&keys), !args.print)?;
    }

    app.redraw_now(term)?;

    if args.print {
        return Ok(app.state.exit_code);
    }

    app.run(term, Duration::from_millis(100))?;

    Ok(app.state.exit_code)
}

/// Handle `events`, keys given on the command line, as typed keys are handled: when `draw`, with
/// the screen drawn before each, so that a menu one opens shows, as does what one is busy with,
/// over the screen rather than a blank one. `--print` draws only the frame after them.
pub(crate) fn replay_keys(
    app: &mut app::App,
    term: &mut Term,
    events: Vec<event::Event>,
    draw: bool,
) -> Res<()> {
    for event in events {
        if draw {
            app.redraw_now(term)?;
        }
        app.handle_event(term, event)?;
    }
    Ok(())
}

/// What git piped to us as its pager, alongside the command already found to
/// have produced it.
fn read_piped_input(
    git_argv: Option<Vec<String>>,
    workdir: Option<&Path>,
    limit: u64,
) -> Res<Paged> {
    paged(io::stdin(), git_argv, workdir, limit)
}

/// What git piped, unless gitu found the command that produced it: then gitu
/// asks git again itself (see [`screen::pager`]), and reading the output would
/// only wait for git to write what is discarded. git, left blocked on a full
/// pipe, ends with SIGPIPE when gitu exits, as it does when less quits early.
fn paged(
    input: impl io::Read,
    git_argv: Option<Vec<String>>,
    workdir: Option<&Path>,
    limit: u64,
) -> Res<Paged> {
    if git_argv.clone().and_then(GitCommand::of).is_some() {
        return Ok(Paged {
            text: String::new(),
            truncated: false,
            git_argv,
            prefix: command_prefix(workdir),
        });
    }

    let input = Capped::read(input, limit).map_err(Error::ReadPipedInput)?;
    Ok(Paged {
        text: input.text,
        truncated: input.truncated,
        git_argv,
        prefix: String::new(),
    })
}

/// Where git ran the command it is paging (see [`GitCommand::prefix`]). git
/// runs its pager from the repository's root and says in `GIT_PREFIX` where it
/// was run; in a shell pipeline, gitu runs where git did, in `workdir`.
fn command_prefix(workdir: Option<&Path>) -> String {
    if let Ok(prefix) = env::var("GIT_PREFIX") {
        return prefix;
    }

    let relative = || {
        let dir = env::current_dir().ok()?.canonicalize().ok()?;
        let root = workdir?.canonicalize().ok()?;
        Some(dir.strip_prefix(root).ok()?.to_str()?.to_owned())
    };
    match relative() {
        Some(dir) if !dir.is_empty() => format!("{dir}/"),
        _ => String::new(),
    }
}

fn open_repo_from_env() -> Res<Repository> {
    match Repository::open_from_env() {
        Ok(repo) => Ok(repo),
        Err(err) => Err(Error::OpenRepo(err)),
    }
}

fn keys_to_events(keys: &[(KeyModifiers, KeyCode)]) -> Vec<Event> {
    keys.iter()
        .map(|(mods, key)| {
            Event::Key(KeyEvent {
                code: *key,
                modifiers: *mods,
                kind: event::KeyEventKind::Press,
                state: KeyEventState::NONE,
            })
        })
        .collect::<Vec<_>>()
}
