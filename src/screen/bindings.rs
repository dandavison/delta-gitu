//! Every operation a key can be bound to, by menu: its keys, its name in
//! config, what it does, and, if the user set its keys, the default keys.

use std::sync::Arc;

use itertools::Itertools;
use unicode_width::UnicodeWidthStr;

use super::Screen;
use crate::{
    Res,
    bindings::Listed,
    config::Config,
    item_data::{ItemData, SectionHeader},
    items::{Item, RenderParams, hash},
    style::Style,
};

pub(crate) fn create(config: Arc<Config>, params: RenderParams) -> Res<Screen> {
    Screen::new(
        Arc::clone(&config),
        params,
        Box::new(move |_params: RenderParams| Ok(items(&config))),
    )
}

fn items(config: &Config) -> Vec<Item> {
    let table = config.bindings.table();
    let keys_width = table
        .iter()
        .map(|listed| keys(&listed.keys).width())
        .max()
        .unwrap_or(0);
    let name_width = table
        .iter()
        .map(|listed| name(listed).width())
        .max()
        .unwrap_or(0);
    let key_style = Style::from(&config.style.menu.key);
    let note_style = Style::from(&config.style.menu.heading);

    let mut items = vec![];
    for (menu, listed) in &table.iter().chunk_by(|listed| listed.menu) {
        items.push(Item {
            id: hash(("bindings", menu.as_ref())),
            data: ItemData::Header(SectionHeader::Bindings(
                menu.to_string(),
                format!("bindings.{}", menu.as_ref()),
            )),
            depth: 0,
            ..Default::default()
        });
        for listed in listed {
            let name = name(listed);
            let mut row = vec![
                (format!(" {:keys_width$}", keys(&listed.keys)), key_style),
                (format!("  {name:name_width$}  "), Style::new()),
                (listed.op.description().into_owned(), Style::new()),
            ];
            if listed.set_by_user {
                row.push((format!("  (default {})", keys(&listed.default)), note_style));
            }
            items.push(Item {
                id: hash(("bindings", menu.as_ref(), &name)),
                data: ItemData::Raw(name),
                depth: 1,
                rendered: Some(row.into()),
                ..Default::default()
            });
        }
    }
    items
}

fn keys(keys: &[String]) -> String {
    keys.join(" ")
}

/// The op's name as git config sets it.
fn name(listed: &Listed) -> String {
    listed.op.config_name().replace('_', "-")
}
