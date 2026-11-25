use super::*;
use std::path::Path;

/// A change difftastic sees as the code's structure changing around expressions that stay the
/// same: a boolean expression rewritten as a `match`.
const OLD: &str = include_str!("fixtures/difftastic/match/old.rs");
const NEW: &str = include_str!("fixtures/difftastic/match/new.rs");

/// difftastic as the tests run it: whatever it is asked, it prints the output real difftastic
/// printed for the fixture `name`.
fn recorded_difftastic(name: &str) -> Vec<String> {
    let json = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/tests/fixtures/difftastic")
        .join(name)
        .join("difft.json");
    ["sh", "-c", &format!("cat '{}'", json.display()), "difft"]
        .map(String::from)
        .to_vec()
}

#[test]
fn unstaged_change_as_difftastic_sees_it() {
    let mut ctx = setup_clone!();
    commit(&ctx.dir, "src.rs", OLD);
    fs::write(ctx.dir.join("src.rs"), NEW).unwrap();
    ctx.config().general.difftastic.command = recorded_difftastic("match");
    snapshot!(ctx, "*jj<tab>");
}

#[test]
fn unstaged_change_as_git_sees_it_after_toggling_twice() {
    let mut ctx = setup_clone!();
    commit(&ctx.dir, "src.rs", OLD);
    fs::write(ctx.dir.join("src.rs"), NEW).unwrap();
    ctx.config().general.difftastic.command = recorded_difftastic("match");
    snapshot!(ctx, "**jj<tab>");
}

/// A line staged from difftastic's view is staged as git's patch has it: gitu stages from
/// git's patch, which adds the line after the lines it removes.
#[test]
fn stage_a_line_of_difftastics_hunk() {
    let mut ctx = setup_clone!();
    commit(&ctx.dir, "src.rs", OLD);
    fs::write(ctx.dir.join("src.rs"), NEW).unwrap();
    ctx.config().general.difftastic.command = recorded_difftastic("match");
    let mut app = ctx.init_app();
    // The hunk, its two context lines, then its sixth changed line, which difftastic finds new
    // throughout.
    ctx.update(&mut app, keys("*jj<tab>jjjjjjjjjs"));
    insta::assert_snapshot!(ctx.redact_buffer());
    assert_eq!(
        run(&ctx.dir, &["git", "show", ":src.rs"]),
        OLD.replace(
            "                && self.line.starts_with(\"+Subproject commit \")\n",
            "                && self.line.starts_with(\"+Subproject commit \")\n                    || self.line.starts_with(\"+Subproject commit \")\n",
        )
    );
}

#[test]
fn commit_as_difftastic_sees_it() {
    let mut ctx = setup_clone!();
    commit(&ctx.dir, "src.rs", OLD);
    commit(&ctx.dir, "src.rs", NEW);
    ctx.config().general.difftastic.command = recorded_difftastic("match");
    let mut app = ctx.init_app_as_pager_of(&["git", "show", "--format=%s"]);
    ctx.update(&mut app, keys("*"));
    insta::assert_snapshot!(ctx.redact_buffer());
}

/// Without difftastic, turning its view on says why it cannot be, and changes stay as git shows
/// them.
#[test]
fn difftastic_not_found() {
    let mut ctx = setup_clone!();
    commit(&ctx.dir, "src.rs", OLD);
    fs::write(ctx.dir.join("src.rs"), NEW).unwrap();
    ctx.config().general.difftastic.command = vec!["no-such-difftastic".to_owned()];
    snapshot!(ctx, "*");
}

/// The view is the delta feature named difftastic: a user whose config enables the feature sees
/// changes as difftastic sees them from the start.
#[test]
fn enabled_by_the_users_config() {
    let mut ctx = setup_clone!();
    commit(&ctx.dir, "src.rs", OLD);
    fs::write(ctx.dir.join("src.rs"), NEW).unwrap();
    ctx.config().general.difftastic.command = recorded_difftastic("match");
    ctx.config().general.diff_renderer.enabled_features =
        crate::config::EnabledFeatures::new(|_| vec!["difftastic".to_owned()]);
    snapshot!(ctx, "jj<tab>");
}

/// A field whose type is wrapped in `Option` is shown once, as it is now. Showing the old field
/// too, with nothing on it marked as removed, would show a struct with the field twice.
#[test]
fn a_changed_line_with_nothing_removed_is_shown_once() {
    let mut ctx = setup_clone!();
    commit(
        &ctx.dir,
        "fields.rs",
        include_str!("fixtures/difftastic/optional-field/old.rs"),
    );
    fs::write(
        ctx.dir.join("fields.rs"),
        include_str!("fixtures/difftastic/optional-field/new.rs"),
    )
    .unwrap();
    ctx.config().general.difftastic.command = recorded_difftastic("optional-field");
    let mut app = ctx.init_app();
    ctx.update(&mut app, keys("*jj<tab>"));
    let screen = ctx.redact_buffer();
    assert!(screen.contains("pub lhs_name: Option<String>,"), "{screen}");
    assert!(!screen.contains("pub lhs_name: String,"), "{screen}");
}

/// With delta's `side-by-side = N` and a screen narrower than `N`, delta draws a line to a row, so
/// a changed line is shown once, as it is when side-by-side is off.
#[test]
fn a_changed_line_is_shown_once_where_side_by_side_needs_a_wider_screen() {
    let mut ctx = setup_clone!();
    commit(
        &ctx.dir,
        "fields.rs",
        include_str!("fixtures/difftastic/optional-field/old.rs"),
    );
    fs::write(
        ctx.dir.join("fields.rs"),
        include_str!("fixtures/difftastic/optional-field/new.rs"),
    )
    .unwrap();
    ctx.config().general.difftastic.command = recorded_difftastic("optional-field");
    let renderer = &mut ctx.config().general.diff_renderer;
    renderer.enabled_features =
        crate::config::EnabledFeatures::new(|_| vec!["side-by-side".to_owned()]);
    renderer.side_by_side = crate::config::RendererSideBySide::new(|_, _, columns| columns >= 120);
    let mut app = ctx.init_app();
    ctx.update(&mut app, keys("*jj<tab>"));
    let screen = ctx.redact_buffer();
    assert!(screen.contains("pub lhs_name: Option<String>,"), "{screen}");
    assert!(!screen.contains("pub lhs_name: String,"), "{screen}");
}

/// Staging the line shown once stages its change: the old field is removed and the new one
/// added, as git's patch has them.
#[test]
fn stage_a_changed_line_shown_once() {
    let mut ctx = setup_clone!();
    let old = include_str!("fixtures/difftastic/optional-field/old.rs");
    let new = include_str!("fixtures/difftastic/optional-field/new.rs");
    commit(&ctx.dir, "fields.rs", old);
    fs::write(ctx.dir.join("fields.rs"), new).unwrap();
    ctx.config().general.difftastic.command = recorded_difftastic("optional-field");
    let mut app = ctx.init_app();
    // The hunk, its first context line, then the changed field.
    ctx.update(&mut app, keys("*jj<tab>jjjs"));
    assert_eq!(run(&ctx.dir, &["git", "show", ":fields.rs"]), new);
}

/// Old lines joined into one new line are not drawn, and neither is the hunk header that keeps the
/// lines after them numbered as they are.
#[test]
fn lines_joined_into_one_are_drawn_once() {
    let mut ctx = setup_clone!();
    commit(
        &ctx.dir,
        "join.rs",
        include_str!("fixtures/difftastic/join/old.rs"),
    );
    fs::write(
        ctx.dir.join("join.rs"),
        include_str!("fixtures/difftastic/join/new.rs"),
    )
    .unwrap();
    ctx.config().general.difftastic.command = recorded_difftastic("join");
    snapshot!(ctx, "*jj<tab>");
}

/// In difftastic's view a line can stand for a removed and an added line, or have only been
/// rewrapped, which a `+` or `-` marker would misstate, so the markers cannot be turned on there,
/// and the user is told why.
#[test]
fn plus_minus_markers_are_refused_in_difftastics_view() {
    let mut ctx = setup_clone!();
    commit(&ctx.dir, "src.rs", OLD);
    fs::write(ctx.dir.join("src.rs"), NEW).unwrap();
    ctx.config().general.difftastic.command = recorded_difftastic("match");
    ctx.config().general.diff_renderer.feature_keys = [
        ("difftastic".into(), '*'),
        ("plus-minus-markers".into(), '#'),
    ]
    .into();
    let mut app = ctx.init_app();
    ctx.update(&mut app, keys("*#"));
    assert!(
        !app.state.features.iter().any(|f| f == "plus-minus-markers"),
        "{:?}",
        app.state.features
    );
    let screen = ctx.redact_buffer();
    assert!(
        screen.contains("+ and - markers are not shown in difftastic's view"),
        "{screen}"
    );
}
