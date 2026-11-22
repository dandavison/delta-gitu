//! A PR's review threads, as rows under the diff lines they are on.

use crate::{
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
    styles: &CommentStyles,
) -> Vec<Item> {
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
                styles,
            ));
        }
    }
    out
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

/// The styles comments are drawn in: `text` for what they say and who said it,
/// and `decoration` for the bar before them and the notes on a thread's first
/// row.
pub(crate) struct CommentStyles {
    pub text: Style,
    pub decoration: Style,
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

/// The rows of `thread`, in `panel`: the first row of its first comment,
/// folding the rest under it. That row says whether the thread is resolved or
/// outdated, and when it is not `on_line` though it has a line, which line.
fn thread_rows(
    thread: &Thread,
    depth: usize,
    on_line: bool,
    panel: Panel,
    styles: &CommentStyles,
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
    let notes = (!notes.is_empty()).then(|| format!(" ({})", notes.join(", ")));

    let text_width = panel
        .width
        .saturating_sub(GUTTER.width())
        .max(MIN_TEXT_WIDTH);
    let mut rows = vec![];
    for (comment_i, comment) in thread.comments.iter().enumerate() {
        let first_row_width = match (&notes, comment_i) {
            (Some(notes), 0) => text_width.saturating_sub(notes.width()).max(MIN_TEXT_WIDTH),
            _ => text_width,
        };
        let comment_rows = comment_rows(
            &comment.author,
            &comment.body,
            first_row_width,
            text_width,
            styles.text,
        );
        for (row_i, mut spans) in comment_rows.into_iter().enumerate() {
            let is_header = comment_i == 0 && row_i == 0;
            if is_header && let Some(notes) = &notes {
                spans.push((notes.clone(), styles.decoration));
            }
            let mut rendered = vec![(
                format!("{}{GUTTER}", " ".repeat(panel.column)),
                styles.decoration,
            )];
            rendered.extend(spans);
            rows.push(if is_header {
                Item {
                    id: hash(("review_thread", &thread.id)),
                    depth,
                    default_collapsed: thread.resolved,
                    data: ItemData::ReviewThread {
                        url: first.url.clone(),
                    },
                    rendered: Some(Rc::new(rendered)),
                    ..Default::default()
                }
            } else {
                Item {
                    id: hash(("review_comment", &thread.id, comment_i, row_i)),
                    depth: depth + 1,
                    data: ItemData::ReviewComment {
                        url: comment.url.clone(),
                    },
                    rendered: Some(Rc::new(rendered)),
                    ..Default::default()
                }
            });
        }
    }
    rows
}

const GUTTER: &str = "  │ ";

/// The fewest columns a comment is wrapped to, however narrow its panel.
const MIN_TEXT_WIDTH: usize = 20;

/// The rows of a comment by `author`, wrapped to `width` columns, the first row
/// to `first_width`: its author's login, then its body, each row after the
/// first indented under the login, all in `style` and the login in bold.
fn comment_rows(
    author: &str,
    body: &str,
    first_width: usize,
    width: usize,
    style: Style,
) -> Vec<Vec<(String, Style)>> {
    let mut lines = body.lines();
    let first_line = format!("{author}: {}", lines.next().unwrap_or_default());
    let mut wrapped = wrap(&first_line, first_width, width - INDENT.len());
    for line in lines {
        wrapped.extend(wrap(line, width - INDENT.len(), width - INDENT.len()));
    }

    wrapped
        .into_iter()
        .enumerate()
        .map(|(i, row)| match (i, row.strip_prefix(author)) {
            (0, Some(rest)) => vec![
                (
                    author.to_owned(),
                    style.patch(Style {
                        add_modifier: Modifier::BOLD,
                        ..Style::new()
                    }),
                ),
                (rest.to_owned(), style),
            ],
            (0, None) => vec![(row, style)],
            _ => vec![(format!("{INDENT}{row}"), style)],
        })
        .collect()
}

const INDENT: &str = "  ";

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
