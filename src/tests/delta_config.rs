//! The TUI's settings in delta's config, which change as the features in force do.

use super::*;

/// `ctx`'s config resolved from git config holding `gitconfig`, as delta would resolve it.
pub(super) fn with_delta_config(mut ctx: TestContext, gitconfig: &str) -> TestContext {
    let path = ctx.dir.join(".git").join("delta.gitconfig");
    // Experimental features on, as in the tests' own config.
    fs::write(
        &path,
        format!("[delta]\n\texperimental = true\n{gitconfig}"),
    )
    .unwrap();
    crate::config::told_about(ctx.config(), path);
    let dir = ctx.dir.clone();
    let resolved = ctx.config().resolved(&dir, None).unwrap();
    *ctx.config() = resolved;
    ctx.debug = false;
    ctx
}

/// `[delta "cmds"]` defines the command `hello`, run by `t`, and `w` turns `cmds` on and off.
const CMDS: &str = "[delta]\n\
                    \tfeature-key = cmds w\n\
                    \tfeature-key = hello t\n\
                    [delta \"cmds\"]\n\
                    \tcommand = hello touch hello-ran\n";

#[test]
fn a_command_in_a_feature_runs_once_the_features_key_turns_it_on() {
    let mut ctx = with_delta_config(setup_clone!(), CMDS);
    let mut app = ctx.init_app();

    ctx.update(&mut app, keys("t"));
    assert!(
        !ctx.dir.join("hello-ran").exists(),
        "ran while cmds was off"
    );

    ctx.update(&mut app, keys("wt"));
    assert!(
        ctx.dir.join("hello-ran").exists(),
        "{}",
        ctx.redact_buffer()
    );
}

#[test]
fn a_command_in_a_feature_does_not_run_once_the_feature_is_turned_off() {
    let mut ctx = with_delta_config(
        setup_clone!(),
        &format!("{CMDS}[delta]\n\tfeatures = cmds\n"),
    );
    let mut app = ctx.init_app();

    ctx.update(&mut app, keys("wt"));

    assert!(
        !ctx.dir.join("hello-ran").exists(),
        "ran while cmds was off"
    );
}
