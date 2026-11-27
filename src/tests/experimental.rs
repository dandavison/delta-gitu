//! With `experimental` off, as by default, code review and messages to agents are not there: no
//! keys, commands or settings for them. `.` opens a file whole, but its lines take no comments.

use super::local_pr::{review_cli_with_input, setup};
use super::*;
use clap::Parser;

/// The app in `ctx`, with experimental features off.
fn init_app_off(ctx: &mut TestContext) -> crate::app::App {
    *ctx.config() = crate::config::init_test_config_with(false).unwrap();
    ctx.init_app()
}

/// The help lists no PR menu, and `O` opens none.
#[test]
fn there_are_no_pr_keys() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    let mut app = init_app_off(&mut ctx);

    ctx.update(&mut app, keys("h"));
    let help = ctx.redact_buffer();
    assert!(!help.contains("PR"), "{help}");
    assert!(!help.contains("Message the agent"), "{help}");
    assert!(help.contains("Open a file"), "{help}");

    drop(app);
    let mut app = init_app_off(&mut ctx);
    ctx.update(&mut app, keys("OO"));
    let screen = ctx.redact_buffer();
    assert!(!screen.contains("added: added.txt"), "{screen}");
}

/// `.` opens a file whole, but `i` writes no comment on it, and nothing is recorded.
#[test]
fn dot_opens_a_file_whole_that_takes_no_comments() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    let mut app = init_app_off(&mut ctx);

    ctx.update(&mut app, keys(".lines<enter>/new<enter>"));
    assert!(
        ctx.redact_buffer().contains("keep"),
        "{}",
        ctx.redact_buffer()
    );
    ctx.update(&mut app, keys("ihello"));

    let screen = ctx.redact_buffer();
    assert!(!screen.contains("New comment"), "{screen}");
    assert!(!ctx.dir.join(".git/delta/review").exists(), "{screen}");
}

/// The status view shows no PR, though the branch has a local PR.
#[test]
fn the_status_view_shows_no_pr() {
    let mut ctx = setup_clone!();
    setup(&mut ctx);
    review_cli_with_input(
        &ctx.dir,
        &["comment", "--path", "lines.txt", "--line", "2"],
        "hello",
    )
    .unwrap();

    let mut app = init_app_off(&mut ctx);
    ctx.update(&mut app, keys(""));

    let screen = ctx.redact_buffer();
    assert!(!screen.contains("PR into main"), "{screen}");
}

/// `review` and the agents' flags are refused, saying how to turn experimental features on.
#[test]
fn review_commands_are_refused() {
    let mut ctx = setup_clone!();
    let config = std::sync::Arc::new(crate::config::init_test_config_with(false).unwrap());
    for args in [
        &["gitu", "review", "show"][..],
        &["gitu", "--agent-instructions"],
        &["gitu", "--pr-state-machine-diagram=d2"],
    ] {
        let args = crate::cli::Args::parse_from(args);
        let error = crate::run(std::sync::Arc::clone(&config), &args, &mut ctx.term)
            .unwrap_err()
            .to_string();
        assert!(error.contains("experimental = true"), "{args:?}: {error}");
    }
}

/// With `experimental = true` in git config, as delta passes the config before reading git
/// config, `review` and the agents' flags run.
#[test]
fn review_commands_run_with_experimental_in_git_config() {
    let mut ctx = setup_clone!();
    let dir = temp_dir::TempDir::new().unwrap();
    let git_path = dir.path().join("gitconfig");
    std::fs::write(&git_path, "[delta]\n\texperimental = true\n").unwrap();
    let mut config = crate::config::init_config_from(dir.path().join("config.toml")).unwrap();
    crate::config::told_about(&mut config, git_path);
    let config = std::sync::Arc::new(config);
    for args in [
        &["gitu", "--agent-instructions"][..],
        &["gitu", "--pr-state-machine-diagram=d2"],
    ] {
        let args = crate::cli::Args::parse_from(args);
        let result = crate::run(std::sync::Arc::clone(&config), &args, &mut ctx.term);
        assert!(result.is_ok(), "{args:?}: {result:?}");
    }
}

/// `--help` doesn't mention them.
#[test]
fn help_does_not_mention_review() {
    let help = crate::cli::Args::try_parse_from(["gitu", "--help"])
        .unwrap_err()
        .to_string();
    assert!(!help.contains("review"), "{help}");
    assert!(!help.contains("agent-instructions"), "{help}");
    assert!(!help.contains("pr-state-machine-diagram"), "{help}");
}

/// `--show-config` lists no setting, style or key of theirs.
#[test]
fn show_config_lists_nothing_of_theirs() {
    let mut ctx = setup_clone!();
    *ctx.config() = crate::config::init_test_config_with(false).unwrap();
    let dir = ctx.dir.clone();

    let shown = crate::show_config::settings(ctx.config(), &dir).unwrap();
    let text = crate::show_config::text(&shown, false);

    for name in [
        "comment-style",
        "experimental",
        "send-message-command",
        "toggle-viewed",
        "pr-menu",
        "message-agent",
    ] {
        assert!(!text.contains(name), "{name}:\n{text}");
    }
    assert!(text.contains("open-file"), "{text}");
}
