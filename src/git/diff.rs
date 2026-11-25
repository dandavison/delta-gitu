use crate::gitu_diff::{ContentLine, FileDiff};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub(crate) struct Diff {
    pub text: String,
    /// What the renderer draws in place of `text`, if anything: difftastic's view of the diff (see
    /// [`crate::difftastic`]).
    pub display: Option<Display>,
    pub diff_type: DiffType,
    pub file_diffs: Vec<FileDiff>,
    pub commit: Option<String>,
}

/// What the renderer draws for a diff in place of its text: not a patch, but each line it draws is a
/// line of the diff's hunks, or stands for a line they remove and one they add.
#[derive(Debug, Clone, Default)]
pub(crate) struct Display {
    pub text: String,
    /// The lines drawn as unchanged that stand for a line the diff's hunks remove and one they
    /// add: by file index and new-version line number, the old version's line number.
    pub stands_for: HashMap<(usize, u32), u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DiffType {
    WorkdirToIndex, // i.e. Unstaged
    IndexToTree,    // i.e. Staged
    TreeToTree,
}

#[derive(Debug)]
pub(crate) enum PatchMode {
    Normal,
    Reverse,
}

impl Diff {
    /// What the renderer draws the diff from.
    pub(crate) fn renderer_input(&self) -> &str {
        self.display
            .as_ref()
            .map_or(&self.text, |display| &display.text)
    }

    pub(crate) fn format_file_patch(&self, file_i: usize) -> String {
        let mut patch = String::new();
        patch.push_str(self.file_diff_header(file_i));
        for hunk in &self.file_diffs[file_i].hunks {
            patch.push_str(&self.text[hunk.range.clone()]);
        }
        patch
    }

    pub(crate) fn format_hunk_patch(&self, file_i: usize, hunk_i: usize) -> String {
        let mut patch = String::new();
        patch.push_str(self.file_diff_header(file_i));
        patch.push_str(self.hunk(file_i, hunk_i));
        patch
    }

    pub(crate) fn file_diff_header(&self, file_i: usize) -> &str {
        &self.text[self.file_diffs[file_i].header.range.clone()]
    }

    pub(crate) fn hunk(&self, file_i: usize, hunk_i: usize) -> &str {
        &self.text[self.file_diffs[file_i].hunks[hunk_i].range.clone()]
    }

    pub(crate) fn hunk_content(&self, file_index: usize, hunk_index: usize) -> &str {
        &self.text[self.file_diffs[file_index].hunks[hunk_index]
            .content
            .range
            .clone()]
    }

    /// Build a patch that stages/reverses exactly the given content lines of a
    /// hunk. `line_indices` need not be contiguous: a side-by-side row that fuses
    /// an in-place change maps to a deletion and its (possibly non-adjacent)
    /// replacement, and both are staged together.
    pub(crate) fn format_lines_patch(
        &self,
        file_i: usize,
        hunk_i: usize,
        line_indices: &[usize],
        mode: PatchMode,
    ) -> String {
        let hunk = &self.file_diffs[file_i].hunks[hunk_i];
        let file_header = &self.text[self.file_diffs[file_i].header.range.clone()];
        let hunk_header = &self.text[hunk.header.range.clone()];
        let hunk_content = &self.text[hunk.content.range.clone()];

        let modified_content = hunk_content
            .split_inclusive('\n')
            .enumerate()
            .filter_map(|(i, line)| {
                let add = match mode {
                    PatchMode::Normal => '+',
                    PatchMode::Reverse => '-',
                };

                let remove = match mode {
                    PatchMode::Normal => '-',
                    PatchMode::Reverse => '+',
                };

                if line_indices.contains(&i) {
                    Some(line.to_string())
                } else if line.starts_with(add) {
                    None
                } else if let Some(stripped) = line.strip_prefix(remove) {
                    Some(format!(" {stripped}"))
                } else {
                    Some(line.to_string())
                }
            })
            .collect::<String>();

        format!("{file_header}{hunk_header}{modified_content}")
    }

    /// Whether a file's hunks are a combined diff's, which have a column per
    /// parent and which `git apply` cannot apply.
    pub(crate) fn is_combined(&self, file_i: usize) -> bool {
        self.file_diffs[file_i]
            .hunks
            .iter()
            .any(|hunk| hunk.header.old_line_starts.len() > 1)
    }

    /// Each line of a hunk's content, with what it is (`None` for a line that is
    /// no line of a file).
    fn content_lines(
        &self,
        file_i: usize,
        hunk_i: usize,
    ) -> impl Iterator<Item = (&str, Option<ContentLine>)> {
        let header = &self.file_diffs[file_i].hunks[hunk_i].header;
        self.hunk_content(file_i, hunk_i)
            .lines()
            .map(move |line| (line, header.content_line(line)))
    }

    pub(crate) fn hunk_first_changed_line_num(&self, file_i: usize, hunk_i: usize) -> u32 {
        let new_line_start = self.file_diffs[file_i].hunks[hunk_i].header.new_line_start;
        let mut new_line = new_line_start;
        for (_, content_line) in self.content_lines(file_i, hunk_i) {
            match content_line {
                Some(ContentLine::Added) => return new_line,
                Some(ContentLine::Removed { .. }) => {}
                _ => new_line += 1,
            }
        }
        new_line_start
    }

    pub(crate) fn hunk_line_new_file_num(
        &self,
        file_i: usize,
        hunk_i: usize,
        line_i: usize,
    ) -> u32 {
        let hunk = &self.file_diffs[file_i].hunks[hunk_i];
        let non_minus_before = self
            .content_lines(file_i, hunk_i)
            .take(line_i)
            .filter(|(_, content_line)| content_line.is_some_and(|c| c.in_new()))
            .count() as u32;
        hunk.header.new_line_start + non_minus_before
    }

    pub(crate) fn hunk_line_old_file_num(
        &self,
        file_i: usize,
        hunk_i: usize,
        line_i: usize,
    ) -> u32 {
        let hunk = &self.file_diffs[file_i].hunks[hunk_i];
        let non_plus_before = self
            .content_lines(file_i, hunk_i)
            .take(line_i)
            .filter(|(line, content_line)| content_line.is_some_and(|c| c.in_parent(0, line)))
            .count() as u32;
        hunk.header.old_line_start + non_plus_before
    }

    pub(crate) fn file_line_of_first_diff(&self, file_i: usize, hunk_i: usize) -> usize {
        let hunk = &self.file_diffs[file_i].hunks[hunk_i];
        let line = hunk.header.new_line_start as usize;

        for (i, (_, content_line)) in self.content_lines(file_i, hunk_i).enumerate() {
            if matches!(
                content_line,
                Some(ContentLine::Added | ContentLine::Removed { .. })
            ) {
                return line + i;
            }
        }
        line
    }
}
