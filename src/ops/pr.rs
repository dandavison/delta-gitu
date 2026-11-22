//! Review a GitHub PR: check it out, see its diff with GitHub's Viewed
//! checkbox on each file, tick them off, and open a line on GitHub to comment
//! on it.

use super::{Action, OpTrait};
use crate::{app::State, item_data::ItemData};
use std::rc::Rc;

pub(crate) struct PrCurrent;
impl OpTrait for PrCurrent {
    fn get_action(&self, _target: &ItemData) -> Option<Action> {
        Some(Rc::new(|_app, _term| Ok(())))
    }

    fn display(&self, _state: &State) -> String {
        "current".into()
    }
}

pub(crate) struct PrOther;
impl OpTrait for PrOther {
    fn get_action(&self, _target: &ItemData) -> Option<Action> {
        Some(Rc::new(|_app, _term| Ok(())))
    }

    fn display(&self, _state: &State) -> String {
        "other".into()
    }
}

pub(crate) struct ToggleViewed;
impl OpTrait for ToggleViewed {
    fn get_action(&self, _target: &ItemData) -> Option<Action> {
        Some(Rc::new(|_app, _term| Ok(())))
    }

    fn is_target_op(&self) -> bool {
        true
    }

    fn display(&self, _state: &State) -> String {
        "Toggle viewed".into()
    }
}

pub(crate) struct ShowOnGithub;
impl OpTrait for ShowOnGithub {
    fn get_action(&self, _target: &ItemData) -> Option<Action> {
        Some(Rc::new(|_app, _term| Ok(())))
    }

    fn is_target_op(&self) -> bool {
        true
    }

    fn display(&self, _state: &State) -> String {
        "Show on GitHub".into()
    }
}
