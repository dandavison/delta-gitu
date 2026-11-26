use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Status {
    #[default]
    Pending,
    Aborted,
    Done,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct TextInput {
    pub(crate) value: String,
    /// Counted in chars, not bytes.
    position: usize,
    pub(crate) status: Status,
    pub(crate) focused: bool,
    /// What the last kill took, for `ctrl+y` to put back.
    pub(crate) killed: String,
}

impl TextInput {
    /// Splits the value around the char the cursor is on, so it can be drawn
    /// as a cursor rather than the cursor taking up a column of its own. That
    /// char is empty with the cursor at the end of the value.
    pub(crate) fn position(&self) -> usize {
        self.position.min(self.len())
    }

    pub(crate) fn move_to(&mut self, position: usize) {
        self.position = position.min(self.len());
    }

    pub(crate) fn split_at_cursor(&self) -> (&str, &str, &str) {
        let byte_at = |position| {
            self.value
                .char_indices()
                .nth(position)
                .map_or(self.value.len(), |(byte, _)| byte)
        };

        let start = byte_at(self.position);
        let end = byte_at(self.position + 1);

        (
            &self.value[..start],
            &self.value[start..end],
            &self.value[end..],
        )
    }

    /// Edit the line as `key` says, with readline's keys.
    ///
    /// Words come in the two kinds readline has: `alt+b`/`alt+f`/`alt+d`/`alt+backspace` step
    /// over runs of letters and digits, so they move within a path, while `ctrl+w` takes back a
    /// whole whitespace-delimited token, such as a pathspec. What a kill takes, `ctrl+y` puts back.
    pub(crate) fn handle_key_event(&mut self, key: KeyEvent) {
        if key.kind == KeyEventKind::Release {
            return;
        }

        const ALT: KeyModifiers = KeyModifiers::ALT;
        const CTRL: KeyModifiers = KeyModifiers::CONTROL;
        let chars: Vec<char> = self.value.chars().collect();
        let cursor = self.position();

        match (key.code, key.modifiers) {
            (KeyCode::Char('b'), ALT) | (KeyCode::Left, ALT | CTRL) => {
                self.move_to(previous_word_start(&chars, cursor));
            }
            (KeyCode::Char('f'), ALT) | (KeyCode::Right, ALT | CTRL) => {
                self.move_to(next_word_end(&chars, cursor));
            }
            (KeyCode::Char('w'), CTRL) => self.kill(previous_token_start(&chars, cursor)..cursor),
            (KeyCode::Backspace, ALT) => self.kill(previous_word_start(&chars, cursor)..cursor),
            (KeyCode::Char('d'), ALT) => self.kill(cursor..next_word_end(&chars, cursor)),
            (KeyCode::Char('k'), CTRL) => self.kill(cursor..chars.len()),
            (KeyCode::Char('u'), CTRL) => self.kill(0..chars.len()),
            (KeyCode::Char('y'), CTRL) => self.yank(),
            (KeyCode::Char('t'), CTRL) => self.transpose(),
            (KeyCode::Enter, _) => self.status = Status::Done,
            (KeyCode::Esc, _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                self.status = Status::Aborted;
            }
            (KeyCode::Left, _) | (KeyCode::Char('b'), KeyModifiers::CONTROL) => {
                self.position = self.position.saturating_sub(1);
            }
            (KeyCode::Right, _) | (KeyCode::Char('f'), KeyModifiers::CONTROL) => {
                self.position = self.position.saturating_add(1).min(self.len());
            }
            (KeyCode::Home, _) | (KeyCode::Char('a'), KeyModifiers::CONTROL) => self.position = 0,
            (KeyCode::End, _) | (KeyCode::Char('e'), KeyModifiers::CONTROL) => {
                self.position = self.len();
            }
            (KeyCode::Backspace, _) | (KeyCode::Char('h'), KeyModifiers::CONTROL) => {
                if self.position > 0 {
                    self.remove(self.position - 1);
                    self.position -= 1;
                }
            }
            (KeyCode::Delete, _) | (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
                if self.position < self.len() {
                    self.remove(self.position);
                }
            }
            (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                self.insert(c);
                self.position += 1;
            }
            _ => (),
        }
    }

    fn len(&self) -> usize {
        self.value.chars().count()
    }

    /// Take the chars in `range` out of the line, for `ctrl+y` to put back. Taking nothing is not
    /// a kill: it leaves what was killed before to be put back.
    fn kill(&mut self, range: std::ops::Range<usize>) {
        if range.is_empty() {
            return;
        }
        let chars: Vec<char> = self.value.chars().collect();
        self.killed = chars[range.clone()].iter().collect();
        self.value = chars[..range.start]
            .iter()
            .chain(&chars[range.end..])
            .collect();
        self.move_to(range.start);
    }

    /// Put what the last kill took back at the cursor.
    fn yank(&mut self) {
        let chars: Vec<char> = self.value.chars().collect();
        let cursor = self.position();
        let killed: Vec<char> = self.killed.chars().collect();
        self.value = chars[..cursor]
            .iter()
            .chain(&killed)
            .chain(&chars[cursor..])
            .collect();
        self.move_to(cursor + killed.len());
    }

    /// Swap the two characters the cursor sits between, and step over them. At the end of the
    /// line there is nothing to the right to swap with, so the last two change places instead.
    fn transpose(&mut self) {
        let mut chars: Vec<char> = self.value.chars().collect();
        if chars.len() < 2 {
            return;
        }
        let cursor = self.position().min(chars.len() - 1).max(1);
        chars.swap(cursor - 1, cursor);
        self.value = chars.iter().collect();
        self.move_to(cursor + 1);
    }

    /// Editing goes via chars, as `String`'s own byte-indexed methods would
    /// split multi-byte ones.
    fn remove(&mut self, position: usize) {
        self.value = self
            .value
            .chars()
            .take(position)
            .chain(self.value.chars().skip(position + 1))
            .collect();
    }

    fn insert(&mut self, c: char) {
        if self.position == self.len() {
            self.value.push(c);
            return;
        }

        self.value = self
            .value
            .chars()
            .take(self.position)
            .chain(std::iter::once(c))
            .chain(self.value.chars().skip(self.position))
            .collect();
    }
}

/// Where the run of letters and digits before `cursor` in `chars` begins. Readline's words, as
/// `alt+b` and `alt+backspace` take them.
pub(crate) fn previous_word_start(chars: &[char], cursor: usize) -> usize {
    let end = skipping_back(chars, cursor, |c| !c.is_alphanumeric());
    skipping_back(chars, end, |c| c.is_alphanumeric())
}

/// Where the run of letters and digits after `cursor` in `chars` ends. Readline's words, as
/// `alt+f` and `alt+d` take them.
pub(crate) fn next_word_end(chars: &[char], cursor: usize) -> usize {
    let start = skipping_on(chars, cursor, |c| !c.is_alphanumeric());
    skipping_on(chars, start, |c| c.is_alphanumeric())
}

/// Where the whitespace-delimited token before `cursor` in `chars` begins, as `ctrl+w` takes it.
pub(crate) fn previous_token_start(chars: &[char], cursor: usize) -> usize {
    let end = skipping_back(chars, cursor, |c| c.is_whitespace());
    skipping_back(chars, end, |c| !c.is_whitespace())
}

fn skipping_back(chars: &[char], from: usize, matching: impl Fn(&char) -> bool) -> usize {
    let kept = chars[..from]
        .iter()
        .rev()
        .take_while(|c| matching(c))
        .count();
    from - kept
}

fn skipping_on(chars: &[char], from: usize, matching: impl Fn(&char) -> bool) -> usize {
    from + chars[from..].iter().take_while(|c| matching(c)).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(input: &mut TextInput, code: KeyCode) {
        input.handle_key_event(KeyEvent::new(code, KeyModifiers::empty()));
    }

    fn ctrl(input: &mut TextInput, c: char) {
        input.handle_key_event(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL));
    }

    fn typed(text: &str) -> TextInput {
        let mut input = TextInput::default();
        for c in text.chars() {
            press(&mut input, KeyCode::Char(c));
        }
        input
    }

    #[test]
    fn types_and_edits() {
        let mut input = typed("abc");
        assert_eq!(input.value, "abc");

        press(&mut input, KeyCode::Backspace);
        assert_eq!(input.value, "ab");

        press(&mut input, KeyCode::Left);
        press(&mut input, KeyCode::Char('x'));
        assert_eq!(input.value, "axb");

        press(&mut input, KeyCode::Delete);
        assert_eq!(input.value, "ax");
    }

    #[test]
    fn edits_multibyte_chars() {
        let mut input = typed("åäö");

        press(&mut input, KeyCode::Backspace);
        assert_eq!(input.value, "åä");

        press(&mut input, KeyCode::Home);
        press(&mut input, KeyCode::Delete);
        assert_eq!(input.value, "ä");

        press(&mut input, KeyCode::Char('ö'));
        assert_eq!(input.value, "öä");
    }

    #[test]
    fn moves_within_bounds() {
        let mut input = typed("ab");

        press(&mut input, KeyCode::Right);
        press(&mut input, KeyCode::Right);
        press(&mut input, KeyCode::Char('c'));
        assert_eq!(input.value, "abc");

        press(&mut input, KeyCode::Home);
        press(&mut input, KeyCode::Left);
        press(&mut input, KeyCode::Char('x'));
        assert_eq!(input.value, "xabc");
    }

    #[test]
    fn kills_and_truncates() {
        let mut input = typed("abcd");
        press(&mut input, KeyCode::Left);
        ctrl(&mut input, 'k');
        assert_eq!(input.value, "abc");

        ctrl(&mut input, 'u');
        assert_eq!(input.value, "");

        press(&mut input, KeyCode::Char('e'));
        assert_eq!(input.value, "e");
    }

    #[test]
    fn tracks_status() {
        let mut input = TextInput::default();
        assert_eq!(input.status, Status::Pending);

        press(&mut input, KeyCode::Enter);
        assert_eq!(input.status, Status::Done);

        let mut input = TextInput::default();
        press(&mut input, KeyCode::Esc);
        assert_eq!(input.status, Status::Aborted);

        let mut input = TextInput::default();
        ctrl(&mut input, 'c');
        assert_eq!(input.status, Status::Aborted);
    }
}
