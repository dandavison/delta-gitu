use crate::style::{Color, Modifier, Style};
use crate::{Res, config::Config, error::Error};
use crossterm::{
    QueueableCommand,
    cursor::{self, MoveTo},
    event::{
        DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event,
    },
    style::{Attribute, Colors, Print, ResetColor, SetAttribute, SetColors},
    terminal::{
        Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode,
        enable_raw_mode,
    },
};
use std::io::{self, Stdout, Write, stdout};
use std::process::Stdio;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

pub type Term = TermBackend;

/// How many times gitu has drawn a frame on the terminal or given it to another program, such as
/// an editor. The busy spinner, which draws from a thread of its own, stops once this changes,
/// and it is held while gitu draws, so that the spinner never draws in the middle of a frame. A
/// test terminal counts its own (see [`TermBackend::turns`]).
static TERMINAL_TURNS: LazyLock<Arc<Mutex<u64>>> = LazyLock::new(|| Arc::new(Mutex::new(0)));

/// `turns`, a terminal's count of [`TERMINAL_TURNS`], counted and held while gitu uses the
/// terminal.
pub(crate) fn take_turn(turns: &Mutex<u64>) -> MutexGuard<'_, u64> {
    let mut turns = turns.lock().unwrap_or_else(PoisonError::into_inner);
    *turns += 1;
    turns
}

/// How long gitu can be busy before it says what with.
const BUSY_DELAY: Duration = Duration::from_millis(200);

/// A spinner saying what gitu is busy with, drawn from a thread of its own, since gitu's own
/// thread is busy and can't draw: on the terminal's last row, from [`BUSY_DELAY`] on, until gitu
/// has finished, or uses the terminal itself, by drawing a frame or giving it to another program.
pub(crate) struct Busy {
    /// Dropped when gitu has finished, which wakes the thread at once.
    finished: Option<Sender<()>>,
    thread: Option<JoinHandle<bool>>,
}

impl Busy {
    /// Stop the spinner, returning whether it said anything, so that the screen can be drawn
    /// again over it.
    pub(crate) fn finish(mut self) -> bool {
        self.stop()
    }

    fn stop(&mut self) -> bool {
        self.finished.take();
        self.thread
            .take()
            .is_some_and(|thread| thread.join().unwrap_or(false))
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Whether `duration` passes before gitu has finished, as `finished` says.
fn waited(finished: &Receiver<()>, duration: Duration) -> bool {
    matches!(
        finished.recv_timeout(duration),
        Err(RecvTimeoutError::Timeout)
    )
}

/// Draw a spinner and `what` on the terminal's last row, in `style`, from [`BUSY_DELAY`] on, until
/// `finished` is set or gitu uses the terminal itself, as [`TERMINAL_TURNS`] counts. Returns
/// whether it drew anything.
fn spin(
    finished: &Receiver<()>,
    turns: &Mutex<u64>,
    turns_at_start: u64,
    what: &str,
    style: Style,
) -> bool {
    let mut drew = false;
    let mut wait = BUSY_DELAY;
    while waited(finished, wait) {
        let turns = turns.lock().unwrap_or_else(PoisonError::into_inner);
        if *turns != turns_at_start {
            break;
        }
        let Ok((columns, rows)) = crossterm::terminal::size() else {
            break;
        };
        // Within the row, which a longer line would wrap and scroll.
        let text: String = format!("{} {what}", crate::ui::cmd_log::spinner())
            .chars()
            .take(columns.saturating_sub(1) as usize)
            .collect();
        // Save the cursor, draw on the last row, and put the cursor back.
        let row = format!("\x1b7\x1b[{rows};1H\x1b[2K{}\x1b8", styled(&text, &style));
        let mut out = stdout();
        if out
            .write_all(row.as_bytes())
            .and_then(|()| out.flush())
            .is_err()
        {
            break;
        }
        drop(turns);
        drew = true;
        wait = crate::ui::cmd_log::SPINNER_STEP;
    }
    drew
}

pub fn backend() -> TermBackend {
    TermBackend::Crossterm(stdout())
}

pub enum TermBackend {
    Crossterm(Stdout),
    #[allow(dead_code)]
    Test {
        buffer: TestBuffer,
        events: Vec<Event>,
    },
}

#[derive(Clone, PartialEq)]
pub struct TestCell {
    pub symbol: String,
    /// The OSC-8 link the cell was printed inside, if any.
    pub link: Option<String>,
    pub fg: Color,
    pub bg: Color,
    pub modifier: Modifier,
}

impl Default for TestCell {
    fn default() -> Self {
        TestCell {
            symbol: " ".to_string(),
            link: None,
            fg: Color::Reset,
            bg: Color::Reset,
            modifier: Modifier::empty(),
        }
    }
}

impl TestCell {
    fn set(&mut self, symbol: &str, style: &Style) {
        self.symbol.clear();
        self.symbol.push_str(symbol);

        if let Some(fg) = style.fg {
            self.fg = fg;
        }
        if let Some(bg) = style.bg {
            self.bg = bg;
        }
        self.modifier.insert(style.add_modifier);
        self.modifier.remove(style.sub_modifier);
    }
}

pub struct TestBuffer {
    pub cells: Vec<TestCell>,
    pub width: u16,
    pub height: u16,
    cursor: (u16, u16),
    /// The link printing is currently inside, if any.
    link: Option<String>,
    /// The text of each frame flushed, so that a test can see what was drawn while the TUI was
    /// busy, such as waiting for a command.
    pub frames: Vec<String>,
    /// What the TUI said it was busy with, in place of the spinner a terminal shows.
    pub busy: Arc<Mutex<Vec<String>>>,
    /// This terminal's count of [`TERMINAL_TURNS`].
    turns: Arc<Mutex<u64>>,
}

impl TestBuffer {
    pub fn new(width: u16, height: u16) -> Self {
        TestBuffer {
            cells: vec![TestCell::default(); width as usize * height as usize],
            width,
            height,
            cursor: (0, 0),
            link: None,
            frames: vec![],
            busy: Arc::new(Mutex::new(vec![])),
            turns: Arc::new(Mutex::new(0)),
        }
    }

    /// The buffer's text, a line to a row.
    fn text(&self) -> String {
        self.cells
            .chunks(self.width as usize)
            .map(|row| {
                row.iter()
                    .map(|cell| cell.symbol.as_str())
                    .collect::<String>()
                    + "\n"
            })
            .collect()
    }

    fn clear(&mut self) {
        self.cells.fill(TestCell::default());
    }

    fn cell_mut(&mut self, x: u16, y: u16) -> Option<&mut TestCell> {
        if x >= self.width || y >= self.height {
            return None;
        }

        self.cells
            .get_mut(y as usize * self.width as usize + x as usize)
    }

    fn print(&mut self, text: &str, style: &Style) {
        let (mut x, y) = self.cursor;

        for grapheme in text.graphemes(true) {
            let grapheme_width = grapheme.width() as u16;
            if grapheme_width == 0 || x >= self.width {
                continue;
            }

            let link = self.link.clone();
            if let Some(cell) = self.cell_mut(x, y) {
                *cell = TestCell::default();
                cell.set(grapheme, style);
                cell.link = link;
            }
            x += 1;

            for _ in 1..grapheme_width {
                if x >= self.width {
                    break;
                }

                if let Some(cell) = self.cell_mut(x, y) {
                    *cell = TestCell::default();
                }
                x += 1;
            }
        }

        self.cursor = (x, y);
    }
}

impl TermBackend {
    pub(crate) fn queue_move_cursor(&mut self, x: u16, y: u16) -> Res<()> {
        match self {
            TermBackend::Crossterm(t) => crossterm::queue!(t, MoveTo(x, y)).map_err(Error::Term),
            TermBackend::Test { buffer, .. } => {
                buffer.cursor = (x, y);
                Ok(())
            }
        }
    }

    /// Open or close an OSC-8 hyperlink around what is printed next. The
    /// sequence takes up no columns, so the test terminal records only that the
    /// link is open.
    pub(crate) fn queue_link(&mut self, uri: Option<&str>) -> Res<()> {
        match self {
            TermBackend::Crossterm(t) => {
                let uri = uri.unwrap_or("");
                crossterm::queue!(t, Print(format!("\x1b]8;;{uri}\x1b\\"))).map_err(Error::Term)
            }
            TermBackend::Test { buffer, .. } => {
                buffer.link = uri.map(str::to_owned);
                Ok(())
            }
        }
    }

    pub(crate) fn queue_newline(&mut self) -> Res<()> {
        match self {
            TermBackend::Crossterm(t) => {
                crossterm::queue!(t, ResetColor, SetAttribute(Attribute::Reset), Print("\n"))
                    .map_err(Error::Term)
            }
            TermBackend::Test { buffer, .. } => {
                buffer.cursor = (0, buffer.cursor.1 + 1);
                Ok(())
            }
        }
    }

    pub fn queue_print(&mut self, text: &str, style: &Style) -> Res<()> {
        match self {
            TermBackend::Crossterm(t) => {
                print_crossterm_span(text, style, t).map_err(Error::Term)?;

                Ok(())
            }
            TermBackend::Test { buffer, .. } => {
                buffer.print(text, style);
                Ok(())
            }
        }
    }
}

const ATTRS: &[(Modifier, Attribute)] = &[
    (Modifier::BOLD, Attribute::Bold),
    (Modifier::DIM, Attribute::Dim),
    (Modifier::ITALIC, Attribute::Italic),
    (Modifier::UNDERLINED, Attribute::Underlined),
    (Modifier::SLOW_BLINK, Attribute::SlowBlink),
    (Modifier::RAPID_BLINK, Attribute::RapidBlink),
    (Modifier::REVERSED, Attribute::Reverse),
    (Modifier::HIDDEN, Attribute::Hidden),
    (Modifier::CROSSED_OUT, Attribute::CrossedOut),
];

fn print_crossterm_span(text: &str, style: &Style, t: &mut impl Write) -> Result<(), io::Error> {
    let fg = style.fg.unwrap_or(Color::Reset);
    let bg = style.bg.unwrap_or(Color::Reset);

    crossterm::queue!(t, SetAttribute(Attribute::Reset))?;

    for (modifier, attribute) in ATTRS {
        if style.add_modifier.contains(*modifier) {
            crossterm::queue!(t, SetAttribute(*attribute))?;
        }
    }

    crossterm::queue!(t, SetColors(Colors::new(fg.into(), bg.into())))?;
    crossterm::queue!(t, Print(text))?;
    Ok(())
}

/// `text` with the escape sequences that draw it in `style` on a terminal, as the TUI draws it,
/// and then reset.
pub(crate) fn styled(text: &str, style: &Style) -> String {
    let mut out = vec![];
    print_crossterm_span(text, style, &mut out).expect("writing to memory doesn't fail");
    crossterm::queue!(out, SetAttribute(Attribute::Reset), ResetColor)
        .expect("writing to memory doesn't fail");
    String::from_utf8(out).expect("text and escape sequences are UTF-8")
}

impl TermBackend {
    pub(crate) fn size(&self) -> io::Result<(u16, u16)> {
        match self {
            TermBackend::Crossterm(_) => crossterm::terminal::size(),
            TermBackend::Test { buffer, .. } => Ok((buffer.width, buffer.height)),
        }
    }

    pub(crate) fn clear(&mut self) -> io::Result<()> {
        match self {
            TermBackend::Crossterm(t) => crossterm::queue!(t, Clear(ClearType::All)),
            TermBackend::Test { buffer, .. } => {
                buffer.clear();
                Ok(())
            }
        }
    }

    pub(crate) fn flush(&mut self) -> io::Result<()> {
        match self {
            TermBackend::Crossterm(t) => t.flush(),
            TermBackend::Test { buffer, .. } => {
                buffer.frames.push(buffer.text());
                Ok(())
            }
        }
    }
}

impl TermBackend {
    /// Say what gitu is busy with, `what`, in `style`, if it still is after [`BUSY_DELAY`]: on a
    /// terminal with a spinner (see [`Busy`]), and in a test, by noting it.
    pub(crate) fn busy(&self, what: &str, style: Style) -> Busy {
        let (finished, finished_w) = mpsc::channel();
        let what = what.to_owned();
        let turns = self.turns();
        let turns_at_start = *turns.lock().unwrap_or_else(PoisonError::into_inner);
        let thread = match self {
            TermBackend::Crossterm(_) => {
                thread::spawn(move || spin(&finished_w, &turns, turns_at_start, &what, style))
            }
            TermBackend::Test { buffer, .. } => {
                let busy = Arc::clone(&buffer.busy);
                thread::spawn(move || {
                    // As the spinner would: unless gitu has used the terminal since.
                    let said = waited(&finished_w, BUSY_DELAY)
                        && *turns.lock().unwrap_or_else(PoisonError::into_inner) == turns_at_start;
                    if said {
                        busy.lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .push(what);
                    }
                    said
                })
            }
        };
        Busy {
            finished: Some(finished),
            thread: Some(thread),
        }
    }

    pub fn setup_term(&mut self, config: &Config) -> io::Result<()> {
        match self {
            TermBackend::Crossterm(crossterm_backend) => {
                enable_raw_mode()?;
                crossterm_backend.queue(EnterAlternateScreen)?;
                crossterm_backend.queue(cursor::Hide)?;
                if config.general.mouse_support {
                    crossterm_backend.queue(EnableMouseCapture)?;
                }
                // A paste arrives whole rather than as keys, for a comment box to take.
                crossterm_backend.queue(EnableBracketedPaste)?;
            }
            TermBackend::Test { .. } => {}
        }

        self.flush()
    }

    /// This terminal's count of [`TERMINAL_TURNS`].
    pub(crate) fn turns(&self) -> Arc<Mutex<u64>> {
        match self {
            TermBackend::Crossterm(_) => Arc::clone(&TERMINAL_TURNS),
            TermBackend::Test { buffer, .. } => Arc::clone(&buffer.turns),
        }
    }

    pub fn reset_term(&mut self, config: &Config) -> io::Result<()> {
        // The terminal goes to another program, or back to the shell.
        let turns = self.turns();
        let _turn = take_turn(&turns);
        match self {
            TermBackend::Crossterm(crossterm_backend) => {
                if config.general.mouse_support {
                    crossterm_backend.queue(DisableMouseCapture)?;
                }
                crossterm_backend.queue(DisableBracketedPaste)?;
                crossterm_backend.queue(cursor::Show)?;
                crossterm_backend.queue(LeaveAlternateScreen)?;
                disable_raw_mode()?;
            }
            TermBackend::Test { .. } => {}
        }

        self.flush()
    }

    /// Stop gitu as job control stops a program on ctrl+z, giving the
    /// terminal back until the shell continues it, and then take it again. The
    /// test terminal has no job control, and stopping would stop the tests.
    pub(crate) fn suspend(&mut self, config: &Config) -> io::Result<()> {
        if let TermBackend::Test { .. } = self {
            return Ok(());
        }
        self.reset_term(config)?;
        #[cfg(unix)]
        // SAFETY: raising a signal touches no memory of this process.
        unsafe {
            libc::raise(libc::SIGTSTP);
        }
        self.setup_term(config)?;
        self.clear()
    }

    /// The input for a program gitu runs in the foreground, such as the editor:
    /// the terminal gitu reads keys from. As git's pager, gitu's stdin is the
    /// pipe git writes to, which the program must not read. On Windows the
    /// program gets gitu's stdin whatever it is.
    pub(crate) fn foreground_input(&self) -> io::Result<Stdio> {
        match self {
            #[cfg(unix)]
            TermBackend::Crossterm(_) if !io::IsTerminal::is_terminal(&io::stdin()) => {
                let tty = std::fs::File::options()
                    .read(true)
                    .write(true)
                    .open("/dev/tty")?;
                Ok(tty.into())
            }
            _ => Ok(Stdio::inherit()),
        }
    }

    pub(crate) fn reset_term_stay_on_alt_screeen(
        &mut self,
        config: &Config,
    ) -> Result<(), io::Error> {
        // The terminal goes to another program.
        let turns = self.turns();
        let _turn = take_turn(&turns);
        match self {
            TermBackend::Crossterm(crossterm_backend) => {
                if config.general.mouse_support {
                    crossterm_backend.queue(DisableMouseCapture)?;
                }
                crossterm_backend.queue(DisableBracketedPaste)?;
                crossterm_backend.queue(cursor::Show)?;
                disable_raw_mode()?;
            }
            TermBackend::Test { .. } => {}
        }

        self.flush()
    }

    pub fn poll_event(&self, timeout: Duration) -> Res<bool> {
        match self {
            TermBackend::Crossterm(_) => crossterm::event::poll(timeout).map_err(Error::Term),
            TermBackend::Test { events, .. } => {
                if events.is_empty() {
                    Err(Error::NoMoreEvents)
                } else {
                    Ok(true)
                }
            }
        }
    }

    /// Whether an event is waiting to be read, without waiting for one.
    pub fn has_pending_event(&self) -> Res<bool> {
        match self {
            TermBackend::Crossterm(_) => {
                crossterm::event::poll(Duration::ZERO).map_err(Error::Term)
            }
            TermBackend::Test { events, .. } => Ok(!events.is_empty()),
        }
    }

    pub fn hung_up(&self) -> bool {
        match self {
            #[cfg(unix)]
            TermBackend::Crossterm(t) => hung_up(std::os::fd::AsFd::as_fd(t)),
            #[cfg(not(unix))]
            TermBackend::Crossterm(_) => false,
            TermBackend::Test { .. } => false,
        }
    }

    pub fn read_event(&mut self) -> Res<Event> {
        match self {
            TermBackend::Crossterm(_) => crossterm::event::read().map_err(Error::Term),
            TermBackend::Test { events, .. } => events.pop().ok_or(Error::NoMoreEvents),
        }
    }
}

/// Whether the terminal behind `fd` has gone, as when its window is closed.
/// Reading it then gives end of file, which crossterm reports as no input, so
/// gitu must ask. Without this, gitu as git's pager in a closed tmux pane spins
/// forever: git, the pane's session leader, takes the SIGHUP and waits for its
/// pager rather than passing it on.
#[cfg(unix)]
fn hung_up(fd: std::os::fd::BorrowedFd) -> bool {
    use std::os::fd::AsRawFd;
    let mut pollfd = libc::pollfd {
        fd: fd.as_raw_fd(),
        events: libc::POLLOUT,
        revents: 0,
    };
    let ready = unsafe { libc::poll(&mut pollfd, 1, 0) };
    ready == 1 && pollfd.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0
}

#[cfg(all(test, unix))]
mod tests {
    use super::hung_up;
    use std::os::fd::{AsFd, FromRawFd, OwnedFd};
    use std::time::{Duration, Instant};

    #[test]
    fn a_terminal_has_hung_up_once_its_other_end_is_closed() {
        let (mut master, mut slave) = (0, 0);
        let opened = unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        assert_eq!(opened, 0);
        // Other tests spawn processes concurrently. A child inherits the
        // master until it execs, or for its lifetime without close-on-exec,
        // and the slave hangs up only once every copy of the master is closed.
        // A child spawned between openpty and fcntl still keeps the master for
        // its lifetime; the deadline below covers only short-lived children.
        unsafe {
            libc::fcntl(master, libc::F_SETFD, libc::FD_CLOEXEC);
            libc::fcntl(slave, libc::F_SETFD, libc::FD_CLOEXEC);
        }
        let (master, slave) =
            unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };

        assert!(!hung_up(slave.as_fd()));
        drop(master);
        let deadline = Instant::now() + Duration::from_secs(5);
        while !hung_up(slave.as_fd()) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(hung_up(slave.as_fd()));
    }
}
