use super::*;

/// `?` in the help opens the list of every binding.
const BINDINGS: &str = "??";

/// The rows of the binding list that mention `name`.
fn rows_of(app: &crate::app::App, name: &str) -> Vec<String> {
    app.screen()
        .row_texts()
        .into_iter()
        .filter(|row| row.split_whitespace().any(|word| word == name))
        .collect()
}

#[test]
fn the_help_offers_the_binding_list() {
    let mut ctx = setup_clone!();
    let mut app = ctx.init_app();
    ctx.resize(&mut app, &[HELP_FITS]);
    ctx.update(&mut app, keys(HELP));
    let buffer = ctx.redact_buffer();
    assert!(
        regex::Regex::new(r"\? +Key bindings")
            .unwrap()
            .is_match(&buffer),
        "{buffer}"
    );
}

#[test]
fn the_binding_list() {
    let mut ctx = setup_clone!();
    let mut app = ctx.init_app();
    ctx.resize(&mut app, &[HELP_FITS]);
    ctx.update(&mut app, keys(BINDINGS));
    insta::assert_snapshot!(ctx.redact_buffer());
}

/// The help's key closes it, as it does a menu.
#[test]
fn h_closes_the_help() {
    let mut ctx = setup_clone!();
    let mut app = ctx.init_app();
    ctx.resize(&mut app, &[HELP_FITS]);
    ctx.update(&mut app, keys("hh"));
    assert!(app.state.pending_menu.is_none());
}

/// A row gives the binding's keys, its name as it is set in git config, and
/// what it does.
#[test]
fn a_row_is_keys_name_and_description() {
    let mut ctx = setup_clone!();
    let mut app = ctx.init_app();
    ctx.update(&mut app, keys(BINDINGS));
    let rows = rows_of(&app, "move-down");
    assert!(
        rows.iter().any(|row| regex::Regex::new(
            r"^ *ctrl\+j ctrl\+down +move-down +Down, stopping only at file headers, hunk headers and commits$"
        )
        .unwrap()
        .is_match(row)),
        "{rows:#?}"
    );
}

/// An operation with no key is listed, so that it can be found to bind.
#[test]
fn an_unbound_operation_is_listed() {
    let mut ctx = setup_clone!();
    let mut app = ctx.init_app();
    ctx.update(&mut app, keys(BINDINGS));
    assert_eq!(rows_of(&app, "search-backward").len(), 1);
}

/// A binding the user set gives the keys it replaced.
#[test]
fn a_binding_set_in_git_config_gives_its_default() {
    let mut ctx = setup_clone!();
    ctx.config().bindings =
        crate::config::configured("", "[delta]\n\tbind = root.move-down ctrl+pagedown\n").bindings;
    let mut app = ctx.init_app();
    ctx.update(&mut app, keys(BINDINGS));
    let rows = rows_of(&app, "move-down");
    assert!(
        rows.iter()
            .any(|row| row.ends_with("(default ctrl+j ctrl+down)")
                && row.trim_start().starts_with("ctrl+pagedown ")),
        "{rows:#?}"
    );
}

#[test]
fn a_binding_set_in_the_config_file_gives_its_default() {
    let mut ctx = setup_clone!();
    ctx.config().bindings =
        crate::config::configured("[bindings]\nroot.move_down = [\"ctrl+pagedown\"]\n", "")
            .bindings;
    let mut app = ctx.init_app();
    ctx.update(&mut app, keys(BINDINGS));
    let rows = rows_of(&app, "move-down");
    assert!(
        rows.iter()
            .any(|row| row.ends_with("(default ctrl+j ctrl+down)")),
        "{rows:#?}"
    );
}

/// As in the root menu, so in the others.
#[test]
fn a_merge_menu_binding_set_in_git_config_gives_its_default() {
    let mut ctx = setup_clone!();
    ctx.config().bindings =
        crate::config::configured("", "[delta]\n\tbind = merge-menu.merge x\n").bindings;
    let mut app = ctx.init_app();
    ctx.update(&mut app, keys(BINDINGS));
    let rows = rows_of(&app, "merge");
    assert!(
        rows.iter().any(|row| row.ends_with("(default m)")),
        "{rows:#?}"
    );
}

/// A default binding gives no default.
#[test]
fn a_default_binding_gives_no_default() {
    let mut ctx = setup_clone!();
    let mut app = ctx.init_app();
    ctx.update(&mut app, keys(BINDINGS));
    let rows = app.screen().row_texts();
    assert!(
        !rows.iter().any(|row| row.contains("(default ")),
        "{rows:#?}"
    );
}

/// The list is searched as any view is.
#[test]
fn the_binding_list_is_searched() {
    let mut ctx = setup_clone!();
    let mut app = ctx.init_app();
    ctx.update(&mut app, keys(BINDINGS));
    ctx.update(&mut app, keys("/search-backward<enter>"));
    let selected = app.screen().get_selected_item();
    let text: String = selected
        .rendered
        .as_ref()
        .unwrap()
        .iter()
        .map(|(text, _)| text.as_str())
        .collect();
    assert!(text.contains("search-backward"), "{text}");
}
