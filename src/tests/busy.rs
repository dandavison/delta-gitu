//! While the TUI is busy for long enough, as opening a PR over a slow network, it says what with,
//! and keys given on the command line are shown as typed keys are.

use super::*;
use std::time::Duration;

/// What the test terminal says the TUI was busy with.
fn busy(ctx: &TestContext) -> Vec<String> {
    let crate::term::TermBackend::Test { buffer, .. } = &ctx.term else {
        unreachable!();
    };
    buffer.busy.lock().unwrap().clone()
}

/// The frames the test terminal has drawn.
fn frames(ctx: &TestContext) -> Vec<String> {
    let crate::term::TermBackend::Test { buffer, .. } = &ctx.term else {
        unreachable!();
    };
    buffer.frames.clone()
}

/// Opening a PR whose GitHub is slow says what the TUI is busy with.
#[test]
fn a_slow_action_says_what_it_is_busy_with() {
    let mut ctx = setup_clone!();
    let github = super::pr::setup(&ctx);
    *github.delay.borrow_mut() = Duration::from_millis(500);
    let mut app = super::pr::init_app(&mut ctx, &github);

    ctx.update(&mut app, keys("OO"));

    assert_eq!(busy(&ctx), ["Open the current branch's PR"]);
}

/// A quick action says nothing of being busy.
#[test]
fn a_quick_action_says_nothing() {
    let mut ctx = setup_clone!();
    let mut app = ctx.init_app();

    ctx.update(&mut app, keys("jk"));

    assert_eq!(busy(&ctx), Vec::<String>::new());
}

/// Keys given on the command line are shown as typed ones are: the screen is drawn before each, so
/// that the menu one opens shows, and a spinner for what one is busy with shows over the screen
/// rather than over a blank one.
#[test]
fn keys_given_on_the_command_line_are_drawn_as_typed() {
    let mut ctx = setup_clone!();
    let mut app = ctx.init_app();
    let before = frames(&ctx).len();

    crate::replay_keys(&mut app, &mut ctx.term, keys("ll"), true).unwrap();

    let frames = frames(&ctx);
    assert_eq!(frames.len(), before + 2, "{frames:?}");
    assert_ne!(frames[before], frames[before + 1], "the log menu shows");
}
