//! difftastic's view of a diff, shown in place of git's.
//!
//! For each file whose two versions gitu can read, gitu runs difftastic on them and draws what
//! difftastic found changed, inside git's own hunks. A line is drawn once, as it is now, unless
//! something on it was removed: a line whose only change is an addition is drawn as an unchanged
//! line with the additions marked, and a line whose only change is to its layout (reindented,
//! rewrapped) is drawn once, as it is now, in its own style. Old lines that difftastic finds joined
//! into another line, with nothing on them changed, are not drawn.
//!
//! What gitu draws is not a patch, and gitu does not stage from it: it stages from git's patch.
//! Each drawn line is a line of git's hunks, or stands for a line git's hunks remove and one they
//! add ([`Shown::stands_for`]), and staging it stages those lines.
//!
//! gitu draws difftastic's view only where it marks no more lines changed than git's does, region
//! by region between lines that both show as unchanged: difftastic pairs a function with the wrong
//! one when another is added before it, for instance, and then shows a rewritten function as new
//! throughout. Elsewhere, git's lines are drawn as git has them.
//!
//! The text given to the renderer marks how each changed line, and each changed token, is to be
//! drawn with colors that git does not use ([`Paint`]). [`map_styles`] tells delta which of its own
//! styles to draw each color in.

use crate::git::diff::{Diff, Display};
use crate::gitu_diff::FileDiff;
use git2::{ObjectType, Oid, Repository};
use std::collections::HashMap;
use std::fs;
use std::ops::Range;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{LazyLock, Mutex};
use std::thread;

/// How a changed line, or part of one, is to be drawn. Each is marked in [`Shown::text`] by a
/// color that git does not use, which delta, not taking the line as git's own colored output,
/// draws in the style [`map_styles`] maps it to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Paint {
    /// A token difftastic found removed.
    MinusEmph,
    /// The rest of a removed line that something was removed from.
    MinusNonEmph,
    /// A removed line changed throughout.
    Minus,
    /// A token difftastic found added.
    PlusEmph,
    /// The rest of a line that something was added to.
    PlusNonEmph,
    /// An added line changed throughout.
    Plus,
    /// A line whose only change is to its layout.
    Reformatted,
}

const PAINTS: [Paint; 7] = [
    Paint::MinusEmph,
    Paint::MinusNonEmph,
    Paint::Minus,
    Paint::PlusEmph,
    Paint::PlusNonEmph,
    Paint::Plus,
    Paint::Reformatted,
];

impl Paint {
    /// The 24-bit color marking this paint: near black, and so unlike any color git colors a diff
    /// with.
    fn rgb(self) -> (u8, u8, u8) {
        match self {
            Paint::MinusEmph => (1, 1, 1),
            Paint::MinusNonEmph => (1, 1, 2),
            Paint::Minus => (1, 1, 3),
            Paint::PlusEmph => (1, 2, 1),
            Paint::PlusNonEmph => (1, 2, 2),
            Paint::Plus => (1, 2, 3),
            Paint::Reformatted => (1, 3, 1),
        }
    }

    /// The delta style the color is drawn in. A line whose only change is to its layout is drawn
    /// in italics, which delta's own styles do not use, so that a change to whitespace is seen.
    fn delta_style(self) -> &'static str {
        match self {
            Paint::MinusEmph => "minus-emph-style",
            Paint::MinusNonEmph => "minus-non-emph-style",
            Paint::Minus => "minus-style",
            Paint::PlusEmph => "plus-emph-style",
            Paint::PlusNonEmph => "plus-non-emph-style",
            Paint::Plus => "plus-style",
            Paint::Reformatted => "syntax italic",
        }
    }

    /// The escape sequence that sets the color.
    fn sgr(self) -> String {
        let (r, g, b) = self.rgb();
        format!("\x1b[38;2;{r};{g};{b}m")
    }
}

/// delta's `map-styles` for the colors in [`Shown::text`]: each to the delta style it is drawn in.
pub(crate) fn map_styles() -> String {
    PAINTS
        .iter()
        .map(|paint| {
            let (r, g, b) = paint.rgb();
            format!("#{r:02x}{g:02x}{b:02x} => {}", paint.delta_style())
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// A file's hunks as difftastic sees the change, for the renderer to draw.
#[derive(Debug, PartialEq)]
pub(crate) struct Shown {
    /// git's hunks, with difftastic's view of their lines.
    pub text: String,
    /// The lines drawn as unchanged that stand for a line git's hunks remove and one they add: by
    /// new-version line number, the old version's. Numbers count from 1.
    pub stands_for: HashMap<usize, usize>,
}

/// `diff`, git's diff, with what the renderer draws ([`Diff::display`]) its files as difftastic
/// sees them: those whose two versions gitu can read, from `repo`, or for a version not in it, from
/// the work tree. `command` runs difftastic. A file that difftastic fails on, or whose output gitu
/// does not understand, is drawn as git has it, as is an added or deleted file.
pub(crate) fn with_display(repo: &Repository, diff: Diff, command: &[String]) -> Diff {
    let display = display(repo, &diff.text, &diff.file_diffs, command);
    Diff {
        display: Some(display),
        ..diff
    }
}

/// What the renderer draws for `text`, a diff whose files are `file_diffs`: see [`with_display`].
pub(crate) fn display(
    repo: &Repository,
    text: &str,
    file_diffs: &[FileDiff],
    command: &[String],
) -> Display {
    let jobs: Vec<Option<Job>> = file_diffs
        .iter()
        .map(|file_diff| Job::of(repo, text, file_diff))
        .collect();
    let outputs = run_all(command, &jobs);

    let mut display = Display::default();
    let mut copied = 0;
    for (file_i, ((file_diff, job), json)) in file_diffs.iter().zip(&jobs).zip(outputs).enumerate()
    {
        let (Some(job), Some(json)) = (job, json) else {
            continue;
        };
        let (Some(first), Some(last)) = (file_diff.hunks.first(), file_diff.hunks.last()) else {
            continue;
        };
        let git_hunks = &text[first.range.start..last.range.end];
        let Some(shown) = view(&job.old, &job.new, &json, git_hunks) else {
            continue;
        };
        display.text.push_str(&text[copied..first.range.start]);
        display.text.push_str(&shown.text);
        display.stands_for.extend(
            shown
                .stands_for
                .into_iter()
                .map(|(new, old)| ((file_i, new as u32), old as u32)),
        );
        copied = last.range.end;
    }
    display.text.push_str(&text[copied..]);
    display
}

/// Whether difftastic runs as `command` says: if not, what to tell the user.
pub(crate) fn check(command: &[String]) -> Result<(), String> {
    let Some((program, args)) = command.split_first() else {
        return Err("No command runs difftastic: set general.difftastic.command".to_owned());
    };
    match Command::new(program).args(args).arg("--version").output() {
        Ok(output) if output.status.success() => Ok(()),
        Ok(output) => Err(format!("{program} --version exited with {}", output.status)),
        Err(e) => Err(format!("difftastic could not be started as {program}: {e}")),
    }
}

/// The two versions of a file to run difftastic on.
struct Job {
    /// The file's name, by which difftastic tells its language.
    name: String,
    old: String,
    new: String,
    /// The versions' blob ids, by which difftastic's output for them is cached.
    ids: (Oid, Oid),
}

impl Job {
    /// The versions of the file `file_diff` of `text` is a diff of, as its `index` line names
    /// them: `None` if either does not exist, cannot be read, or is not UTF-8.
    fn of(repo: &Repository, text: &str, file_diff: &FileDiff) -> Option<Self> {
        let header = &text[file_diff.header.range.clone()];
        let ids = header
            .lines()
            .find_map(|line| line.strip_prefix("index "))?;
        let (old_id, new_id) = ids.split(' ').next()?.split_once("..")?;
        // An id of zeros says the version does not exist: the file is added or deleted.
        if [old_id, new_id]
            .iter()
            .any(|id| id.bytes().all(|byte| byte == b'0'))
        {
            return None;
        }
        let path = file_diff.header.new_file.fmt(text);
        let (old_oid, old) = blob(repo, old_id)?;
        let (new_oid, new) = blob(repo, new_id).or_else(|| work_tree_file(repo, &path, new_id))?;
        Some(Self {
            name: Path::new(&*path)
                .file_name()?
                .to_string_lossy()
                .into_owned(),
            old,
            new,
            ids: (old_oid, new_oid),
        })
    }
}

/// The blob `id`, perhaps abbreviated, names in `repo`.
fn blob(repo: &Repository, id: &str) -> Option<(Oid, String)> {
    let blob = repo
        .find_object_by_prefix(id, Some(ObjectType::Blob))
        .ok()?
        .peel_to_blob()
        .ok()?;
    Some((blob.id(), String::from_utf8(blob.content().to_vec()).ok()?))
}

/// The work tree's version of the file at `path`, if it is the blob `id`, as the new version of an
/// unstaged change is: git names it without writing it to the repository.
fn work_tree_file(repo: &Repository, path: &str, id: &str) -> Option<(Oid, String)> {
    let bytes = fs::read(repo.workdir()?.join(path)).ok()?;
    let oid = Oid::hash_object(ObjectType::Blob, &bytes).ok()?;
    if !oid.to_string().starts_with(id) {
        return None;
    }
    Some((oid, String::from_utf8(bytes).ok()?))
}

/// difftastic's output for each job, run on as many at once as there are processors.
fn run_all(command: &[String], jobs: &[Option<Job>]) -> Vec<Option<String>> {
    let outputs: Vec<Mutex<Option<String>>> = jobs.iter().map(|_| Mutex::new(None)).collect();
    let next = AtomicUsize::new(0);
    let threads = thread::available_parallelism().map_or(4, |n| n.get());
    thread::scope(|scope| {
        for _ in 0..threads.min(jobs.len()) {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(job) = jobs.get(i) else { break };
                    if let Some(job) = job {
                        *outputs[i].lock().unwrap() = output(command, job);
                    }
                }
            });
        }
    });
    outputs
        .into_iter()
        .map(|output| output.into_inner().unwrap())
        .collect()
}

/// difftastic's output for `job`. A file's versions are run once a session: the outputs are kept,
/// all of them, until gitu exits.
fn output(command: &[String], job: &Job) -> Option<String> {
    type Cache = HashMap<(Vec<String>, String, Oid, Oid), String>;
    static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(Default::default);
    let key = (command.to_vec(), job.name.clone(), job.ids.0, job.ids.1);
    if let Some(json) = CACHE.lock().unwrap().get(&key) {
        return Some(json.clone());
    }
    let json = run(command, job)?;
    CACHE.lock().unwrap().insert(key, json.clone());
    Some(json)
}

/// Run difftastic on `job`'s versions, written to files named as the file is.
fn run(command: &[String], job: &Job) -> Option<String> {
    static RUNS: AtomicUsize = AtomicUsize::new(0);
    let (program, args) = command.split_first()?;
    let dir = std::env::temp_dir().join(format!(
        "gitu-difftastic-{}-{}",
        std::process::id(),
        RUNS.fetch_add(1, Ordering::Relaxed)
    ));
    let paths = ["old", "new"].map(|version| dir.join(version).join(&job.name));
    let written = paths
        .iter()
        .zip([&job.old, &job.new])
        .try_for_each(|(path, text)| {
            fs::create_dir_all(path.parent().expect("a file in a directory"))?;
            fs::write(path, text)
        });
    let output = written.and_then(|()| {
        Command::new(program)
            .args(args)
            .arg("--display=json")
            .args(&paths)
            .env("DFT_UNSTABLE", "yes")
            .output()
    });
    let _ = fs::remove_dir_all(&dir);
    let output = output
        .inspect_err(|e| log::warn!("difftastic could not be run: {e}"))
        .ok()?;
    if !output.status.success() {
        log::warn!(
            "difftastic exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

/// difftastic's view of the change from `old` to `new`, from its `json` output for them, drawn
/// inside `git_hunks`, git's hunks for the change: `None` if the output is not what gitu
/// understands, or difftastic pairs lines that are not in the same one of git's hunks, in which
/// case git's hunks are drawn as they are.
pub(crate) fn view(old: &str, new: &str, json: &str, git_hunks: &str) -> Option<Shown> {
    let file: json::File = serde_json::from_str(json)
        .inspect_err(|e| log::warn!("difftastic's output is not what gitu expects: {e}"))
        .ok()?;
    let versions = [Version::of(old), Version::of(new)];
    let (git, git_hunks) = git(git_hunks, &versions)?;
    let (rows, changes) = match file.status {
        // A change only to layout: each line git's hunks change is drawn as such.
        json::Status::Unchanged => {
            let rows = git
                .into_iter()
                .map(|taken| Taken {
                    from_difftastic: true,
                    ..taken
                })
                .collect();
            (rows, Changes::new())
        }
        json::Status::Changed => {
            let changes = changes(&file.chunks, &versions)?;
            let difftastic: Vec<Taken> = rows(&file.aligned_lines, &versions)?
                .into_iter()
                .map(|row| Taken {
                    row,
                    changed: row.is_changed(&versions, &changes),
                    from_difftastic: true,
                })
                .collect();
            (fewer_changed(&difftastic, &git), changes)
        }
        // One version is empty: every line is added or removed, as git shows it.
        json::Status::Created | json::Status::Deleted => return None,
    };
    let drawn: Vec<Drawn> = rows
        .iter()
        .map(|taken| Drawn::of(taken, &versions, &changes))
        .collect();
    shown(&drawn, &versions, &git_hunks)
}

/// A line's paint, and the ranges of the tokens on it in the emph paint of its version.
type Painted = (Paint, Vec<Range<usize>>);

/// How a row of the view is drawn.
enum Drawn {
    /// Not drawn: an old line difftastic finds joined into another line, with nothing on it
    /// changed.
    Hidden { old: usize },
    /// An unchanged line, or a line drawn as unchanged with `paint`: one whose only change is an
    /// addition, or to its layout.
    Unchanged {
        old: usize,
        new: usize,
        paint: Option<Painted>,
    },
    /// A removed line, an added line, or both, each with its paint, if any: git's own lines have
    /// none, delta finding what changed on them itself.
    Changed {
        old: Option<(usize, Option<Painted>)>,
        new: Option<(usize, Option<Painted>)>,
    },
}

impl Drawn {
    fn of(taken: &Taken, versions: &[Version; 2], changes: &Changes) -> Self {
        let Row([old, new]) = taken.row;
        if !taken.changed {
            return Drawn::Unchanged {
                old: old.expect("an unchanged row has a line of each version"),
                new: new.expect("an unchanged row has a line of each version"),
                paint: None,
            };
        }
        if !taken.from_difftastic {
            return Drawn::Changed {
                old: old.map(|old| (old, None)),
                new: new.map(|new| (new, None)),
            };
        }
        let ranges = |version: usize, line: usize| {
            changes.get(&(version, line)).cloned().unwrap_or_default()
        };
        match (old, new) {
            (Some(old), Some(new)) => {
                let same_text = versions[OLD].lines[old] == versions[NEW].lines[new]
                    && versions[OLD].lacks_newline(old) == versions[NEW].lacks_newline(new);
                let (removed, added) = (ranges(OLD, old), ranges(NEW, new));
                if same_text {
                    Drawn::Unchanged {
                        old,
                        new,
                        paint: None,
                    }
                } else if !removed.is_empty()
                    || versions[OLD].lacks_newline(old) != versions[NEW].lacks_newline(new)
                {
                    Drawn::Changed {
                        old: Some((old, Some(painted(OLD, versions[OLD].lines[old], removed)))),
                        new: Some((new, Some(painted(NEW, versions[NEW].lines[new], added)))),
                    }
                } else if !added.is_empty() {
                    Drawn::Unchanged {
                        old,
                        new,
                        paint: Some(painted(NEW, versions[NEW].lines[new], added)),
                    }
                } else {
                    Drawn::Unchanged {
                        old,
                        new,
                        paint: Some((Paint::Reformatted, vec![])),
                    }
                }
            }
            (Some(old), None) => {
                let removed = ranges(OLD, old);
                if removed.is_empty() {
                    Drawn::Hidden { old }
                } else {
                    Drawn::Changed {
                        old: Some((old, Some(painted(OLD, versions[OLD].lines[old], removed)))),
                        new: None,
                    }
                }
            }
            (None, Some(new)) => {
                let added = ranges(NEW, new);
                let paint = if added.is_empty() {
                    (Paint::Reformatted, vec![])
                } else {
                    painted(NEW, versions[NEW].lines[new], added)
                };
                Drawn::Changed {
                    old: None,
                    new: Some((new, Some(paint))),
                }
            }
            (None, None) => unreachable!("a row has a line of one version or both"),
        }
    }

    /// The lines of each version the row is of.
    fn lines(&self) -> [Option<usize>; 2] {
        match self {
            Drawn::Hidden { old } => [Some(*old), None],
            Drawn::Unchanged { old, new, .. } => [Some(*old), Some(*new)],
            Drawn::Changed { old, new } => [old.as_ref().map(|o| o.0), new.as_ref().map(|n| n.0)],
        }
    }
}

/// The paint of a line of `version` with `text`, and the ranges of the tokens on it that changed:
/// changed throughout, or with changes on it.
fn painted(version: usize, text: &str, ranges: Vec<Range<usize>>) -> Painted {
    let changed_throughout = !ranges.is_empty()
        && text
            .char_indices()
            .all(|(i, c)| c.is_whitespace() || ranges.iter().any(|range| range.contains(&i)));
    match (version, changed_throughout) {
        (OLD, true) => (Paint::Minus, vec![]),
        (OLD, false) => (Paint::MinusNonEmph, ranges),
        (_, true) => (Paint::Plus, vec![]),
        (_, false) => (Paint::PlusNonEmph, ranges),
    }
}

/// `drawn`, rows covering every line of both `versions`, drawn inside `git_hunks`: `None` if a row
/// drawn as changed is not inside one of them, or a row's lines are in two.
fn shown(drawn: &[Drawn], versions: &[Version; 2], git_hunks: &[GitHunk]) -> Option<Shown> {
    let mut shown = Shown {
        text: String::new(),
        stands_for: HashMap::new(),
    };
    let mut rows = drawn.iter().peekable();
    for git_hunk in git_hunks {
        let inside = |row: &Drawn| {
            row.lines()
                .iter()
                .zip(&git_hunk.lines)
                .any(|(line, range)| line.is_some_and(|line| range.contains(&line)))
        };
        // Rows before the hunk are unchanged lines between hunks, which are not drawn.
        while let Some(row) = rows.next_if(|row| !inside(row)) {
            if !matches!(row, Drawn::Unchanged { paint: None, .. }) {
                return None;
            }
        }
        shown.text.push_str(&git_hunk.header);
        shown.text.push('\n');
        let mut next = git_hunk.lines.clone().map(|range| range.start);
        // Whether an old line has not been drawn since the last header: the next line that counts
        // an old line then needs one, or the renderer numbers it as the line not drawn.
        let mut hidden = false;
        while let Some(row) = rows.next_if(|row| inside(row)) {
            let lines = row.lines();
            if lines
                .iter()
                .zip(&git_hunk.lines)
                .any(|(line, range)| line.is_some_and(|line| !range.contains(&line)))
                || lines
                    .iter()
                    .zip(next)
                    .any(|(line, next)| line.is_some_and(|line| line != next))
            {
                return None;
            }
            let counts_old_line =
                !matches!(row, Drawn::Hidden { .. } | Drawn::Changed { old: None, .. });
            if hidden && counts_old_line {
                let [old, new] = next.map(|line| line + 1);
                shown.text.push_str(&format!("@@ -{old} +{new} @@\n"));
                hidden = false;
            }
            for version in [OLD, NEW] {
                next[version] += usize::from(lines[version].is_some());
            }
            match row {
                Drawn::Hidden { .. } => hidden = true,
                Drawn::Unchanged { old, new, paint } => {
                    let text = versions[NEW].lines[*new];
                    shown.text.push_str(&line(' ', text, paint.as_ref()));
                    if paint.is_some() {
                        shown.stands_for.insert(new + 1, old + 1);
                    }
                    no_newline_marker(&mut shown.text, versions[NEW].lacks_newline(*new));
                }
                Drawn::Changed { old, new } => {
                    for (version, marker, line_and_paint) in [(OLD, '-', old), (NEW, '+', new)] {
                        let Some((number, paint)) = line_and_paint else {
                            continue;
                        };
                        let text = versions[version].lines[*number];
                        shown.text.push_str(&line(marker, text, paint.as_ref()));
                        no_newline_marker(
                            &mut shown.text,
                            versions[version].lacks_newline(*number),
                        );
                    }
                }
            }
        }
    }
    if rows.any(|row| !matches!(row, Drawn::Unchanged { paint: None, .. })) {
        return None;
    }
    Some(shown)
}

fn no_newline_marker(text: &mut String, lacks_newline: bool) {
    if lacks_newline {
        text.push_str("\\ No newline at end of file\n");
    }
}

/// A line of the text for the renderer: `text` after `marker`, in `paint`, with the tokens at its
/// ranges in the emph paint of the same version, or as it is if it has no paint.
fn line(marker: char, text: &str, paint: Option<&Painted>) -> String {
    let Some((paint, ranges)) = paint else {
        return format!("{marker}{text}\n");
    };
    let emph = match paint {
        Paint::MinusNonEmph => Paint::MinusEmph,
        _ => Paint::PlusEmph,
    };
    let mut line = format!("{}{marker}", paint.sgr());
    let mut pos = 0;
    for range in ranges {
        line.push_str(&text[pos..range.start]);
        line.push_str(&emph.sgr());
        line.push_str(&text[range.clone()]);
        line.push_str(&paint.sgr());
        pos = range.end;
    }
    line.push_str(&text[pos..]);
    line.push_str("\x1b[0m\n");
    line
}

/// difftastic's JSON output for a file, as `--display=json` writes it with `DFT_UNSTABLE=yes`.
/// Line numbers count from 0, and a change's `start` and `end` are byte offsets into its line.
mod json {
    use serde::Deserialize;

    #[derive(Deserialize)]
    pub(super) struct File {
        /// Every line of both versions, each paired with a line of the other version or not, in
        /// order. Absent when one version is empty or nothing changed.
        #[serde(default)]
        pub aligned_lines: Vec<(Option<usize>, Option<usize>)>,
        /// The lines with changed tokens on them, grouped by where they are together.
        #[serde(default)]
        pub chunks: Vec<Vec<Line>>,
        pub status: Status,
    }

    #[derive(Deserialize, PartialEq)]
    #[serde(rename_all = "lowercase")]
    pub(super) enum Status {
        Unchanged,
        Changed,
        Created,
        Deleted,
    }

    #[derive(Deserialize)]
    pub(super) struct Line {
        pub lhs: Option<Side>,
        pub rhs: Option<Side>,
    }

    #[derive(Deserialize)]
    pub(super) struct Side {
        pub line_number: usize,
        pub changes: Vec<Change>,
    }

    #[derive(Deserialize)]
    pub(super) struct Change {
        pub start: usize,
        pub end: usize,
        /// The token's text.
        pub content: String,
    }
}

/// The old version of a file, or the new: `OLD` and `NEW` index the pair.
const OLD: usize = 0;
const NEW: usize = 1;

/// A version of a file, by line.
struct Version<'a> {
    /// The lines, without their newlines.
    lines: Vec<&'a str>,
    /// Whether the last line ends with a newline.
    ends_with_newline: bool,
}

impl<'a> Version<'a> {
    fn of(text: &'a str) -> Self {
        let ends_with_newline = text.is_empty() || text.ends_with('\n');
        let lines = match text.strip_suffix('\n') {
            _ if text.is_empty() => vec![],
            Some(body) => body.split('\n').collect(),
            None => text.split('\n').collect(),
        };
        Self {
            lines,
            ends_with_newline,
        }
    }

    /// Whether line `i` is the last, with no newline after it.
    fn lacks_newline(&self, i: usize) -> bool {
        !self.ends_with_newline && i + 1 == self.lines.len()
    }
}

/// A row of difftastic's pairing of the two versions' lines: a line of each, or of one.
#[derive(Clone, Copy)]
struct Row([Option<usize>; 2]);

/// The byte ranges of the tokens difftastic found changed, by version and line.
type Changes = HashMap<(usize, usize), Vec<Range<usize>>>;

impl Row {
    /// Whether the row is to be shown as removed and added lines rather than as a context line:
    /// it is not one line, the same in both versions, with nothing changed on it.
    fn is_changed(self, versions: &[Version; 2], changes: &Changes) -> bool {
        let Row([Some(old), Some(new)]) = self else {
            return true;
        };
        versions[OLD].lines[old] != versions[NEW].lines[new]
            || versions[OLD].lacks_newline(old) != versions[NEW].lacks_newline(new)
            || changes.contains_key(&(OLD, old))
            || changes.contains_key(&(NEW, new))
    }
}

/// difftastic's `aligned_lines` as rows: `None` unless every line of each version is in exactly
/// one row, in order, as a patch needs them to be.
fn rows(
    aligned_lines: &[(Option<usize>, Option<usize>)],
    versions: &[Version; 2],
) -> Option<Vec<Row>> {
    // difftastic numbers a line after the last newline, which is not a line of the patch.
    let rows: Vec<Row> = aligned_lines
        .iter()
        .map(|&(old, new)| {
            Row([
                old.filter(|&i| i < versions[OLD].lines.len()),
                new.filter(|&i| i < versions[NEW].lines.len()),
            ])
        })
        .filter(|Row(lines)| lines.iter().any(Option::is_some))
        .collect();
    let covers = |version: usize| {
        rows.iter()
            .filter_map(|Row(lines)| lines[version])
            .eq(0..versions[version].lines.len())
    };
    if !(covers(OLD) && covers(NEW)) {
        log::warn!("difftastic's pairing of lines does not cover both versions in order");
        return None;
    }
    Some(rows)
}

/// A row of the view, taken from difftastic's pairing of the lines or from git's hunks, and whether it
/// is shown as changed rather than as an unchanged line.
#[derive(Clone, Copy)]
struct Taken {
    row: Row,
    changed: bool,
    from_difftastic: bool,
}

/// One of git's hunks: the lines of each version it covers, by index, and its header.
struct GitHunk {
    lines: [Range<usize>; 2],
    /// The header line, without its newline.
    header: String,
}

/// git's `hunks` for the change between `versions`, as rows covering every line of both: the
/// hunks' lines, with the lines between hunks as context. `None` unless the hunks cover both
/// versions and each context line is the same in both, which it is not when git was asked to
/// ignore some changes, such as to whitespace.
fn git(hunks: &str, versions: &[Version; 2]) -> Option<(Vec<Taken>, Vec<GitHunk>)> {
    let mut rows = Vec::new();
    let mut git_hunks = Vec::new();
    // The next line of each version.
    let mut next = [0, 0];
    let context_to = |rows: &mut Vec<Taken>, next: &mut [usize; 2], old_end: usize| {
        while next[OLD] < old_end {
            rows.push(Taken {
                row: Row([Some(next[OLD]), Some(next[NEW])]),
                changed: false,
                from_difftastic: false,
            });
            next[OLD] += 1;
            next[NEW] += 1;
        }
    };
    for line in hunks.lines() {
        if let Some(header) = line.strip_prefix("@@ -") {
            let (ranges, _) = header.split_once(" @@")?;
            let (old, new) = ranges.split_once(" +")?;
            let lines = [old, new].map(|range| {
                let (start, count) = range.split_once(',').unwrap_or((range, "1"));
                let (start, count): (usize, usize) = (start.parse().ok()?, count.parse().ok()?);
                // An empty range starts at the line before it.
                let first = if count == 0 {
                    start
                } else {
                    start.checked_sub(1)?
                };
                Some(first..first + count)
            });
            let [Some(old), Some(new)] = lines else {
                return None;
            };
            // Between hunks, the lines are the same in both versions.
            if old.start < next[OLD]
                || new.start < next[NEW]
                || old.start - next[OLD] != new.start - next[NEW]
            {
                return None;
            }
            context_to(&mut rows, &mut next, old.start);
            git_hunks.push(GitHunk {
                lines: [old, new],
                header: line.to_owned(),
            });
            continue;
        }
        let (row, changed) = match line.chars().next() {
            Some(' ') => (Row([Some(next[OLD]), Some(next[NEW])]), false),
            Some('-') => (Row([Some(next[OLD]), None]), true),
            Some('+') => (Row([None, Some(next[NEW])]), true),
            // "\ No newline at end of file".
            _ => continue,
        };
        for version in [OLD, NEW] {
            next[version] += usize::from(row.0[version].is_some());
        }
        rows.push(Taken {
            row,
            changed,
            from_difftastic: false,
        });
    }
    let ends = [versions[OLD].lines.len(), versions[NEW].lines.len()];
    if next[OLD] > ends[OLD]
        || next[NEW] > ends[NEW]
        || ends[OLD] - next[OLD] != ends[NEW] - next[NEW]
    {
        return None;
    }
    context_to(&mut rows, &mut next, ends[OLD]);
    let fits = rows.iter().all(|shown| match shown.row {
        Row([Some(old), Some(new)]) => {
            versions[OLD].lines.get(old) == versions[NEW].lines.get(new)
                && versions[OLD].lacks_newline(old) == versions[NEW].lacks_newline(new)
        }
        Row([Some(old), None]) => old < versions[OLD].lines.len(),
        Row([None, Some(new)]) => new < versions[NEW].lines.len(),
        Row([None, None]) => false,
    });
    fits.then_some((rows, git_hunks))
}

/// The rows of `difftastic` and `git`, two pairings of the same versions' lines, taken from
/// whichever marks fewer lines changed, region by region: the regions are separated by the
/// lines both show as the same unchanged line. Where they mark as many, difftastic's are taken.
fn fewer_changed(difftastic: &[Taken], git: &[Taken]) -> Vec<Taken> {
    let unchanged = |rows: &[Taken]| -> HashMap<(usize, usize), usize> {
        rows.iter()
            .enumerate()
            .filter(|(_, shown)| !shown.changed)
            .filter_map(|(i, shown)| match shown.row {
                Row([Some(old), Some(new)]) => Some(((old, new), i)),
                _ => None,
            })
            .collect()
    };
    let in_difftastic = unchanged(difftastic);
    let mut both: Vec<[usize; 2]> = git
        .iter()
        .enumerate()
        .filter(|(_, shown)| !shown.changed)
        .filter_map(|(i, shown)| match shown.row {
            Row([Some(old), Some(new)]) => Some([*in_difftastic.get(&(old, new))?, i]),
            _ => None,
        })
        .collect();
    both.push([difftastic.len(), git.len()]);

    let changed_lines = |rows: &[Taken]| -> usize {
        rows.iter()
            .filter(|shown| shown.changed)
            .map(|shown| shown.row.0.iter().flatten().count())
            .sum()
    };
    let mut rows = Vec::new();
    let mut from = [0, 0];
    for [d, g] in both {
        let (in_difftastic, in_git) = (&difftastic[from[0]..d], &git[from[1]..g]);
        rows.extend(if changed_lines(in_difftastic) <= changed_lines(in_git) {
            in_difftastic
        } else {
            in_git
        });
        rows.extend(difftastic.get(d));
        from = [d + 1, g + 1];
    }
    rows
}

/// The tokens difftastic found changed, as byte ranges of their lines, with a run of changed
/// tokens separated only by whitespace made one range. A range that is not within its line is
/// left out, as difftastic gives for some tokens that span lines. `None` if a token is not at
/// its range, as when the output is for other versions of the file.
fn changes(chunks: &[Vec<json::Line>], versions: &[Version; 2]) -> Option<Changes> {
    let mut changes = Changes::new();
    for line in chunks.iter().flatten() {
        for (version, side) in [(OLD, &line.lhs), (NEW, &line.rhs)] {
            let Some(side) = side else { continue };
            let text = versions[version].lines.get(side.line_number)?;
            let ranges = changes.entry((version, side.line_number)).or_default();
            for change in &side.changes {
                let range = change.start..change.end;
                if range.is_empty() {
                    continue;
                }
                match text.get(range.clone()) {
                    Some(token) if token == change.content => ranges.push(range),
                    Some(_) => {
                        log::warn!("difftastic's output is not for the versions gitu has");
                        return None;
                    }
                    None => {}
                }
            }
        }
    }
    for ((version, line), ranges) in &mut changes {
        let text = versions[*version].lines[*line];
        ranges.sort_by_key(|range| range.start);
        let mut merged: Vec<Range<usize>> = Vec::new();
        for range in ranges.drain(..) {
            match merged.last_mut() {
                Some(last)
                    if range.start <= last.end || text[last.end..range.start].trim().is_empty() =>
                {
                    last.end = last.end.max(range.end);
                }
                _ => merged.push(range),
            }
        }
        *ranges = merged;
    }
    changes.retain(|_, ranges| !ranges.is_empty());
    Some(changes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    struct Fixture {
        old: &'static str,
        new: &'static str,
        json: &'static str,
        /// git's hunks for the change, as `git diff --no-index` gives them.
        git: &'static str,
    }

    macro_rules! fixture {
        ($name:literal) => {
            Fixture {
                old: include_str!(concat!("tests/fixtures/difftastic/", $name, "/old.rs")),
                new: include_str!(concat!("tests/fixtures/difftastic/", $name, "/new.rs")),
                json: include_str!(concat!("tests/fixtures/difftastic/", $name, "/difft.json")),
                git: include_str!(concat!("tests/fixtures/difftastic/", $name, "/git.diff")),
            }
        };
    }

    const FIXTURES: [(&str, Fixture); 7] = [
        ("rename", fixture!("rename")),
        ("match", fixture!("match")),
        ("no-newline", fixture!("no-newline")),
        ("insert", fixture!("insert")),
        ("reformat", fixture!("reformat")),
        ("wrong-function", fixture!("wrong-function")),
        ("join", fixture!("join")),
    ];

    /// Changes a view could misrepresent, beside those above.
    const MORE_FIXTURES: [(&str, Fixture); 11] = [
        ("optional-field", fixture!("optional-field")),
        ("remove-from-line", fixture!("remove-from-line")),
        ("delete-line", fixture!("delete-line")),
        ("swap-statements", fixture!("swap-statements")),
        ("replace-function", fixture!("replace-function")),
        ("change-and-blank-lines", fixture!("change-and-blank-lines")),
        ("newline-added", fixture!("newline-added")),
        ("comment-changed", fixture!("comment-changed")),
        ("multibyte", fixture!("multibyte")),
        ("tabs", fixture!("tabs")),
        ("crlf", fixture!("crlf")),
    ];

    fn shown(fixture: &Fixture) -> Shown {
        view(fixture.old, fixture.new, fixture.json, fixture.git)
            .unwrap_or_else(|| panic!("expected difftastic's view"))
    }

    /// `colored` with each changed line's colors written out: after its `-`, `+` or ` `, `!` for
    /// a line changed throughout, `.` for one with changes on it, which are in `⟦⟧`, and `~` for
    /// one whose only change is to its layout.
    fn legible(colored: &str) -> String {
        colored.split_inclusive('\n').map(legible_line).collect()
    }

    fn legible_line(line: &str) -> String {
        let paint_of = |code: &str| {
            PAINTS
                .into_iter()
                .find(|paint| paint.sgr() == format!("\x1b[{code}m"))
        };
        let Some(rest) = line.strip_prefix("\x1b[") else {
            return line.to_owned();
        };
        let (code, rest) = rest.split_once('m').unwrap();
        let tag = match paint_of(code) {
            Some(Paint::Minus | Paint::Plus) => '!',
            Some(Paint::MinusNonEmph | Paint::PlusNonEmph) => '.',
            Some(Paint::Reformatted) => '~',
            other => panic!("unexpected color for a whole line: {other:?}"),
        };
        let (marker, mut rest) = rest.split_at(1);
        let mut out = format!("{marker}{tag}");
        let mut in_emph = false;
        while let Some(start) = rest.find("\x1b[") {
            out.push_str(&rest[..start]);
            let (code, after) = rest[start + 2..].split_once('m').unwrap();
            let emph = matches!(paint_of(code), Some(Paint::MinusEmph | Paint::PlusEmph));
            match (in_emph, emph) {
                (false, true) => out.push('⟦'),
                (true, false) => out.push('⟧'),
                _ => {}
            }
            in_emph = emph;
            rest = after;
        }
        out.push_str(rest);
        out
    }

    /// The line numbers of git's hunks' removed, added and context lines.
    fn git_lines(git: &str) -> [Vec<usize>; 3] {
        let mut lines = [vec![], vec![], vec![]];
        let mut next = [0, 0];
        for line in git.lines() {
            if let Some(header) = line.strip_prefix("@@ -") {
                let (ranges, _) = header.split_once(" @@").unwrap();
                let (old, new) = ranges.split_once(" +").unwrap();
                for (version, range) in [old, new].into_iter().enumerate() {
                    let (start, count) = range.split_once(',').unwrap_or((range, "1"));
                    let (start, count): (usize, usize) =
                        (start.parse().unwrap(), count.parse().unwrap());
                    next[version] = if count == 0 { start + 1 } else { start };
                }
                continue;
            }
            match line.chars().next() {
                Some('-') => lines[0].push(next[0]),
                Some('+') => lines[1].push(next[1]),
                Some(' ') => lines[2].push(next[1]),
                _ => continue,
            }
            for (version, marker) in [(0, '+'), (1, '-')] {
                if !line.starts_with(marker) {
                    next[version] += 1;
                }
            }
        }
        lines
    }

    /// `text` without its color codes, and otherwise as it is.
    fn uncolored(text: &str) -> String {
        let mut out = String::new();
        let mut rest = text;
        while let Some(start) = rest.find("\x1b[") {
            out.push_str(&rest[..start]);
            rest = &rest[start..];
            rest = &rest[rest.find('m').unwrap() + 1..];
        }
        out.push_str(rest);
        out
    }

    /// The tokens of `text`: runs of word characters, and each other character but whitespace.
    fn tokens(text: &str) -> Vec<String> {
        let mut tokens = Vec::new();
        let mut word = String::new();
        for c in text.chars() {
            if c.is_alphanumeric() || c == '_' {
                word.push(c);
                continue;
            }
            if !word.is_empty() {
                tokens.push(std::mem::take(&mut word));
            }
            if !c.is_whitespace() {
                tokens.push(c.to_string());
            }
        }
        if !word.is_empty() {
            tokens.push(word);
        }
        tokens
    }

    /// What difftastic's view of a change tells the reader is true of the change, whatever
    /// difftastic found:
    /// - In each hunk, the lines drawn as unchanged or added, in order, are the new version's
    ///   lines, each once.
    /// - A line drawn as unchanged, with no paint, is unchanged: the old version has it at the
    ///   same place.
    /// - The tokens of an old line that is not drawn as removed are among the tokens of the new
    ///   version's lines in the hunk: nothing that was removed is drawn as kept.
    /// - Every line git's hunks remove is drawn as removed, drawn as unchanged in a line that stands
    ///   for it, or not drawn because its tokens are kept.
    /// - A line drawn as removed shows what was removed: it is removed throughout, or a token on it
    ///   is marked removed. Drawn with nothing marked, a removed line looks unchanged, as if it were
    ///   still there.
    #[test]
    fn what_the_view_shows_is_true() {
        for (name, fixture) in FIXTURES.iter().chain(&MORE_FIXTURES) {
            let Some(shown) = view(fixture.old, fixture.new, fixture.json, fixture.git) else {
                continue;
            };
            let versions = [Version::of(fixture.old), Version::of(fixture.new)];
            let [removed, _, _] = git_lines(fixture.git);
            let mut hunks: Vec<(std::ops::Range<usize>, std::ops::Range<usize>)> = vec![];
            for line in fixture.git.lines() {
                if let Some(header) = line.strip_prefix("@@ -") {
                    let (old, new) = header
                        .split_once(" @@")
                        .unwrap()
                        .0
                        .split_once(" +")
                        .unwrap();
                    let range = |range: &str| {
                        let (start, count) = range.split_once(',').unwrap_or((range, "1"));
                        let (start, count): (usize, usize) =
                            (start.parse().unwrap(), count.parse().unwrap());
                        let first = if count == 0 { start + 1 } else { start };
                        first..first + count
                    };
                    hunks.push((range(old), range(new)));
                }
            }
            let mut drawn_new: Vec<usize> = vec![];
            let mut drawn_removed: Vec<usize> = vec![];
            let mut next = [0, 0];
            for colored in shown.text.split_terminator('\n') {
                let line = uncolored(colored);
                let line = line.trim_end_matches('\n');
                if let Some(header) = line.strip_prefix("@@ -") {
                    let (old, new) = header
                        .split_once(" @@")
                        .unwrap()
                        .0
                        .split_once(" +")
                        .unwrap();
                    for (version, range) in [old, new].into_iter().enumerate() {
                        next[version] = range.split(',').next().unwrap().parse().unwrap();
                    }
                    continue;
                }
                if line.starts_with('\\') {
                    continue;
                }
                let (marker, content) = line.split_at(1);
                match marker {
                    "-" => {
                        if colored.starts_with('\x1b') {
                            assert!(
                                colored.starts_with(&Paint::Minus.sgr())
                                    || colored.contains(&Paint::MinusEmph.sgr()),
                                "{name}: {line:?} is drawn as removed with nothing on it marked"
                            );
                        }
                        drawn_removed.push(next[OLD]);
                        next[OLD] += 1;
                    }
                    "+" => {
                        drawn_new.push(next[NEW]);
                        next[NEW] += 1;
                    }
                    _ => {
                        if !colored.starts_with('\x1b') {
                            assert_eq!(
                                versions[OLD].lines[next[OLD] - 1],
                                content,
                                "{name}: {line:?} is drawn as unchanged but line {} of the old \
                                 version is not the same",
                                next[OLD]
                            );
                        }
                        drawn_new.push(next[NEW]);
                        next[OLD] += 1;
                        next[NEW] += 1;
                    }
                }
            }
            for (old, new) in &hunks {
                let in_hunk: Vec<usize> = drawn_new
                    .iter()
                    .copied()
                    .filter(|n| new.contains(n))
                    .collect();
                assert_eq!(
                    in_hunk,
                    new.clone().collect::<Vec<_>>(),
                    "{name}: the new version's lines are not each drawn once"
                );
                let new_tokens: Vec<String> = new
                    .clone()
                    .flat_map(|n| tokens(versions[NEW].lines[n - 1]))
                    .collect();
                for line in old
                    .clone()
                    .filter(|o| removed.contains(o) && !drawn_removed.contains(o))
                {
                    for token in tokens(versions[OLD].lines[line - 1]) {
                        assert!(
                            new_tokens.contains(&token),
                            "{name}: old line {line} is not drawn as removed, but its {token:?} \
                             is not in the new version"
                        );
                    }
                }
            }
        }
    }

    /// Each row of difftastic's view is what it claims to be: a removed row is the old version's
    /// line, removed in git's hunks; an added row is the new version's line, added in git's hunks;
    /// and an unchanged row is the new version's line, unchanged in git's hunks or standing for a
    /// line git's hunks remove and one they add.
    #[test]
    fn every_row_is_the_line_it_is_shown_as() {
        for (name, fixture) in FIXTURES.iter().chain(&MORE_FIXTURES) {
            let Some(shown) = view(fixture.old, fixture.new, fixture.json, fixture.git) else {
                continue;
            };
            let [removed, added, context] = git_lines(fixture.git);
            let versions = [Version::of(fixture.old), Version::of(fixture.new)];
            let text = uncolored(&shown.text);
            let mut next = [0, 0];
            for line in text.split_terminator('\n') {
                if let Some(header) = line.strip_prefix("@@ -") {
                    let (old, new) = header
                        .split_once(" @@")
                        .unwrap()
                        .0
                        .split_once(" +")
                        .unwrap();
                    for (version, range) in [old, new].into_iter().enumerate() {
                        next[version] = range.split(',').next().unwrap().parse().unwrap();
                    }
                    continue;
                }
                if line.starts_with('\\') {
                    continue;
                }
                let (marker, content) = line.split_at(1);
                let (version, number) = match marker {
                    "-" => (OLD, next[OLD]),
                    _ => (NEW, next[NEW]),
                };
                assert_eq!(
                    versions[version].lines[number - 1],
                    content,
                    "{name}: {line:?} is not line {number} of its version"
                );
                let in_git = match marker {
                    "-" => removed.contains(&number),
                    "+" => added.contains(&number),
                    _ => {
                        context.contains(&number)
                            || shown
                                .stands_for
                                .get(&number)
                                .is_some_and(|old| removed.contains(old) && added.contains(&number))
                    }
                };
                assert!(in_git, "{name}: {line:?} is not a line of git's hunks");
                for (version, other) in [(OLD, "+"), (NEW, "-")] {
                    if marker != other {
                        next[version] += 1;
                    }
                }
            }
        }
    }

    #[test]
    fn a_renamed_variable_in_rewrapped_and_reindented_code() {
        let shown = shown(&FIXTURES[0].1);
        assert_eq!(
            legible(&shown.text),
            r#"@@ -1,6 +1,12 @@
 .fn load(p: &Path⟦, v: bool⟧) -> Cfg {
 .    // parse the ⟦TOML⟧ file
-.    let ⟦c⟧ = parse(&read(p), Strict, None);
+.    let ⟦cfg⟧ = parse(
+~        &read(p),
+~        Strict,
+~        None,
+~    );
+!    if v {
-.    check(&⟦c⟧);
+.        check(&⟦cfg⟧);
-!    c
+!    }
+!    cfg
 }
"#
        );
        assert_eq!(shown.stands_for, HashMap::from([(1, 1), (2, 2)]));
    }

    #[test]
    fn a_boolean_expression_rewritten_as_a_match() {
        let shown = shown(&FIXTURES[1].1);
        assert_eq!(
            legible(&shown.text),
            r#"@@ -1,8 +1,12 @@
 impl StateMachine {
     fn test_submodule_short_line(&self) -> bool {
+!        match self.state {
-.        ⟦matches!(self.state,⟧ State::HunkHeader(_, _, _, _)⟦)⟧
+.            State::HunkHeader(_, _, _, _) ⟦=> {⟧
-.            ⟦&&⟧ self.line.starts_with("-Subproject commit ")
+.                self.line.starts_with("-Subproject commit ")
+!                    || self.line.starts_with("+Subproject commit ")
+!            }
-.            ⟦|| matches!(self.state,⟧ State::SubmoduleShort(_)⟦)⟧
+.            State::SubmoduleShort(_) ⟦=>⟧ self.line.starts_with("+Subproject commit ")⟦,⟧
-.                ⟦&&⟧ self.line.starts_with("+Subproject commit ")
+!            _ => false,
+!        }
     }
 }
"#
        );
    }

    #[test]
    fn a_last_line_without_a_newline() {
        let shown = shown(&FIXTURES[2].1);
        assert_eq!(
            legible(&shown.text),
            "@@ -1,3 +1,3 @@
 fn f() {
-.    ⟦a⟧();
+.    ⟦b⟧();
 }
\\ No newline at end of file
"
        );
    }

    #[test]
    fn an_added_line() {
        let shown = shown(&FIXTURES[3].1);
        assert_eq!(
            legible(&shown.text),
            "@@ -1,2 +1,3 @@
 fn f() {}
+!fn g() {}
 fn h() {}
"
        );
    }

    /// A change only to layout is shown as the new version's lines, marked as such, and not as
    /// the old version's lines as well.
    #[test]
    fn a_change_only_to_layout() {
        let shown = shown(&FIXTURES[4].1);
        assert_eq!(
            legible(&shown.text),
            "@@ -1 +1,3 @@
+~fn f() {
+~    g();
+~}
"
        );
    }

    /// Old lines joined into a new line, with nothing changed on them, are not shown: the new
    /// line is.
    #[test]
    fn lines_joined_into_one() {
        let shown = shown(&FIXTURES[6].1);
        assert_eq!(
            legible(&shown.text),
            "@@ -1,6 +1,3 @@
 fn f() {
-.    let ⟦x⟧ = add(
+.    let ⟦y⟧ = add(a, b);
@@ -6 +3 @@
 }
"
        );
    }

    /// Where difftastic pairs the wrong lines, as here where it pairs a rewritten function with
    /// the new function after it and so shows the rewritten one as new throughout, git's hunks are
    /// shown; where it shows the change better, its own are.
    #[test]
    fn git_where_difftastic_marks_more_lines_changed() {
        let text = uncolored(&shown(&FIXTURES[5].1).text);
        assert!(
            text.contains(
                "\n     pub fn handle_submodule_short_line(&mut self) -> std::io::Result<bool> {\n"
            ),
            "{text}"
        );
        assert!(
            text.contains(
                "+        match self.state {\n-        matches!(self.state, State::HunkHeader(_, _, _, _))\n"
            ),
            "{text}"
        );
    }

    /// The view's hunks are git's, headed as git heads them.
    #[test]
    fn hunks_are_headed_as_git_heads_them() {
        let text = uncolored(&shown(&FIXTURES[5].1).text);
        let header = text.lines().next().unwrap();
        assert_eq!(
            header,
            "@@ -35,63 +35,89 @@ fn submodule_log_osc(&self) -> String {"
        );
    }

    /// Output for other versions, whose changed tokens are not at the lines and columns it says,
    /// is not used, even where the lines it pairs exist.
    #[test]
    fn output_for_other_versions_is_not_used() {
        let fixture = &FIXTURES[1].1;
        let new = fixture.new.replace("match self.state", "mutch self.state");
        assert!(view(fixture.old, &new, fixture.json, fixture.git).is_none());
    }
}
