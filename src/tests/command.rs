//! Shell commands the user defines in config and binds to keys.

use super::*;
use std::time::{Duration, Instant};

/// `ctx` with `command` defined as `name`, bound to `key`.
fn with_command(mut ctx: TestContext, name: &str, command: &str, key: char) -> TestContext {
    let config = ctx.config();
    config
        .general
        .commands
        .insert(name.to_owned(), command.to_owned());
    config
        .general
        .diff_renderer
        .feature_keys
        .insert(name.to_owned(), key);
    ctx
}

/// `{commit}` is the commit under the cursor.
#[test]
fn a_command_key_runs_the_command_on_the_commit_under_the_cursor() {
    let ctx = setup_clone!();
    commit(&ctx.dir, "a", "");
    let mut ctx = with_command(ctx, "record", "echo {commit} > recorded", 't');
    let mut app = ctx.init_app();

    ctx.update(&mut app, keys("llt"));

    let head = run(&ctx.dir, &["git", "rev-parse", "HEAD"]);
    assert_eq!(
        fs::read_to_string(ctx.dir.join("recorded")).unwrap(),
        head,
        "{}",
        ctx.redact_buffer()
    );
}

/// `{file}` is the file under the cursor, quoted for the shell.
#[test]
fn a_command_key_runs_the_command_on_the_file_under_the_cursor() {
    let ctx = setup_clone!();
    fs::write(ctx.dir.join("a file"), "").unwrap();
    let mut ctx = with_command(ctx, "record", "echo {file} > recorded", 't');
    let mut app = ctx.init_app();

    ctx.update(&mut app, keys("/a file<enter>t"));

    assert_eq!(
        fs::read_to_string(ctx.dir.join("recorded")).unwrap(),
        "a file\n",
        "{}",
        ctx.redact_buffer()
    );
}

/// A command asking for what is not under the cursor is not run, and says why.
#[test]
fn a_command_needing_a_commit_is_not_run_off_one() {
    let ctx = setup_clone!();
    let mut ctx = with_command(ctx, "record", "echo {commit} > recorded", 't');
    let mut app = ctx.init_app();

    ctx.update(&mut app, keys("t"));

    assert!(!ctx.dir.join("recorded").exists());
    assert!(
        ctx.redact_buffer().contains("{commit}"),
        "{}",
        ctx.redact_buffer()
    );
}

/// The list of features lists the commands too, where their keys are set.
#[test]
fn the_feature_list_lists_commands() {
    let ctx = setup_clone!();
    let mut ctx = with_command(ctx, "record", "echo {commit} > recorded", 't');
    let mut app = ctx.init_app();

    ctx.update(&mut app, keys("H"));

    assert!(
        ctx.redact_buffer().contains("t record"),
        "{}",
        ctx.redact_buffer()
    );
}

/// A command that fails is shown as it was written, filled in, on a line of its
/// own, to be pasted into a shell (not the `sh -c` that runs it), then how it
/// exited and what it wrote to stderr.
#[test]
fn a_failing_command_shows_its_stderr_and_the_command_as_written() {
    let ctx = setup_clone!();
    commit(&ctx.dir, "a", "");
    let mut ctx = with_command(ctx, "fail", "echo {commit} oops >&2; exit 3", 't');
    let mut app = ctx.init_app();

    ctx.update(&mut app, keys("llt"));

    let head = run(&ctx.dir, &["git", "rev-parse", "HEAD"]);
    let head = head.trim();
    let buffer = ctx.redact_buffer();
    let rows: Vec<&str> = buffer
        .lines()
        .skip_while(|row| !row.starts_with('!'))
        .take(4)
        .map(str::trim_end)
        .map(|row| row.trim_end_matches('|').trim_end())
        .collect();
    assert_eq!(
        rows,
        [
            format!("! echo {head} oops >&2; exit 3"),
            "  exit code: 3".to_owned(),
            "  stderr:".to_owned(),
            format!("  {head} oops"),
        ],
        "{buffer}"
    );
}

/// `{command}` is the git command the screen is showing, as `:` shows it.
#[test]
fn a_command_key_runs_the_command_on_the_git_command_shown() {
    let ctx = setup_clone!();
    let mut ctx = with_command(ctx, "record", "echo {command} > recorded", 't');
    let mut app = ctx.init_app();

    ctx.update(&mut app, keys("llt"));

    let recorded = fs::read_to_string(ctx.dir.join("recorded")).unwrap_or_default();
    assert!(recorded.starts_with("git log"), "{recorded:?}");
}

/// A command is given the width and the features gitu draws with, as delta
/// takes them, so that a delta it runs draws as the screen is drawn.
#[test]
fn a_command_gets_the_width_and_features_gitu_draws_with() {
    let ctx = setup_clone!();
    let mut ctx = with_command(
        ctx,
        "record",
        r#"printf '%s %s' "$COLUMNS" "$DELTA_FEATURES" > recorded"#,
        't',
    );
    let mut app = ctx.init_app();

    ctx.update(&mut app, keys("St"));

    let recorded = fs::read_to_string(ctx.dir.join("recorded")).unwrap_or_default();
    assert!(recorded.starts_with("78 "), "{recorded:?}");
    assert!(recorded.contains("side-by-side"), "{recorded:?}");
}

/// A command may write more than a pipe holds. Were its output read only once
/// it exits, it would wait forever for room to write; here the command stops
/// its writer after 5s, and does not record that it finished.
#[test]
fn a_command_writing_much_output_finishes() {
    let ctx = setup_clone!();
    let mut ctx = with_command(
        ctx,
        "write",
        "yes | head -c 1000000 & w=$!; \
         (sleep 5; kill $w) >/dev/null 2>&1 & \
         wait $w && touch finished",
        't',
    );
    let mut app = ctx.init_app();

    ctx.update(&mut app, keys("t"));

    assert!(ctx.dir.join("finished").exists(), "{}", ctx.redact_buffer());
}

/// While a command runs that the TUI waits for, such as `git checkout`, its spinner turns.
#[test]
fn the_spinner_turns_while_the_tui_waits_for_a_command() {
    let mut ctx = setup_clone!();
    ctx.debug = false;
    run(&ctx.dir, &["git", "branch", "other"]);
    let hook = ctx.dir.join(".git/hooks/post-checkout");
    fs::write(&hook, "#!/bin/sh\nsleep 0.5\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let mut app = ctx.init_app_streaming_with_args(Default::default());

    ctx.update(&mut app, keys("bbother<enter>"));

    let crate::term::TermBackend::Test { buffer, .. } = &ctx.term else {
        unreachable!();
    };
    let spinners: std::collections::HashSet<char> = buffer
        .frames
        .iter()
        .flat_map(|frame| frame.lines())
        .filter_map(|line| {
            let row = line.trim();
            let spinner = row.strip_suffix("git checkout other")?.trim_end();
            let mut chars = spinner.chars();
            let spinner = chars.next()?;
            (chars.next().is_none() && spinner != '$').then_some(spinner)
        })
        .collect();
    assert!(
        spinners.len() > 1,
        "the spinner did not turn: {spinners:?}\n{}",
        ctx.redact_buffer()
    );
}

/// While a command runs, the bottom row says so, with a spinner that turns;
/// once it exits, the row is gone.
#[test]
fn a_running_command_is_shown_with_a_spinner_until_it_exits() {
    let ctx = setup_clone!();
    let mut ctx = with_command(
        ctx,
        "wait",
        "while [ ! -e go-on ]; do sleep 0.02; done",
        't',
    );
    ctx.debug = false;
    let mut app = ctx.init_app_streaming_with_args(Default::default());
    let shown = |ctx: &TestContext| ctx.redact_buffer().contains("while [ ! -e go-on ]");

    ctx.update(&mut app, keys("t"));
    assert!(shown(&ctx), "{}", ctx.redact_buffer());

    let before = ctx.redact_buffer();
    let started = Instant::now();
    while ctx.redact_buffer() == before {
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "the spinner did not turn:\n{before}"
        );
        std::thread::sleep(Duration::from_millis(50));
        app.update(&mut ctx.term, None).unwrap();
    }

    fs::write(ctx.dir.join("go-on"), "").unwrap();
    let started = Instant::now();
    while shown(&ctx) {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "still shown after the command exited:\n{}",
            ctx.redact_buffer()
        );
        std::thread::sleep(Duration::from_millis(50));
        app.update(&mut ctx.term, None).unwrap();
    }
}

/// `{input}` is what is typed at a prompt the command's key opens, labelled with
/// the command's name, and quoted for the shell.
#[test]
fn a_command_with_input_runs_with_what_is_typed_at_its_prompt() {
    let ctx = setup_clone!();
    let mut ctx = with_command(ctx, "record", "echo {input} > recorded", 't');
    let mut app = ctx.init_app();

    ctx.update(&mut app, keys("t"));
    assert_eq!(ctx.prompt_line(), "record: |");

    // A prompt left open when the keys run out is cancelled, so it is opened again.
    ctx.update(&mut app, keys("tsay \"hi\"; there<enter>"));
    assert_eq!(
        fs::read_to_string(ctx.dir.join("recorded")).unwrap(),
        "say \"hi\"; there\n",
        "{}",
        ctx.redact_buffer()
    );
}

/// `esc` at a command's prompt cancels it, and the command is not run.
#[test]
fn escape_at_a_commands_prompt_does_not_run_it() {
    let ctx = setup_clone!();
    let mut ctx = with_command(ctx, "record", "echo {input} > recorded", 't');
    let mut app = ctx.init_app();

    ctx.update(&mut app, keys("thello<esc>"));

    assert!(!ctx.dir.join("recorded").exists());
}

/// Nothing typed at a command's prompt cancels it, as `esc` does.
#[test]
fn nothing_typed_at_a_commands_prompt_does_not_run_it() {
    let ctx = setup_clone!();
    let mut ctx = with_command(ctx, "record", "echo {input} > recorded", 't');
    let mut app = ctx.init_app();

    ctx.update(&mut app, keys("t<enter>"));

    assert!(!ctx.dir.join("recorded").exists());
}

/// A command asking for what is not under the cursor says so before its prompt
/// opens, not after something has been typed.
#[test]
fn a_command_needing_a_commit_says_so_before_its_prompt_opens() {
    let ctx = setup_clone!();
    let mut ctx = with_command(ctx, "record", "echo {commit} {input} > recorded", 't');
    let mut app = ctx.init_app();

    ctx.update(&mut app, keys("t"));

    assert!(!ctx.prompt_line().starts_with("record:"));
    assert!(
        ctx.redact_buffer().contains("{commit}"),
        "{}",
        ctx.redact_buffer()
    );
}

/// Each command's prompt has a history of its own, which outlives gitu.
#[test]
fn each_commands_prompt_has_its_own_history() {
    let ctx = setup_clone!();
    let ctx = with_command(ctx, "record", "echo {input} > recorded", 't');
    let mut ctx = with_command(ctx, "other", "echo {input} > other", 'w');
    let mut app = ctx.init_app();
    ctx.update(&mut app, keys("tfirst<enter>"));
    drop(app);

    let mut app = ctx.init_app();
    ctx.update(&mut app, keys("t<up>"));
    assert_eq!(ctx.prompt_line(), "record: first|");

    let mut app = ctx.init_app();
    ctx.update(&mut app, keys("w<up>"));
    assert_eq!(ctx.prompt_line(), "other: |");
}
