//! A PR's review threads, as rows under the diff lines they are on.

use crate::{
    config::ColorMode,
    config_git::{DecorationStyle, parse_delta_style},
    item_data::ItemData,
    items::{Item, ItemId, hash},
    review::{self, CommentEditor, EditorTarget, OpenPr, Side, Thread},
    style::{Modifier, Style},
};
use git2::Repository;
use std::{
    collections::{HashMap, HashSet},
    rc::Rc,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// `items`, a PR's diff, under a summary of the PR in `summary_style` and a blank line, and under a warning when
/// HEAD is not the commit the PR's lines are numbered against, and with each of the PR's threads under the line it is on. A thread whose
/// line is not among the rows, as GitHub has it outdated or as its line is outside the diff's
/// hunks, goes under its file's header. A thread on a file not among the rows is not shown, and a
/// row at the top says so for one on a file not in the PR's diff, which can never be shown.
/// While the renderer is still drawing, a thread on a line not yet drawn shows under its file's
/// header until the line is. Comments are wrapped to the width of the panel
/// of the side their line is on (see [`Layout`]).
pub(crate) fn with_threads(
    items: Vec<Item>,
    pr: &OpenPr,
    repo: &Repository,
    layout: Layout,
    options: &CommentOptions,
    summary_style: Style,
    error_style: Style,
) -> Vec<Item> {
    let items = with_comment_counts(items, pr, layout.width, options.count);
    let threads = pr.threads.borrow();
    let mut editor = pr.editor.borrow_mut();
    let editor_path = match editor.as_ref().map(|editor| &editor.target) {
        Some(EditorTarget::Line { path, .. }) => Some(path.clone()),
        _ => None,
    };
    let paths: HashSet<&str> = threads
        .iter()
        .map(|thread| thread.path.as_str())
        .chain(editor_path.as_deref())
        .collect();
    // Only the files with threads are indexed, as a big PR's rows are built again and again
    // while the renderer draws them.
    let mut lines: HashMap<(String, Side, u32), usize> = HashMap::new();
    let mut files: HashMap<String, usize> = HashMap::new();
    let mut path_of: HashMap<usize, Option<String>> = HashMap::new();
    let mut hunk_lines: HashMap<(usize, usize), NumberedLines> = HashMap::new();
    // The hunk, by file and hunk index, of each row a thread can go under.
    let mut hunk_of: HashMap<usize, (usize, usize)> = HashMap::new();
    // Where the right column starts in each hunk the renderer draws side by side: the first column
    // past 0 that a record of one of its rows starts at. A hunk with none is drawn unified.
    let mut split: HashMap<(usize, usize), usize> = HashMap::new();
    /// A line's row: its depth, its lines and its hunk.
    struct LineRow {
        depth: usize,
        keys: Vec<(String, Side, u32)>,
        hunk: (usize, usize),
    }
    let mut last_line: Option<LineRow> = None;
    for (i, item) in items.iter().enumerate() {
        // A row nested under a line's row, as the rest of a line the renderer draws on more rows,
        // is part of the line, so that a thread on the line goes after it.
        if let Some(line) = &last_line
            && item.depth > line.depth
            && !matches!(
                item.data,
                ItemData::HunkLine { .. } | ItemData::Delta { .. }
            )
        {
            for key in &line.keys {
                lines.insert(key.clone(), i);
            }
            hunk_of.insert(i, line.hunk);
            continue;
        }
        last_line = None;
        let (ItemData::Delta { diff, file_i, .. } | ItemData::HunkLine { diff, file_i, .. }) =
            &item.data
        else {
            continue;
        };
        let Some(path) = path_of
            .entry(*file_i)
            .or_insert_with(|| {
                Some(review::file_path(diff, *file_i)).filter(|path| paths.contains(path.as_str()))
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
                record_columns,
                ..
            } => {
                let hunk = (*file_i, *hunk_i);
                if let Some(&column) = record_columns.iter().filter(|&&column| column > 0).min() {
                    split
                        .entry(hunk)
                        .and_modify(|split| *split = (*split).min(column))
                        .or_insert(column);
                }
                let numbered = hunk_lines
                    .entry(hunk)
                    .or_insert_with(|| numbered_lines(diff, *file_i, *hunk_i));
                let mut keys = vec![];
                for &line_i in line_indices {
                    for &(side, n) in numbered.get(line_i).into_iter().flatten() {
                        lines.insert((path.clone(), side, n), i);
                        keys.push((path.clone(), side, n));
                    }
                }
                hunk_of.insert(i, hunk);
                last_line = Some(LineRow {
                    depth: item.depth,
                    keys,
                    hunk,
                });
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
    let editor_under = match editor.as_ref().map(|editor| &editor.target) {
        Some(EditorTarget::Line { path, line }) => lines
            .get(&(path.clone(), line.0, line.1))
            .map(|&i| (i, Some(line.0)))
            .or_else(|| files.get(path).map(|&i| (i, None))),
        _ => None,
    };

    let mut out = Vec::with_capacity(items.len());
    if let Some(summary) = pr.summary.borrow().clone() {
        out.push(Item {
            data: ItemData::PrSummary,
            rendered: Some(Rc::new(vec![(summary, summary_style)])),
            ..Default::default()
        });
        out.push(Item {
            unselectable: true,
            rendered: Some(Rc::new(vec![])),
            ..Default::default()
        });
    }
    for error in &options.errors {
        out.push(Item {
            unselectable: true,
            rendered: Some(Rc::new(vec![(error.clone(), error_style)])),
            ..Default::default()
        });
    }
    if pr.backend.head_mismatch(repo) {
        out.push(warning_row());
    }
    for error in not_in_diff(&threads, &pr.viewed.borrow(), &files) {
        out.push(Item {
            unselectable: true,
            rendered: Some(Rc::new(vec![(error, error_style)])),
            ..Default::default()
        });
    }
    for (i, item) in items.into_iter().enumerate() {
        let threads = under.remove(&i).unwrap_or_default();
        let split = hunk_of.get(&i).and_then(|hunk| split.get(hunk)).copied();
        // Under a file's header a thread is a level down; under a line, beside it.
        let depth = match item.data {
            ItemData::Delta { .. } => item.depth + 1,
            _ => item.depth,
        };
        let on_line = !matches!(item.data, ItemData::Delta { .. });
        out.push(item);
        for thread in threads {
            let side = thread.line.filter(|_| on_line).map(|(side, _)| side);
            let panel = layout.panel(side, split);
            // A comment being changed is shown only in the box, which goes after its thread, as
            // a reply's does.
            let (in_box, edited) = match editor.as_ref().map(|editor| &editor.target) {
                Some(EditorTarget::Reply { thread_id }) => (*thread_id == thread.id, None),
                Some(EditorTarget::Edit {
                    thread_id,
                    comment_id,
                }) if *thread_id == thread.id => (true, Some(comment_id.clone())),
                _ => (false, None),
            };
            match edited {
                Some(comment_id) => {
                    let mut thread = thread.clone();
                    thread.comments.retain(|comment| comment.id != comment_id);
                    out.extend(thread_rows(&thread, depth, on_line, panel, options));
                }
                None => out.extend(thread_rows(thread, depth, on_line, panel, options)),
            }
            if let (true, Some(editor)) = (in_box, editor.as_mut()) {
                out.extend(editor_rows(editor, depth, panel, options));
            }
        }
        if let (Some((_, side)), Some(editor)) =
            (editor_under.filter(|(at, _)| *at == i), &mut *editor)
        {
            out.extend(editor_rows(
                editor,
                depth,
                layout.panel(side, split),
                options,
            ));
        }
    }
    out
}

/// What to say of the threads on files neither in the PR on GitHub, `pr_files`, nor among the
/// rows, `rows`, one line for each file.
fn not_in_diff(
    threads: &[Thread],
    pr_files: &HashMap<String, review::ViewedState>,
    rows: &HashMap<String, usize>,
) -> Vec<String> {
    let mut counts: std::collections::BTreeMap<&str, usize> = Default::default();
    for thread in threads {
        if !pr_files.contains_key(&thread.path) && !rows.contains_key(&thread.path) {
            *counts.entry(thread.path.as_str()).or_default() += 1;
        }
    }
    counts
        .into_iter()
        .map(|(path, n)| {
            let (threads, are) = if n == 1 {
                ("thread", "is")
            } else {
                ("threads", "are")
            };
            format!("{n} {threads} on {path} {are} not shown: the file is not in the PR's diff")
        })
        .collect()
}

/// `items` with each file's comment count: at the end of the names' column of its line of the
/// diffstat, against the `|`, or, for a file headed by the renderer's row, at the end of that
/// row, which ends with the file's name. `width` is the width the diffstat was laid out in.
fn with_comment_counts(mut items: Vec<Item>, pr: &OpenPr, width: usize, style: Style) -> Vec<Item> {
    // Laying out the diffstat again counts every file's lines, which is not done for nothing.
    if pr.threads.borrow().iter().all(|thread| thread.resolved) {
        return items;
    }
    let count = |diff: &crate::git::diff::Diff, file_i: usize| {
        pr.comment_count(&review::file_path(diff, file_i))
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

/// A delta option gitu draws comments with: its name, what it does, and its default where it has
/// one of its own.
pub(crate) struct CommentOption {
    pub name: &'static str,
    pub description: &'static str,
    pub default: Option<&'static str>,
}

/// The delta options gitu draws comments with. delta's config sets them, as it sets its own,
/// so that a delta feature can.
pub(crate) const OPTIONS: [CommentOption; 13] = [
    CommentOption {
        name: "comment-style",
        description: "A comment: its text, and the background of the box around it. By default, \
                      GitHub's background for comments, for delta's light or dark mode.",
        default: None,
    },
    CommentOption {
        name: "comment-author-style",
        description: "Who wrote a comment.",
        default: Some("bold"),
    },
    CommentOption {
        name: "comment-timestamp-style",
        description: "When a comment was written, which links to it on GitHub.",
        default: Some("dim"),
    },
    CommentOption {
        name: "comment-count-style",
        description: "How many comments a file's unresolved threads have, after its name.",
        default: Some("blue"),
    },
    CommentOption {
        name: "comment-decoration-style",
        description: "What is drawn around a thread, as one of delta's decoration styles: a box, \
                      or a line over or under it. Also the notes on its first row, such as that \
                      it is resolved.",
        default: Some("blue"),
    },
    CommentOption {
        name: "comment-timestamp-format",
        description: "A strftime format for when a comment was written, such as %Y-%m-%d. By \
                      default, how long ago it was.",
        default: None,
    },
    CommentOption {
        name: "comment-editor-style",
        description: "The box a comment is written in. By default, comment-style.",
        default: None,
    },
    CommentOption {
        name: "comment-editor-decoration-style",
        description: "What is drawn around the box a comment is written in. By default, \
                      comment-decoration-style.",
        default: None,
    },
    CommentOption {
        name: "comment-submitted-style",
        description: "A comment in a submitted review, which everyone can see, drawn over \
                      comment-style.",
        default: None,
    },
    CommentOption {
        name: "comment-link-style",
        description: "The text of a markdown link in a comment, which links to its URL, drawn \
                      over the comment's style.",
        default: Some("ul"),
    },
    CommentOption {
        name: "comment-draft-style",
        description: "A comment in the draft review, which sync posts, drawn over comment-style.",
        default: None,
    },
    CommentOption {
        name: "comment-local-only-style",
        description: "A local-only comment, which sync never posts, drawn over comment-style.",
        default: None,
    },
    CommentOption {
        name: "comment-pending-style",
        description: "A comment in your pending review on GitHub, which only you see until you \
                      submit it, drawn over comment-style.",
        default: None,
    },
];

/// The names of [`OPTIONS`], in order.
pub(crate) const OPTION_NAMES: [&str; 13] = {
    let mut names = [""; 13];
    let mut i = 0;
    while i < OPTIONS.len() {
        names[i] = OPTIONS[i].name;
        i += 1;
    }
    names
};

/// comment-style's default: GitHub's background for comments, for a light or a dark background,
/// or none when gitu is not run by delta.
pub(crate) fn comment_style_default(color_mode: Option<ColorMode>) -> &'static str {
    match color_mode {
        Some(ColorMode::Light) => "normal #f6f8fa",
        Some(ColorMode::Dark) => "normal #161b22",
        None => "",
    }
}

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
    /// What a local-only comment, which sync never posts, is drawn in over `text`, if set.
    pub local_only: Option<Style>,
    /// What a comment in the draft review, which sync posts, is drawn in over `text`, if set.
    pub draft: Option<Style>,
    /// What a comment in the user's pending review on GitHub, which only they see, is drawn in
    /// over `text`, if set.
    pub pending: Option<Style>,
    /// What a comment in a submitted review, which everyone can see, is drawn in over `text`,
    /// if set.
    pub submitted: Option<Style>,
    /// What a markdown link's text is drawn in over the comment's text.
    pub link: Style,
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
        let default = |i: usize| OPTIONS[i].default.unwrap_or_default();
        let text = style(0, comment_style_default(color_mode), false).style;
        let author = style(1, default(1), false).style;
        let timestamp = style(2, default(2), false).style;
        let count = style(3, default(3), false).style;
        let decoration = style(4, default(4), true);
        let link = style(9, default(9), false).style;
        let mut set_style = |i: usize, decoration: bool| {
            let value = values.get(i).cloned().flatten()?;
            parse_delta_style(&value, decoration)
                .map_err(|error| errors.push(format!("{}: {error}", OPTION_NAMES[i])))
                .ok()
        };
        let editor_text = set_style(6, false).map_or(text, |parsed| parsed.style);
        let editor_decoration = set_style(7, true).unwrap_or(decoration);
        let submitted = set_style(8, false).map(|parsed| parsed.style);
        let draft = set_style(10, false).map(|parsed| parsed.style);
        let local_only = set_style(11, false).map(|parsed| parsed.style);
        let pending = set_style(12, false).map(|parsed| parsed.style);
        CommentOptions {
            editor_text,
            editor_decoration,
            local_only,
            draft,
            pending,
            submitted,
            link,
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

/// The width the renderer draws the diff at.
#[derive(Clone, Copy)]
pub(crate) struct Layout {
    pub width: usize,
}

impl Layout {
    /// Where the rows of a thread on `side` go, in a hunk whose right column starts at `split` if
    /// the renderer draws it side by side: that side's column, and otherwise, or for a thread not
    /// on a line, the whole width.
    fn panel(&self, side: Option<Side>, split: Option<usize>) -> Panel {
        match (split, side) {
            (Some(split), Some(Side::Old)) => Panel {
                column: 0,
                width: split,
            },
            (Some(split), Some(Side::New)) => Panel {
                column: split,
                width: self.width.saturating_sub(split),
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
/// over it, or, with none of those, a bar down its left. A blank row comes between one comment
/// and the next. The first row is the first comment's first, and folds the rest; a box, or lines over and under, stay around it while it is folded.
/// The thread's first row says whether the thread is resolved or outdated, and when it is not
/// `on_line` though it has a line, which line. A comment in a draft review, or local-only, says so.
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
        None if thread.subject == review::Subject::File => notes.push("file".to_owned()),
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
    if thread.deleted_on_github {
        notes.push("thread deleted on GitHub".to_owned());
    }

    let frame = Frame::new(panel, &options.decoration);
    let mut rows: Vec<(usize, usize, Row)> = vec![];
    for (comment_i, comment) in thread.comments.iter().enumerate() {
        let mut notes = if comment_i == 0 {
            notes.clone()
        } else {
            vec![]
        };
        if comment.pending {
            notes.push("pending".to_owned());
        } else if comment.draft {
            notes.push("draft".to_owned());
        }
        if comment.local_only {
            notes.push("local-only".to_owned());
        }
        let notes = (!notes.is_empty()).then(|| format!("({})", notes.join(", ")));
        if comment_i > 0 {
            // A blank row, in the comment style, between one comment and the next.
            let blank = vec![(" ".repeat(frame.text_width), options.text)];
            rows.push((comment_i, BLANK_ROW, frame.row(blank)));
        }
        for (row_i, row) in comment_rows(comment, notes.as_deref(), frame.text_width, options)
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
                comment_id: first.id.clone(),
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
                comment_id: thread.comments[comment_i].id.clone(),
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

/// The rows of `editor`, a comment being written, in `panel`, in the editor's styles: what it is,
/// then its text, with the cursor drawn in it while it has focus. Records in `editor` the columns
/// its text is wrapped to.
fn editor_rows(
    editor: &mut CommentEditor,
    depth: usize,
    panel: Panel,
    options: &CommentOptions,
) -> Vec<Item> {
    let frame = Frame::new(panel, &options.editor_decoration);
    let width = frame.text_width;
    editor.width = width;
    let text = options.editor_text;
    let padded = |mut row: Row| -> Row {
        let used: usize = row.iter().map(|(span, _)| span.width()).sum();
        row.push((" ".repeat(width.saturating_sub(used)), text));
        frame.row(row)
    };
    let what = match editor.target {
        EditorTarget::Reply { .. } => "Reply",
        EditorTarget::Line { .. } => "New comment",
        EditorTarget::Edit { .. } => "Edit comment",
    };
    let hint = if editor.focused && editor.text.value().is_empty() {
        "(ctrl+s saves, esc cancels)"
    } else if editor.focused {
        "(ctrl+s saves, esc leaves)"
    } else {
        "(i to go on writing)"
    };
    let header = vec![
        (what.to_owned(), text.patch(options.author)),
        (" ".to_owned(), text),
        (hint.to_owned(), text.patch(options.editor_decoration.style)),
    ];

    let chars: Vec<char> = editor.text.value().chars().collect();
    let (cursor_row, cursor_column) = editor.text.cursor_at(width);
    let cursor_style = text.patch(Style {
        add_modifier: Modifier::REVERSED,
        ..Style::new()
    });
    let mut rows = vec![(EDITOR_HEADER, padded(header))];
    for (row_i, range) in editor.text.rows(width).into_iter().enumerate() {
        let row: String = chars[range].iter().collect();
        if !(editor.focused && row_i == cursor_row) {
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
            data: ItemData::CommentEditor { row },
            rendered: Some(Rc::new(rendered)),
            ..Default::default()
        });
    }
    items.extend(frame.bottom(hash("review_draft_bottom"), depth));
    items
}

/// The row before each comment of a thread after its first, which is blank.
const BLANK_ROW: usize = usize::MAX;

/// The row of a comment box that says what it is, as [`ItemData::CommentEditor`] gives it.
pub(crate) const EDITOR_HEADER: usize = usize::MAX;

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
fn timestamp(comment: &review::Comment, options: &CommentOptions) -> String {
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
/// which links to the comment's page if it has one, with `notes`, then its body wrapped to the
/// width. All of it is drawn over the comment style, so that its background fills the rows.
fn comment_rows(
    comment: &review::Comment,
    notes: Option<&str>,
    width: usize,
    options: &CommentOptions,
) -> Vec<Row> {
    // Where the comment is, from where only this repository has it to where everyone sees it.
    let state = if comment.local_only {
        options.local_only
    } else if comment.pending {
        options.pending
    } else if comment.draft {
        options.draft
    } else if comment.submitted {
        options.submitted
    } else {
        None
    };
    let text = state.map_or(options.text, |state| options.text.patch(state));
    let padded = |mut row: Row| -> Row {
        let used: usize = row
            .iter()
            .map(|(span, _)| crate::ui::display_text(span).width())
            .sum();
        row.push((" ".repeat(width.saturating_sub(used)), text));
        row
    };
    let when = timestamp(comment, options);
    let mut header = vec![
        (
            comment
                .agent
                .clone()
                .unwrap_or_else(|| comment.author.clone()),
            text.patch(options.author),
        ),
        (" ".to_owned(), text),
        ("[".to_owned(), text.patch(options.timestamp)),
        (
            match &comment.url {
                Some(url) => crate::ui::osc8_hyperlink(&when, url),
                None => when,
            },
            text.patch(options.timestamp),
        ),
        ("]".to_owned(), text.patch(options.timestamp)),
    ];
    if let Some(notes) = notes {
        header.push((" ".to_owned(), text));
        header.push((notes.to_owned(), text.patch(options.decoration.style)));
    }
    let mut rows = vec![padded(header)];
    let link = text.patch(options.link);
    for line in comment.body.lines() {
        for row in wrap(&shown(line), width, width) {
            rows.push(padded(spans(&row, text, link)));
        }
    }
    if !comment.reactions.is_empty() {
        // Linked, but not drawn as links are, which would underline the emoji.
        for row in wrap(&reactions_shown(&comment.reactions), width, width) {
            rows.push(padded(spans(&row, text, text)));
        }
    }
    rows
}

/// A comment's reactions as they are shown: the emoji once for each person who reacted with it,
/// linked to their page, and how many more reacted than GitHub gave.
fn reactions_shown(reactions: &[review::Reaction]) -> Vec<Shown<'_>> {
    let plain = |out: &mut Vec<Shown>, text: &str| out.extend(text.chars().map(|c| (c, None)));
    let mut out = vec![];
    for (i, reaction) in reactions.iter().enumerate() {
        if i > 0 {
            plain(&mut out, "  ");
        }
        if reaction.people.is_empty() {
            plain(&mut out, &format!("{} {}", reaction.emoji, reaction.count));
            continue;
        }
        for person in &reaction.people {
            out.extend(
                reaction
                    .emoji
                    .chars()
                    .map(|c| (c, Some(person.url.as_str()))),
            );
        }
        let more = reaction.count.saturating_sub(reaction.people.len());
        if more > 0 {
            plain(&mut out, &format!(" +{more}"));
        }
    }
    out
}

/// A character of a comment as it is shown, with the URL of the markdown link whose text it is in,
/// if it is in one.
type Shown<'a> = (char, Option<&'a str>);

/// `line` as it is shown: a markdown link, `[text](url)`, as its text, each character of which
/// has its URL. A link in a code span is code, and is shown as written.
fn shown(line: &str) -> Vec<Shown<'_>> {
    let mut out = vec![];
    let mut rest = line;
    while let Some(c) = rest.chars().next() {
        // A code span runs to the next backtick; without one, a backtick is just text.
        if c == '`'
            && let Some(end) = rest[1..].find('`')
        {
            out.extend(rest[..end + 2].chars().map(|c| (c, None)));
            rest = &rest[end + 2..];
            continue;
        }
        if c == '['
            && let Some((text, url, len)) = link_at(rest)
        {
            out.extend(text.chars().map(|c| (c, Some(url))));
            rest = &rest[len..];
            continue;
        }
        out.push((c, None));
        rest = &rest[c.len_utf8()..];
    }
    out
}

/// The markdown link `[text](url)` at the start of `s`, as its text, its URL and its length in
/// bytes. Its text may have code spans, and brackets in pairs; its URL has no spaces, and
/// parentheses only in pairs, so that a link with a title, `[text](url "title")`, is not one.
fn link_at(s: &str) -> Option<(&str, &str, usize)> {
    let mut depth = 0;
    let mut in_code = false;
    let mut close = None;
    for (i, c) in s.char_indices().skip(1) {
        match c {
            '`' => in_code = !in_code,
            '[' if !in_code => depth += 1,
            ']' if !in_code && depth == 0 => {
                close = Some(i);
                break;
            }
            ']' if !in_code => depth -= 1,
            _ => {}
        }
    }
    let close = close?;
    let text = &s[1..close];
    let target = s[close + 1..].strip_prefix('(')?;
    let mut depth = 0;
    for (i, c) in target.char_indices() {
        match c {
            '(' => depth += 1,
            ')' if depth == 0 => {
                let url = &target[..i];
                return (!text.is_empty() && !url.is_empty()).then_some((
                    text,
                    url,
                    close + 2 + i + 1,
                ));
            }
            ')' => depth -= 1,
            c if c.is_whitespace() => return None,
            _ => {}
        }
    }
    None
}

/// `row` as spans: its text in `text`, and a link's text in `link`, linked to its URL.
fn spans(row: &[Shown], text: Style, link: Style) -> Row {
    let mut spans: Row = vec![];
    for chunk in row.chunk_by(|a, b| a.1 == b.1) {
        let shown: String = chunk.iter().map(|(c, _)| c).collect();
        spans.push(match chunk[0].1 {
            Some(url) => (crate::ui::osc8_hyperlink(&shown, url), link),
            None => (shown, text),
        });
    }
    spans
}

/// `text` split into rows at spaces, the first at most `first` columns wide and
/// the rest at most `rest`. A word wider than a row is split where the row
/// ends. Each character keeps what it has with it, such as a link.
fn wrap<T: Clone>(text: &[(char, T)], first: usize, rest: usize) -> Vec<Vec<(char, T)>> {
    let width =
        |chars: &[(char, T)]| -> usize { chars.iter().map(|(c, _)| c.width().unwrap_or(0)).sum() };
    let spaces: Vec<&(char, T)> = text.iter().filter(|(c, _)| *c == ' ').collect();
    let mut rows = vec![];
    let mut row = vec![];
    let mut room = first;
    for (i, word) in text.split(|(c, _)| *c == ' ').enumerate() {
        if !row.is_empty() && width(&row) + 1 + width(word) > room {
            rows.push(std::mem::take(&mut row));
            room = rest;
        }
        if !row.is_empty() {
            // The space before the word, which is not the first.
            row.push(spaces[i - 1].clone());
        }
        for c in word {
            if width(&row) + c.0.width().unwrap_or(0) > room {
                rows.push(std::mem::take(&mut row));
                room = rest;
            }
            row.push(c.clone());
        }
    }
    rows.push(row);
    rows
}

#[cfg(test)]
mod tests {
    use super::{link_at, shown, wrap};

    fn wrapped(text: &str, first: usize, rest: usize) -> Vec<String> {
        let chars: Vec<(char, ())> = text.chars().map(|c| (c, ())).collect();
        wrap(&chars, first, rest)
            .into_iter()
            .map(|row| row.into_iter().map(|(c, _)| c).collect())
            .collect()
    }

    #[test]
    fn wrap_breaks_at_spaces_and_splits_a_word_wider_than_a_row() {
        assert_eq!(wrapped("aa bb cc", 5, 5), ["aa bb", "cc"]);
        assert_eq!(wrapped("aa bb cc", 2, 5), ["aa", "bb cc"]);
        assert_eq!(wrapped("abcdefg", 3, 3), ["abc", "def", "g"]);
        assert_eq!(wrapped("", 3, 3), [""]);
    }

    /// The text shown for a line, with the URL of each run of it that is a link's text.
    fn links(line: &str) -> (String, Vec<(String, &str)>) {
        let shown = shown(line);
        let text = shown.iter().map(|(c, _)| c).collect();
        let links = shown
            .chunk_by(|a, b| a.1 == b.1)
            .filter_map(|run| Some((run.iter().map(|(c, _)| c).collect(), run[0].1?)))
            .collect();
        (text, links)
    }

    #[test]
    fn a_markdown_link_is_shown_as_its_text_with_its_url() {
        assert_eq!(
            links("See [the check](https://e.com/a), here"),
            (
                "See the check, here".to_owned(),
                vec![("the check".to_owned(), "https://e.com/a")]
            )
        );
        assert_eq!(
            links("[`f`](https://e.com/f_(x)) and [`g`](https://e.com/g)"),
            (
                "`f` and `g`".to_owned(),
                vec![
                    ("`f`".to_owned(), "https://e.com/f_(x)"),
                    ("`g`".to_owned(), "https://e.com/g")
                ]
            )
        );
    }

    #[test]
    fn what_is_not_a_markdown_link_is_shown_as_written() {
        for line in [
            "`[text](https://e.com)`",
            "[text](https://e.com \"title\")",
            "[text] (https://e.com)",
            "[](https://e.com)",
            "[text]()",
            "[text](https://e.com",
            "[text",
            "a ` b [text](c",
        ] {
            assert_eq!(links(line), (line.to_owned(), vec![]), "{line}");
        }
        assert_eq!(link_at("[a [b] c](u)"), Some(("a [b] c", "u", 12)));
    }
}
