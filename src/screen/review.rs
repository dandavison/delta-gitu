//! A PR's review threads, as rows under the diff lines they are on.

use crate::{
    config::ColorMode,
    config_git::{DecorationStyle, parse_delta_style},
    github::{self, Draft, DraftTarget, Review, Side, Thread},
    item_data::ItemData,
    items::{Item, ItemId, hash},
    style::{Modifier, Style},
};
use git2::Repository;
use std::{
    collections::{HashMap, HashSet},
    rc::Rc,
};
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
    let mut draft = review.draft.borrow_mut();
    let draft_path = match draft.as_ref().map(|draft| &draft.target) {
        Some(DraftTarget::Line { path, .. }) => Some(path.clone()),
        _ => None,
    };
    let paths: HashSet<&str> = threads
        .iter()
        .map(|thread| thread.path.as_str())
        .chain(draft_path.as_deref())
        .collect();
    // Only the files with threads are indexed, as a big PR's rows are built again and again
    // while the renderer draws them.
    let mut lines: HashMap<(String, Side, u32), usize> = HashMap::new();
    let mut files: HashMap<String, usize> = HashMap::new();
    let mut path_of: HashMap<usize, Option<String>> = HashMap::new();
    let mut hunk_lines: HashMap<(usize, usize), NumberedLines> = HashMap::new();
    for (i, item) in items.iter().enumerate() {
        let (ItemData::Delta { diff, file_i, .. } | ItemData::HunkLine { diff, file_i, .. }) =
            &item.data
        else {
            continue;
        };
        let Some(path) = path_of
            .entry(*file_i)
            .or_insert_with(|| {
                Some(github::file_path(diff, *file_i)).filter(|path| paths.contains(path.as_str()))
            })
            .clone()
        else {
            continue;
        };
        match &item.data {
            ItemData::Delta { .. } => {
                files.insert(path, i);
            }
            ItemData::HunkLine {
                hunk_i,
                line_indices,
                ..
            } => {
                let numbered = hunk_lines
                    .entry((*file_i, *hunk_i))
                    .or_insert_with(|| numbered_lines(diff, *file_i, *hunk_i));
                for &line_i in line_indices {
                    for &(side, n) in numbered.get(line_i).into_iter().flatten() {
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
    // A new comment goes under its line, after the line's threads, or under its file's header
    // if the line is not among the rows; a reply goes after its thread.
    let draft_under = match draft.as_ref().map(|draft| &draft.target) {
        Some(DraftTarget::Line { path, line }) => lines
            .get(&(path.clone(), line.0, line.1))
            .map(|&i| (i, Some(line.0)))
            .or_else(|| files.get(path).map(|&i| (i, None))),
        _ => None,
    };

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
            let panel = layout.panel(side);
            out.extend(thread_rows(thread, depth, on_line, panel, options));
            if let Some(draft) = draft.as_mut()
                && draft.target
                    == (DraftTarget::Reply {
                        thread_id: thread.id.clone(),
                    })
            {
                out.extend(draft_rows(draft, depth, panel, options));
            }
        }
        if let (Some((_, side)), Some(draft)) =
            (draft_under.filter(|(at, _)| *at == i), &mut *draft)
        {
            out.extend(draft_rows(draft, depth, layout.panel(side), options));
        }
    }
    out
}

/// `items` with each file's comment count: at the end of the names' column of its line of the
/// diffstat, against the `|`, or, for a file headed by the renderer's row, at the end of that
/// row, which ends with the file's name. `width` is the width the diffstat was laid out in.
fn with_comment_counts(
    mut items: Vec<Item>,
    review: &Review,
    width: usize,
    style: Style,
) -> Vec<Item> {
    // Laying out the diffstat again counts every file's lines, which is not done for nothing.
    if review.threads.borrow().iter().all(|thread| thread.resolved) {
        return items;
    }
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

/// Each line of a hunk's content, as the lines of the old and new files it is.
type NumberedLines = Vec<Vec<(Side, u32)>>;

/// The lines of a hunk each line of its content is, by side: a removed line is a line of the
/// old file, an added one of the new, and an unchanged one of both. One pass over the hunk.
fn numbered_lines(diff: &crate::git::diff::Diff, file_i: usize, hunk_i: usize) -> NumberedLines {
    let header = &diff.file_diffs[file_i].hunks[hunk_i].header;
    let (mut old, mut new) = (header.old_line_start, header.new_line_start);
    diff.hunk_content(file_i, hunk_i)
        .lines()
        .map(|line| match line.chars().next() {
            Some('-') => {
                old += 1;
                vec![(Side::Old, old - 1)]
            }
            Some('+') => {
                new += 1;
                vec![(Side::New, new - 1)]
            }
            _ => {
                old += 1;
                new += 1;
                vec![(Side::Old, old - 1), (Side::New, new - 1)]
            }
        })
        .collect()
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
pub(crate) const OPTION_NAMES: [&str; 8] = [
    "comment-style",
    "comment-author-style",
    "comment-timestamp-style",
    "comment-count-style",
    "comment-decoration-style",
    "comment-timestamp-format",
    "comment-editor-style",
    "comment-editor-decoration-style",
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
    /// What is written in a comment box, `text` unless set.
    pub editor_text: Style,
    /// What is drawn around a comment box, `decoration` unless set.
    pub editor_decoration: DecorationStyle,
    /// An option whose value is not a style, as what to tell the user. Its default is used.
    pub errors: Vec<String>,
    /// Whether delta draws the diff side by side, which decides the panel a comment goes in.
    pub side_by_side: bool,
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
        let mut set_style = |i: usize, decoration: bool| {
            let value = values.get(i).cloned().flatten()?;
            parse_delta_style(&value, decoration)
                .map_err(|error| errors.push(format!("{}: {error}", OPTION_NAMES[i])))
                .ok()
        };
        let editor_text = set_style(6, false).map_or(text, |parsed| parsed.style);
        let editor_decoration = set_style(7, true).unwrap_or(decoration);
        CommentOptions {
            editor_text,
            editor_decoration,
            text,
            author,
            timestamp,
            count,
            decoration,
            timestamp_format: values.get(5).cloned().flatten(),
            errors,
            side_by_side: false,
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

    let frame = Frame::new(panel, &options.decoration);
    let mut rows: Vec<(usize, usize, Row)> = vec![];
    for (comment_i, comment) in thread.comments.iter().enumerate() {
        let notes = notes.as_deref().filter(|_| comment_i == 0);
        for (row_i, row) in comment_rows(comment, notes, frame.text_width, options)
            .into_iter()
            .enumerate()
        {
            rows.push((comment_i, row_i, frame.row(row)));
        }
    }

    let mut items = vec![];
    items.extend(frame.top(hash(("review_thread_top", &thread.id)), depth));
    let mut rows = rows.into_iter();
    if let Some((_, _, header)) = rows.next() {
        let header = Rc::new(header);
        items.push(Item {
            id: hash(("review_thread", &thread.id)),
            depth,
            default_collapsed: thread.resolved,
            data: ItemData::ReviewThread {
                url: first.url.clone(),
                thread_id: thread.id.clone(),
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
                thread_id: thread.id.clone(),
            },
            rendered: Some(Rc::new(row)),
            ..Default::default()
        });
    }
    // Beside the first row rather than under it, so that it stays while the thread is folded,
    // as the decoration above it does.
    items.extend(frame.bottom(hash(("review_thread_bottom", &thread.id)), depth));
    items
}

/// The rows of `draft`, a comment being written, in `panel`, in the editor's styles: what it is,
/// then its text, with the cursor drawn in it while it has focus. Records in `draft` the columns
/// its text is wrapped to.
fn draft_rows(
    draft: &mut Draft,
    depth: usize,
    panel: Panel,
    options: &CommentOptions,
) -> Vec<Item> {
    let frame = Frame::new(panel, &options.editor_decoration);
    let width = frame.text_width;
    draft.width = width;
    let text = options.editor_text;
    let padded = |mut row: Row| -> Row {
        let used: usize = row.iter().map(|(span, _)| span.width()).sum();
        row.push((" ".repeat(width.saturating_sub(used)), text));
        frame.row(row)
    };
    let what = match draft.target {
        DraftTarget::Reply { .. } => "Reply",
        DraftTarget::Line { .. } => "New comment",
    };
    let hint = if draft.focused {
        "(alt+enter posts, esc leaves)"
    } else {
        "(i to go on writing)"
    };
    let header = vec![
        (what.to_owned(), text.patch(options.author)),
        (" ".to_owned(), text),
        (hint.to_owned(), text.patch(options.editor_decoration.style)),
    ];

    let chars: Vec<char> = draft.text.value().chars().collect();
    let (cursor_row, cursor_column) = draft.text.cursor_at(width);
    let cursor_style = text.patch(Style {
        add_modifier: Modifier::REVERSED,
        ..Style::new()
    });
    let mut rows = vec![(DRAFT_HEADER, padded(header))];
    for (row_i, range) in draft.text.rows(width).into_iter().enumerate() {
        let row: String = chars[range].iter().collect();
        if !(draft.focused && row_i == cursor_row) {
            rows.push((row_i, padded(vec![(row, text)])));
            continue;
        }
        // The cursor is drawn on the char it is at, or on a space after the row's end.
        let at = row
            .char_indices()
            .scan(0, |column, (byte, c)| {
                let here = *column;
                *column += c.width().unwrap_or(0);
                Some((here, byte, c))
            })
            .find(|(column, _, _)| *column == cursor_column);
        let spans = match at {
            Some((_, byte, c)) => vec![
                (row[..byte].to_owned(), text),
                (c.to_string(), cursor_style),
                (row[byte + c.len_utf8()..].to_owned(), text),
            ],
            None => vec![(row, text), (" ".to_owned(), cursor_style)],
        };
        rows.push((row_i, padded(spans)));
    }

    let mut items = vec![];
    items.extend(frame.top(hash("review_draft_top"), depth));
    for (row, rendered) in rows {
        items.push(Item {
            id: hash(("review_draft", row)),
            depth,
            data: ItemData::ReviewDraft { row },
            rendered: Some(Rc::new(rendered)),
            ..Default::default()
        });
    }
    items.extend(frame.bottom(hash("review_draft_bottom"), depth));
    items
}

/// The row of a comment box that says what it is, as [`ItemData::ReviewDraft`] gives it.
pub(crate) const DRAFT_HEADER: usize = usize::MAX;

/// What is drawn around a thread or a comment box in a panel, as a decoration style says: a box,
/// lines over and under it, or, with none of those, a bar down its left.
struct Frame<'a> {
    decoration: &'a DecorationStyle,
    lines: &'static Lines,
    indent: String,
    /// What goes before and after the text on each row.
    before: String,
    after: String,
    /// The columns of the text on each row.
    text_width: usize,
}

impl<'a> Frame<'a> {
    fn new(panel: Panel, decoration: &'a DecorationStyle) -> Self {
        let lines = if decoration.style.add_modifier.contains(Modifier::BOLD) {
            &HEAVY
        } else {
            &LIGHT
        };
        let bar = !decoration.boxed && !decoration.underline && !decoration.overline;
        let (before, after) = if decoration.boxed || bar {
            (
                format!("{}{} ", MARGIN, lines.vertical),
                format!(" {}", lines.vertical),
            )
        } else {
            (format!("{MARGIN}  "), String::new())
        };
        let chrome = before.width() + if decoration.boxed { after.width() } else { 0 };
        Self {
            decoration,
            lines,
            indent: " ".repeat(panel.column),
            text_width: panel.width.saturating_sub(chrome).max(MIN_TEXT_WIDTH),
            before,
            after,
        }
    }

    /// `content`, `text_width` columns of text, in the frame.
    fn row(&self, content: Row) -> Row {
        let mut row = vec![(
            format!("{}{}", self.indent, self.before),
            self.decoration.style,
        )];
        row.extend(content);
        if self.decoration.boxed {
            row.push((self.after.clone(), self.decoration.style));
        }
        row
    }

    /// The row above the text: a box's top, or a line over it.
    fn top(&self, id: ItemId, depth: usize) -> Option<Item> {
        let lines = self.lines;
        match (self.decoration.boxed, self.decoration.overline) {
            (true, _) => Some(self.border_item(id, depth, lines.down_right, lines.down_left)),
            (false, true) => Some(self.border_item(id, depth, "", "")),
            (false, false) => None,
        }
    }

    /// The row under the text: a box's bottom, or a line under it.
    fn bottom(&self, id: ItemId, depth: usize) -> Option<Item> {
        let lines = self.lines;
        match (self.decoration.boxed, self.decoration.underline) {
            (true, _) => Some(self.border_item(id, depth, lines.up_right, lines.up_left)),
            (false, true) => Some(self.border_item(id, depth, "", "")),
            (false, false) => None,
        }
    }

    fn border_item(&self, id: ItemId, depth: usize, left: &str, right: &str) -> Item {
        let chrome = self.before.width()
            + if self.decoration.boxed {
                self.after.width()
            } else {
                0
            };
        let span = self.text_width + chrome - MARGIN.len() - left.width() - right.width();
        let row = vec![(
            format!(
                "{}{MARGIN}{left}{}{right}",
                self.indent,
                self.lines.horizontal.repeat(span)
            ),
            self.decoration.style,
        )];
        Item {
            id,
            depth,
            unselectable: true,
            rendered: Some(Rc::new(row)),
            ..Default::default()
        }
    }
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
/// which links to the comment on GitHub, with `notes`, then its body wrapped to the width. All of it is drawn over the comment style,
/// so that its background fills the rows.
fn comment_rows(
    comment: &github::Comment,
    notes: Option<&str>,
    width: usize,
    options: &CommentOptions,
) -> Vec<Row> {
    let text = options.text;
    let padded = |mut row: Row| -> Row {
        let used: usize = row
            .iter()
            .map(|(span, _)| crate::ui::display_text(span).width())
            .sum();
        row.push((" ".repeat(width.saturating_sub(used)), text));
        row
    };
    let mut header = vec![
        (comment.author.clone(), text.patch(options.author)),
        (" ".to_owned(), text),
        ("[".to_owned(), text.patch(options.timestamp)),
        (
            crate::ui::osc8_hyperlink(&timestamp(comment, options), &comment.url),
            text.patch(options.timestamp),
        ),
        ("]".to_owned(), text.patch(options.timestamp)),
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
