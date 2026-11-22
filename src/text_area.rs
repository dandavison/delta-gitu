//! A multi-line text box, such as a PR comment being written: its text, wrapped
//! to a width, and where the cursor is in it, edited by keys.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::ops::Range;
use unicode_width::UnicodeWidthChar;

/// What a key did to a [`TextArea`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// The text changed.
    Edited,
    /// Only the cursor moved.
    Moved,
    /// The text is to be sent: `alt+enter` or `ctrl+s`.
    Submit,
    /// The box is to be left, its text kept: `esc`.
    Leave,
    /// The key does nothing here.
    Ignored,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct TextArea {
    value: String,
    /// Where the cursor is, counted in chars.
    cursor: usize,
    /// The column moving up or down keeps to, as it was before a shorter row
    /// was passed through.
    goal_column: Option<usize>,
}

impl TextArea {
    pub(crate) fn value(&self) -> &str {
        &self.value
    }

    /// The rows the text is drawn in at `width` columns, as char ranges of
    /// it: each line, split at spaces where it is wider than `width`, a space
    /// a row is split at ending the row before. A word wider than `width` is
    /// split where the row ends. A line break is in no row.
    pub(crate) fn rows(&self, width: usize) -> Vec<Range<usize>> {
        let chars = self.chars();
        let width = width.max(1);
        let mut rows = vec![];
        let mut line_start = 0;
        loop {
            let line_end = chars[line_start..]
                .iter()
                .position(|&c| c == '\n')
                .map_or(chars.len(), |i| line_start + i);
            wrap_line(&chars, line_start..line_end, width, &mut rows);
            if line_end == chars.len() {
                return rows;
            }
            line_start = line_end + 1;
        }
    }

    /// The row and column the cursor is drawn at, at `width` columns.
    pub(crate) fn cursor_at(&self, width: usize) -> (usize, usize) {
        let chars = self.chars();
        let rows = self.rows(width);
        // Where a line is split at a space, the cursor after the space is at the next row's start.
        let row = rows
            .iter()
            .rposition(|row| row.start <= self.cursor && self.cursor <= row.end)
            .unwrap_or(0);
        let column = columns(&chars[rows[row].start..self.cursor]);
        (row, column)
    }

    /// Insert `text` where the cursor is, as a paste does.
    pub(crate) fn insert_str(&mut self, text: &str) {
        let mut chars = self.chars();
        let inserted: Vec<char> = text.chars().collect();
        let count = inserted.len();
        chars.splice(self.cursor..self.cursor, inserted);
        self.value = chars.into_iter().collect();
        self.cursor += count;
        self.goal_column = None;
    }

    /// Edit or move as `key` says, with the text drawn at `width` columns,
    /// which moving up and down goes by.
    pub(crate) fn handle_key(&mut self, key: KeyEvent, width: usize) -> Outcome {
        if key.kind == KeyEventKind::Release {
            return Outcome::Ignored;
        }
        let ctrl = |c: char| (KeyCode::Char(c), KeyModifiers::CONTROL);
        let code_and_modifiers = (key.code, key.modifiers);
        let outcome = match code_and_modifiers {
            (KeyCode::Enter, KeyModifiers::ALT) => return Outcome::Submit,
            c if c == ctrl('s') => return Outcome::Submit,
            (KeyCode::Esc, _) => return Outcome::Leave,
            (KeyCode::Enter, _) => {
                self.insert_str("\n");
                Outcome::Edited
            }
            (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                self.insert_str(&c.to_string());
                Outcome::Edited
            }
            (KeyCode::Up, _) => return self.move_vertically(width, false),
            c if c == ctrl('p') => return self.move_vertically(width, false),
            (KeyCode::Down, _) => return self.move_vertically(width, true),
            c if c == ctrl('n') => return self.move_vertically(width, true),
            (KeyCode::Backspace, _) => self.delete(self.cursor.saturating_sub(1)..self.cursor),
            c if c == ctrl('h') => self.delete(self.cursor.saturating_sub(1)..self.cursor),
            (KeyCode::Delete, _) => self.delete(self.cursor..self.cursor + 1),
            c if c == ctrl('d') => self.delete(self.cursor..self.cursor + 1),
            (KeyCode::Left, _) => self.move_to(self.cursor.saturating_sub(1)),
            c if c == ctrl('b') => self.move_to(self.cursor.saturating_sub(1)),
            (KeyCode::Right, _) => self.move_to(self.cursor + 1),
            c if c == ctrl('f') => self.move_to(self.cursor + 1),
            (KeyCode::Home, _) => self.move_to(self.line().start),
            c if c == ctrl('a') => self.move_to(self.line().start),
            (KeyCode::End, _) => self.move_to(self.line().end),
            c if c == ctrl('e') => self.move_to(self.line().end),
            c if c == ctrl('u') => self.delete(self.line().start..self.cursor),
            c if c == ctrl('k') => self.delete(self.cursor..self.line().end),
            c if c == ctrl('w') => self.delete(self.word_start()..self.cursor),
            _ => return Outcome::Ignored,
        };
        self.goal_column = None;
        outcome
    }

    fn chars(&self) -> Vec<char> {
        self.value.chars().collect()
    }

    fn move_to(&mut self, cursor: usize) -> Outcome {
        self.cursor = cursor.min(self.value.chars().count());
        Outcome::Moved
    }

    /// Delete the chars in `range`, cut to the text, leaving the cursor where they were.
    fn delete(&mut self, range: Range<usize>) -> Outcome {
        let mut chars = self.chars();
        let range = range.start.min(chars.len())..range.end.min(chars.len());
        if range.is_empty() {
            return Outcome::Ignored;
        }
        self.cursor = range.start;
        chars.drain(range);
        self.value = chars.into_iter().collect();
        Outcome::Edited
    }

    /// The line the cursor is in, as a char range of the text without its line break.
    fn line(&self) -> Range<usize> {
        let chars = self.chars();
        let start = chars[..self.cursor]
            .iter()
            .rposition(|&c| c == '\n')
            .map_or(0, |i| i + 1);
        let end = chars[self.cursor..]
            .iter()
            .position(|&c| c == '\n')
            .map_or(chars.len(), |i| self.cursor + i);
        start..end
    }

    /// Where the word before the cursor starts, past any spaces between it and the cursor.
    fn word_start(&self) -> usize {
        let chars = self.chars();
        let mut start = self.cursor;
        while start > 0 && chars[start - 1] == ' ' {
            start -= 1;
        }
        while start > 0 && !chars[start - 1].is_whitespace() {
            start -= 1;
        }
        start
    }

    /// Move to the row below, or above, at the column moving up and down keeps to, or the row's
    /// end if it is shorter.
    fn move_vertically(&mut self, width: usize, down: bool) -> Outcome {
        let rows = self.rows(width);
        let (row, column) = self.cursor_at(width);
        let target = if down {
            row + 1
        } else {
            match row.checked_sub(1) {
                Some(target) => target,
                None => return Outcome::Ignored,
            }
        };
        let Some(range) = rows.get(target) else {
            return Outcome::Ignored;
        };
        let goal = *self.goal_column.get_or_insert(column);
        let chars = self.chars();
        // A row split at a space ends with it, and the cursor after it is at the next row's start.
        let split = rows
            .get(target + 1)
            .is_some_and(|next| next.start == range.end);
        let last = if split && range.end > range.start {
            range.end - 1
        } else {
            range.end
        };
        let mut cursor = range.start;
        let mut used = 0;
        while cursor < last {
            let next = used + columns(&chars[cursor..cursor + 1]);
            if next > goal {
                break;
            }
            used = next;
            cursor += 1;
        }
        self.cursor = cursor;
        Outcome::Moved
    }
}

/// The columns `chars` take on the screen.
fn columns(chars: &[char]) -> usize {
    chars.iter().map(|c| c.width().unwrap_or(0)).sum()
}

/// Add to `rows` the rows of the line `line`, a char range of `chars` without a line break, at
/// `width` columns.
fn wrap_line(chars: &[char], line: Range<usize>, width: usize, rows: &mut Vec<Range<usize>>) {
    let mut start = line.start;
    loop {
        let mut end = start;
        let mut used = 0;
        while end < line.end {
            let next = used + columns(&chars[end..end + 1]);
            if next > width {
                break;
            }
            used = next;
            end += 1;
        }
        if end == line.end {
            rows.push(start..end);
            return;
        }
        let split = if chars[end] == ' ' {
            end + 1
        } else {
            match chars[start..end].iter().rposition(|&c| c == ' ') {
                Some(space) => start + space + 1,
                // A word wider than a row; at least one char a row, however narrow.
                None => end.max(start + 1),
            }
        };
        rows.push(start..split);
        start = split;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIDTH: usize = 80;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    fn press(area: &mut TextArea, code: KeyCode) -> Outcome {
        area.handle_key(key(code, KeyModifiers::NONE), WIDTH)
    }

    fn ctrl(area: &mut TextArea, c: char) -> Outcome {
        area.handle_key(key(KeyCode::Char(c), KeyModifiers::CONTROL), WIDTH)
    }

    fn typed(text: &str) -> TextArea {
        let mut area = TextArea::default();
        for c in text.chars() {
            if c == '\n' {
                press(&mut area, KeyCode::Enter);
            } else {
                press(&mut area, KeyCode::Char(c));
            }
        }
        area
    }

    /// The text of each row at `width`.
    fn rows(area: &TextArea, width: usize) -> Vec<String> {
        let chars: Vec<char> = area.value().chars().collect();
        area.rows(width)
            .into_iter()
            .map(|range| chars[range].iter().collect())
            .collect()
    }

    #[test]
    fn enter_starts_a_new_line() {
        let mut area = typed("ab");
        assert_eq!(press(&mut area, KeyCode::Enter), Outcome::Edited);
        press(&mut area, KeyCode::Char('c'));
        assert_eq!(area.value(), "ab\nc");
    }

    #[test]
    fn backspace_at_a_lines_start_joins_it_to_the_line_before() {
        let mut area = typed("ab\nc");
        ctrl(&mut area, 'a');
        press(&mut area, KeyCode::Backspace);
        assert_eq!(area.value(), "abc");
        press(&mut area, KeyCode::Char('x'));
        assert_eq!(area.value(), "abxc");
    }

    #[test]
    fn rows_wrap_at_spaces_and_at_line_breaks() {
        assert_eq!(rows(&typed("aa bb cc"), 4), ["aa ", "bb ", "cc"]);
        assert_eq!(rows(&typed("aa bb cc"), 5), ["aa bb ", "cc"]);
        assert_eq!(rows(&typed("ab\ncd"), 5), ["ab", "cd"]);
        assert_eq!(rows(&typed("ab\n"), 5), ["ab", ""]);
        assert_eq!(rows(&TextArea::default(), 5), [""]);
    }

    #[test]
    fn a_word_wider_than_a_row_is_split_where_the_row_ends() {
        assert_eq!(rows(&typed("abcdefg"), 3), ["abc", "def", "g"]);
    }

    #[test]
    fn up_and_down_keep_to_a_column() {
        let mut area = typed("abcd\nx\nabcd");
        assert_eq!(area.cursor_at(WIDTH), (2, 4));
        assert_eq!(press(&mut area, KeyCode::Up), Outcome::Moved);
        assert_eq!(area.cursor_at(WIDTH), (1, 1));
        press(&mut area, KeyCode::Up);
        assert_eq!(area.cursor_at(WIDTH), (0, 4));
        press(&mut area, KeyCode::Up);
        assert_eq!(area.cursor_at(WIDTH), (0, 4), "the first row");
        press(&mut area, KeyCode::Down);
        press(&mut area, KeyCode::Down);
        assert_eq!(area.cursor_at(WIDTH), (2, 4));
    }

    #[test]
    fn up_and_down_go_through_a_lines_wrapped_rows() {
        let mut area = typed("aa bb cc");
        ctrl(&mut area, 'a');
        area.handle_key(key(KeyCode::Down, KeyModifiers::NONE), 4);
        assert_eq!(area.cursor_at(4), (1, 0));
        press(&mut area, KeyCode::Char('X'));
        assert_eq!(area.value(), "aa Xbb cc");
    }

    #[test]
    fn left_and_right_go_across_line_breaks() {
        let mut area = typed("ab\ncd");
        ctrl(&mut area, 'a');
        press(&mut area, KeyCode::Left);
        assert_eq!(area.cursor_at(WIDTH), (0, 2));
        press(&mut area, KeyCode::Right);
        assert_eq!(area.cursor_at(WIDTH), (1, 0));
    }

    #[test]
    fn home_and_end_go_to_the_lines_start_and_end() {
        let mut area = typed("ab\ncd");
        press(&mut area, KeyCode::Home);
        press(&mut area, KeyCode::Char('x'));
        assert_eq!(area.value(), "ab\nxcd");
        ctrl(&mut area, 'e');
        press(&mut area, KeyCode::Char('y'));
        assert_eq!(area.value(), "ab\nxcdy");
    }

    #[test]
    fn ctrl_w_deletes_the_word_before_the_cursor() {
        let mut area = typed("foo bar");
        assert_eq!(ctrl(&mut area, 'w'), Outcome::Edited);
        assert_eq!(area.value(), "foo ");
        ctrl(&mut area, 'w');
        assert_eq!(area.value(), "");
    }

    #[test]
    fn ctrl_u_and_ctrl_k_delete_to_the_lines_start_and_end() {
        let mut area = typed("ab\ncdef\ngh");
        press(&mut area, KeyCode::Up);
        ctrl(&mut area, 'k');
        assert_eq!(area.value(), "ab\ncd\ngh");
        ctrl(&mut area, 'u');
        assert_eq!(area.value(), "ab\n\ngh");
    }

    #[test]
    fn delete_removes_the_char_at_the_cursor() {
        let mut area = typed("ab\ncd");
        press(&mut area, KeyCode::Up);
        press(&mut area, KeyCode::Delete);
        assert_eq!(area.value(), "abcd");
    }

    #[test]
    fn alt_enter_and_ctrl_s_submit_and_esc_leaves() {
        let mut area = typed("ab");
        assert_eq!(
            area.handle_key(key(KeyCode::Enter, KeyModifiers::ALT), WIDTH),
            Outcome::Submit
        );
        assert_eq!(ctrl(&mut area, 's'), Outcome::Submit);
        assert_eq!(press(&mut area, KeyCode::Esc), Outcome::Leave);
        assert_eq!(area.value(), "ab");
    }

    #[test]
    fn edits_multibyte_chars() {
        let mut area = typed("åäö");
        press(&mut area, KeyCode::Backspace);
        assert_eq!(area.value(), "åä");
        press(&mut area, KeyCode::Left);
        press(&mut area, KeyCode::Char('x'));
        assert_eq!(area.value(), "åxä");
    }

    #[test]
    fn wide_chars_take_two_columns() {
        let area = typed("日本");
        assert_eq!(area.cursor_at(WIDTH), (0, 4));
        assert_eq!(rows(&typed("日本語"), 4), ["日本", "語"]);
    }

    #[test]
    fn a_paste_is_inserted_whole() {
        let mut area = typed("ad");
        press(&mut area, KeyCode::Left);
        area.insert_str("b\nc");
        assert_eq!(area.value(), "ab\ncd");
        assert_eq!(area.cursor_at(WIDTH), (1, 1));
    }
}
