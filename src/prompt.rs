use crate::Res;
use crate::error::Error;
use crate::text_input::TextInput;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::borrow::Cow;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

pub(crate) struct PromptData {
    pub(crate) prompt_text: Cow<'static, str>,
    /// Drawn alone in front of the line typed on, as less and vim draw `/` for
    /// a search, rather than as a question.
    pub(crate) bare: bool,
}

#[derive(Clone)]
pub(crate) enum HistoryKind {
    GitCommand,
    FilePatterns,
    Search,
    /// What was typed for `{input}` in the command or view of this name.
    Input(String),
    /// Messages to agents.
    AgentMessage,
}

pub(crate) struct Prompt {
    pub(crate) data: Option<PromptData>,
    pub(crate) state: TextInput,
    history_dir: Option<PathBuf>,
    /// Whether history is read but not added to.
    history_read_only: bool,
    active_history: Option<ActiveHistory>,
}

struct ActiveHistory {
    entries: Vec<String>,
    position: usize,
    draft: String,
}

impl Prompt {
    pub(crate) fn new() -> Self {
        Prompt {
            data: None,
            state: TextInput::default(),
            history_dir: None,
            history_read_only: false,
            active_history: None,
        }
    }

    pub(crate) fn with_history(history_dir: PathBuf, read_only: bool) -> Self {
        Self {
            history_dir: Some(history_dir),
            history_read_only: read_only,
            ..Self::new()
        }
    }

    pub(crate) fn set(&mut self, data: PromptData) {
        self.data = Some(data);
        self.state.focused = true;
    }

    pub(crate) fn reset(&mut self) {
        self.data = None;
        // What was killed can be put back at the next prompt.
        let killed = std::mem::take(&mut self.state.killed);
        self.state = TextInput::default();
        self.state.killed = killed;
        self.active_history = None;
    }

    pub(crate) fn start_history(&mut self, kind: Option<&HistoryKind>) -> Res<()> {
        let (Some(kind), Some(dir)) = (kind, &self.history_dir) else {
            return Ok(());
        };
        let draft = self.state.value.as_str().to_owned();
        let entries = read_history(dir, kind)?
            .into_iter()
            .filter(|entry| entry != &draft)
            .collect::<Vec<_>>();
        self.active_history = Some(ActiveHistory {
            position: entries.len(),
            entries,
            draft,
        });
        Ok(())
    }

    pub(crate) fn remember(&self, kind: &HistoryKind, value: &str) -> Res<()> {
        let Some(dir) = &self.history_dir else {
            return Ok(());
        };
        if self.history_read_only || value.is_empty() {
            return Ok(());
        }

        let path = history_path(dir, kind);
        fs::create_dir_all(path.parent().unwrap()).map_err(Error::WritePromptHistory)?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(Error::WritePromptHistory)?;
        file.write_all(format!("{value}\n").as_bytes())
            .map_err(Error::WritePromptHistory)
    }

    /// Up and down step through the prompt's history; the line is edited with readline's keys
    /// (see [`TextInput::handle_key_event`]).
    pub(crate) fn handle_key(&mut self, key: KeyEvent) {
        if key.kind == KeyEventKind::Release {
            return;
        }
        match (key.code, key.modifiers) {
            (KeyCode::Up, KeyModifiers::NONE) => self.history_previous(),
            (KeyCode::Down, KeyModifiers::NONE) => self.history_next(),
            _ => self.state.handle_key_event(key),
        }
    }

    fn history_previous(&mut self) {
        let Some(history) = &mut self.active_history else {
            return;
        };
        save_history_edit(history, self.state.value.as_str());
        if history.position > 0 {
            history.position -= 1;
        }
        self.show_history_value();
    }

    fn history_next(&mut self) {
        let Some(history) = &mut self.active_history else {
            return;
        };
        save_history_edit(history, self.state.value.as_str());
        if history.position < history.entries.len() {
            history.position += 1;
        }
        self.show_history_value();
    }

    fn show_history_value(&mut self) {
        let Some(history) = &self.active_history else {
            return;
        };
        let value = history
            .entries
            .get(history.position)
            .unwrap_or(&history.draft);
        self.state.value = value.clone();
        self.state.move_to(usize::MAX);
    }
}

fn save_history_edit(history: &mut ActiveHistory, value: &str) {
    if let Some(entry) = history.entries.get_mut(history.position) {
        *entry = value.to_owned();
    } else {
        history.draft = value.to_owned();
    }
}

fn read_history(dir: &Path, kind: &HistoryKind) -> Res<Vec<String>> {
    match fs::read_to_string(history_path(dir, kind)) {
        Ok(contents) => Ok(contents.lines().map(str::to_owned).collect()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(err) => Err(Error::ReadPromptHistory(err)),
    }
}

fn history_path(dir: &Path, kind: &HistoryKind) -> PathBuf {
    match kind {
        HistoryKind::GitCommand => dir.join("git-command-history"),
        HistoryKind::FilePatterns => dir.join("file-pattern-history"),
        HistoryKind::Search => dir.join("search-history"),
        HistoryKind::AgentMessage => dir.join("agent-message-history"),
        // A name is any string, so it is escaped to be one file name.
        HistoryKind::Input(name) => dir
            .join("input-history")
            .join(name.replace('%', "%25").replace('/', "%2F")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::key_parser::parse_test_keys;

    /// The line and cursor after typing `keys`, both written with `|` where
    /// the cursor is.
    fn edited(before: &str, keys: &str) -> String {
        let (value, rest) = before.split_once('|').expect("no cursor in the line");
        let mut prompt = Prompt::new();
        prompt.state.value = format!("{value}{rest}");
        prompt.state.move_to(value.chars().count());

        let ("", keys) = parse_test_keys(keys).unwrap() else {
            unreachable!()
        };
        for (modifiers, code) in keys {
            prompt.handle_key(KeyEvent::new(code, modifiers));
        }

        let chars: Vec<char> = prompt.state.value.chars().collect();
        let cursor = prompt.state.position();
        format!(
            "{}|{}",
            chars[..cursor].iter().collect::<String>(),
            chars[cursor..].iter().collect::<String>()
        )
    }

    /// A word is a run of letters and digits, so a motion stops inside a path
    /// rather than stepping over the whole of it.
    #[test]
    fn a_word_motion_steps_over_letters_and_digits() {
        assert_eq!(edited("git diff main|", "<alt+b>"), "git diff |main");
        assert_eq!(edited("git diff main|", "<alt+b><alt+b>"), "git |diff main");
        assert_eq!(edited("|src/lib.rs", "<alt+f>"), "src|/lib.rs");
        assert_eq!(edited("|src/lib.rs", "<alt+f><alt+f>"), "src/lib|.rs");
        assert_eq!(edited("|a", "<alt+b>"), "|a");
        assert_eq!(edited("a|", "<alt+f>"), "a|");
        assert_eq!(edited("a b|", "<ctrl+left>"), "a |b");
        assert_eq!(edited("|a b", "<ctrl+right>"), "a| b");
    }

    /// `ctrl+w` takes back a whole token, which is what a pathspec is: nobody
    /// wants it in the pieces its punctuation makes.
    #[test]
    fn ctrl_w_takes_back_a_whole_token() {
        assert_eq!(
            edited("git diff -- ':(top,exclude)*_test.go'|", "<ctrl+w>"),
            "git diff -- |"
        );
        assert_eq!(edited("git diff  |", "<ctrl+w>"), "git |");
        assert_eq!(edited("|", "<ctrl+w>"), "|");
    }

    #[test]
    fn a_word_can_be_killed_either_side_of_the_cursor() {
        assert_eq!(edited("src/lib.rs|", "<alt+backspace>"), "src/lib.|");
        assert_eq!(edited("|src/lib.rs", "<alt+d>"), "|/lib.rs");
        assert_eq!(edited("a |b c", "<alt+d>"), "a | c");
    }

    /// What a kill took, `ctrl+y` puts back — which is how a token is moved,
    /// and how a mistaken kill is undone.
    #[test]
    fn what_was_killed_can_be_put_back() {
        assert_eq!(
            edited("git diff main|", "<ctrl+w><ctrl+y>"),
            "git diff main|"
        );
        assert_eq!(
            edited("git diff main|", "<ctrl+w><ctrl+a><ctrl+y>"),
            "main|git diff "
        );
        // A kill of nothing leaves the last kill to be put back.
        assert_eq!(edited("a b|", "<ctrl+w><ctrl+a><ctrl+w><ctrl+y>"), "b|a ");
    }

    #[test]
    fn a_line_can_be_killed_to_the_end_or_entirely() {
        assert_eq!(edited("git |diff main", "<ctrl+k>"), "git |");
        assert_eq!(
            edited("git |diff main", "<ctrl+k><ctrl+y>"),
            "git diff main|"
        );
        assert_eq!(edited("git |diff", "<ctrl+u>"), "|");
        assert_eq!(edited("git |diff", "<ctrl+u><ctrl+y>"), "git diff|");
    }

    #[test]
    fn two_characters_change_places() {
        assert_eq!(edited("ab|cd", "<ctrl+t>"), "acb|d");
        assert_eq!(edited("ab|", "<ctrl+t>"), "ba|");
        assert_eq!(edited("|ab", "<ctrl+t>"), "ba|");
        assert_eq!(edited("a|", "<ctrl+t>"), "a|");
    }

    /// The keys `TextInput` binds still do what they did.
    #[test]
    fn the_single_character_keys_are_untouched() {
        assert_eq!(edited("git dif|", "f"), "git diff|");
        assert_eq!(edited("git diff|", "<backspace>"), "git dif|");
        assert_eq!(edited("git |diff", "<ctrl+e>"), "git diff|");
        assert_eq!(edited("git diff|", "<ctrl+a>"), "|git diff");
        assert_eq!(edited("git diff|", "<left>"), "git dif|f");
    }

    /// Characters are characters, not bytes: a motion over them lands between
    /// them.
    #[test]
    fn a_multibyte_line_edits_by_character() {
        assert_eq!(edited("héllo wörld|", "<ctrl+w>"), "héllo |");
        assert_eq!(edited("héllo wörld|", "<alt+b>"), "héllo |wörld");
        assert_eq!(edited("héllo|", "<backspace>"), "héll|");
    }
}
