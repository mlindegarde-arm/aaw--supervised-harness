#![cfg(unix)]

use std::collections::BTreeMap;
use std::ffi::{c_char, c_int, c_void};
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{FromRawFd, RawFd};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PtySize {
    pub rows: u16,
    pub cols: u16,
}

impl PtySize {
    pub const fn new(cols: u16, rows: u16) -> Self {
        Self { rows, cols }
    }
}

#[derive(Debug)]
pub struct PtyHarness {
    bin: PathBuf,
    current_dir: Option<PathBuf>,
    env: BTreeMap<String, String>,
    size: PtySize,
}

#[derive(Debug)]
pub struct PtyProcess {
    child: Child,
    writer: File,
    output: Vec<u8>,
    screen: VirtualScreen,
    chunks: Receiver<io::Result<Vec<u8>>>,
    reader: Option<JoinHandle<()>>,
}

#[derive(Debug)]
pub struct PtyExit {
    pub status: ExitStatus,
    pub output: String,
    pub screen: ScreenSnapshot,
}

impl PtyHarness {
    pub fn new(bin: impl Into<PathBuf>) -> Self {
        Self {
            bin: bin.into(),
            current_dir: None,
            env: BTreeMap::new(),
            size: PtySize::new(100, 30),
        }
    }

    pub fn current_dir(mut self, current_dir: impl Into<PathBuf>) -> Self {
        self.current_dir = Some(current_dir.into());
        self
    }

    pub fn size(mut self, size: PtySize) -> Self {
        self.size = size;
        self
    }

    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }

    pub fn spawn<I, S>(&self, args: I) -> io::Result<PtyProcess>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let (master, slave) = open_pty(self.size)?;
        let slave = unsafe { File::from_raw_fd(slave) };
        let mut command = Command::new(&self.bin);
        command.args(args.into_iter().map(|arg| arg.as_ref().to_string()));
        command.env_clear();
        command.envs(base_env(self.size));
        command.envs(self.env.clone());
        if let Some(current_dir) = &self.current_dir {
            command.current_dir(current_dir);
        }

        command.stdin(Stdio::from(slave.try_clone()?));
        command.stdout(Stdio::from(slave.try_clone()?));
        command.stderr(Stdio::from(slave));

        let child = command.spawn()?;
        let writer = unsafe { File::from_raw_fd(master) };
        let mut reader = writer.try_clone()?;
        let (sender, chunks) = mpsc::channel();
        let reader_handle = thread::spawn(move || {
            let mut buffer = [0_u8; 8192];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(read) => {
                        if sender.send(Ok(buffer[..read].to_vec())).is_err() {
                            break;
                        }
                    }
                    Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                    Err(err) => {
                        let _ = sender.send(Err(err));
                        break;
                    }
                }
            }
        });

        Ok(PtyProcess {
            child,
            writer,
            output: Vec::new(),
            screen: VirtualScreen::new(self.size.cols as usize, self.size.rows as usize),
            chunks,
            reader: Some(reader_handle),
        })
    }
}

impl PtyProcess {
    pub fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.writer.write_all(bytes)?;
        self.writer.flush()
    }

    pub fn type_text(&mut self, text: &str) -> io::Result<()> {
        self.write(text.as_bytes())
    }

    pub fn press_enter(&mut self) -> io::Result<()> {
        self.write(b"\r")
    }

    pub fn press_tab(&mut self) -> io::Result<()> {
        self.write(b"\t")
    }

    pub fn press_down(&mut self) -> io::Result<()> {
        self.write(b"\x1b[B")
    }

    pub fn press_page_up(&mut self) -> io::Result<()> {
        self.write(b"\x1b[5~")
    }

    pub fn press_page_down(&mut self) -> io::Result<()> {
        self.write(b"\x1b[6~")
    }

    pub fn press_escape(&mut self) -> io::Result<()> {
        self.write(b"\x1b")
    }

    pub fn press_ctrl_c(&mut self) -> io::Result<()> {
        self.write(&[0x03])
    }

    pub fn press_ctrl_d(&mut self) -> io::Result<()> {
        self.write(&[0x04])
    }

    pub fn press_ctrl_n(&mut self) -> io::Result<()> {
        self.write(&[0x0e])
    }

    pub fn press_ctrl_p(&mut self) -> io::Result<()> {
        self.write(&[0x10])
    }

    pub fn press_ctrl_u(&mut self) -> io::Result<()> {
        self.write(&[0x15])
    }

    pub fn poll(&mut self) -> io::Result<ScreenSnapshot> {
        self.drain_chunks()?;
        Ok(self.screen.snapshot())
    }

    pub fn wait_for_text(&mut self, needle: &str, timeout: Duration) -> io::Result<ScreenSnapshot> {
        let deadline = Instant::now() + timeout;
        let mut last = self.poll()?;
        while Instant::now() < deadline {
            if last.text.contains(needle) {
                return Ok(last);
            }
            thread::sleep(Duration::from_millis(10));
            last = self.poll()?;
        }
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!("timed out waiting for {needle:?}\n{}", last.text),
        ))
    }

    pub fn wait_for_absence(
        &mut self,
        needle: &str,
        timeout: Duration,
    ) -> io::Result<ScreenSnapshot> {
        let deadline = Instant::now() + timeout;
        let mut last = self.poll()?;
        while Instant::now() < deadline {
            if !last.text.contains(needle) {
                return Ok(last);
            }
            thread::sleep(Duration::from_millis(10));
            last = self.poll()?;
        }
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!("timed out waiting for absence of {needle:?}\n{}", last.text),
        ))
    }

    pub fn wait_for_exit(&mut self, timeout: Duration) -> io::Result<PtyExit> {
        let deadline = Instant::now() + timeout;
        loop {
            self.drain_chunks()?;
            if let Some(status) = self.child.try_wait()? {
                self.drain_until_reader_done()?;
                return Ok(PtyExit {
                    status,
                    output: String::from_utf8_lossy(&self.output).into_owned(),
                    screen: self.screen.snapshot(),
                });
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!(
                        "timed out waiting for process exit\n{}",
                        self.screen.snapshot().text
                    ),
                ));
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn drain_chunks(&mut self) -> io::Result<()> {
        loop {
            match self.chunks.try_recv() {
                Ok(Ok(chunk)) => {
                    self.screen.process(&chunk);
                    self.output.extend_from_slice(&chunk);
                }
                Ok(Err(err)) => return Err(err),
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => return Ok(()),
            }
        }
    }

    fn drain_until_reader_done(&mut self) -> io::Result<()> {
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        self.drain_chunks()
    }
}

impl Drop for PtyProcess {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenSnapshot {
    pub lines: Vec<String>,
    pub text: String,
}

impl ScreenSnapshot {
    pub fn nonblank_lines(&self) -> Vec<&str> {
        self.lines
            .iter()
            .map(String::as_str)
            .filter(|line| !line.trim().is_empty())
            .collect()
    }
}

#[derive(Debug)]
struct VirtualScreen {
    width: usize,
    height: usize,
    cells: Vec<Vec<char>>,
    row: usize,
    col: usize,
    parser: ParserState,
}

#[derive(Debug)]
enum ParserState {
    Ground,
    Escape,
    Csi { private: bool, bytes: String },
}

impl VirtualScreen {
    fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            cells: vec![vec![' '; width]; height],
            row: 0,
            col: 0,
            parser: ParserState::Ground,
        }
    }

    fn snapshot(&self) -> ScreenSnapshot {
        let lines = self
            .cells
            .iter()
            .map(|line| line.iter().collect::<String>().trim_end().to_string())
            .collect::<Vec<_>>();
        let text = lines.join("\n");
        ScreenSnapshot { lines, text }
    }

    fn process(&mut self, bytes: &[u8]) {
        for ch in String::from_utf8_lossy(bytes).chars() {
            self.process_char(ch);
        }
    }

    fn process_char(&mut self, ch: char) {
        let state = std::mem::replace(&mut self.parser, ParserState::Ground);
        match state {
            ParserState::Ground => self.ground(ch),
            ParserState::Escape => match ch {
                '[' => {
                    self.parser = ParserState::Csi {
                        private: false,
                        bytes: String::new(),
                    };
                }
                'c' => self.clear_screen(),
                _ => self.parser = ParserState::Ground,
            },
            ParserState::Csi {
                mut private,
                mut bytes,
            } => {
                if ch == '?' {
                    private = true;
                    self.parser = ParserState::Csi { private, bytes };
                } else if ch.is_ascii_digit() || ch == ';' {
                    bytes.push(ch);
                    self.parser = ParserState::Csi { private, bytes };
                } else {
                    self.apply_csi(private, &bytes, ch);
                }
            }
        }
    }

    fn ground(&mut self, ch: char) {
        match ch {
            '\x1b' => self.parser = ParserState::Escape,
            '\r' => self.col = 0,
            '\n' => self.line_feed(),
            '\x08' => self.col = self.col.saturating_sub(1),
            ch if ch.is_control() => {}
            ch => self.put_char(ch),
        }
    }

    fn put_char(&mut self, ch: char) {
        if self.row >= self.height {
            self.scroll_up();
            self.row = self.height.saturating_sub(1);
        }
        if self.col >= self.width {
            self.col = 0;
            self.line_feed();
        }
        if self.row < self.height && self.col < self.width {
            self.cells[self.row][self.col] = ch;
        }
        self.col += 1;
    }

    fn line_feed(&mut self) {
        if self.row + 1 >= self.height {
            self.scroll_up();
        } else {
            self.row += 1;
        }
    }

    fn scroll_up(&mut self) {
        if !self.cells.is_empty() {
            self.cells.remove(0);
            self.cells.push(vec![' '; self.width]);
        }
    }

    fn apply_csi(&mut self, private: bool, bytes: &str, final_byte: char) {
        let params = parse_params(bytes);
        match final_byte {
            'H' | 'f' => {
                let row = params.first().copied().unwrap_or(1).saturating_sub(1);
                let col = params.get(1).copied().unwrap_or(1).saturating_sub(1);
                self.row = row.min(self.height.saturating_sub(1));
                self.col = col.min(self.width.saturating_sub(1));
            }
            'A' => {
                self.row = self
                    .row
                    .saturating_sub(params.first().copied().unwrap_or(1))
            }
            'B' => {
                self.row = (self.row + params.first().copied().unwrap_or(1)).min(self.height - 1);
            }
            'C' => self.col = (self.col + params.first().copied().unwrap_or(1)).min(self.width - 1),
            'D' => {
                self.col = self
                    .col
                    .saturating_sub(params.first().copied().unwrap_or(1))
            }
            'J' => self.clear_display(params.first().copied().unwrap_or(0)),
            'K' => self.clear_line(params.first().copied().unwrap_or(0)),
            'h' if private && params.contains(&1049) => self.clear_screen(),
            'l' if private && params.contains(&1049) => self.clear_screen(),
            'm' | 'h' | 'l' | 's' | 'u' => {}
            _ => {}
        }
    }

    fn clear_display(&mut self, mode: usize) {
        match mode {
            0 => {
                for col in self.col..self.width {
                    self.cells[self.row][col] = ' ';
                }
                for row in (self.row + 1)..self.height {
                    self.cells[row].fill(' ');
                }
            }
            1 => {
                for row in 0..self.row {
                    self.cells[row].fill(' ');
                }
                for col in 0..=self.col.min(self.width.saturating_sub(1)) {
                    self.cells[self.row][col] = ' ';
                }
            }
            2 | 3 => self.clear_screen(),
            _ => {}
        }
    }

    fn clear_line(&mut self, mode: usize) {
        match mode {
            0 => {
                for col in self.col..self.width {
                    self.cells[self.row][col] = ' ';
                }
            }
            1 => {
                for col in 0..=self.col.min(self.width.saturating_sub(1)) {
                    self.cells[self.row][col] = ' ';
                }
            }
            2 => self.cells[self.row].fill(' '),
            _ => {}
        }
    }

    fn clear_screen(&mut self) {
        for row in &mut self.cells {
            row.fill(' ');
        }
        self.row = 0;
        self.col = 0;
    }
}

fn parse_params(bytes: &str) -> Vec<usize> {
    if bytes.is_empty() {
        return Vec::new();
    }
    bytes
        .split(';')
        .map(|part| part.parse::<usize>().unwrap_or(0))
        .collect()
}

pub fn assert_terminal_cleanup(exit: &PtyExit) {
    assert!(exit.status.success(), "{:?}\n{}", exit.status, exit.output);
    assert!(
        exit.output.contains("\u{1b}[?1049l"),
        "TUI did not leave the alternate screen\n{}",
        exit.output.escape_debug()
    );
    assert!(
        exit.output.contains("\u{1b}[?25h"),
        "TUI did not restore cursor visibility\n{}",
        exit.output.escape_debug()
    );
}

fn base_env(size: PtySize) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    env.insert(
        "PATH".to_string(),
        std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".to_string()),
    );
    env.insert(
        "HOME".to_string(),
        std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string()),
    );
    env.insert("TERM".to_string(), "xterm-256color".to_string());
    env.insert("COLUMNS".to_string(), size.cols.to_string());
    env.insert("LINES".to_string(), size.rows.to_string());
    env
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct Winsize {
    ws_row: u16,
    ws_col: u16,
    ws_xpixel: u16,
    ws_ypixel: u16,
}

#[cfg_attr(target_os = "linux", link(name = "util"))]
unsafe extern "C" {
    fn openpty(
        amaster: *mut c_int,
        aslave: *mut c_int,
        name: *mut c_char,
        termp: *const c_void,
        winp: *const Winsize,
    ) -> c_int;
}

fn open_pty(size: PtySize) -> io::Result<(RawFd, RawFd)> {
    let mut master = 0;
    let mut slave = 0;
    let winsize = Winsize {
        ws_row: size.rows,
        ws_col: size.cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let result = unsafe {
        openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null(),
            &winsize,
        )
    };
    if result == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok((master, slave))
    }
}

#[allow(dead_code)]
pub fn fixture_tui(bin: impl Into<PathBuf>, fixture: &Path, size: PtySize) -> PtyHarness {
    PtyHarness::new(bin).current_dir(fixture).size(size)
}
