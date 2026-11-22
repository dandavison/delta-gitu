//! A PR's review threads, as rows under the diff lines they are on.

use crate::{
    github::{self, Review, Side, Thread},
    item_data::ItemData,
    items::{Item, hash},
    style::{Modifier, Style},
};
use git2::Repository;
use std::{collections::HashMap, rc::Rc};

/// `items`, a PR's diff, under a warning when HEAD is not the commit GitHub
/// numbers the PR's lines against, and with each of the review's threads under the line it
/// is on. A thread whose line is not among the rows, as GitHub has it outdated
/// or as its line is outside the diff's hunks, goes under its file's header. A
/// thread on a file not among the rows is not shown. While the renderer is
/// still drawing, a thread on a line not yet drawn shows under its file's
/// header until the line is.
pub(crate) fn with_threads(items: Vec<Item>, review: &Review, repo: &Repository) -> Vec<Item> {
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
            out.extend(thread_rows(thread, depth, on_line));
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

/// The rows of `thread`: its first comment's first line, folding the rest
/// under it. It says whether the thread is resolved or outdated, and when it
/// is not `on_line` though it has a line, which line.
fn thread_rows(thread: &Thread, depth: usize, on_line: bool) -> Vec<Item> {
    let Some((first, replies)) = thread.comments.split_first() else {
        return vec![];
    };
    let mut first_lines = first.body.lines();
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
    let mut header = comment_line(&first.author, first_lines.next().unwrap_or_default());
    if !notes.is_empty() {
        header.push((format!(" ({})", notes.join(", ")), dim()));
    }

    let mut rows = vec![Item {
        id: hash(("review_thread", &thread.id)),
        depth,
        default_collapsed: thread.resolved,
        data: ItemData::ReviewThread {
            url: first.url.clone(),
        },
        rendered: Some(Rc::new(header)),
        ..Default::default()
    }];
    let row = |id: (usize, usize), url: &str, rendered| Item {
        id: hash(("review_comment", &thread.id, id)),
        depth: depth + 1,
        data: ItemData::ReviewComment {
            url: url.to_owned(),
        },
        rendered: Some(Rc::new(rendered)),
        ..Default::default()
    };
    for (line_i, line) in first_lines.enumerate() {
        rows.push(row((0, line_i + 1), &first.url, body_line(line)));
    }
    for (comment_i, comment) in replies.iter().enumerate() {
        let mut lines = comment.body.lines();
        let author_line = comment_line(&comment.author, lines.next().unwrap_or_default());
        rows.push(row((comment_i + 1, 0), &comment.url, author_line));
        for (line_i, line) in lines.enumerate() {
            rows.push(row(
                (comment_i + 1, line_i + 1),
                &comment.url,
                body_line(line),
            ));
        }
    }
    rows
}

const GUTTER: &str = "  │ ";

/// A comment's first line, after its author's login.
fn comment_line(author: &str, line: &str) -> Vec<(String, Style)> {
    vec![
        (GUTTER.to_owned(), dim()),
        (
            author.to_owned(),
            Style {
                add_modifier: Modifier::BOLD,
                ..Style::new()
            },
        ),
        (format!(": {line}"), Style::new()),
    ]
}

/// A line of a comment after its first, indented under the author's login.
fn body_line(line: &str) -> Vec<(String, Style)> {
    vec![
        (GUTTER.to_owned(), dim()),
        (format!("  {line}"), Style::new()),
    ]
}

fn dim() -> Style {
    Style {
        add_modifier: Modifier::DIM,
        ..Style::new()
    }
}
