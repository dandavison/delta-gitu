use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::style::Style;

use crate::cmd_log::{CmdLog, CmdLogEntry};
use crate::config::Config;
use crate::ui::layout::opts;
use crate::ui::{DASHES, UiTree, layout_line, layout_span, repeat_chars};

pub(crate) fn layout_cmd_log<'a>(
    layout: &mut UiTree<'a>,
    log: &CmdLog,
    config: &Config,
    width: usize,
    debug: bool,
) {
    if log.is_empty() || log.entries.iter().all(|entry| !shown(entry, debug)) {
        return;
    }

    repeat_chars(layout, width, DASHES, Style::from(&config.style.separator));

    layout.col(opts(), |layout| {
        for entry in &log.entries {
            if !shown(entry, debug) {
                continue;
            }
            layout_entry(layout, entry, config);
        }
    });
}

/// How often the spinner beside a running command turns.
pub(crate) const SPINNER_STEP: Duration = Duration::from_millis(100);

/// A command is shown while it runs; once it has finished, only with `--debug`.
fn shown(entry: &Arc<RwLock<CmdLogEntry>>, debug: bool) -> bool {
    debug
        || !matches!(
            &*entry.read().unwrap(),
            CmdLogEntry::Cmd { out: Some(_), .. }
        )
}

/// A spinner frame that turns every [`SPINNER_STEP`].
fn spinner() -> &'static str {
    const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    let steps = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        / SPINNER_STEP.as_millis();
    FRAMES[steps as usize % FRAMES.len()]
}

/// The entry is read under a lock, so its spans have to be owned.
fn layout_entry<'a>(layout: &mut UiTree<'a>, entry: &Arc<RwLock<CmdLogEntry>>, config: &Config) {
    match &*entry.read().unwrap() {
        CmdLogEntry::Cmd { args, out } => {
            layout.row(opts(), |layout| {
                layout_span(
                    layout,
                    (
                        match out {
                            Some(_) => "$ ".into(),
                            None => format!("{} ", spinner()).into(),
                        },
                        Style::from(&config.style.info_msg),
                    ),
                );
                layout_span(
                    layout,
                    (args.to_string().into(), Style::from(&config.style.command)),
                );
            });

            for line in out.iter().flat_map(|out| out.lines()) {
                layout_line(layout, line.to_string().into(), Style::new());
            }
        }
        // An error's lines after the first, such as a command's stderr, are
        // indented under it.
        CmdLogEntry::Error(err) => {
            for (i, line) in err.lines().enumerate() {
                let prefix = if i == 0 { "! " } else { "  " };
                layout_line(
                    layout,
                    format!("{prefix}{line}").into(),
                    Style::from(&config.style.error_msg),
                );
            }
        }
        CmdLogEntry::Info(msg) => layout_line(
            layout,
            format!("> {msg}").into(),
            Style::from(&config.style.info_msg),
        ),
    }
}
