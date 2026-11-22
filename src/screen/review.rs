//! A PR's review threads, as rows under the diff lines they are on.

use crate::{
    config::ColorMode,
    config_git::{DecorationStyle, parse_delta_style},
    github::{self, Review, Side, Thread},
    item_data::ItemData,
    items::{Item, hash},
    style::{Modifier, Style},
};
use git2::Repository;
use std::{collections::HashMap, rc::Rc};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// `items`, a PR's diff, under a warning when HEAD is not the commit GitHub
/// numbers the PR's lines against, and with each of the review's threads under the line it
/// is on. A thread whose line is not among the rows, as GitHub has it outdated
/// or as its line is outside the diff's hunks, goes under its file's header. A
/// thread on a file not among the rows is not shown. While the renderer is
/// still drawing, a thread on a line not yet drawn shows under its file's
/// header until the line is. Comments are wrapped to the width of the panel
/// of the side their line is on (see [`Layout`]).
pub(crate) fn with_threads(
    items: Vec<Item>,
    review: &Review,
    repo: &Repository,
    layout: Layout,
    options: &CommentOptions,
    error_style: Style,
) -> Vec<Item> {
    let items = with_comment_counts(items, review, layout.width, options.count);
    let threads = review.threads.borrow();
    let mut lines: HashMap<(String, Side, u32), usize> = HashMap::new();
    let mut files: HashMap<String, usize> = HashMap::new();
    for (i, item) in items.iter().enumerate() {
        match &item.data {
            ItemData::Delta { diff, file_i, .. } => {
                files.insert(github::file_path(diff, *file_i), i);
            }
            ItemData::HunkLine {
                diff,
                file_i,
                hunk_i,
                line_indices,
                ..
            } => {
                let path = github::file_path(diff, *file_i);
                let content = diff.hunk_content(*file_i, *hunk_i);
                for &line_i in line_indices {
                    let line = content.lines().nth(line_i).unwrap_or_default();
                    let old = (
                        Side::Old,
                        diff.hunk_line_old_file_num(*file_i, *hunk_i, line_i),
                    );
                    let new = (
                        Side::New,
                        diff.hunk_line_new_file_num(*file_i, *hunk_i, line_i),
                    );
                    let sides = match line.chars().next() {
                        Some('-') => vec![old],
                        Some('+') => vec![new],
                        _ => vec![old, new],
                    };
                    for (side, n) in sides {
                        lines.insert((path.clone(), side, n), i);
                    }
                }
            }
            _ => {}
        }
    }

    let mut under: HashMap<usize, Vec<&Thread>> = HashMap::new();
    for thread in threads.iter() {
        let on_line = thread
            .line
            .and_then(|(side, n)| lines.get(&(thread.path.clone(), side, n)));
        if let Some(&i) = on_line.or_else(|| files.get(&thread.path)) {
            under.entry(i).or_default().push(thread);
        }
    }

    let mut out = Vec::with_capacity(items.len());
    for error in &options.errors {
        out.push(Item {
            unselectable: true,
            rendered: Some(Rc::new(vec![(error.clone(), error_style)])),
            ..Default::default()
        });
    }
    if !at_pr_head(repo, &review.pr.head_oid) {
        out.push(warning_row());
    }
    for (i, item) in items.into_iter().enumerate() {
        let threads = under.remove(&i).unwrap_or_default();
        // Under a file's header a thread is a level down; under a line, beside it.
        let depth = match item.data {
            ItemData::Delta { .. } => item.depth + 1,
            _ => item.depth,
        };
        let on_line = !matches!(item.data, ItemData::Delta { .. });
        out.push(item);
        for thread in threads {
            let side = thread.line.filter(|_| on_line).map(|(side, _)| side);
            out.extend(thread_rows(
                thread,
                depth,
                on_line,
                layout.panel(side),
                options,
            ));
        }
    }
    out
}

/// `items` with each file's comment count after its name: in the names' column of its line of
/// the diffstat, or, for a file headed by the renderer's row, at the end of that row, which ends
/// with the file's name. `width` is the width the diffstat was laid out in.
fn with_comment_counts(
    mut items: Vec<Item>,
    review: &Review,
    width: usize,
    style: Style,
) -> Vec<Item> {
    let count = |diff: &crate::git::diff::Diff, file_i: usize| {
        review
            .comment_count(&github::file_path(diff, file_i))
            .map(|count| (count, style))
    };
    crate::items::label_stat_rows(&mut items, width, count);
    for item in &mut items {
        if let (ItemData::Delta { diff, file_i, .. }, None, Some(rendered)) =
            (&item.data, &item.header, &item.rendered)
            && let Some(label) = count(diff, *file_i)
        {
            let mut row = (**rendered).clone();
            while let Some((text, _)) = row.last_mut() {
                let trimmed = text.trim_end().len();
                if trimmed > 0 {
                    text.truncate(trimmed);
                    break;
                }
                row.pop();
            }
            row.push((" ".to_owned(), Style::new()));
            row.push(label);
            item.rendered = Some(Rc::new(row));
        }
    }
    items
}

/// Whether HEAD is the commit the PR's branch is at on GitHub.
fn at_pr_head(repo: &Repository, head_oid: &str) -> bool {
    repo.head()
        .ok()
        .and_then(|head| head.target())
        .is_some_and(|oid| oid.to_string() == head_oid)
}

fn warning_row() -> Item {
    Item {
        unselectable: true,
        rendered: Some(Rc::new(vec![(
            "HEAD is not the PR's head on GitHub: comments and links to GitHub may be on the wrong lines"
                .to_owned(),
            Style {
                add_modifier: Modifier::DIM,
                ..Style::new()
            },
        )])),
        ..Default::default()
    }
}

/// The delta options gitu draws comments with. delta's config sets them, as it sets its own,
/// so that a delta feature can.
pub(crate) const OPTION_NAMES: [&str; 6] = [
    "comment-style",
    "comment-author-style",
    "comment-timestamp-style",
    "comment-count-style",
    "comment-decoration-style",
    "comment-timestamp-format",
];

/// How comments are drawn, from the delta options in [`OPTION_NAMES`].
pub(crate) struct CommentOptions {
    /// What a comment says.
    pub text: Style,
    /// Who said it.
    pub author: Style,
    /// When.
    pub timestamp: Style,
    /// How many comments a file's unresolved threads have, after its name.
    pub count: Style,
    /// What is drawn around a thread, and the notes on its first row, such as that it is
    /// resolved.
    pub decoration: DecorationStyle,
    /// A strftime format for when a comment was made, or `None` for how long ago.
    pub timestamp_format: Option<String>,
    /// An option whose value is not a style, as what to tell the user. Its default is used.
    pub errors: Vec<String>,
}

impl CommentOptions {
    /// From the values of [`OPTION_NAMES`], in that order, `None` for one not set, with
    /// `color_mode`, delta's, for the default background of comments: GitHub's, for a light or a
    /// dark background, or none when gitu is not run by delta.
    pub(crate) fn of(values: &[Option<String>], color_mode: Option<ColorMode>) -> Self {
        let mut errors = vec![];
        let mut style = |i: usize, default: &str, decoration: bool| {
            let value = values.get(i).cloned().flatten();
            let parsed = value
                .as_deref()
                .map(|value| parse_delta_style(value, decoration))
                .transpose()
                .unwrap_or_else(|error| {
                    errors.push(format!("{}: {error}", OPTION_NAMES[i]));
                    None
                });
            parsed.unwrap_or_else(|| parse_delta_style(default, decoration).unwrap())
        };
        let text_default = match color_mode {
            Some(ColorMode::Light) => "normal #f6f8fa",
            Some(ColorMode::Dark) => "normal #161b22",
            None => "",
        };
        let text = style(0, text_default, false).style;
        let author = style(1, "bold", false).style;
        let timestamp = style(2, "dim", false).style;
        let count = style(3, "blue", false).style;
        let decoration = style(4, "blue", true);
        CommentOptions {
            text,
            author,
            timestamp,
            count,
            decoration,
            timestamp_format: values.get(5).cloned().flatten(),
            errors,
        }
    }
}

impl Default for CommentOptions {
    fn default() -> Self {
        Self::of(&[], None)
    }
}

/// Where a thread's rows go across the screen: from `column`, `width` columns
/// wide, within the width delta draws the diff at.
#[derive(Clone, Copy)]
struct Panel {
    column: usize,
    width: usize,
}

/// The width delta draws the diff at, and whether it draws the old and new
/// files side by side, each in half of it.
#[derive(Clone, Copy)]
pub(crate) struct Layout {
    pub width: usize,
    pub side_by_side: bool,
}

impl Layout {
    /// Where the rows of a thread on `side` go: in side-by-side, that side's
    /// panel, and otherwise, or for a thread not on a line, the whole width.
    fn panel(&self, side: Option<Side>) -> Panel {
        let half = self.width / 2;
        match (self.side_by_side, side) {
            (true, Some(Side::Old)) => Panel {
                column: 0,
                width: half,
            },
            (true, Some(Side::New)) => Panel {
                column: half,
                width: half,
            },
            _ => Panel {
                column: 0,
                width: self.width,
            },
        }
    }
}

type Row = Vec<(String, Style)>;

/// The characters a decoration is drawn with: delta's light box-drawing characters, or its
/// heavy ones when the decoration style is bold.
struct Lines {
    horizontal: &'static str,
    vertical: &'static str,
    down_right: &'static str,
    down_left: &'static str,
    up_right: &'static str,
    up_left: &'static str,
}

const LIGHT: Lines = Lines {
    horizontal: "─",
    vertical: "│",
    down_right: "┌",
    down_left: "┐",
    up_right: "└",
    up_left: "┘",
};

const HEAVY: Lines = Lines {
    horizontal: "━",
    vertical: "┃",
    down_right: "┏",
    down_left: "┓",
    up_right: "┗",
    up_left: "┛",
};

/// The columns between a panel's edge and what is drawn there.
const MARGIN: &str = "  ";

/// The fewest columns a comment is wrapped to, however narrow its panel.
const MIN_TEXT_WIDTH: usize = 20;

/// The rows of `thread`, in `panel`, decorated as `options` say: with a box, a line under or
/// over it, or, with none of those, a bar down its left. The first row is the first comment's
/// first, and folds the rest; a box, or lines over and under, stay around it while it is folded.
/// The thread's first row says whether the thread is resolved or outdated, and when it is not
/// `on_line` though it has a line, which line.
fn thread_rows(
    thread: &Thread,
    depth: usize,
    on_line: bool,
    panel: Panel,
    options: &CommentOptions,
) -> Vec<Item> {
    let Some(first) = thread.comments.first() else {
        return vec![];
    };
    let mut notes = vec![];
    match thread.line {
        None => notes.push("outdated".to_owned()),
        Some((side, n)) if !on_line => notes.push(match side {
            Side::Old => format!("old line {n}"),
            Side::New => format!("line {n}"),
        }),
        Some(_) => {}
    }
    if thread.resolved {
        notes.push("resolved".to_owned());
    }
    let notes = (!notes.is_empty()).then(|| format!("({})", notes.join(", ")));

    let decoration = &options.decoration;
    let lines = if decoration.style.add_modifier.contains(Modifier::BOLD) {
        &HEAVY
    } else {
        &LIGHT
    };
    let boxed = decoration.boxed;
    let bar = !boxed && !decoration.underline && !decoration.overline;
    // What goes before and after a comment's text on each row.
    let (before, after) = if boxed || bar {
        (
            format!("{}{} ", MARGIN, lines.vertical),
            format!(" {}", lines.vertical),
        )
    } else {
        (format!("{MARGIN}  "), String::new())
    };
    let chrome = before.width() + if boxed { after.width() } else { 0 };
    let text_width = panel.width.saturating_sub(chrome).max(MIN_TEXT_WIDTH);
    let indent = " ".repeat(panel.column);
    let decorated = |content: Row| -> Row {
        let mut row = vec![(format!("{indent}{before}"), decoration.style)];
        row.extend(content);
        if boxed {
            row.push((after.clone(), decoration.style));
        }
        row
    };
    let border = |left: &str, right: &str| -> Row {
        let span = text_width + chrome - MARGIN.len() - left.width() - right.width();
        vec![(
            format!(
                "{indent}{MARGIN}{left}{}{right}",
                lines.horizontal.repeat(span)
            ),
            decoration.style,
        )]
    };

    let mut rows: Vec<(usize, usize, Row)> = vec![];
    for (comment_i, comment) in thread.comments.iter().enumerate() {
        let notes = notes.as_deref().filter(|_| comment_i == 0);
        for (row_i, row) in comment_rows(comment, notes, text_width, options)
            .into_iter()
            .enumerate()
        {
            rows.push((comment_i, row_i, decorated(row)));
        }
    }

    let mut items = vec![];
    if boxed || decoration.overline {
        let top = if boxed {
            border(lines.down_right, lines.down_left)
        } else {
            border("", "")
        };
        items.push(Item {
            id: hash(("review_thread_top", &thread.id)),
            depth,
            unselectable: true,
            rendered: Some(Rc::new(top)),
            ..Default::default()
        });
    }
    let mut rows = rows.into_iter();
    if let Some((_, _, header)) = rows.next() {
        let header = Rc::new(header);
        items.push(Item {
            id: hash(("review_thread", &thread.id)),
            depth,
            default_collapsed: thread.resolved,
            data: ItemData::ReviewThread {
                url: first.url.clone(),
            },
            rendered: Some(Rc::clone(&header)),
            // Folded, the row stands in for the thread, rather than being followed by the
            // ellipsis that a folded section is, after the decoration's right edge.
            folded: Some(header),
            ..Default::default()
        });
    }
    for (comment_i, row_i, row) in rows {
        items.push(Item {
            id: hash(("review_comment", &thread.id, comment_i, row_i)),
            depth: depth + 1,
            data: ItemData::ReviewComment {
                url: thread.comments[comment_i].url.clone(),
            },
            rendered: Some(Rc::new(row)),
            ..Default::default()
        });
    }
    if boxed || decoration.underline {
        let bottom = if boxed {
            border(lines.up_right, lines.up_left)
        } else {
            border("", "")
        };
        // Beside the first row rather than under it, so that it stays while the thread is
        // folded, as the decoration above it does.
        items.push(Item {
            id: hash(("review_thread_bottom", &thread.id)),
            depth,
            unselectable: true,
            rendered: Some(Rc::new(bottom)),
            ..Default::default()
        });
    }
    items
}

/// When `comment` was made: as `options.timestamp_format` formats it in local time, or how long
/// ago.
fn timestamp(comment: &github::Comment, options: &CommentOptions) -> String {
    match &options.timestamp_format {
        Some(format) => comment
            .created_at
            .with_timezone(&chrono::Local)
            .format(format)
            .to_string(),
        None => chrono_humanize::HumanTime::from(comment.created_at).to_string(),
    }
}

/// The rows of `comment`, each `width` columns wide: its author's login and when it was made,
/// with `notes`, then its body wrapped to the width. All of it is drawn over the comment style,
/// so that its background fills the rows.
fn comment_rows(
    comment: &github::Comment,
    notes: Option<&str>,
    width: usize,
    options: &CommentOptions,
) -> Vec<Row> {
    let text = options.text;
    let padded = |mut row: Row| -> Row {
        let used: usize = row.iter().map(|(span, _)| span.width()).sum();
        row.push((" ".repeat(width.saturating_sub(used)), text));
        row
    };
    let mut header = vec![
        (comment.author.clone(), text.patch(options.author)),
        (" ".to_owned(), text),
        (
            format!("[{}]", timestamp(comment, options)),
            text.patch(options.timestamp),
        ),
    ];
    if let Some(notes) = notes {
        header.push((" ".to_owned(), text));
        header.push((notes.to_owned(), text.patch(options.decoration.style)));
    }
    let mut rows = vec![padded(header)];
    for line in comment.body.lines() {
        for row in wrap(line, width, width) {
            rows.push(padded(vec![(row, text)]));
        }
    }
    rows
}

/// `text` split into rows at spaces, the first at most `first` columns wide and
/// the rest at most `rest`. A word wider than a row is split where the row
/// ends.
fn wrap(text: &str, first: usize, rest: usize) -> Vec<String> {
    let mut rows = vec![];
    let mut row = String::new();
    let mut room = first;
    for word in text.split(' ') {
        if !row.is_empty() && row.width() + 1 + word.width() > room {
            rows.push(std::mem::take(&mut row));
            room = rest;
        }
        if !row.is_empty() {
            row.push(' ');
        }
        for c in word.chars() {
            if row.width() + c.width().unwrap_or(0) > room {
                rows.push(std::mem::take(&mut row));
                room = rest;
            }
            row.push(c);
        }
    }
    rows.push(row);
    rows
}

#[cfg(test)]
mod tests {
    use super::wrap;

    #[test]
    fn wrap_breaks_at_spaces_and_splits_a_word_wider_than_a_row() {
        assert_eq!(wrap("aa bb cc", 5, 5), ["aa bb", "cc"]);
        assert_eq!(wrap("aa bb cc", 2, 5), ["aa", "bb cc"]);
        assert_eq!(wrap("abcdefg", 3, 3), ["abc", "def", "g"]);
        assert_eq!(wrap("", 3, 3), [""]);
    }
}
