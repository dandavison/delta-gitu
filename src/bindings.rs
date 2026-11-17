use std::collections::BTreeMap;

use crate::{
    error::Error,
    key_parser,
    menu::{Menu, PendingMenu},
    ops::Op,
};
use crossterm::event::{KeyCode, KeyModifiers};

pub struct Bindings {
    vec: Vec<Binding>,
    /// Every operation each menu can bind, bound or not, for the list of them.
    table: Vec<Listed>,
}

/// An operation as the list of bindings shows it.
pub(crate) struct Listed {
    pub menu: Menu,
    pub op: Op,
    pub keys: Vec<String>,
    /// Where the user set the keys, if they did.
    pub set_in: Option<SetIn>,
    pub default: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SetIn {
    GitConfig,
    ConfigFile,
}

impl SetIn {
    pub(crate) fn describe(self) -> &'static str {
        match self {
            SetIn::GitConfig => "git config",
            SetIn::ConfigFile => "config file",
        }
    }
}

// Used to parse the bindings coming out of `figment`. If there are any issues,
// these are collected so the user can be informed.
impl TryFrom<BTreeMap<Menu, BTreeMap<Op, Vec<String>>>> for Bindings {
    type Error = crate::error::Error;

    fn try_from(value: BTreeMap<Menu, BTreeMap<Op, Vec<String>>>) -> Result<Self, Self::Error> {
        let mut bindings = Vec::new();
        let mut table = Vec::new();
        let mut bad_bindings = Vec::new();
        for (menu, ops) in value {
            for (op, binds) in ops {
                if !matches!(op, Op::ToggleArg(_) | Op::MoveToScreenLine(_)) {
                    table.push(Listed {
                        menu,
                        op: op.clone(),
                        keys: binds.clone(),
                        set_in: None,
                        default: binds.clone(),
                    });
                }
                for keys in binds {
                    if let Some(binding) = Binding::parse(menu, &keys, op.clone()) {
                        bindings.push(binding);
                    } else {
                        bad_bindings.push(format!(
                            "- {}.{} = {} (not a key)",
                            menu.as_ref(),
                            op.config_name(),
                            keys
                        ));
                    }
                }
            }
        }

        // If any bindings are bad, present them all to the user as an error.
        if !bad_bindings.is_empty() {
            return Err(Error::Bindings {
                bad_key_bindings: bad_bindings,
            });
        }

        Ok(Self {
            vec: bindings,
            table,
        })
    }
}

impl Bindings {
    /// Says which bindings the user set, given where each binding's path
    /// (`root.move_down`) was set and the default bindings.
    pub(crate) fn set_by_user(
        mut self,
        defaults: &BTreeMap<Menu, BTreeMap<Op, Vec<String>>>,
        set_in: impl Fn(&str) -> Option<SetIn>,
    ) -> Self {
        for listed in &mut self.table {
            listed.set_in = set_in(&format!(
                "{}.{}",
                listed.menu.as_ref(),
                listed.op.config_name()
            ));
            listed.default = defaults
                .get(&listed.menu)
                .and_then(|ops| ops.get(&listed.op))
                .cloned()
                .unwrap_or_default();
        }
        self
    }

    /// Every operation each menu can bind, in order of menu.
    pub(crate) fn table(&self) -> &[Listed] {
        &self.table
    }

    pub(crate) fn match_bindings<'a>(
        &'a self,
        pending: &'a Menu,
        events: &'a [(KeyModifiers, KeyCode)],
    ) -> impl Iterator<Item = &'a Binding> + 'a {
        self.vec
            .iter()
            .filter(move |binding| &binding.menu == pending)
            .filter(|binding| binding.keys.starts_with(events))
    }

    /// Whether `key` does something at the top level, alone or as the first of
    /// several keys.
    pub(crate) fn binds_at_top_level(&self, key: char) -> bool {
        self.match_bindings(&Menu::Root, &[(KeyModifiers::NONE, KeyCode::Char(key))])
            .next()
            .is_some()
    }

    /// The bindings a menu lists. The help lists the top-level keys, and its
    /// own but for closing it.
    pub(crate) fn list<'a>(&'a self, pending: &Menu) -> impl Iterator<Item = &'a Binding> {
        let pending = *pending;
        self.vec
            .iter()
            .filter(|keybind| !matches!(keybind.op, Op::ToggleArg(_)))
            .filter(move |keybind| match pending {
                Menu::Help => {
                    (keybind.menu == Menu::Help && keybind.op != Op::Quit)
                        || (keybind.menu == Menu::Root && keybind.op != Op::OpenMenu(Menu::Help))
                }
                menu => keybind.menu == menu,
            })
    }

    pub(crate) fn arg_list<'a>(
        &'a self,
        pending: &'a PendingMenu,
    ) -> impl Iterator<Item = &'a Binding> {
        let expected = if pending.menu == Menu::Help {
            Menu::Root
        } else {
            pending.menu
        };

        self.vec
            .iter()
            .filter(|keybind| {
                if let Op::ToggleArg(ref arg) = keybind.op {
                    pending.args.contains_key(arg.as_str())
                } else {
                    false
                }
            })
            .filter(move |keybind| keybind.menu == expected)
    }
}

pub(crate) struct Binding {
    pub menu: Menu,
    pub raw: String,
    pub keys: Vec<(KeyModifiers, KeyCode)>,
    pub op: Op,
}

impl Binding {
    /// Attempt to parse the key combination passed in. If it fails, `None` is
    /// returned; the caller should report this to the user.
    pub fn parse(menu: Menu, raw_keys: &str, op: Op) -> Option<Self> {
        if let Ok(("", keys)) = key_parser::parse_config_keys(raw_keys) {
            Some(Self {
                menu,
                raw: raw_keys.to_string(),
                keys,
                op,
            })
        } else {
            None
        }
    }
}
