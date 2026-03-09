#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use rfd::FileDialog;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::collections::VecDeque;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;
use vt100::Parser;

#[cfg(target_os = "macos")]
use objc::runtime::Object;
#[cfg(target_os = "macos")]
use objc::{class, msg_send, sel, sel_impl};

const WINDOW_WIDTH: f32 = 1180.0;
const WINDOW_HEIGHT: f32 = 760.0;
const TOP_BAR_HEIGHT: f32 = 40.0;
const TAB_BAR_HEIGHT: f32 = 38.0;
const MINIMIZED_HEIGHT: f32 = 40.0;
const SIDEBAR_DEFAULT_WIDTH: f32 = 340.0;
const TERMINAL_SCROLLBACK: usize = 5_000;
const PANE_SEPARATOR_WIDTH: f32 = 1.0;

#[derive(Clone, Copy)]
#[allow(dead_code)]
struct ThemePalette {
    bg: egui::Color32,
    bar_bg: egui::Color32,
    border: egui::Color32,
    text: egui::Color32,
    muted_text: egui::Color32,
    selection: egui::Color32,
    terminal_bg: egui::Color32,
    sidebar_bg: egui::Color32,
    sidebar_soft_bg: egui::Color32,
    accent: egui::Color32,
    accent_dim: egui::Color32,
    tab_bg: egui::Color32,
    active_tab_bg: egui::Color32,
    tab_text: egui::Color32,
    active_tab_text: egui::Color32,
    input_bg: egui::Color32,
    surface: egui::Color32,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum ThemePreset {
    Warp,
    WarpLight,
    Terminal,
    Midnight,
}

#[derive(Clone, Copy)]
struct TerminalPoint {
    row: u16,
    col: u16,
}

// ── A single terminal session (pane) ──
struct TerminalPane {
    uid: u64, // unique ID that survives reordering
    cwd: PathBuf,
    parser: Parser,
    rx: Option<Receiver<Vec<u8>>>,
    writer: Option<Box<dyn Write + Send>>,
    master: Option<Box<dyn MasterPty + Send>>,
    child: Option<Box<dyn Child + Send + Sync>>,
    rows: u16,
    cols: u16,
    status: String,
    has_focus: bool,
    selection: Option<(TerminalPoint, TerminalPoint)>,
}

// ── A tab containing one or more panes ──
struct TerminalTab {
    title: String,
    panes: Vec<TerminalPane>,
    active_pane: usize,
}

struct GhostStickiesApp {
    notes_markdown: String,
    notes_root: Option<PathBuf>,
    current_note_file: Option<PathBuf>,
    note_status: String,
    editing_notes: bool,
    theme: ThemePreset,
    minimized: bool,
    sidebar_open: bool,
    privacy_mode: bool,
    startup_tasks_run: bool,
    applied_privacy_mode: Option<bool>,
    next_tab_number: usize,
    next_pane_uid: u64,
    terminal_tabs: Vec<TerminalTab>,
    active_terminal: usize,
    renaming_tab: Option<usize>,
    rename_buffer: String,
    // Debug log
    debug_log: VecDeque<String>,
    show_debug: bool,
}

const DEBUG_LOG_MAX: usize = 200;

#[derive(Serialize, Deserialize, Default)]
struct AppConfig {
    notes_root: Option<PathBuf>,
    current_note_file: Option<PathBuf>,
    theme: ThemePreset,
}

#[derive(Clone, Copy)]
enum AppSymbol {
    Privacy,
}

impl Default for ThemePreset {
    fn default() -> Self {
        Self::Warp
    }
}

impl ThemePreset {
    const ALL: [Self; 4] = [Self::Warp, Self::WarpLight, Self::Terminal, Self::Midnight];

    fn label(self) -> &'static str {
        match self {
            Self::Warp => "Warp Dark",
            Self::WarpLight => "Warp Blue",
            Self::Terminal => "Terminal",
            Self::Midnight => "Midnight",
        }
    }

    fn palette(self) -> ThemePalette {
        match self {
            Self::Warp => ThemePalette {
                bg: egui::Color32::from_rgb(20, 20, 28),
                terminal_bg: egui::Color32::from_rgb(17, 17, 24),
                sidebar_bg: egui::Color32::from_rgb(24, 24, 33),
                sidebar_soft_bg: egui::Color32::from_rgb(30, 30, 40),
                bar_bg: egui::Color32::from_rgb(26, 26, 36),
                border: egui::Color32::from_rgba_premultiplied(255, 255, 255, 8),
                text: egui::Color32::from_rgb(229, 229, 236),
                muted_text: egui::Color32::from_rgb(110, 110, 128),
                selection: egui::Color32::from_rgba_premultiplied(60, 80, 140, 140),
                accent: egui::Color32::from_rgb(82, 182, 154),
                accent_dim: egui::Color32::from_rgb(55, 120, 100),
                tab_bg: egui::Color32::from_rgb(32, 32, 44),
                active_tab_bg: egui::Color32::from_rgb(48, 48, 64),
                tab_text: egui::Color32::from_rgb(140, 140, 158),
                active_tab_text: egui::Color32::from_rgb(229, 229, 236),
                input_bg: egui::Color32::from_rgb(28, 28, 38),
                surface: egui::Color32::from_rgb(34, 34, 46),
            },
            Self::WarpLight => ThemePalette {
                bg: egui::Color32::from_rgb(14, 24, 42),
                terminal_bg: egui::Color32::from_rgb(12, 20, 36),
                sidebar_bg: egui::Color32::from_rgb(18, 30, 52),
                sidebar_soft_bg: egui::Color32::from_rgb(24, 38, 64),
                bar_bg: egui::Color32::from_rgb(20, 32, 54),
                border: egui::Color32::from_rgba_premultiplied(120, 175, 255, 18),
                text: egui::Color32::from_rgb(215, 234, 255),
                muted_text: egui::Color32::from_rgb(100, 140, 190),
                selection: egui::Color32::from_rgba_premultiplied(66, 110, 185, 148),
                accent: egui::Color32::from_rgb(100, 200, 255),
                accent_dim: egui::Color32::from_rgb(60, 130, 180),
                tab_bg: egui::Color32::from_rgb(24, 38, 60),
                active_tab_bg: egui::Color32::from_rgb(36, 52, 80),
                tab_text: egui::Color32::from_rgb(120, 160, 210),
                active_tab_text: egui::Color32::from_rgb(215, 234, 255),
                input_bg: egui::Color32::from_rgb(16, 26, 46),
                surface: egui::Color32::from_rgb(26, 40, 66),
            },
            Self::Terminal => ThemePalette {
                bg: egui::Color32::from_rgb(9, 13, 10),
                terminal_bg: egui::Color32::from_rgb(10, 13, 11),
                sidebar_bg: egui::Color32::from_rgb(16, 22, 18),
                sidebar_soft_bg: egui::Color32::from_rgb(20, 28, 23),
                bar_bg: egui::Color32::from_rgb(17, 22, 18),
                border: egui::Color32::from_rgba_premultiplied(90, 220, 150, 20),
                text: egui::Color32::from_rgb(168, 255, 196),
                muted_text: egui::Color32::from_rgb(80, 130, 96),
                selection: egui::Color32::from_rgba_premultiplied(44, 104, 70, 150),
                accent: egui::Color32::from_rgb(90, 220, 150),
                accent_dim: egui::Color32::from_rgb(50, 140, 90),
                tab_bg: egui::Color32::from_rgb(14, 20, 16),
                active_tab_bg: egui::Color32::from_rgb(24, 36, 28),
                tab_text: egui::Color32::from_rgb(80, 130, 96),
                active_tab_text: egui::Color32::from_rgb(168, 255, 196),
                input_bg: egui::Color32::from_rgb(12, 16, 13),
                surface: egui::Color32::from_rgb(22, 30, 24),
            },
            Self::Midnight => ThemePalette {
                bg: egui::Color32::from_rgb(12, 12, 16),
                terminal_bg: egui::Color32::from_rgb(8, 8, 12),
                sidebar_bg: egui::Color32::from_rgb(16, 16, 22),
                sidebar_soft_bg: egui::Color32::from_rgb(22, 22, 30),
                bar_bg: egui::Color32::from_rgb(18, 18, 24),
                border: egui::Color32::from_rgba_premultiplied(255, 255, 255, 6),
                text: egui::Color32::from_rgb(220, 220, 228),
                muted_text: egui::Color32::from_rgb(90, 90, 108),
                selection: egui::Color32::from_rgba_premultiplied(80, 60, 140, 140),
                accent: egui::Color32::from_rgb(200, 140, 255),
                accent_dim: egui::Color32::from_rgb(130, 90, 180),
                tab_bg: egui::Color32::from_rgb(20, 20, 28),
                active_tab_bg: egui::Color32::from_rgb(34, 34, 46),
                tab_text: egui::Color32::from_rgb(100, 100, 118),
                active_tab_text: egui::Color32::from_rgb(220, 220, 228),
                input_bg: egui::Color32::from_rgb(14, 14, 20),
                surface: egui::Color32::from_rgb(24, 24, 34),
            },
        }
    }
}

// ── TerminalPane implementation ──

impl TerminalPane {
    fn new(uid: u64, cwd: PathBuf) -> Self {
        let rows = 28;
        let cols = 120;

        Self {
            uid,
            cwd,
            parser: Parser::new(rows, cols, TERMINAL_SCROLLBACK),
            rx: None,
            writer: None,
            master: None,
            child: None,
            rows,
            cols,
            status: "Starting shell...".to_owned(),
            has_focus: false,
            selection: None,
        }
    }

    fn shell_builder(&self) -> CommandBuilder {
        #[cfg(target_os = "windows")]
        {
            let mut command = CommandBuilder::new("cmd.exe");
            command.cwd(self.cwd.as_os_str());
            command
        }

        #[cfg(not(target_os = "windows"))]
        {
            let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_owned());
            let mut command = CommandBuilder::new(shell);
            command.arg("-il");
            command.cwd(self.cwd.as_os_str());
            command.env("TERM", "xterm-256color");
            command.env("COLORTERM", "truecolor");
            command
        }
    }

    fn ensure_started(&mut self) {
        if self.rx.is_some() {
            return;
        }

        let pty_system = native_pty_system();
        let pair = match pty_system.openpty(PtySize {
            rows: self.rows,
            cols: self.cols,
            pixel_width: 0,
            pixel_height: 0,
        }) {
            Ok(pair) => pair,
            Err(err) => {
                self.status = format!("Could not open terminal: {err}");
                return;
            }
        };

        let command = self.shell_builder();
        let child = match pair.slave.spawn_command(command) {
            Ok(child) => child,
            Err(err) => {
                self.status = format!("Could not start shell: {err}");
                return;
            }
        };

        let writer = match pair.master.take_writer() {
            Ok(writer) => writer,
            Err(err) => {
                self.status = format!("Could not connect terminal input: {err}");
                return;
            }
        };

        let mut reader = match pair.master.try_clone_reader() {
            Ok(reader) => reader,
            Err(err) => {
                self.status = format!("Could not connect terminal output: {err}");
                return;
            }
        };

        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut buffer = [0_u8; 8192];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(count) => {
                        if tx.send(buffer[..count].to_vec()).is_err() {
                            break;
                        }
                    }
                    Err(err) => {
                        let message = format!("\r\n[terminal read error: {err}]\r\n");
                        let _ = tx.send(message.into_bytes());
                        break;
                    }
                }
            }
        });

        self.writer = Some(writer);
        self.master = Some(pair.master);
        self.child = Some(child);
        self.rx = Some(rx);
        self.status = format!("Interactive shell in {}", self.cwd.display());
    }

    fn restart(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
        }

        self.rx = None;
        self.writer = None;
        self.master = None;
        self.child = None;
        self.selection = None;
        self.parser = Parser::new(self.rows, self.cols, TERMINAL_SCROLLBACK);
        self.status = format!("Restarting shell in {}", self.cwd.display());
        self.ensure_started();
    }

    fn set_scrollback(&mut self, rows: usize) {
        self.parser.screen_mut().set_scrollback(rows);
    }

    fn adjust_scrollback(&mut self, delta_rows: i32) {
        let current = self.parser.screen().scrollback() as i32;
        let next = (current + delta_rows).max(0) as usize;
        self.set_scrollback(next);
    }

    fn drain_output(&mut self) -> bool {
        let mut received_output = false;

        if let Some(rx) = &self.rx {
            while let Ok(bytes) = rx.try_recv() {
                self.parser.process(&bytes);
                received_output = true;
            }
        }

        if let Some(child) = self.child.as_mut() {
            if let Ok(Some(status)) = child.try_wait() {
                self.status = format!("Shell exited: {status:?}");
            }
        }

        received_output
    }

    fn resize(&mut self, rows: u16, cols: u16) {
        let rows = rows.max(6);
        let cols = cols.max(20);

        if rows == self.rows && cols == self.cols {
            return;
        }

        self.rows = rows;
        self.cols = cols;
        self.parser.screen_mut().set_size(rows, cols);

        if let Some(master) = self.master.as_ref() {
            let _ = master.resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            });
        }
    }

    fn write_bytes(&mut self, bytes: &[u8]) {
        if let Some(writer) = self.writer.as_mut() {
            let _ = writer.write_all(bytes);
            let _ = writer.flush();
        }
    }

    fn send_text(&mut self, text: &str) {
        self.selection = None;
        self.set_scrollback(0);
        self.write_bytes(text.as_bytes());
    }

    fn point_is_before(a: TerminalPoint, b: TerminalPoint) -> bool {
        a.row < b.row || (a.row == b.row && a.col <= b.col)
    }

    fn normalized_selection(&self) -> Option<(TerminalPoint, TerminalPoint)> {
        let (anchor, focus) = self.selection?;
        if Self::point_is_before(anchor, focus) {
            Some((anchor, focus))
        } else {
            Some((focus, anchor))
        }
    }

    fn selection_exists(&self) -> bool {
        matches!(
            self.normalized_selection(),
            Some((start, end)) if start.row != end.row || start.col != end.col
        )
    }

    fn select_all(&mut self) {
        self.selection = Some((
            TerminalPoint { row: 0, col: 0 },
            TerminalPoint {
                row: self.rows.saturating_sub(1),
                col: self.cols.saturating_sub(1),
            },
        ));
    }

    fn selected_text(&self) -> Option<String> {
        let (start, end) = self.normalized_selection()?;
        let end_col = end.col.saturating_add(1).min(self.cols);
        let text = self
            .parser
            .screen()
            .contents_between(start.row, start.col, end.row, end_col);

        if text.is_empty() {
            None
        } else {
            Some(text)
        }
    }

    fn copy_selection(&self, ctx: &egui::Context) -> bool {
        if let Some(text) = self.selected_text() {
            ctx.copy_text(text);
            return true;
        }

        false
    }

    fn cell_from_pos(
        &self,
        rect: egui::Rect,
        pointer_pos: egui::Pos2,
        char_width: f32,
        row_height: f32,
        padding: f32,
    ) -> TerminalPoint {
        let x = (pointer_pos.x - rect.left() - padding).max(0.0);
        let y = (pointer_pos.y - rect.top() - padding).max(0.0);

        let col = (x / char_width).floor() as u16;
        let row = (y / row_height).floor() as u16;

        TerminalPoint {
            row: row.min(self.rows.saturating_sub(1)),
            col: col.min(self.cols.saturating_sub(1)),
        }
    }

    fn cell_selected(&self, row: u16, col: u16) -> bool {
        let Some((start, end)) = self.normalized_selection() else {
            return false;
        };

        if row < start.row || row > end.row {
            return false;
        }

        if start.row == end.row {
            return row == start.row && col >= start.col && col <= end.col;
        }

        if row == start.row {
            return col >= start.col;
        }

        if row == end.row {
            return col <= end.col;
        }

        true
    }

    fn handle_input(&mut self, ctx: &egui::Context) {
        if !self.has_focus {
            return;
        }

        let events = ctx.input(|input| input.events.clone());
        for event in events {
            match event {
                egui::Event::Text(text) => {
                    if !text.chars().all(char::is_control) {
                        self.selection = None;
                        self.set_scrollback(0);
                        self.write_bytes(text.as_bytes());
                    }
                }
                egui::Event::Paste(text) => {
                    self.selection = None;
                    self.set_scrollback(0);
                    if self.parser.screen().bracketed_paste() {
                        let mut bytes = b"\x1b[200~".to_vec();
                        bytes.extend_from_slice(text.as_bytes());
                        bytes.extend_from_slice(b"\x1b[201~");
                        self.write_bytes(&bytes);
                    } else {
                        self.write_bytes(text.as_bytes());
                    }
                }
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => {
                    if modifiers.command {
                        match key {
                            egui::Key::C => {
                                if self.copy_selection(ctx) {
                                    continue;
                                }
                            }
                            egui::Key::A => {
                                self.select_all();
                                continue;
                            }
                            egui::Key::Backspace | egui::Key::ArrowLeft | egui::Key::ArrowRight => {
                            }
                            _ => continue,
                        }
                    }

                    if let Some(bytes) = self.key_bytes(key, modifiers) {
                        self.selection = None;
                        self.set_scrollback(0);
                        self.write_bytes(&bytes);
                    }
                }
                _ => {}
            }
        }
    }

    fn key_bytes(&self, key: egui::Key, modifiers: egui::Modifiers) -> Option<Vec<u8>> {
        let application_cursor = self.parser.screen().application_cursor();

        if modifiers.command {
            let command_bytes = match key {
                egui::Key::Backspace => Some(vec![0x15]),
                egui::Key::ArrowLeft => Some(vec![0x01]),
                egui::Key::ArrowRight => Some(vec![0x05]),
                _ => None,
            };

            if let Some(bytes) = command_bytes {
                return Some(bytes);
            }
        }

        if modifiers.alt {
            let alt_bytes = match key {
                egui::Key::Backspace => Some(b"\x1b\x7f".to_vec()),
                egui::Key::ArrowLeft => Some(b"\x1bb".to_vec()),
                egui::Key::ArrowRight => Some(b"\x1bf".to_vec()),
                _ => None,
            };

            if let Some(bytes) = alt_bytes {
                return Some(bytes);
            }
        }

        if modifiers.ctrl {
            let ctrl_byte = match key {
                egui::Key::A => Some(0x01),
                egui::Key::B => Some(0x02),
                egui::Key::C => Some(0x03),
                egui::Key::D => Some(0x04),
                egui::Key::E => Some(0x05),
                egui::Key::F => Some(0x06),
                egui::Key::H => Some(0x08),
                egui::Key::K => Some(0x0B),
                egui::Key::L => Some(0x0C),
                egui::Key::N => Some(0x0E),
                egui::Key::P => Some(0x10),
                egui::Key::U => Some(0x15),
                egui::Key::W => Some(0x17),
                egui::Key::Z => Some(0x1A),
                _ => None,
            };

            if let Some(byte) = ctrl_byte {
                return Some(vec![byte]);
            }
        }

        let bytes = match key {
            egui::Key::Enter => b"\r".to_vec(),
            egui::Key::Tab => {
                if modifiers.shift {
                    b"\x1b[Z".to_vec()
                } else {
                    b"\t".to_vec()
                }
            }
            egui::Key::Backspace => vec![0x7F],
            egui::Key::Escape => vec![0x1B],
            egui::Key::ArrowUp => {
                if application_cursor {
                    b"\x1bOA".to_vec()
                } else {
                    b"\x1b[A".to_vec()
                }
            }
            egui::Key::ArrowDown => {
                if application_cursor {
                    b"\x1bOB".to_vec()
                } else {
                    b"\x1b[B".to_vec()
                }
            }
            egui::Key::ArrowRight => {
                if application_cursor {
                    b"\x1bOC".to_vec()
                } else {
                    b"\x1b[C".to_vec()
                }
            }
            egui::Key::ArrowLeft => {
                if application_cursor {
                    b"\x1bOD".to_vec()
                } else {
                    b"\x1b[D".to_vec()
                }
            }
            egui::Key::Home => {
                if application_cursor {
                    b"\x1bOH".to_vec()
                } else {
                    b"\x1b[H".to_vec()
                }
            }
            egui::Key::End => {
                if application_cursor {
                    b"\x1bOF".to_vec()
                } else {
                    b"\x1b[F".to_vec()
                }
            }
            egui::Key::Insert => b"\x1b[2~".to_vec(),
            egui::Key::Delete => b"\x1b[3~".to_vec(),
            egui::Key::PageUp => b"\x1b[5~".to_vec(),
            egui::Key::PageDown => b"\x1b[6~".to_vec(),
            _ => return None,
        };

        Some(bytes)
    }
}

impl Drop for TerminalPane {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
        }
    }
}

fn shell_escape_path(path: &Path) -> String {
    let display = path.to_string_lossy();
    let escaped = display.replace('\'', "'\\''");
    format!("'{escaped}'")
}

// ── TerminalTab implementation ──

impl TerminalTab {
    fn new(number: usize, uid: u64, cwd: PathBuf) -> Self {
        Self {
            title: format!("Tab {number}"),
            panes: vec![TerminalPane::new(uid, cwd)],
            active_pane: 0,
        }
    }

    fn active_pane(&self) -> &TerminalPane {
        &self.panes[self.active_pane]
    }

    fn active_pane_mut(&mut self) -> &mut TerminalPane {
        &mut self.panes[self.active_pane]
    }

    fn split_pane(&mut self, uid: u64) {
        let cwd = self.panes[self.active_pane].cwd.clone();
        let insert_at = self.active_pane + 1;
        self.panes.insert(insert_at, TerminalPane::new(uid, cwd));
        self.active_pane = insert_at;
    }

    fn close_active_pane(&mut self) -> bool {
        if self.panes.len() <= 1 {
            return false;
        }
        self.panes.remove(self.active_pane);
        if self.active_pane >= self.panes.len() {
            self.active_pane = self.panes.len() - 1;
        }
        true
    }

    fn focus_next_pane(&mut self) {
        if self.panes.len() > 1 {
            self.active_pane = (self.active_pane + 1) % self.panes.len();
        }
    }

    fn focus_prev_pane(&mut self) {
        if self.panes.len() > 1 {
            self.active_pane = if self.active_pane == 0 {
                self.panes.len() - 1
            } else {
                self.active_pane - 1
            };
        }
    }

    fn ensure_all_started(&mut self) {
        for pane in &mut self.panes {
            pane.ensure_started();
        }
    }

    fn drain_all_output(&mut self) -> bool {
        let mut any = false;
        for pane in &mut self.panes {
            if pane.drain_output() {
                any = true;
            }
        }
        any
    }
}

fn default_terminal_cwd() -> PathBuf {
    let home_dir = std::env::var_os("HOME").map(PathBuf::from);

    let current_dir = std::env::current_dir().ok();
    if let Some(dir) = current_dir {
        let launched_from_bundle = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|parent| parent == dir))
            .unwrap_or(false);

        if dir != PathBuf::from("/") && !launched_from_bundle {
            return dir;
        }
    }

    home_dir.unwrap_or_else(|| PathBuf::from("."))
}

fn display_path_short(path: &Path) -> String {
    if let Some(home) = std::env::var_os("HOME") {
        let home_path = PathBuf::from(&home);
        if let Ok(relative) = path.strip_prefix(&home_path) {
            return format!("~/{}", relative.display());
        }
    }
    path.display().to_string()
}

fn git_branch(dir: &Path) -> Option<String> {
    let head_file = dir.join(".git").join("HEAD");
    let contents = fs::read_to_string(head_file).ok()?;
    let trimmed = contents.trim();
    if let Some(branch) = trimmed.strip_prefix("ref: refs/heads/") {
        Some(branch.to_owned())
    } else {
        Some(trimmed[..8.min(trimmed.len())].to_owned())
    }
}

fn git_repo_name(dir: &Path) -> Option<String> {
    let mut current = dir;
    loop {
        if current.join(".git").exists() {
            return current
                .file_name()
                .and_then(|n| n.to_str())
                .map(|s| s.to_owned());
        }
        current = current.parent()?;
    }
}

impl Default for GhostStickiesApp {
    fn default() -> Self {
        let cwd = default_terminal_cwd();

        Self {
            notes_markdown: "# TODO\n- [ ] Keep this shell usable for Codex CLI\n- [ ] Add quick project tabs\n- [ ] Save notes between sessions\n\n## Notes\nWrite markdown on the left.\nUse the right side like a real terminal.".to_owned(),
            notes_root: None,
            current_note_file: None,
            note_status: "Set your notes folder to start saving notes.".to_owned(),
            editing_notes: false,
            theme: ThemePreset::default(),
            minimized: false,
            sidebar_open: false,
            privacy_mode: false,
            startup_tasks_run: false,
            applied_privacy_mode: None,
            next_tab_number: 2,
            next_pane_uid: 2,
            terminal_tabs: vec![TerminalTab::new(1, 1, cwd)],
            active_terminal: 0,
            renaming_tab: None,
            rename_buffer: String::new(),
            debug_log: VecDeque::new(),
            show_debug: false,
        }
    }
}

impl GhostStickiesApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        egui_extras::install_image_loaders(&cc.egui_ctx);
        let fonts = egui::FontDefinitions::default();
        cc.egui_ctx.set_fonts(fonts);

        let mut app = Self::default();
        app.load_saved_config();
        app
    }

    fn symbol_image(symbol: AppSymbol) -> egui::Image<'static> {
        let image = match symbol {
            AppSymbol::Privacy => egui::include_image!("../assets/eye.circle.png"),
        };

        egui::Image::new(image).fit_to_exact_size(egui::vec2(14.0, 14.0))
    }

    fn symbol_button(
        ui: &mut egui::Ui,
        symbol: AppSymbol,
        tooltip: &str,
        selected: bool,
    ) -> egui::Response {
        ui.add(
            egui::Button::image(Self::symbol_image(symbol))
                .selected(selected)
                .frame(true)
                .corner_radius(egui::CornerRadius::same(4))
                .min_size(egui::vec2(22.0, 22.0)),
        )
        .on_hover_text(tooltip)
    }

    fn app_support_dir() -> PathBuf {
        if let Some(home) = std::env::var_os("HOME") {
            PathBuf::from(home)
                .join("Library")
                .join("Application Support")
                .join("StickyTerminal")
        } else {
            PathBuf::from(".stickyterminal")
        }
    }

    fn config_path() -> PathBuf {
        Self::app_support_dir().join("config.json")
    }

    fn load_saved_config(&mut self) {
        let config_path = Self::config_path();
        let Ok(contents) = fs::read_to_string(&config_path) else {
            return;
        };

        let Ok(config) = serde_json::from_str::<AppConfig>(&contents) else {
            self.note_status = "Could not read saved settings. Using defaults.".to_owned();
            return;
        };

        self.theme = config.theme;
        self.current_note_file = config.current_note_file;

        if let Some(root) = config.notes_root {
            self.notes_root = Some(root);
            if self.current_note_file.is_none() {
                self.current_note_file = self.default_note_file();
            }
            self.load_current_note();
        }
    }

    fn save_config(&mut self) {
        let config = AppConfig {
            notes_root: self.notes_root.clone(),
            current_note_file: self.current_note_file.clone(),
            theme: self.theme,
        };

        let support_dir = Self::app_support_dir();
        if let Err(err) = fs::create_dir_all(&support_dir) {
            self.note_status = format!("Could not create app settings folder: {err}");
            return;
        }

        match serde_json::to_string_pretty(&config) {
            Ok(contents) => {
                if let Err(err) = fs::write(Self::config_path(), contents) {
                    self.note_status = format!("Could not save settings: {err}");
                }
            }
            Err(err) => {
                self.note_status = format!("Could not encode settings: {err}");
            }
        }
    }

    fn normalize_notes_root(path: PathBuf) -> PathBuf {
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .map(|name| name.eq_ignore_ascii_case("StickyTerminal"))
            .unwrap_or(false)
        {
            path
        } else {
            path.join("StickyTerminal")
        }
    }

    fn default_note_file(&self) -> Option<PathBuf> {
        self.notes_root.as_ref().map(|root| root.join("inbox.md"))
    }

    fn note_file_path(&self) -> Option<PathBuf> {
        self.current_note_file.clone()
    }

    fn choose_notes_root(&mut self) {
        let start_dir = self
            .notes_root
            .clone()
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("."));

        let Some(selected_dir) = FileDialog::new().set_directory(start_dir).pick_folder() else {
            return;
        };

        let root = Self::normalize_notes_root(selected_dir);
        if let Err(err) = fs::create_dir_all(&root) {
            self.note_status = format!("Could not create notes folder: {err}");
            return;
        }

        self.notes_root = Some(root.clone());
        let note_still_inside_root = self
            .current_note_file
            .as_ref()
            .map(|path| path.starts_with(&root))
            .unwrap_or(false);
        if !note_still_inside_root {
            self.current_note_file = self.default_note_file();
        }
        self.note_status = format!("Using notes folder: {}", root.display());
        self.save_config();
        self.load_current_note();
    }

    fn choose_existing_note(&mut self) {
        let Some(root) = self.notes_root.clone() else {
            self.note_status = "Choose your notes folder first.".to_owned();
            return;
        };

        let Some(file) = FileDialog::new()
            .set_directory(&root)
            .add_filter("Markdown", &["md", "markdown", "txt"])
            .pick_file()
        else {
            return;
        };

        if !file.starts_with(&root) {
            self.note_status = "Pick a note inside your notes folder.".to_owned();
            return;
        }

        self.current_note_file = Some(file);
        self.load_current_note();
    }

    fn save_current_note(&mut self) {
        let Some(path) = self.note_file_path() else {
            self.note_status = "Choose your notes folder and a note first.".to_owned();
            return;
        };

        if let Some(parent) = path.parent() {
            if let Err(err) = fs::create_dir_all(parent) {
                self.note_status = format!("Could not create note folders: {err}");
                return;
            }
        }

        match fs::write(&path, &self.notes_markdown) {
            Ok(_) => {
                self.note_status = format!("Saved {}", path.display());
                self.save_config();
            }
            Err(err) => {
                self.note_status = format!("Could not save note: {err}");
            }
        }
    }

    fn load_current_note(&mut self) {
        let Some(path) = self.note_file_path() else {
            self.note_status = "Pick a note file to start writing.".to_owned();
            return;
        };

        self.save_config();

        match fs::read_to_string(&path) {
            Ok(contents) => {
                self.notes_markdown = contents;
                self.note_status = format!("Loaded {}", path.display());
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                self.notes_markdown = "# Inbox\n\nStart writing here.".to_owned();
                self.note_status = format!("New note ready: {}", path.display());
            }
            Err(err) => {
                self.note_status = format!("Could not load note: {err}");
            }
        }
    }

    fn create_new_note(&mut self) {
        let Some(root) = self.notes_root.clone() else {
            self.note_status = "Choose your notes folder first.".to_owned();
            return;
        };

        let Some(path) = FileDialog::new()
            .set_directory(&root)
            .set_file_name("note.md")
            .add_filter("Markdown", &["md"])
            .save_file()
        else {
            return;
        };

        if !path.starts_with(&root) {
            self.note_status = "Save the note inside your notes folder.".to_owned();
            return;
        }

        self.current_note_file = Some(if path.extension().is_none() {
            path.with_extension("md")
        } else {
            path
        });
        self.notes_markdown = "# New note\n\n".to_owned();
        self.note_status = "New note ready. Press Save to write it to disk.".to_owned();
        self.save_config();
    }

    fn note_surface_frame(palette: ThemePalette) -> egui::Frame {
        egui::Frame::NONE
            .fill(palette.surface)
            .corner_radius(egui::CornerRadius::same(8))
            .inner_margin(egui::Margin::same(10))
    }

    /// Calculate indent level from leading whitespace (each 2 spaces or 1 tab = 1 level)
    fn indent_level(line: &str) -> usize {
        let leading: usize = line
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .map(|c| if c == '\t' { 2 } else { 1 })
            .sum();
        leading / 2
    }

    fn render_markdown_preview(
        ui: &mut egui::Ui,
        markdown: &mut String,
        palette: ThemePalette,
        available_height: f32,
    ) -> bool {
        let mut changed = false;
        let indent_px = 16.0; // pixels per indent level

        egui::ScrollArea::vertical()
            .max_height(available_height)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                let wrap_width = ui.available_width();

                let lines: Vec<String> = markdown.lines().map(|s| s.to_owned()).collect();
                let mut line_idx = 0;

                while line_idx < lines.len() {
                    let line = &lines[line_idx];
                    let trimmed = line.trim();
                    let indent = Self::indent_level(line);
                    let left_margin = indent as f32 * indent_px;

                    if trimmed.is_empty() {
                        ui.add_space(6.0);
                        line_idx += 1;
                        continue;
                    }

                    // Headings (no indent)
                    if let Some(heading) = trimmed.strip_prefix("### ") {
                        ui.add_space(4.0);
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(heading)
                                    .size(14.0)
                                    .strong()
                                    .color(palette.text),
                            )
                            .wrap_mode(egui::TextWrapMode::Wrap),
                        );
                        ui.add_space(2.0);
                    } else if let Some(heading) = trimmed.strip_prefix("## ") {
                        ui.add_space(6.0);
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(heading)
                                    .size(16.0)
                                    .strong()
                                    .color(palette.text),
                            )
                            .wrap_mode(egui::TextWrapMode::Wrap),
                        );
                        ui.add_space(3.0);
                    } else if let Some(heading) = trimmed.strip_prefix("# ") {
                        ui.add_space(8.0);
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(heading)
                                    .size(20.0)
                                    .strong()
                                    .color(palette.text),
                            )
                            .wrap_mode(egui::TextWrapMode::Wrap),
                        );
                        ui.add_space(4.0);
                    }
                    // Checked checkbox
                    else if trimmed.starts_with("- [x] ") || trimmed.starts_with("- [X] ") {
                        let task_text = &trimmed[6..];
                        ui.horizontal_wrapped(|ui| {
                            if left_margin > 0.0 {
                                ui.add_space(left_margin);
                            }
                            let mut checked = true;
                            if ui.checkbox(&mut checked, "").changed() {
                                Self::toggle_line_checkbox(markdown, line_idx, false);
                                changed = true;
                            }
                            let text_width = (wrap_width - left_margin - 28.0).max(40.0);
                            ui.add_sized(
                                [text_width, 0.0],
                                egui::Label::new(
                                    egui::RichText::new(task_text)
                                        .strikethrough()
                                        .color(palette.muted_text),
                                )
                                .wrap_mode(egui::TextWrapMode::Wrap),
                            );
                        });
                    }
                    // Unchecked checkbox
                    else if trimmed.starts_with("- [ ] ") {
                        let task_text = &trimmed[6..];
                        ui.horizontal_wrapped(|ui| {
                            if left_margin > 0.0 {
                                ui.add_space(left_margin);
                            }
                            let mut checked = false;
                            if ui.checkbox(&mut checked, "").changed() {
                                Self::toggle_line_checkbox(markdown, line_idx, true);
                                changed = true;
                            }
                            let text_width = (wrap_width - left_margin - 28.0).max(40.0);
                            ui.add_sized(
                                [text_width, 0.0],
                                egui::Label::new(
                                    egui::RichText::new(task_text).color(palette.text),
                                )
                                .wrap_mode(egui::TextWrapMode::Wrap),
                            );
                        });
                    }
                    // Bullet point
                    else if let Some(bullet_text) = trimmed.strip_prefix("- ") {
                        ui.horizontal_wrapped(|ui| {
                            if left_margin > 0.0 {
                                ui.add_space(left_margin);
                            }
                            ui.label(egui::RichText::new("\u{2022}").color(palette.accent));
                            let text_width = (wrap_width - left_margin - 16.0).max(40.0);
                            ui.add_sized(
                                [text_width, 0.0],
                                egui::Label::new(
                                    egui::RichText::new(Self::render_inline_markdown(bullet_text))
                                        .color(palette.text),
                                )
                                .wrap_mode(egui::TextWrapMode::Wrap),
                            );
                        });
                    }
                    // Horizontal rule
                    else if trimmed == "---" || trimmed == "***" || trimmed == "___" {
                        ui.add_space(4.0);
                        ui.separator();
                        ui.add_space(4.0);
                    }
                    // Regular text
                    else {
                        if left_margin > 0.0 {
                            ui.horizontal_wrapped(|ui| {
                                ui.add_space(left_margin);
                                let text_width = (wrap_width - left_margin).max(40.0);
                                ui.add_sized(
                                    [text_width, 0.0],
                                    egui::Label::new(
                                        egui::RichText::new(Self::render_inline_markdown(trimmed))
                                            .color(palette.text),
                                    )
                                    .wrap_mode(egui::TextWrapMode::Wrap),
                                );
                            });
                        } else {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(Self::render_inline_markdown(trimmed))
                                        .color(palette.text),
                                )
                                .wrap_mode(egui::TextWrapMode::Wrap),
                            );
                        }
                    }

                    line_idx += 1;
                }
            });

        changed
    }

    fn render_inline_markdown(text: &str) -> String {
        text.replace("**", "").replace('*', "")
    }

    fn toggle_line_checkbox(markdown: &mut String, line_index: usize, checked: bool) {
        let mut lines: Vec<String> = markdown.lines().map(|s| s.to_owned()).collect();
        if line_index < lines.len() {
            if checked {
                lines[line_index] = lines[line_index].replacen("- [ ] ", "- [x] ", 1);
            } else {
                let line = &lines[line_index];
                if line.contains("- [x] ") {
                    lines[line_index] = line.replacen("- [x] ", "- [ ] ", 1);
                } else if line.contains("- [X] ") {
                    lines[line_index] = line.replacen("- [X] ", "- [ ] ", 1);
                }
            }
            *markdown = lines.join("\n");
        }
    }

    fn insert_checkbox_line(markdown: &mut String) {
        if markdown.ends_with('\n') || markdown.is_empty() {
            markdown.push_str("- [ ] ");
        } else {
            markdown.push_str("\n- [ ] ");
        }
    }

    #[cfg(target_os = "macos")]
    fn apply_macos_share_privacy(&self, enabled: bool) {
        unsafe {
            let ns_app_class = class!(NSApplication);
            let app: *mut Object = msg_send![ns_app_class, sharedApplication];
            if app.is_null() {
                return;
            }

            let windows: *mut Object = msg_send![app, windows];
            if windows.is_null() {
                return;
            }

            let count: usize = msg_send![windows, count];
            for i in 0..count {
                let window: *mut Object = msg_send![windows, objectAtIndex: i];
                if window.is_null() {
                    continue;
                }

                let sharing_type = if enabled { 0isize } else { 1isize };
                let _: () = msg_send![window, setSharingType: sharing_type];
            }
        }
    }

    #[cfg(not(target_os = "macos"))]
    fn apply_macos_share_privacy(&self, _enabled: bool) {}

    #[cfg(target_os = "macos")]
    fn toggle_fullscreen() {
        unsafe {
            let ns_app_class = class!(NSApplication);
            let app: *mut Object = msg_send![ns_app_class, sharedApplication];
            if app.is_null() {
                return;
            }
            let key_window: *mut Object = msg_send![app, keyWindow];
            if key_window.is_null() {
                return;
            }
            let _: () = msg_send![key_window, toggleFullScreen: std::ptr::null::<Object>()];
        }
    }

    #[cfg(not(target_os = "macos"))]
    fn toggle_fullscreen() {}

    fn apply_window_mode(&mut self, ctx: &egui::Context) {
        if self.applied_privacy_mode == Some(self.privacy_mode) {
            return;
        }

        self.apply_macos_share_privacy(self.privacy_mode);
        ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(if self.privacy_mode {
            egui::WindowLevel::AlwaysOnTop
        } else {
            egui::WindowLevel::Normal
        }));
        self.applied_privacy_mode = Some(self.privacy_mode);
    }

    fn active_tab(&self) -> &TerminalTab {
        &self.terminal_tabs[self.active_terminal]
    }

    fn active_tab_mut(&mut self) -> &mut TerminalTab {
        &mut self.terminal_tabs[self.active_terminal]
    }

    fn alloc_pane_uid(&mut self) -> u64 {
        let uid = self.next_pane_uid;
        self.next_pane_uid += 1;
        uid
    }

    fn log_debug(&mut self, msg: impl Into<String>) {
        let msg = msg.into();
        self.debug_log.push_back(msg);
        if self.debug_log.len() > DEBUG_LOG_MAX {
            self.debug_log.pop_front();
        }
    }

    fn add_terminal_tab(&mut self) {
        let cwd = self.active_tab().active_pane().cwd.clone();
        let number = self.next_tab_number;
        self.next_tab_number += 1;
        let uid = self.alloc_pane_uid();
        self.terminal_tabs.push(TerminalTab::new(number, uid, cwd));
        self.active_terminal = self.terminal_tabs.len().saturating_sub(1);
        self.renaming_tab = None;
        self.rename_buffer.clear();
        self.log_debug(format!("add_terminal_tab: tab={number} pane_uid={uid}"));
    }

    fn switch_terminal_tab(&mut self, index: usize) {
        if index < self.terminal_tabs.len() {
            self.active_terminal = index;
        }
    }

    fn move_terminal_tab(&mut self, from: usize, to: usize) {
        if from >= self.terminal_tabs.len() || to >= self.terminal_tabs.len() || from == to {
            return;
        }

        let tab = self.terminal_tabs.remove(from);
        self.terminal_tabs.insert(to, tab);

        self.active_terminal = if self.active_terminal == from {
            to
        } else if from < self.active_terminal && to >= self.active_terminal {
            self.active_terminal.saturating_sub(1)
        } else if from > self.active_terminal && to <= self.active_terminal {
            self.active_terminal.saturating_add(1)
        } else {
            self.active_terminal
        };

        if let Some(index) = self.renaming_tab {
            self.renaming_tab = if index == from {
                Some(to)
            } else if from < index && to >= index {
                Some(index.saturating_sub(1))
            } else if from > index && to <= index {
                Some(index.saturating_add(1))
            } else {
                Some(index)
            };
        }
    }

    fn close_terminal_tab(&mut self, index: usize) {
        if self.terminal_tabs.len() <= 1 || index >= self.terminal_tabs.len() {
            return;
        }

        self.terminal_tabs.remove(index);

        if self.active_terminal >= self.terminal_tabs.len() {
            self.active_terminal = self.terminal_tabs.len().saturating_sub(1);
        } else if index < self.active_terminal {
            self.active_terminal = self.active_terminal.saturating_sub(1);
        }

        if let Some(rename_index) = self.renaming_tab {
            self.renaming_tab = if rename_index == index {
                None
            } else if index < rename_index {
                Some(rename_index.saturating_sub(1))
            } else {
                Some(rename_index)
            };
        }
    }

    fn start_tab_rename(&mut self, index: usize) {
        if index >= self.terminal_tabs.len() {
            return;
        }

        self.renaming_tab = Some(index);
        self.rename_buffer = self.terminal_tabs[index].title.clone();
    }

    fn commit_tab_rename(&mut self) {
        let Some(index) = self.renaming_tab else {
            return;
        };

        let name = self.rename_buffer.trim();
        if !name.is_empty() && index < self.terminal_tabs.len() {
            self.terminal_tabs[index].title = name.to_owned();
        }

        self.renaming_tab = None;
        self.rename_buffer.clear();
    }

    fn cancel_tab_rename(&mut self) {
        self.renaming_tab = None;
        self.rename_buffer.clear();
    }

    fn ansi_index_color(index: u8) -> egui::Color32 {
        match index {
            0 => egui::Color32::from_rgb(0, 0, 0),
            1 => egui::Color32::from_rgb(205, 49, 49),
            2 => egui::Color32::from_rgb(13, 188, 121),
            3 => egui::Color32::from_rgb(229, 229, 16),
            4 => egui::Color32::from_rgb(36, 114, 200),
            5 => egui::Color32::from_rgb(188, 63, 188),
            6 => egui::Color32::from_rgb(17, 168, 205),
            7 => egui::Color32::from_rgb(229, 229, 229),
            8 => egui::Color32::from_rgb(102, 102, 102),
            9 => egui::Color32::from_rgb(241, 76, 76),
            10 => egui::Color32::from_rgb(35, 209, 139),
            11 => egui::Color32::from_rgb(245, 245, 67),
            12 => egui::Color32::from_rgb(59, 142, 234),
            13 => egui::Color32::from_rgb(214, 112, 214),
            14 => egui::Color32::from_rgb(41, 184, 219),
            15 => egui::Color32::from_rgb(255, 255, 255),
            16..=231 => {
                let value = index - 16;
                let r = value / 36;
                let g = (value % 36) / 6;
                let b = value % 6;
                let channel = |component: u8| {
                    if component == 0 {
                        0
                    } else {
                        55 + component * 40
                    }
                };
                egui::Color32::from_rgb(channel(r), channel(g), channel(b))
            }
            232..=255 => {
                let level = 8 + (index - 232) * 10;
                egui::Color32::from_rgb(level, level, level)
            }
        }
    }

    fn resolve_terminal_color(color: vt100::Color, default_color: egui::Color32) -> egui::Color32 {
        match color {
            vt100::Color::Default => default_color,
            vt100::Color::Idx(index) => Self::ansi_index_color(index),
            vt100::Color::Rgb(r, g, b) => egui::Color32::from_rgb(r, g, b),
        }
    }

    /// Render a single terminal pane
    fn render_pane(
        pane: &mut TerminalPane,
        ui: &mut egui::Ui,
        palette: ThemePalette,
        ctx: &egui::Context,
        pane_id: egui::Id,
        is_active: bool,
    ) {
        let frame = egui::Frame::NONE
            .fill(palette.terminal_bg)
            .corner_radius(egui::CornerRadius::same(6))
            .inner_margin(egui::Margin::same(6));

        frame.show(ui, |ui| {
            let terminal_id = pane_id.with("terminal_surface");
            let size = ui.available_size();
            let (rect, _response) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
            let response = ui.interact(rect, terminal_id, egui::Sense::click_and_drag());
            let hovered_files = ctx.input(|input| input.raw.hovered_files.clone());
            let dropped_files = ctx.input(|input| input.raw.dropped_files.clone());

            let font_id = egui::TextStyle::Monospace.resolve(ui.style());
            let measure =
                ui.painter()
                    .layout_no_wrap("W".to_owned(), font_id.clone(), palette.text);
            let char_width = measure.size().x.max(8.0);
            let row_height = measure.size().y.max(16.0) + 2.0;
            let inner_padding = 4.0;

            let rows = ((rect.height() - inner_padding * 2.0) / row_height).floor() as u16;
            let cols = ((rect.width() - inner_padding * 2.0) / char_width).floor() as u16;

            pane.resize(rows, cols);

            if response.clicked() {
                response.request_focus();
                pane.selection = None;
            }

            if response.drag_started() {
                response.request_focus();
                if let Some(pointer_pos) = response.interact_pointer_pos() {
                    let point =
                        pane.cell_from_pos(rect, pointer_pos, char_width, row_height, inner_padding);
                    pane.selection = Some((point, point));
                }
            }

            if response.dragged() {
                if let Some(pointer_pos) = response.interact_pointer_pos() {
                    let point =
                        pane.cell_from_pos(rect, pointer_pos, char_width, row_height, inner_padding);
                    if let Some((anchor, _)) = pane.selection {
                        pane.selection = Some((anchor, point));
                    }
                }
            }

            if response.hovered() {
                let scroll_delta = ctx.input(|input| input.smooth_scroll_delta.y);
                if scroll_delta.abs() > f32::EPSILON {
                    let rows_delta = (scroll_delta / row_height).round() as i32;
                    if rows_delta != 0 {
                        pane.adjust_scrollback(rows_delta);
                    }
                }
            }

            if response.hovered() && !dropped_files.is_empty() {
                let dropped_paths = dropped_files
                    .iter()
                    .filter_map(|file| {
                        file.path
                            .as_ref()
                            .map(|path| shell_escape_path(path))
                            .or_else(|| {
                                (!file.name.is_empty())
                                    .then(|| shell_escape_path(Path::new(&file.name)))
                            })
                    })
                    .collect::<Vec<_>>();
                if !dropped_paths.is_empty() {
                    response.request_focus();
                    pane.send_text(&(dropped_paths.join(" ") + " "));
                }
            }

            pane.has_focus = ui.memory(|memory| memory.has_focus(terminal_id));
            if pane.has_focus {
                ui.memory_mut(|memory| {
                    memory.set_focus_lock_filter(
                        terminal_id,
                        egui::EventFilter {
                            tab: true,
                            horizontal_arrows: true,
                            vertical_arrows: true,
                            escape: true,
                        },
                    );
                });
            }
            pane.handle_input(ctx);

            let painter = ui.painter_at(rect);

            // Draw a subtle active-pane indicator
            if is_active {
                painter.rect(
                    rect.shrink(0.5),
                    egui::CornerRadius::same(6),
                    egui::Color32::TRANSPARENT,
                    egui::Stroke::new(1.0, palette.accent.linear_multiply(0.3)),
                    egui::StrokeKind::Inside,
                );
            }

            let screen = pane.parser.screen();
            for row in 0..pane.rows {
                for col in 0..pane.cols {
                    let Some(cell) = screen.cell(row, col) else {
                        continue;
                    };

                    if cell.is_wide_continuation() {
                        continue;
                    }

                    let mut fg = Self::resolve_terminal_color(cell.fgcolor(), palette.text);
                    let mut bg =
                        Self::resolve_terminal_color(cell.bgcolor(), palette.terminal_bg);

                    if cell.inverse() {
                        std::mem::swap(&mut fg, &mut bg);
                    }

                    if cell.dim() {
                        fg = fg.linear_multiply(0.7);
                    }

                    let cell_width = if cell.is_wide() {
                        char_width * 2.0
                    } else {
                        char_width
                    };
                    let min = egui::pos2(
                        rect.left() + inner_padding + col as f32 * char_width,
                        rect.top() + inner_padding + row as f32 * row_height,
                    );
                    let cell_rect = egui::Rect::from_min_size(
                        min,
                        egui::vec2(cell_width.max(char_width), row_height),
                    );

                    if !matches!(cell.bgcolor(), vt100::Color::Default) || cell.inverse() {
                        painter.rect_filled(cell_rect, egui::CornerRadius::ZERO, bg);
                    }

                    if pane.cell_selected(row, col) {
                        painter.rect_filled(
                            cell_rect,
                            egui::CornerRadius::ZERO,
                            palette.selection,
                        );
                    }

                    if !cell.has_contents() {
                        continue;
                    }

                    let mut draw_font = font_id.clone();
                    if cell.bold() {
                        draw_font.size += 1.0;
                    }

                    painter.text(min, egui::Align2::LEFT_TOP, cell.contents(), draw_font, fg);

                    if cell.underline() {
                        let y = cell_rect.bottom() - 3.0;
                        painter.line_segment(
                            [
                                egui::pos2(cell_rect.left(), y),
                                egui::pos2(cell_rect.right(), y),
                            ],
                            egui::Stroke::new(1.0, fg),
                        );
                    }
                }
            }

            let (cursor_row, cursor_col) = screen.cursor_position();
            if pane.has_focus {
                let x = rect.left() + inner_padding + cursor_col as f32 * char_width;
                let y = rect.top() + inner_padding + cursor_row as f32 * row_height;
                let cursor_rect = egui::Rect::from_min_size(
                    egui::pos2(x, y),
                    egui::vec2(2.0, (row_height - 2.0).max(12.0)),
                );
                painter.rect_filled(cursor_rect, egui::CornerRadius::same(1), palette.accent);
            } else {
                painter.text(
                    rect.right_top() + egui::vec2(-10.0, 6.0),
                    egui::Align2::RIGHT_TOP,
                    "click to focus",
                    egui::TextStyle::Small.resolve(ui.style()),
                    palette.muted_text,
                );
            }

            if response.hovered() && !hovered_files.is_empty() {
                painter.rect_filled(
                    rect,
                    egui::CornerRadius::same(8),
                    palette.selection.linear_multiply(0.3),
                );
                painter.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "Drop files or folders to paste their paths",
                    egui::TextStyle::Button.resolve(ui.style()),
                    palette.text,
                );
            }
        });
    }

    /// Compute grid dimensions for n panes
    fn grid_dims(n: usize) -> (usize, usize) {
        match n {
            0 | 1 => (1, 1),
            2 => (2, 1),
            3 => (3, 1),
            4 => (2, 2),
            5 | 6 => (3, 2),
            7..=9 => (3, 3),
            10..=12 => (4, 3),
            _ => {
                let cols = (n as f32).sqrt().ceil() as usize;
                let rows = (n + cols - 1) / cols;
                (cols, rows)
            }
        }
    }

    /// Render all panes in an auto-grid layout with drag-to-swap
    fn render_panes(&mut self, ui: &mut egui::Ui, palette: ThemePalette, ctx: &egui::Context) {
        let tab_idx = self.active_terminal;
        let num_panes = self.terminal_tabs[tab_idx].panes.len();
        let active_pane_idx = self.terminal_tabs[tab_idx].active_pane;

        if num_panes == 1 {
            let pane_uid = self.terminal_tabs[tab_idx].panes[0].uid;
            let pane_id = ui.id().with(("pane_uid", pane_uid));
            let pane = &mut self.terminal_tabs[tab_idx].panes[0];
            Self::render_pane(pane, ui, palette, ctx, pane_id, true);
            return;
        }

        let (cols, rows) = Self::grid_dims(num_panes);
        let total_width = ui.available_width();
        let total_height = ui.available_height();
        let gap = PANE_SEPARATOR_WIDTH;
        let pane_width = (total_width - gap * (cols as f32 - 1.0)) / cols as f32;
        let pane_height = (total_height - gap * (rows as f32 - 1.0)) / rows as f32;

        // Collect rects for each pane slot to detect drag targets
        let mut pane_rects: Vec<egui::Rect> = Vec::with_capacity(num_panes);
        let origin = ui.cursor().min;

        for idx in 0..num_panes {
            let col = idx % cols;
            let row = idx / cols;
            let x = origin.x + col as f32 * (pane_width + gap);
            let y = origin.y + row as f32 * (pane_height + gap);
            pane_rects.push(egui::Rect::from_min_size(
                egui::pos2(x, y),
                egui::vec2(pane_width, pane_height),
            ));
        }

        // Draw separators
        let painter = ui.painter();
        for row in 0..rows {
            let panes_in_row = if (row + 1) * cols <= num_panes {
                cols
            } else {
                num_panes - row * cols
            };

            // Vertical separators between columns
            for col in 1..panes_in_row {
                let x = origin.x + col as f32 * (pane_width + gap) - gap;
                let y_top = origin.y + row as f32 * (pane_height + gap);
                let sep_rect = egui::Rect::from_min_size(
                    egui::pos2(x, y_top),
                    egui::vec2(gap, pane_height),
                );
                painter.rect_filled(sep_rect, egui::CornerRadius::ZERO, palette.border);
            }

            // Horizontal separator below this row (if not last row)
            if row + 1 < rows {
                let y = origin.y + (row + 1) as f32 * (pane_height + gap) - gap;
                let sep_rect = egui::Rect::from_min_size(
                    egui::pos2(origin.x, y),
                    egui::vec2(total_width, gap),
                );
                painter.rect_filled(sep_rect, egui::CornerRadius::ZERO, palette.border);
            }
        }

        // Render each pane in its grid slot
        let mut swap_target: Option<(usize, usize)> = None;

        for pane_idx in 0..num_panes {
            let rect = pane_rects[pane_idx];
            let is_active = pane_idx == active_pane_idx;
            let pane_uid = self.terminal_tabs[tab_idx].panes[pane_idx].uid;
            let pane_id = ui.id().with(("pane_uid", pane_uid));

            // Create a child UI constrained to this pane's rect
            let mut child_ui = ui.new_child(egui::UiBuilder::new().max_rect(rect));
            child_ui.set_clip_rect(rect);

            let pane = &mut self.terminal_tabs[tab_idx].panes[pane_idx];
            Self::render_pane(pane, &mut child_ui, palette, ctx, pane_id, is_active);

            if pane.has_focus && !is_active {
                let old_active = self.terminal_tabs[tab_idx].active_pane;
                self.terminal_tabs[tab_idx].active_pane = pane_idx;
                self.log_debug(format!(
                    "focus_change: pane {old_active} -> {pane_idx} (uid={})",
                    self.terminal_tabs[tab_idx].panes[pane_idx].uid
                ));
            }

            // Drag-to-swap detection: if this pane is being dragged,
            // check if pointer moved into another pane's rect
            let drag_id = pane_id.with("drag_swap");
            let drag_resp = ui.interact(rect, drag_id, egui::Sense::drag());

            if drag_resp.drag_started() {
                // Store which pane started dragging
                ui.data_mut(|d| d.insert_temp(egui::Id::new("dragging_pane"), pane_idx));
            }

            if drag_resp.dragged() {
                if let Some(pos) = drag_resp.interact_pointer_pos() {
                    let dragging_from: Option<usize> =
                        ui.data(|d| d.get_temp(egui::Id::new("dragging_pane")));
                    if let Some(from) = dragging_from {
                        // Find which pane rect the pointer is over
                        for (target_idx, target_rect) in pane_rects.iter().enumerate() {
                            if target_idx != from && target_rect.contains(pos) {
                                swap_target = Some((from, target_idx));
                                break;
                            }
                        }
                    }
                }
            }

            if drag_resp.drag_stopped() {
                ui.data_mut(|d| d.remove_by_type::<usize>());
            }
        }

        // Reserve the full grid area so egui knows it's used
        let grid_rect = egui::Rect::from_min_size(
            origin,
            egui::vec2(total_width, rows as f32 * (pane_height + gap) - gap),
        );
        ui.allocate_rect(grid_rect, egui::Sense::hover());

        // Perform swap if drag landed on another pane
        if let Some((from, to)) = swap_target {
            let from_uid = self.terminal_tabs[tab_idx].panes[from].uid;
            let to_uid = self.terminal_tabs[tab_idx].panes[to].uid;
            self.terminal_tabs[tab_idx].panes.swap(from, to);
            // Update active pane to follow the swap
            let active = self.terminal_tabs[tab_idx].active_pane;
            if active == from {
                self.terminal_tabs[tab_idx].active_pane = to;
            } else if active == to {
                self.terminal_tabs[tab_idx].active_pane = from;
            }
            self.log_debug(format!(
                "drag_swap: idx {from}(uid={from_uid}) <-> idx {to}(uid={to_uid}), active_pane={}",
                self.terminal_tabs[tab_idx].active_pane
            ));
        }
    }

    /// Render the tab bar
    fn render_tab_bar(
        &mut self,
        ui: &mut egui::Ui,
        palette: ThemePalette,
    ) -> (
        Option<usize>,
        Option<usize>,
        Option<usize>,
        Option<(usize, usize)>,
    ) {
        let mut switch_to = None;
        let mut close_tab = None;
        let mut rename_tab = None;
        let mut move_tab = None;

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;

            for index in 0..self.terminal_tabs.len() {
                let selected = index == self.active_terminal;
                let renaming = self.renaming_tab == Some(index);

                if renaming {
                    let response = ui.add_sized(
                        [140.0, 28.0],
                        egui::TextEdit::singleline(&mut self.rename_buffer)
                            .clip_text(false)
                            .desired_width(140.0),
                    );

                    if response.lost_focus()
                        && ui.input(|input| input.key_pressed(egui::Key::Enter))
                    {
                        self.commit_tab_rename();
                    }

                    if ui.input(|input| input.key_pressed(egui::Key::Escape)) {
                        self.cancel_tab_rename();
                    }

                    response.request_focus();
                    continue;
                }

                let (tab_fill, tab_text_color) = if selected {
                    (palette.active_tab_bg, palette.active_tab_text)
                } else {
                    (egui::Color32::TRANSPARENT, palette.tab_text)
                };

                // Show pane count if > 1
                let tab_label = {
                    let pane_count = self.terminal_tabs[index].panes.len();
                    if pane_count > 1 {
                        format!("{} ({})", self.terminal_tabs[index].title, pane_count)
                    } else {
                        self.terminal_tabs[index].title.clone()
                    }
                };

                let tab_frame = egui::Frame::NONE
                    .fill(tab_fill)
                    .corner_radius(egui::CornerRadius::same(6))
                    .inner_margin(egui::Margin::symmetric(12, 4));

                let response = tab_frame
                    .show(ui, |ui| {
                        ui.label(
                            egui::RichText::new(&tab_label)
                                .size(12.5)
                                .color(tab_text_color),
                        );
                    })
                    .response;

                let response = response.interact(egui::Sense::click_and_drag());

                if response.clicked() {
                    switch_to = Some(index);
                }

                response.context_menu(|ui| {
                    if ui.button("Rename").clicked() {
                        rename_tab = Some(index);
                        ui.close();
                    }

                    if ui
                        .add_enabled(
                            self.terminal_tabs.len() > 1,
                            egui::Button::new("Close"),
                        )
                        .clicked()
                    {
                        close_tab = Some(index);
                        ui.close();
                    }
                });

                if response.dragged() {
                    if let Some(pointer_pos) = response.interact_pointer_pos() {
                        if pointer_pos.x < response.rect.left() && index > 0 {
                            move_tab = Some((index, index - 1));
                        } else if pointer_pos.x > response.rect.right()
                            && index + 1 < self.terminal_tabs.len()
                        {
                            move_tab = Some((index, index + 1));
                        }
                    }
                }
            }

            ui.add_space(4.0);
            let plus_btn = ui.add(
                egui::Button::new(
                    egui::RichText::new("+").size(16.0).color(palette.muted_text),
                )
                .frame(false)
                .min_size(egui::vec2(28.0, 28.0)),
            );
            if plus_btn.clicked() {
                self.add_terminal_tab();
            }
            plus_btn.on_hover_text("New tab (Cmd+T)");

            // Show split hint
            let tab = &self.terminal_tabs[self.active_terminal];
            if tab.panes.len() > 1 {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(format!(
                            "pane {}/{}",
                            tab.active_pane + 1,
                            tab.panes.len()
                        ))
                        .small()
                        .color(palette.muted_text),
                    );
                });
            }
        });

        (switch_to, close_tab, rename_tab, move_tab)
    }
}

impl eframe::App for GhostStickiesApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if !self.startup_tasks_run {
            self.startup_tasks_run = true;
        }

        self.apply_window_mode(ctx);

        let open_new_tab =
            ctx.input(|input| input.modifiers.command && input.key_pressed(egui::Key::T));
        let insert_checkbox =
            ctx.input(|input| input.modifiers.command && input.key_pressed(egui::Key::L));
        let split_pane =
            ctx.input(|input| input.modifiers.command && input.key_pressed(egui::Key::D));
        let close_pane = ctx.input(|input| {
            input.modifiers.command && input.modifiers.shift && input.key_pressed(egui::Key::D)
        });
        let next_pane = ctx.input(|input| {
            input.modifiers.command && input.key_pressed(egui::Key::CloseBracket)
        });
        let prev_pane = ctx.input(|input| {
            input.modifiers.command && input.key_pressed(egui::Key::OpenBracket)
        });
        // Cmd+Shift+Arrow to move/swap the active pane in the grid
        let move_pane_left = ctx.input(|input| {
            input.modifiers.command
                && input.modifiers.shift
                && input.key_pressed(egui::Key::ArrowLeft)
        });
        let move_pane_right = ctx.input(|input| {
            input.modifiers.command
                && input.modifiers.shift
                && input.key_pressed(egui::Key::ArrowRight)
        });
        let move_pane_up = ctx.input(|input| {
            input.modifiers.command
                && input.modifiers.shift
                && input.key_pressed(egui::Key::ArrowUp)
        });
        let move_pane_down = ctx.input(|input| {
            input.modifiers.command
                && input.modifiers.shift
                && input.key_pressed(egui::Key::ArrowDown)
        });
        let toggle_debug = ctx.input(|input| {
            input.modifiers.command
                && input.modifiers.shift
                && input.key_pressed(egui::Key::L)
        });

        let mut received_output = false;
        for tab in &mut self.terminal_tabs {
            tab.ensure_all_started();
            if tab.drain_all_output() {
                received_output = true;
            }
        }
        if received_output {
            ctx.request_repaint();
        }
        ctx.request_repaint_after(Duration::from_millis(16));

        if open_new_tab {
            self.add_terminal_tab();
        }

        if split_pane && !close_pane {
            let uid = self.alloc_pane_uid();
            self.active_tab_mut().split_pane(uid);
            self.log_debug(format!("split_pane: new pane_uid={uid}, total_panes={}", self.active_tab().panes.len()));
        }

        if close_pane {
            let before = self.active_tab().panes.len();
            self.active_tab_mut().close_active_pane();
            self.log_debug(format!("close_pane: {before} -> {} panes", self.active_tab().panes.len()));
        }

        if next_pane {
            let before = self.active_tab().active_pane;
            self.active_tab_mut().focus_next_pane();
            let after = self.active_tab().active_pane;
            self.log_debug(format!("focus_next_pane: {before} -> {after}"));
        }

        if prev_pane {
            let before = self.active_tab().active_pane;
            self.active_tab_mut().focus_prev_pane();
            let after = self.active_tab().active_pane;
            self.log_debug(format!("focus_prev_pane: {before} -> {after}"));
        }

        // Move active pane in the grid
        let mut kbd_swap: Option<(usize, usize)> = None;
        {
            let tab = self.active_tab_mut();
            let n = tab.panes.len();
            if n > 1 {
                let idx = tab.active_pane;
                let (cols, _rows) = Self::grid_dims(n);

                let swap_with = if move_pane_left && idx % cols > 0 {
                    Some(idx - 1)
                } else if move_pane_right && idx % cols < cols - 1 && idx + 1 < n {
                    Some(idx + 1)
                } else if move_pane_up && idx >= cols {
                    Some(idx - cols)
                } else if move_pane_down && idx + cols < n {
                    Some(idx + cols)
                } else {
                    None
                };

                if let Some(target) = swap_with {
                    tab.panes.swap(idx, target);
                    tab.active_pane = target;
                    kbd_swap = Some((idx, target));
                }
            }
        }
        if let Some((from, to)) = kbd_swap {
            self.log_debug(format!("keyboard_swap_pane: {from} <-> {to}"));
        }

        if insert_checkbox && self.sidebar_open {
            Self::insert_checkbox_line(&mut self.notes_markdown);
            self.editing_notes = true;
        }

        if toggle_debug {
            self.show_debug = !self.show_debug;
            self.log_debug(format!("debug window toggled: {}", self.show_debug));
        }

        let palette = self.theme.palette();

        let mut style = (*ctx.style()).clone();
        style.visuals.window_fill = palette.bg;
        style.visuals.panel_fill = palette.bg;
        style.visuals.extreme_bg_color = palette.input_bg;
        style.visuals.selection.bg_fill = palette.selection;
        style.visuals.widgets.active.bg_fill = palette.surface;
        style.visuals.widgets.hovered.bg_fill = palette.surface;
        style.visuals.widgets.inactive.bg_fill = palette.tab_bg;
        style.visuals.widgets.noninteractive.bg_fill = palette.surface;
        style.visuals.widgets.active.fg_stroke.color = palette.text;
        style.visuals.widgets.hovered.fg_stroke.color = palette.text;
        style.visuals.widgets.inactive.fg_stroke.color = palette.tab_text;
        style.visuals.widgets.active.weak_bg_fill = palette.surface;
        style.visuals.widgets.hovered.weak_bg_fill = palette.surface;
        style.visuals.widgets.inactive.weak_bg_fill = palette.tab_bg;
        style.visuals.override_text_color = Some(palette.text);
        style.visuals.window_corner_radius = egui::CornerRadius::same(12);
        style.visuals.widgets.active.corner_radius = egui::CornerRadius::same(6);
        style.visuals.widgets.hovered.corner_radius = egui::CornerRadius::same(6);
        style.visuals.widgets.inactive.corner_radius = egui::CornerRadius::same(6);
        style.visuals.widgets.noninteractive.corner_radius = egui::CornerRadius::same(6);
        ctx.set_style(style);

        let mut start_drag = false;
        let mut quit_requested = false;
        let mut restart_clicked = false;
        let mut privacy_toggled = false;
        let mut theme_changed = None;

        // ── Top bar ──
        egui::TopBottomPanel::top("top_bar")
            .exact_height(TOP_BAR_HEIGHT)
            .frame(
                egui::Frame::NONE
                    .fill(palette.bar_bg)
                    .inner_margin(egui::Margin::symmetric(10, 6)),
            )
            .show(ctx, |ui| {
                let drag_response = ui.interact(
                    ui.max_rect(),
                    ui.id().with("top_bar_drag"),
                    egui::Sense::drag(),
                );
                if drag_response.dragged() {
                    start_drag = true;
                }

                ui.horizontal(|ui| {
                    // Traffic light buttons
                    let dot_size = egui::vec2(12.0, 12.0);

                    let (close_rect, close_resp) =
                        ui.allocate_exact_size(dot_size, egui::Sense::click());
                    let close_color = if close_resp.hovered() {
                        egui::Color32::from_rgb(255, 95, 86)
                    } else {
                        egui::Color32::from_rgb(255, 95, 86).linear_multiply(0.7)
                    };
                    ui.painter()
                        .circle_filled(close_rect.center(), 6.0, close_color);
                    if close_resp.clicked() {
                        quit_requested = true;
                    }

                    ui.add_space(4.0);
                    let (min_rect, min_resp) =
                        ui.allocate_exact_size(dot_size, egui::Sense::click());
                    let min_color = if min_resp.hovered() {
                        egui::Color32::from_rgb(255, 189, 46)
                    } else {
                        egui::Color32::from_rgb(255, 189, 46).linear_multiply(0.7)
                    };
                    ui.painter()
                        .circle_filled(min_rect.center(), 6.0, min_color);
                    if min_resp.clicked() {
                        self.minimized = !self.minimized;
                        let target_height = if self.minimized {
                            MINIMIZED_HEIGHT
                        } else {
                            WINDOW_HEIGHT
                        };
                        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
                            WINDOW_WIDTH,
                            target_height,
                        )));
                    }

                    ui.add_space(4.0);
                    let (max_rect, max_resp) =
                        ui.allocate_exact_size(dot_size, egui::Sense::click());
                    let max_color = if max_resp.hovered() {
                        egui::Color32::from_rgb(39, 201, 63)
                    } else {
                        egui::Color32::from_rgb(39, 201, 63).linear_multiply(0.7)
                    };
                    ui.painter()
                        .circle_filled(max_rect.center(), 6.0, max_color);
                    if max_resp.clicked() {
                        Self::toggle_fullscreen();
                    }

                    ui.add_space(16.0);

                    // Sidebar toggle
                    let sidebar_icon_color = if self.sidebar_open {
                        palette.accent
                    } else {
                        palette.muted_text
                    };
                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new("|||").size(11.0).color(sidebar_icon_color),
                            )
                            .frame(false),
                        )
                        .on_hover_text(if self.sidebar_open {
                            "Hide sidebar"
                        } else {
                            "Show sidebar"
                        })
                        .clicked()
                    {
                        self.sidebar_open = !self.sidebar_open;
                    }

                    ui.add_space(12.0);

                    // Git / path info
                    let cwd = &self.active_tab().active_pane().cwd;
                    let short_path = display_path_short(cwd);
                    if let Some(repo) = git_repo_name(cwd) {
                        ui.label(
                            egui::RichText::new(&repo)
                                .size(12.5)
                                .color(palette.accent)
                                .strong(),
                        );
                        if let Some(branch) = git_branch(cwd) {
                            ui.label(
                                egui::RichText::new("|")
                                    .size(11.0)
                                    .color(palette.muted_text),
                            );
                            ui.label(
                                egui::RichText::new(&branch)
                                    .size(12.0)
                                    .color(palette.muted_text),
                            );
                        }
                    } else {
                        ui.label(
                            egui::RichText::new(&short_path)
                                .size(12.5)
                                .color(palette.text),
                        );
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add(
                                egui::Button::new(
                                    egui::RichText::new("\u{21BB}")
                                        .size(14.0)
                                        .color(palette.muted_text),
                                )
                                .frame(false),
                            )
                            .on_hover_text("Restart shell")
                            .clicked()
                        {
                            restart_clicked = true;
                        }

                        if Self::symbol_button(
                            ui,
                            AppSymbol::Privacy,
                            if self.privacy_mode {
                                "Disable privacy mode"
                            } else {
                                "Enable privacy mode"
                            },
                            self.privacy_mode,
                        )
                        .clicked()
                        {
                            privacy_toggled = true;
                        }

                        ui.menu_button(
                            egui::RichText::new("Theme")
                                .size(12.0)
                                .color(palette.muted_text),
                            |ui| {
                                for preset in ThemePreset::ALL {
                                    let label = if self.theme == preset {
                                        egui::RichText::new(preset.label()).color(palette.accent)
                                    } else {
                                        egui::RichText::new(preset.label()).color(palette.text)
                                    };
                                    if ui.selectable_label(self.theme == preset, label).clicked() {
                                        theme_changed = Some(preset);
                                        ui.close();
                                    }
                                }
                            },
                        );
                    });
                });
            });

        if let Some(theme) = theme_changed {
            self.theme = theme;
            self.save_config();
        }

        if privacy_toggled {
            self.privacy_mode = !self.privacy_mode;
            self.apply_window_mode(ctx);
        }

        if restart_clicked {
            self.active_tab_mut().active_pane_mut().restart();
        }

        if quit_requested {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        if start_drag {
            ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }

        if self.minimized {
            return;
        }

        // ── Tab bar ──
        egui::TopBottomPanel::top("tab_bar")
            .exact_height(TAB_BAR_HEIGHT)
            .frame(
                egui::Frame::NONE
                    .fill(palette.bar_bg)
                    .inner_margin(egui::Margin::symmetric(10, 4)),
            )
            .show(ctx, |ui| {
                let (switch_to, close_tab, rename_tab, move_tab) =
                    self.render_tab_bar(ui, palette);

                if let Some((from, to)) = move_tab {
                    self.move_terminal_tab(from, to);
                }

                if let Some(index) = close_tab {
                    self.close_terminal_tab(index);
                }

                if let Some(index) = rename_tab {
                    self.start_tab_rename(index);
                }

                if let Some(index) = switch_to {
                    self.switch_terminal_tab(index);
                }
            });

        // ── Sidebar (Notes) ──
        if self.sidebar_open {
            egui::SidePanel::left("notes_sidebar")
                .resizable(true)
                .default_width(SIDEBAR_DEFAULT_WIDTH)
                .width_range(250.0..=560.0)
                .frame(
                    egui::Frame::NONE
                        .fill(palette.sidebar_bg)
                        .inner_margin(egui::Margin::same(12)),
                )
                .show(ctx, |ui| {
                    let mut choose_folder = false;
                    let mut open_note = false;
                    let mut save_note = false;
                    let mut new_note = false;
                    let note_text = self
                        .current_note_file
                        .as_ref()
                        .map(|path| {
                            if let Some(root) = &self.notes_root {
                                path.strip_prefix(root)
                                    .map(|relative| relative.display().to_string())
                                    .unwrap_or_else(|_| path.display().to_string())
                            } else {
                                path.display().to_string()
                            }
                        })
                        .unwrap_or_else(|| "No note selected".to_owned());

                    // Header row
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("Notes")
                                .strong()
                                .size(16.0)
                                .color(palette.text),
                        );

                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                let toggle_label =
                                    if self.editing_notes { "Preview" } else { "Edit" };
                                if ui
                                    .add(
                                        egui::Button::new(
                                            egui::RichText::new(toggle_label)
                                                .small()
                                                .color(palette.muted_text),
                                        )
                                        .frame(false),
                                    )
                                    .clicked()
                                {
                                    self.editing_notes = !self.editing_notes;
                                }
                            },
                        );
                    });
                    ui.add_space(4.0);

                    // File controls
                    Self::note_surface_frame(palette).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(&note_text)
                                    .small()
                                    .color(palette.muted_text),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui
                                        .add(
                                            egui::Button::new(
                                                egui::RichText::new("Save")
                                                    .small()
                                                    .color(palette.muted_text),
                                            )
                                            .frame(false),
                                        )
                                        .clicked()
                                    {
                                        save_note = true;
                                    }
                                    ui.label(
                                        egui::RichText::new("\u{2022}")
                                            .small()
                                            .color(palette.border),
                                    );
                                    if ui
                                        .add(
                                            egui::Button::new(
                                                egui::RichText::new("New")
                                                    .small()
                                                    .color(palette.muted_text),
                                            )
                                            .frame(false),
                                        )
                                        .clicked()
                                    {
                                        new_note = true;
                                    }
                                    ui.label(
                                        egui::RichText::new("\u{2022}")
                                            .small()
                                            .color(palette.border),
                                    );
                                    if ui
                                        .add(
                                            egui::Button::new(
                                                egui::RichText::new("Open")
                                                    .small()
                                                    .color(palette.muted_text),
                                            )
                                            .frame(false),
                                        )
                                        .clicked()
                                    {
                                        open_note = true;
                                    }
                                    ui.label(
                                        egui::RichText::new("\u{2022}")
                                            .small()
                                            .color(palette.border),
                                    );
                                    if ui
                                        .add(
                                            egui::Button::new(
                                                egui::RichText::new("Folder")
                                                    .small()
                                                    .color(palette.muted_text),
                                            )
                                            .frame(false),
                                        )
                                        .clicked()
                                    {
                                        choose_folder = true;
                                    }
                                },
                            );
                        });
                    });

                    ui.add_space(6.0);

                    let status_height = 20.0;
                    let content_height = (ui.available_height() - status_height - 8.0).max(100.0);

                    Self::note_surface_frame(palette).show(ui, |ui| {
                        if self.editing_notes {
                            let editor = egui::TextEdit::multiline(&mut self.notes_markdown)
                                .desired_width(f32::INFINITY)
                                .desired_rows(18)
                                .hint_text("Write markdown here. Cmd+L to add a checkbox.");
                            ui.add_sized([ui.available_width(), content_height], editor);
                        } else {
                            let preview_changed = Self::render_markdown_preview(
                                ui,
                                &mut self.notes_markdown,
                                palette,
                                content_height,
                            );
                            if preview_changed {
                                save_note = true;
                            }
                        }
                    });

                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(&self.note_status)
                            .small()
                            .color(palette.muted_text),
                    );

                    if choose_folder {
                        self.choose_notes_root();
                    }

                    if open_note {
                        self.choose_existing_note();
                    }

                    if new_note {
                        self.create_new_note();
                    }

                    if save_note {
                        self.save_current_note();
                    }
                });
        }

        // ── Debug log window ──
        if self.show_debug {
            egui::Window::new("Debug Log")
                .default_size([480.0, 320.0])
                .collapsible(true)
                .resizable(true)
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(format!("{} entries", self.debug_log.len()))
                                .small()
                                .color(palette.muted_text),
                        );
                        if ui.button("Clear").clicked() {
                            self.debug_log.clear();
                        }
                    });
                    ui.separator();
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .stick_to_bottom(true)
                        .show(ui, |ui| {
                            for entry in &self.debug_log {
                                ui.label(
                                    egui::RichText::new(entry)
                                        .monospace()
                                        .size(11.0)
                                        .color(palette.text),
                                );
                            }
                        });
                });
        }

        // ── Central panel: terminal panes ──
        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(palette.bg)
                    .inner_margin(egui::Margin::same(4)),
            )
            .show(ctx, |ui| {
                self.render_panes(ui, palette, ctx);
            });
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        self.theme.palette().bg.to_normalized_gamma_f32()
    }
}

fn main() -> Result<(), eframe::Error> {
    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon-1.png"))
        .expect("app icon should decode");

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([WINDOW_WIDTH, WINDOW_HEIGHT])
            .with_title("StickyTerminal")
            .with_transparent(true)
            .with_decorations(false)
            .with_icon(icon),
        ..Default::default()
    };

    eframe::run_native(
        "StickyTerminal",
        options,
        Box::new(|cc| Ok(Box::new(GhostStickiesApp::new(cc)))),
    )
}
