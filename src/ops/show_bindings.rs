use super::{Action, OpTrait};
use crate::{
    app::{App, State},
    item_data::ItemData,
    screen,
    term::Term,
};
use std::{rc::Rc, sync::Arc};

pub(crate) struct ShowBindings;
impl OpTrait for ShowBindings {
    fn get_action(&self, _target: &ItemData) -> Option<Action> {
        Some(Rc::new(|app: &mut App, _term: &mut Term| {
            let params = app.render_params(app.screen().size);
            let screen = screen::bindings::create(Arc::clone(&app.state.config), params)?;
            app.state.screens.push(screen);
            Ok(())
        }))
    }

    fn display(&self, _state: &State) -> String {
        "Key bindings".into()
    }
}
