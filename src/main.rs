#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use rfd::FileDialog;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
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
    title: String,
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
    paste_chip: Option<String>, // filename shown in chip
    pending_logs: Vec<String>,
}

// ── A tab containing one or more panes ──
struct TerminalTab {
    title: String,
    panes: Vec<TerminalPane>,
    active_pane: usize,
    notes_markdown: String,
    current_note_file: Option<PathBuf>,
    note_status: String,
    editing_notes: bool,
    notes_dirty: bool,
    last_type_time: Option<std::time::Instant>,
}

struct GhostStickiesApp {
    notes_root: Option<PathBuf>,
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
    recent_notes: Vec<PathBuf>,
    renaming_pane: Option<(usize, usize)>, // (tab_idx, pane_idx)
    pane_rename_buffer: String,
}

const DEBUG_LOG_MAX: usize = 200;

#[derive(Serialize, Deserialize, Default)]
struct AppConfig {
    notes_root: Option<PathBuf>,
    current_note_file: Option<PathBuf>,
    theme: ThemePreset,
    #[serde(default)]
    recent_notes: Vec<PathBuf>,
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
                bg: egui::Color32::from_rgb(0, 0, 0),
                terminal_bg: egui::Color32::from_rgb(0, 0, 0),
                sidebar_bg: egui::Color32::from_rgb(6, 6, 8),
                sidebar_soft_bg: egui::Color32::from_rgb(10, 10, 12),
                bar_bg: egui::Color32::from_rgb(0, 0, 0),
                border: egui::Color32::from_rgba_premultiplied(255, 255, 255, 18),
                text: egui::Color32::from_rgb(250, 250, 252),
                muted_text: egui::Color32::from_rgb(170, 170, 178),
                selection: egui::Color32::from_rgba_premultiplied(74, 98, 176, 132),
                accent: egui::Color32::from_rgb(98, 224, 192),
                accent_dim: egui::Color32::from_rgb(60, 140, 118),
                tab_bg: egui::Color32::from_rgb(12, 12, 14),
                active_tab_bg: egui::Color32::from_rgb(20, 20, 24),
                tab_text: egui::Color32::from_rgb(180, 180, 188),
                active_tab_text: egui::Color32::from_rgb(250, 250, 252),
                input_bg: egui::Color32::from_rgb(8, 8, 10),
                surface: egui::Color32::from_rgb(14, 14, 18),
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
            title: String::new(),
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
            paste_chip: None,
            pending_logs: Vec::new(),
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

    fn scrollback_position(&self) -> usize {
        self.parser.screen().scrollback()
    }

    fn max_scrollback(&mut self) -> usize {
        let current = self.scrollback_position();
        self.parser.screen_mut().set_scrollback(usize::MAX);
        let max = self.parser.screen().scrollback();
        self.parser.screen_mut().set_scrollback(current.min(max));
        max
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

    fn paste_text(&mut self, text: &str) {
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
                    self.pending_logs.push(format!(
                        "img_paste: Event::Paste fired, text.len()={}",
                        text.len()
                    ));
                    if !text.is_empty() {
                        self.paste_text(&text);
                    } else {
                        self.pending_logs
                            .push("img_paste: text empty, trying save_clipboard_image".to_owned());
                        if let Some(img_path) = save_clipboard_image(&mut self.pending_logs) {
                            let filename = img_path
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or("image")
                                .to_owned();
                            let path_str = if let Some(home) = std::env::var_os("HOME") {
                                let abs = img_path.to_string_lossy();
                                let home_str = home.to_string_lossy();
                                if abs.starts_with(home_str.as_ref()) {
                                    format!("~{}", &abs[home_str.len()..])
                                } else {
                                    abs.into_owned()
                                }
                            } else {
                                img_path.to_string_lossy().into_owned()
                            };
                            self.pending_logs
                                .push(format!("img_paste: pasting path: {path_str}"));
                            self.paste_chip = Some(filename);
                            self.paste_text(&path_str);
                        } else {
                            self.pending_logs
                                .push("img_paste: save_clipboard_image returned None".to_owned());
                        }
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
                            egui::Key::V => {
                                self.pending_logs
                                    .push("img_paste: Key::V handler fired".to_owned());
                                let text = read_clipboard().filter(|t| !t.is_empty());
                                if let Some(t) = text {
                                    self.paste_text(&t);
                                } else {
                                    self.pending_logs.push(
                                        "img_paste: no text in clipboard, trying image".to_owned(),
                                    );
                                    if let Some(img_path) =
                                        save_clipboard_image(&mut self.pending_logs)
                                    {
                                        let filename = img_path
                                            .file_name()
                                            .and_then(|n| n.to_str())
                                            .unwrap_or("image")
                                            .to_owned();
                                        let path_str = if let Some(home) = std::env::var_os("HOME")
                                        {
                                            let abs = img_path.to_string_lossy();
                                            let home_str = home.to_string_lossy();
                                            if abs.starts_with(home_str.as_ref()) {
                                                format!("~{}", &abs[home_str.len()..])
                                            } else {
                                                abs.into_owned()
                                            }
                                        } else {
                                            img_path.to_string_lossy().into_owned()
                                        };
                                        self.pending_logs
                                            .push(format!("img_paste: pasting path: {path_str}"));
                                        self.paste_chip = Some(filename);
                                        self.paste_text(&path_str);
                                    }
                                }
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
                egui::Event::Copy => {
                    self.copy_selection(ctx);
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
                egui::Key::Backspace => Some(vec![0x17]), // Ctrl+W = backward-kill-word
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
            egui::Key::Enter => {
                if modifiers.shift {
                    b"\n".to_vec()
                } else {
                    b"\r".to_vec()
                }
            }
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

// ── Low-level Cmd+V detection via NSEvent local monitor ─────────────────────
//
// macOS routes Cmd+V through the `paste:` responder action. For text on the
// clipboard that generates Event::Paste in egui. For image-only clipboard
// content the action finds no text and egui sees *nothing*. We install an
// NSEvent local key-down monitor that fires before any responder processing,
// so we can detect Cmd+V independently of clipboard content.
//
// The monitor callback is an Objective-C block. We implement the block ABI
// manually (a "global block" with no captures) to avoid a new crate dependency.

static CMD_V_PRESSED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(target_os = "macos")]
fn install_paste_monitor() {
    use std::sync::OnceLock;

    extern "C" {
        // Linker symbol; value is the vtable pointer for global ObjC blocks.
        static _NSConcreteGlobalBlock: std::ffi::c_void;
    }

    #[repr(C)]
    struct BlockDescriptor {
        reserved: u64,
        size: u64,
    }

    // Matches the Clang block ABI layout for a block with no captured variables.
    #[repr(C)]
    struct GlobalBlock {
        isa: *const std::ffi::c_void,
        flags: i32,
        reserved: i32,
        invoke: unsafe extern "C" fn(*const GlobalBlock, *mut Object) -> *mut Object,
        descriptor: *const BlockDescriptor,
    }

    // SAFETY: the block is never mutated after creation and lives for the entire
    // process lifetime.
    unsafe impl Sync for GlobalBlock {}
    unsafe impl Send for GlobalBlock {}

    unsafe extern "C" fn invoke(_block: *const GlobalBlock, event: *mut Object) -> *mut Object {
        let flags: u64 = msg_send![event, modifierFlags];
        let keycode: u16 = msg_send![event, keyCode];
        // kVK_ANSI_V = 9   NSEventModifierFlagCommand = 1 << 20 = 0x100000
        if keycode == 9 && (flags & 0x100000) != 0 {
            CMD_V_PRESSED.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        event
    }

    static DESCRIPTOR: BlockDescriptor = BlockDescriptor {
        reserved: 0,
        size: std::mem::size_of::<GlobalBlock>() as u64,
    };

    static BLOCK: OnceLock<GlobalBlock> = OnceLock::new();

    let block = BLOCK.get_or_init(|| unsafe {
        GlobalBlock {
            isa: &_NSConcreteGlobalBlock as *const _ as *const std::ffi::c_void,
            flags: 0x10000000i32, // BLOCK_IS_GLOBAL
            reserved: 0,
            invoke,
            descriptor: &DESCRIPTOR,
        }
    });

    unsafe {
        // NSEventMaskKeyDown = 1 << 10
        let monitor: *mut Object = msg_send![
            class!(NSEvent),
            addLocalMonitorForEventsMatchingMask: (1u64 << 10)
            handler: block as *const GlobalBlock
        ];
        // The monitor is retained internally by NSEvent; we intentionally
        // let this object live forever (app lifetime).
        std::mem::forget(monitor);
    }
}

#[cfg(not(target_os = "macos"))]
fn install_paste_monitor() {}

#[cfg(target_os = "macos")]
fn read_clipboard() -> Option<String> {
    unsafe {
        let pasteboard: *mut Object = msg_send![class!(NSPasteboard), generalPasteboard];
        if pasteboard.is_null() {
            return None;
        }
        let ns_string_class = class!(NSString);
        let type_str: *mut Object = msg_send![
            ns_string_class,
            stringWithUTF8String: b"public.utf8-plain-text\0".as_ptr()
        ];
        let content: *mut Object = msg_send![pasteboard, stringForType: type_str];
        if content.is_null() {
            return None;
        }
        let utf8: *const std::os::raw::c_char = msg_send![content, UTF8String];
        if utf8.is_null() {
            return None;
        }
        let cstr = std::ffi::CStr::from_ptr(utf8);
        Some(cstr.to_string_lossy().into_owned())
    }
}

#[cfg(not(target_os = "macos"))]
fn read_clipboard() -> Option<String> {
    None
}

/// Save clipboard image to ~/Desktop/pasted-image-{ts}.png.
/// Logs every step into `log` so failures are visible in the debug window.
#[cfg(target_os = "macos")]
fn save_clipboard_image(log: &mut Vec<String>) -> Option<PathBuf> {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);

    let desktop =
        PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_owned())).join("Desktop");
    log.push(format!("img_paste: desktop = {}", desktop.display()));

    if let Err(e) = std::fs::create_dir_all(&desktop) {
        log.push(format!("img_paste: create_dir_all failed: {e}"));
    }
    let out_path = desktop.join(format!("pasted-image-{ts}.png"));

    unsafe {
        let pasteboard: *mut Object = msg_send![class!(NSPasteboard), generalPasteboard];
        if pasteboard.is_null() {
            log.push("img_paste: NSPasteboard is NULL".to_owned());
            return None;
        }
        log.push("img_paste: NSPasteboard OK".to_owned());

        // Log available pasteboard types
        let types: *mut Object = msg_send![pasteboard, types];
        if !types.is_null() {
            let count: usize = msg_send![types, count];
            log.push(format!("img_paste: pasteboard has {count} types"));
            for i in 0..count.min(10) {
                let t: *mut Object = msg_send![types, objectAtIndex: i];
                if !t.is_null() {
                    let utf8: *const std::os::raw::c_char = msg_send![t, UTF8String];
                    if !utf8.is_null() {
                        let s = std::ffi::CStr::from_ptr(utf8).to_string_lossy();
                        log.push(format!("img_paste:   type[{i}] = {s}"));
                    }
                }
            }
        } else {
            log.push("img_paste: pasteboard.types is NULL".to_owned());
        }

        // Try NSImage initWithPasteboard
        let image_alloc: *mut Object = msg_send![class!(NSImage), alloc];
        log.push(format!("img_paste: NSImage alloc = {:p}", image_alloc));
        let image: *mut Object = msg_send![image_alloc, initWithPasteboard: pasteboard];
        if image.is_null() {
            log.push(
                "img_paste: NSImage initWithPasteboard returned NULL — no image on clipboard"
                    .to_owned(),
            );
            return None;
        }
        log.push(format!("img_paste: NSImage OK ({:p})", image));

        let tiff: *mut Object = msg_send![image, TIFFRepresentation];
        if tiff.is_null() {
            log.push("img_paste: TIFFRepresentation is NULL".to_owned());
            return None;
        }
        let tiff_len: usize = msg_send![tiff, length];
        log.push(format!("img_paste: TIFF data = {tiff_len} bytes"));

        let rep: *mut Object = msg_send![class!(NSBitmapImageRep), imageRepWithData: tiff];
        if rep.is_null() {
            log.push("img_paste: NSBitmapImageRep is NULL — saving raw TIFF".to_owned());
            let ptr: *const u8 = msg_send![tiff, bytes];
            let bytes = std::slice::from_raw_parts(ptr, tiff_len);
            let tiff_path = out_path.with_extension("tiff");
            return match std::fs::write(&tiff_path, bytes) {
                Ok(_) => {
                    log.push(format!("img_paste: saved TIFF to {}", tiff_path.display()));
                    Some(tiff_path)
                }
                Err(e) => {
                    log.push(format!("img_paste: fs::write TIFF failed: {e}"));
                    None
                }
            };
        }
        log.push("img_paste: NSBitmapImageRep OK".to_owned());

        // NSBitmapImageFileTypePNG = 4
        let props: *mut Object = msg_send![class!(NSDictionary), dictionary];
        let png_data: *mut Object =
            msg_send![rep, representationUsingType: 4usize properties: props];

        let (data_ptr, data_len, path): (*const u8, usize, PathBuf) = if !png_data.is_null() {
            let len: usize = msg_send![png_data, length];
            log.push(format!("img_paste: PNG data = {len} bytes"));
            let ptr: *const u8 = msg_send![png_data, bytes];
            (ptr, len, out_path)
        } else {
            log.push("img_paste: PNG conversion failed — saving raw TIFF".to_owned());
            let ptr: *const u8 = msg_send![tiff, bytes];
            (ptr, tiff_len, out_path.with_extension("tiff"))
        };

        let bytes = std::slice::from_raw_parts(data_ptr, data_len);
        match std::fs::write(&path, bytes) {
            Ok(_) => {
                log.push(format!("img_paste: saved to {}", path.display()));
                Some(path)
            }
            Err(e) => {
                log.push(format!("img_paste: fs::write failed: {e}"));
                None
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn save_clipboard_image(log: &mut Vec<String>) -> Option<PathBuf> {
    log.push("img_paste: save_clipboard_image — not macOS".to_owned());
    None
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
            notes_markdown: "# TODO\n- [ ] Keep this shell usable for Codex CLI\n- [ ] Add quick project tabs\n- [ ] Save notes between sessions\n\n## Notes\nWrite markdown on the left.\nUse the right side like a real terminal.".to_owned(),
            current_note_file: None,
            note_status: "Set your notes folder to start saving notes.".to_owned(),
            editing_notes: false,
            notes_dirty: false,
            last_type_time: None,
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

    fn close_pane(&mut self, idx: usize) -> bool {
        if self.panes.len() <= 1 || idx >= self.panes.len() {
            return false;
        }
        self.panes.remove(idx);
        if self.active_pane >= self.panes.len() {
            self.active_pane = self.panes.len() - 1;
        } else if self.active_pane > idx {
            self.active_pane -= 1;
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
            notes_root: None,
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
            recent_notes: Vec::new(),
            renaming_pane: None,
            pane_rename_buffer: String::new(),
        }
    }
}

impl GhostStickiesApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        egui_extras::install_image_loaders(&cc.egui_ctx);
        let fonts = egui::FontDefinitions::default();
        cc.egui_ctx.set_fonts(fonts);

        install_paste_monitor();

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
            self.terminal_tabs[0].note_status =
                "Could not read saved settings. Using defaults.".to_owned();
            return;
        };

        self.theme = config.theme;
        self.terminal_tabs[0].current_note_file = config.current_note_file;
        self.recent_notes = config.recent_notes;

        if let Some(root) = config.notes_root {
            self.notes_root = Some(root);
            if self.terminal_tabs[0].current_note_file.is_none() {
                self.terminal_tabs[0].current_note_file = self.default_note_file();
            }
            self.load_current_note();
        }
    }

    fn save_config(&mut self) {
        let ti = self.active_terminal;
        let config = AppConfig {
            notes_root: self.notes_root.clone(),
            current_note_file: self.terminal_tabs[ti].current_note_file.clone(),
            theme: self.theme,
            recent_notes: self.recent_notes.clone(),
        };

        let support_dir = Self::app_support_dir();
        if let Err(err) = fs::create_dir_all(&support_dir) {
            self.terminal_tabs[ti].note_status =
                format!("Could not create app settings folder: {err}");
            return;
        }

        match serde_json::to_string_pretty(&config) {
            Ok(contents) => {
                if let Err(err) = fs::write(Self::config_path(), contents) {
                    self.terminal_tabs[ti].note_status = format!("Could not save settings: {err}");
                }
            }
            Err(err) => {
                self.terminal_tabs[ti].note_status = format!("Could not encode settings: {err}");
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
        self.terminal_tabs[self.active_terminal]
            .current_note_file
            .clone()
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
            let ti = self.active_terminal;
            self.terminal_tabs[ti].note_status = format!("Could not create notes folder: {err}");
            return;
        }

        self.notes_root = Some(root.clone());
        let ti = self.active_terminal;
        let note_still_inside_root = self.terminal_tabs[ti]
            .current_note_file
            .as_ref()
            .map(|path| path.starts_with(&root))
            .unwrap_or(false);
        if !note_still_inside_root {
            self.terminal_tabs[ti].current_note_file = self.default_note_file();
        }
        self.terminal_tabs[ti].note_status = format!("Using notes folder: {}", root.display());
        self.save_config();
        self.load_current_note();
    }

    fn choose_existing_note(&mut self) {
        let ti = self.active_terminal;
        let Some(root) = self.notes_root.clone() else {
            self.terminal_tabs[ti].note_status = "Choose your notes folder first.".to_owned();
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
            self.terminal_tabs[ti].note_status = "Pick a note inside your notes folder.".to_owned();
            return;
        }

        self.terminal_tabs[ti].current_note_file = Some(file);
        self.load_current_note();
    }

    fn add_to_recent_notes(&mut self) {
        if let Some(path) = self.terminal_tabs[self.active_terminal]
            .current_note_file
            .clone()
        {
            self.recent_notes.retain(|p| p != &path);
            self.recent_notes.insert(0, path);
            self.recent_notes.truncate(10);
        }
    }

    fn save_current_note_silent(&mut self) {
        let Some(path) = self.note_file_path() else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let ti = self.active_terminal;
        if fs::write(&path, &self.terminal_tabs[ti].notes_markdown).is_ok() {
            self.terminal_tabs[ti].notes_dirty = false;
            self.terminal_tabs[ti].last_type_time = None;
        }
    }

    fn save_current_note(&mut self) {
        let ti = self.active_terminal;
        let Some(path) = self.note_file_path() else {
            self.terminal_tabs[ti].note_status =
                "Choose your notes folder and a note first.".to_owned();
            return;
        };

        if let Some(parent) = path.parent() {
            if let Err(err) = fs::create_dir_all(parent) {
                self.terminal_tabs[ti].note_status =
                    format!("Could not create note folders: {err}");
                return;
            }
        }

        match fs::write(&path, &self.terminal_tabs[ti].notes_markdown) {
            Ok(_) => {
                self.terminal_tabs[ti].note_status = format!("Saved {}", path.display());
                self.terminal_tabs[ti].notes_dirty = false;
                self.terminal_tabs[ti].last_type_time = None;
                self.save_config();
            }
            Err(err) => {
                self.terminal_tabs[ti].note_status = format!("Could not save note: {err}");
            }
        }
    }

    fn load_current_note(&mut self) {
        let ti = self.active_terminal;
        let Some(path) = self.note_file_path() else {
            self.terminal_tabs[ti].note_status = "Pick a note file to start writing.".to_owned();
            return;
        };

        self.add_to_recent_notes();
        self.save_config();

        let ti = self.active_terminal;
        match fs::read_to_string(&path) {
            Ok(contents) => {
                self.terminal_tabs[ti].notes_markdown = contents;
                self.terminal_tabs[ti].notes_dirty = false;
                self.terminal_tabs[ti].last_type_time = None;
                self.terminal_tabs[ti].note_status = format!("Loaded {}", path.display());
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                self.terminal_tabs[ti].notes_markdown = "# Inbox\n\nStart writing here.".to_owned();
                self.terminal_tabs[ti].notes_dirty = false;
                self.terminal_tabs[ti].last_type_time = None;
                self.terminal_tabs[ti].note_status = format!("New note ready: {}", path.display());
            }
            Err(err) => {
                self.terminal_tabs[ti].note_status = format!("Could not load note: {err}");
            }
        }
    }

    fn create_new_note(&mut self) {
        let ti = self.active_terminal;
        let Some(root) = self.notes_root.clone() else {
            self.terminal_tabs[ti].note_status = "Choose your notes folder first.".to_owned();
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
            self.terminal_tabs[ti].note_status =
                "Save the note inside your notes folder.".to_owned();
            return;
        }

        self.terminal_tabs[ti].current_note_file = Some(if path.extension().is_none() {
            path.with_extension("md")
        } else {
            path
        });
        self.terminal_tabs[ti].notes_markdown = "# New note\n\n".to_owned();
        self.terminal_tabs[ti].note_status =
            "New note ready. Press Save to write it to disk.".to_owned();
        self.save_config();
    }

    fn note_surface_frame(palette: ThemePalette) -> egui::Frame {
        egui::Frame::NONE
            .fill(palette.surface)
            .corner_radius(egui::CornerRadius::same(8))
            .inner_margin(egui::Margin::same(10))
    }

    fn note_action_button(label: &str, palette: ThemePalette) -> egui::Button<'static> {
        egui::Button::new(
            egui::RichText::new(label.to_owned())
                .small()
                .color(palette.muted_text),
        )
        .corner_radius(egui::CornerRadius::same(6))
        .min_size(egui::vec2(0.0, 24.0))
    }

    fn tab_plus_button(ui: &mut egui::Ui, palette: ThemePalette) -> egui::Response {
        let (rect, response) = ui.allocate_exact_size(egui::vec2(30.0, 28.0), egui::Sense::click());

        if ui.is_rect_visible(rect) {
            let fill = if response.is_pointer_button_down_on() {
                palette.active_tab_bg
            } else if response.hovered() {
                palette.tab_bg
            } else {
                egui::Color32::TRANSPARENT
            };
            let stroke = if response.hovered() {
                egui::Stroke::new(1.0, palette.border)
            } else {
                egui::Stroke::NONE
            };

            ui.painter().rect(
                rect,
                egui::CornerRadius::same(6),
                fill,
                stroke,
                egui::StrokeKind::Inside,
            );
            ui.painter().text(
                rect.center() + egui::vec2(0.0, -0.5),
                egui::Align2::CENTER_CENTER,
                "+",
                egui::FontId::proportional(16.0),
                if response.hovered() {
                    palette.text
                } else {
                    palette.muted_text
                },
            );
        }

        response
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
                            ui.add(
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
                            ui.add(
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
                            ui.add(
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
                                ui.add(
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
        let text = text.replace("**", "").replace('*', "");
        // Strip [link text](url) -> link text
        let mut result = String::new();
        let mut rest = text.as_str();
        while let Some(bracket_start) = rest.find('[') {
            result.push_str(&rest[..bracket_start]);
            let after_open = &rest[bracket_start + 1..];
            if let Some(bracket_end) = after_open.find("](") {
                let link_text = &after_open[..bracket_end];
                let after_paren = &after_open[bracket_end + 2..];
                if let Some(paren_end) = after_paren.find(')') {
                    result.push_str(link_text);
                    rest = &after_paren[paren_end + 1..];
                } else {
                    result.push('[');
                    rest = after_open;
                }
            } else {
                result.push('[');
                rest = after_open;
            }
        }
        result.push_str(rest);
        result
    }

    /// Scan one terminal row and return URL spans as (start_col, end_col_inclusive, url).
    fn find_row_url_spans(screen: &vt100::Screen, row: u16, cols: u16) -> Vec<(u16, u16, String)> {
        // Build a char→column map so byte positions in the string map back to terminal cols.
        let mut char_to_col: Vec<u16> = Vec::with_capacity(cols as usize);
        let mut row_str = String::with_capacity(cols as usize);
        for col in 0..cols {
            if let Some(cell) = screen.cell(row, col) {
                if cell.is_wide_continuation() {
                    continue;
                }
                let content = cell.contents();
                if content.is_empty() {
                    char_to_col.push(col);
                    row_str.push(' ');
                } else {
                    for ch in content.chars() {
                        char_to_col.push(col);
                        row_str.push(ch);
                    }
                }
            } else {
                char_to_col.push(col);
                row_str.push(' ');
            }
        }

        let mut spans: Vec<(u16, u16, String)> = Vec::new();
        let mut search_from = 0usize;
        loop {
            let found = ["https://", "http://", "ftp://"]
                .iter()
                .filter_map(|p| {
                    row_str[search_from..]
                        .find(p)
                        .map(|pos| (search_from + pos, *p))
                })
                .min_by_key(|(pos, _)| *pos);
            let Some((abs_start, prefix)) = found else {
                break;
            };
            let url_tail = &row_str[abs_start..];
            let url_end = url_tail
                .find(|c: char| {
                    c.is_whitespace() || matches!(c, '"' | '\'' | ')' | ']' | '>' | '<')
                })
                .unwrap_or(url_tail.len());
            if url_end > prefix.len() {
                let url = url_tail[..url_end].to_string();
                let start_col = char_to_col.get(abs_start).copied().unwrap_or(0);
                let end_col = char_to_col
                    .get(abs_start + url_end - 1)
                    .copied()
                    .unwrap_or(start_col);
                spans.push((start_col, end_col, url));
                search_from = abs_start + url_end;
            } else {
                search_from = abs_start + prefix.len();
            }
        }
        spans
    }

    fn open_url(url: &str) {
        #[cfg(target_os = "macos")]
        let _ = std::process::Command::new("open").arg(url).spawn();
        #[cfg(target_os = "linux")]
        let _ = std::process::Command::new("xdg-open").arg(url).spawn();
        #[cfg(target_os = "windows")]
        let _ = std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .spawn();
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

    fn toggle_fullscreen(ctx: &egui::Context) {
        let is_fullscreen = ctx.input(|input| input.viewport().fullscreen.unwrap_or(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!is_fullscreen));
    }

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
        const DROP_TARGET_ID: &str = "terminal_drop_target";
        const SCROLLBAR_WIDTH: f32 = 10.0;
        const SCROLLBAR_GAP: f32 = 6.0;

        let frame = egui::Frame::NONE
            .fill(palette.terminal_bg)
            .corner_radius(egui::CornerRadius::same(6))
            .inner_margin(egui::Margin::same(6));

        frame.show(ui, |ui| {
            let terminal_id = pane_id.with("terminal_surface");
            let drop_target_id = egui::Id::new(DROP_TARGET_ID);
            let size = ui.available_size();
            let (_, rect) = ui.allocate_space(size);
            let content_rect = egui::Rect::from_min_max(
                rect.min,
                egui::pos2(
                    (rect.max.x - SCROLLBAR_WIDTH - SCROLLBAR_GAP).max(rect.min.x),
                    rect.max.y,
                ),
            );
            let scrollbar_rect = egui::Rect::from_min_max(
                egui::pos2(content_rect.max.x + SCROLLBAR_GAP, rect.top() + 4.0),
                egui::pos2(rect.right(), rect.bottom() - 4.0),
            );
            let response = ui.interact(content_rect, terminal_id, egui::Sense::click_and_drag());
            let scrollbar_response = ui.interact(
                scrollbar_rect,
                pane_id.with("terminal_scrollbar"),
                egui::Sense::click_and_drag(),
            );
            let hovered_files = ctx.input(|input| input.raw.hovered_files.clone());
            let dropped_files = ctx.input(|input| input.raw.dropped_files.clone());

            let font_id = egui::TextStyle::Monospace.resolve(ui.style());
            let measure =
                ui.painter()
                    .layout_no_wrap("W".to_owned(), font_id.clone(), palette.text);
            let char_width = measure.size().x.max(8.0);
            let row_height = measure.size().y.max(16.0) + 2.0;
            let inner_padding = 4.0;

            let rows = ((content_rect.height() - inner_padding * 2.0) / row_height).floor() as u16;
            let cols = ((content_rect.width() - inner_padding * 2.0) / char_width).floor() as u16;

            pane.resize(rows, cols);
            let max_scrollback = pane.max_scrollback();

            let cmd_held = ctx.input(|i| i.modifiers.command);

            if response.clicked() {
                let mut url_opened = false;
                if cmd_held {
                    if let Some(pos) = response.interact_pointer_pos() {
                        let point = pane.cell_from_pos(
                            content_rect,
                            pos,
                            char_width,
                            row_height,
                            inner_padding,
                        );
                        let url_to_open = {
                            let screen = pane.parser.screen();
                            let spans = Self::find_row_url_spans(screen, point.row, pane.cols);
                            spans
                                .into_iter()
                                .find(|(s, e, _)| point.col >= *s && point.col <= *e)
                                .map(|(_, _, url)| url)
                        };
                        if let Some(url) = url_to_open {
                            Self::open_url(&url);
                            url_opened = true;
                        }
                    }
                }
                if !url_opened {
                    response.request_focus();
                    pane.selection = None;
                }
            }

            if response.drag_started() {
                response.request_focus();
                if let Some(pointer_pos) = response.interact_pointer_pos() {
                    let point = pane.cell_from_pos(
                        content_rect,
                        pointer_pos,
                        char_width,
                        row_height,
                        inner_padding,
                    );
                    pane.selection = Some((point, point));
                }
            }

            if response.dragged() {
                if let Some(pointer_pos) = response.interact_pointer_pos() {
                    let point = pane.cell_from_pos(
                        content_rect,
                        pointer_pos,
                        char_width,
                        row_height,
                        inner_padding,
                    );
                    if let Some((anchor, _)) = pane.selection {
                        pane.selection = Some((anchor, point));
                    }
                }
            }

            if response.hovered() || scrollbar_response.hovered() {
                let scroll_delta = ctx.input(|input| input.smooth_scroll_delta.y);
                if scroll_delta.abs() > f32::EPSILON {
                    let rows_delta = (scroll_delta / row_height).round() as i32;
                    if rows_delta != 0 {
                        pane.adjust_scrollback(rows_delta);
                    }
                }
            }

            if !hovered_files.is_empty() && response.hovered() {
                ui.data_mut(|data| data.insert_temp(drop_target_id, Some(pane_id)));
            }

            let is_drop_target = ui
                .data(|data| data.get_temp::<Option<egui::Id>>(drop_target_id).flatten())
                == Some(pane_id);

            if is_drop_target && !dropped_files.is_empty() {
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
                    let pasted_paths = dropped_paths.join(" ");
                    ctx.copy_text(pasted_paths.clone());
                    pane.paste_text(&(pasted_paths + " "));
                    pane.status = if dropped_paths.len() == 1 {
                        "Dropped path copied and pasted.".to_owned()
                    } else {
                        format!("{} dropped paths copied and pasted.", dropped_paths.len())
                    };
                }
                ui.data_mut(|data| {
                    data.remove_temp::<Option<egui::Id>>(drop_target_id);
                });
            } else if hovered_files.is_empty() && dropped_files.is_empty() && is_drop_target {
                ui.data_mut(|data| {
                    data.remove_temp::<Option<egui::Id>>(drop_target_id);
                });
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

            if max_scrollback > 0 {
                let scrollback_offset = pane.scrollback_position();
                let total_rows = max_scrollback + usize::from(pane.rows.max(1));
                let visible_ratio = pane.rows as f32 / total_rows as f32;
                let thumb_height =
                    (scrollbar_rect.height() * visible_ratio).clamp(28.0, scrollbar_rect.height());

                painter.rect_filled(
                    scrollbar_rect,
                    egui::CornerRadius::same(5),
                    palette.surface.linear_multiply(0.55),
                );

                let travel = (scrollbar_rect.height() - thumb_height).max(0.0);
                let thumb_top = if travel <= f32::EPSILON {
                    scrollbar_rect.top()
                } else {
                    scrollbar_rect.top()
                        + (1.0 - scrollback_offset as f32 / max_scrollback as f32) * travel
                };
                let thumb_rect = egui::Rect::from_min_size(
                    egui::pos2(scrollbar_rect.left(), thumb_top),
                    egui::vec2(scrollbar_rect.width(), thumb_height),
                );
                let thumb_color = if scrollbar_response.dragged() {
                    palette.accent
                } else if scrollbar_response.hovered() || scrollback_offset > 0 {
                    palette.text.linear_multiply(0.72)
                } else {
                    palette.muted_text.linear_multiply(0.5)
                };
                painter.rect_filled(thumb_rect, egui::CornerRadius::same(5), thumb_color);

                if let Some(pointer_pos) = scrollbar_response.interact_pointer_pos() {
                    if scrollbar_response.clicked() || scrollbar_response.dragged() {
                        let travel = (scrollbar_rect.height() - thumb_height).max(1.0);
                        let thumb_y = (pointer_pos.y - scrollbar_rect.top() - thumb_height * 0.5)
                            .clamp(0.0, travel);
                        let top_ratio = thumb_y / travel;
                        pane.set_scrollback(
                            ((1.0 - top_ratio) * max_scrollback as f32).round() as usize
                        );
                    }
                }
            }

            let screen = pane.parser.screen();

            // Cell position under the pointer (for URL hover highlight).
            let hovered_cell = if cmd_held && response.hovered() {
                ctx.input(|i| i.pointer.hover_pos())
                    .filter(|p| content_rect.contains(*p))
                    .map(|p| {
                        pane.cell_from_pos(content_rect, p, char_width, row_height, inner_padding)
                    })
            } else {
                None
            };
            let mut set_hand_cursor = false;

            for row in 0..pane.rows {
                let url_spans: Vec<(u16, u16, String)> = if cmd_held {
                    Self::find_row_url_spans(screen, row, pane.cols)
                } else {
                    Vec::new()
                };

                for col in 0..pane.cols {
                    let Some(cell) = screen.cell(row, col) else {
                        continue;
                    };

                    if cell.is_wide_continuation() {
                        continue;
                    }

                    let mut fg = Self::resolve_terminal_color(cell.fgcolor(), palette.text);
                    let mut bg = Self::resolve_terminal_color(cell.bgcolor(), palette.terminal_bg);

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
                        content_rect.left() + inner_padding + col as f32 * char_width,
                        content_rect.top() + inner_padding + row as f32 * row_height,
                    );
                    let cell_rect = egui::Rect::from_min_size(
                        min,
                        egui::vec2(cell_width.max(char_width), row_height),
                    );

                    if !matches!(cell.bgcolor(), vt100::Color::Default) || cell.inverse() {
                        painter.rect_filled(cell_rect, egui::CornerRadius::ZERO, bg);
                    }

                    if pane.cell_selected(row, col) {
                        painter.rect_filled(cell_rect, egui::CornerRadius::ZERO, palette.selection);
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

                    // URL underline when Cmd is held.
                    if let Some((us, ue, _)) =
                        url_spans.iter().find(|(s, e, _)| col >= *s && col <= *e)
                    {
                        let is_hovered = hovered_cell
                            .map(|hp| hp.row == row && hp.col >= *us && hp.col <= *ue)
                            .unwrap_or(false);
                        if is_hovered {
                            set_hand_cursor = true;
                        }
                        let ucolor = if is_hovered {
                            palette.accent
                        } else {
                            palette.accent.linear_multiply(0.5)
                        };
                        let y = cell_rect.bottom() - 2.0;
                        painter.line_segment(
                            [
                                egui::pos2(cell_rect.left(), y),
                                egui::pos2(cell_rect.right(), y),
                            ],
                            egui::Stroke::new(1.0, ucolor),
                        );
                    }
                }
            }

            if set_hand_cursor {
                ctx.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
            }

            let (cursor_row, cursor_col) = screen.cursor_position();
            if pane.has_focus {
                let x = content_rect.left() + inner_padding + cursor_col as f32 * char_width;
                let y = content_rect.top() + inner_padding + cursor_row as f32 * row_height;
                let cursor_rect = egui::Rect::from_min_size(
                    egui::pos2(x, y),
                    egui::vec2(2.0, (row_height - 2.0).max(12.0)),
                );
                painter.rect_filled(cursor_rect, egui::CornerRadius::same(1), palette.accent);
            } else {
                painter.text(
                    content_rect.right_top() + egui::vec2(-10.0, 6.0),
                    egui::Align2::RIGHT_TOP,
                    "click to focus",
                    egui::TextStyle::Small.resolve(ui.style()),
                    palette.muted_text,
                );
            }

            if is_drop_target && !hovered_files.is_empty() {
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

            // ── Paste-image chip overlay ─────────────────────────────────
            let chip_dismiss = if let Some(ref filename) = pane.paste_chip {
                let font = egui::FontId::proportional(12.0);
                let label = format!("\u{1F5BC} {filename}");
                let text_width = painter
                    .layout_no_wrap(label.clone(), font.clone(), egui::Color32::WHITE)
                    .size()
                    .x;
                let chip_w = text_width + 20.0 + 20.0; // text + padding + close btn
                let chip_h = 26.0;
                let chip_margin = 8.0;
                // Position at bottom-left so it doesn't cover terminal output
                let chip_rect = egui::Rect::from_min_size(
                    egui::pos2(
                        rect.left() + chip_margin,
                        rect.bottom() - chip_margin - chip_h,
                    ),
                    egui::vec2(chip_w, chip_h),
                );

                let chip_bg = egui::Color32::from_rgb(40, 42, 56);
                let chip_border = egui::Color32::from_rgb(100, 105, 140);
                let text_color = egui::Color32::from_rgb(210, 215, 235);
                let close_color = egui::Color32::from_rgb(160, 160, 180);

                painter.rect(
                    chip_rect,
                    egui::CornerRadius::same(6),
                    chip_bg,
                    egui::Stroke::new(1.0, chip_border),
                    egui::StrokeKind::Outside,
                );
                painter.text(
                    egui::pos2(chip_rect.left() + 8.0, chip_rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    &label,
                    font,
                    text_color,
                );

                // × close button
                let close_rect = egui::Rect::from_min_size(
                    egui::pos2(chip_rect.right() - 22.0, chip_rect.top()),
                    egui::vec2(22.0, chip_h),
                );
                let close_id = pane_id.with("paste_chip_close");
                let close_resp = ui.interact(close_rect, close_id, egui::Sense::click());
                let x_color = if close_resp.hovered() {
                    egui::Color32::from_rgb(220, 80, 80)
                } else {
                    close_color
                };
                painter.text(
                    close_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "×",
                    egui::FontId::proportional(14.0),
                    x_color,
                );
                close_resp.clicked()
            } else {
                false
            };
            if chip_dismiss {
                pane.paste_chip = None;
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
            let logs = std::mem::take(&mut self.terminal_tabs[tab_idx].panes[0].pending_logs);
            for msg in logs {
                self.log_debug(msg);
            }
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
                let sep_rect =
                    egui::Rect::from_min_size(egui::pos2(x, y_top), egui::vec2(gap, pane_height));
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

        const BAR_H: f32 = 24.0;

        // Render each pane in its grid slot
        let mut pending_focus: Option<usize> = None;
        let mut pending_swap: Option<(usize, usize)> = None;
        let mut pending_close: Option<usize> = None;
        let mut pending_rename_start: Option<(usize, String)> = None; // (pane_idx, current_title)
        let mut pending_rename_commit = false;
        let mut pending_rename_cancel = false;

        for pane_idx in 0..num_panes {
            let full_rect = pane_rects[pane_idx];
            let is_active = pane_idx == active_pane_idx;
            let pane_uid = self.terminal_tabs[tab_idx].panes[pane_idx].uid;
            let pane_id = ui.id().with(("pane_uid", pane_uid));

            // Split full rect into bar + content
            let bar_rect =
                egui::Rect::from_min_size(full_rect.min, egui::vec2(full_rect.width(), BAR_H));
            let content_rect = egui::Rect::from_min_max(
                egui::pos2(full_rect.min.x, full_rect.min.y + BAR_H),
                full_rect.max,
            );

            // ── Title bar ──────────────────────────────────────────────────
            let bar_bg = if is_active {
                palette.surface
            } else {
                palette.bar_bg
            };
            ui.painter()
                .rect_filled(bar_rect, egui::CornerRadius::ZERO, bar_bg);

            // Drag handle (left side) — click focuses pane, drag swaps
            let handle_rect = egui::Rect::from_min_size(
                egui::pos2(bar_rect.left(), bar_rect.top()),
                egui::vec2(28.0, BAR_H),
            );
            let handle_id = pane_id.with("bar_handle");
            let handle_resp = ui.interact(handle_rect, handle_id, egui::Sense::click_and_drag());
            let handle_color = if handle_resp.hovered() || handle_resp.dragged() {
                palette.accent
            } else {
                palette.muted_text.linear_multiply(0.5)
            };
            // Draw ⠿ grid dots as 6 tiny circles arranged 2×3
            {
                let cx = handle_rect.center().x;
                let cy = handle_rect.center().y;
                let dx = 3.0_f32;
                let dy = 3.0_f32;
                let r = 1.2_f32;
                for row in [-1i32, 0, 1] {
                    for col in [-1i32, 1] {
                        ui.painter().circle_filled(
                            egui::pos2(cx + col as f32 * dx, cy + row as f32 * dy),
                            r,
                            handle_color,
                        );
                    }
                }
            }

            if handle_resp.clicked() {
                pending_focus = Some(pane_idx);
            }
            if handle_resp.drag_started() {
                let handle_center = handle_rect.center();
                ui.data_mut(|d| {
                    d.insert_temp(egui::Id::new("bar_drag_from"), pane_idx);
                    d.insert_temp(egui::Id::new("bar_drag_origin"), handle_center);
                });
            }
            if handle_resp.drag_stopped() {
                let from: Option<usize> = ui.data(|d| d.get_temp(egui::Id::new("bar_drag_from")));
                if let Some(from_idx) = from {
                    if let Some(pos) = handle_resp.interact_pointer_pos() {
                        for (to_idx, to_rect) in pane_rects.iter().enumerate() {
                            if to_idx != from_idx && to_rect.contains(pos) {
                                pending_swap = Some((from_idx, to_idx));
                                break;
                            }
                        }
                    }
                }
                ui.data_mut(|d| {
                    d.remove_by_type::<usize>();
                    d.remove_by_type::<egui::Pos2>();
                });
            }

            // X close button (right side)
            let close_btn_size = egui::vec2(BAR_H, BAR_H);
            let close_rect = egui::Rect::from_min_size(
                egui::pos2(bar_rect.right() - close_btn_size.x, bar_rect.top()),
                close_btn_size,
            );
            let close_id = pane_id.with("bar_close");
            let close_resp = ui.interact(close_rect, close_id, egui::Sense::click());
            let close_color = if close_resp.hovered() {
                egui::Color32::from_rgb(220, 80, 80)
            } else {
                palette.muted_text.linear_multiply(0.5)
            };
            ui.painter().text(
                close_rect.center(),
                egui::Align2::CENTER_CENTER,
                "×",
                egui::FontId::proportional(14.0),
                close_color,
            );
            if close_resp.clicked() {
                pending_close = Some(pane_idx);
            }

            // Pane title (center of bar, between handle and close button)
            let title_rect = egui::Rect::from_min_max(
                egui::pos2(bar_rect.left() + 28.0, bar_rect.top()),
                egui::pos2(bar_rect.right() - close_btn_size.x, bar_rect.bottom()),
            );
            let is_renaming = self.renaming_pane == Some((tab_idx, pane_idx));

            if is_renaming {
                // Inline text edit for rename
                let rename_id = pane_id.with("bar_rename_edit");
                let mut rename_ui =
                    ui.new_child(egui::UiBuilder::new().max_rect(title_rect.shrink(2.0)));
                let edit_resp = rename_ui.add(
                    egui::TextEdit::singleline(&mut self.pane_rename_buffer)
                        .id(rename_id)
                        .desired_width(title_rect.width() - 4.0)
                        .font(egui::TextStyle::Small)
                        .frame(false),
                );
                edit_resp.request_focus();
                // Check keys via ctx — single-line TextEdit does NOT lose focus on Enter
                let pressed_enter = ctx.input(|i| i.key_pressed(egui::Key::Enter));
                let pressed_esc = ctx.input(|i| i.key_pressed(egui::Key::Escape));
                if pressed_esc {
                    pending_rename_cancel = true;
                } else if pressed_enter {
                    pending_rename_commit = true;
                } else if edit_resp.lost_focus() {
                    // Clicked somewhere else — commit
                    pending_rename_commit = true;
                }
            } else {
                // Display title; double-click to rename
                let current_title = &self.terminal_tabs[tab_idx].panes[pane_idx].title;
                let display_title = if current_title.is_empty() {
                    format!("Terminal {}", pane_idx + 1)
                } else {
                    current_title.clone()
                };
                let title_color = if is_active {
                    palette.text
                } else {
                    palette.muted_text
                };
                let title_id = pane_id.with("bar_title");
                let title_resp = ui.interact(title_rect, title_id, egui::Sense::click());
                ui.painter().text(
                    title_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    &display_title,
                    egui::FontId::proportional(11.0),
                    title_color,
                );
                if title_resp.double_clicked() {
                    pending_rename_start = Some((pane_idx, display_title));
                }
                if title_resp.clicked() {
                    pending_focus = Some(pane_idx);
                }
            }

            // ── Terminal content ────────────────────────────────────────────
            let mut child_ui = ui.new_child(egui::UiBuilder::new().max_rect(content_rect));
            child_ui.set_clip_rect(content_rect);

            let pane = &mut self.terminal_tabs[tab_idx].panes[pane_idx];
            Self::render_pane(pane, &mut child_ui, palette, ctx, pane_id, is_active);
            let logs =
                std::mem::take(&mut self.terminal_tabs[tab_idx].panes[pane_idx].pending_logs);
            for msg in logs {
                self.log_debug(msg);
            }

            if self.terminal_tabs[tab_idx].panes[pane_idx].has_focus && !is_active {
                let old_active = self.terminal_tabs[tab_idx].active_pane;
                self.terminal_tabs[tab_idx].active_pane = pane_idx;
                self.log_debug(format!(
                    "focus_change: pane {old_active} -> {pane_idx} (uid={})",
                    self.terminal_tabs[tab_idx].panes[pane_idx].uid
                ));
            }
        }

        // Draw drag line while a bar handle is being dragged
        {
            let from: Option<usize> = ui.data(|d| d.get_temp(egui::Id::new("bar_drag_from")));
            if from.is_some() {
                if let Some(origin_pos) =
                    ui.data(|d| d.get_temp::<egui::Pos2>(egui::Id::new("bar_drag_origin")))
                {
                    if let Some(ptr) = ctx.input(|i| i.pointer.hover_pos()) {
                        ui.painter().line_segment(
                            [origin_pos, ptr],
                            egui::Stroke::new(1.5, palette.accent.linear_multiply(0.55)),
                        );
                        ctx.request_repaint();
                    }
                }
            }
        }

        // Apply pending rename
        if let Some((pane_idx, current)) = pending_rename_start {
            self.renaming_pane = Some((tab_idx, pane_idx));
            self.pane_rename_buffer = current;
        }
        if pending_rename_commit {
            if let Some((t, p)) = self.renaming_pane {
                let new_title = self.pane_rename_buffer.trim().to_owned();
                self.terminal_tabs[t].panes[p].title = new_title;
            }
            self.renaming_pane = None;
        }
        if pending_rename_cancel {
            self.renaming_pane = None;
        }

        // Apply pending focus / swap / close
        if let Some(pane_idx) = pending_focus {
            self.terminal_tabs[tab_idx].active_pane = pane_idx;
        }
        if let Some((from, to)) = pending_swap {
            let from_uid = self.terminal_tabs[tab_idx].panes[from].uid;
            let to_uid = self.terminal_tabs[tab_idx].panes[to].uid;
            self.terminal_tabs[tab_idx].panes.swap(from, to);
            let active = self.terminal_tabs[tab_idx].active_pane;
            if active == from {
                self.terminal_tabs[tab_idx].active_pane = to;
            } else if active == to {
                self.terminal_tabs[tab_idx].active_pane = from;
            }
            self.log_debug(format!(
                "bar_swap: {from}(uid={from_uid}) <-> {to}(uid={to_uid})"
            ));
        }
        if let Some(close_idx) = pending_close {
            let before = self.terminal_tabs[tab_idx].panes.len();
            self.terminal_tabs[tab_idx].close_pane(close_idx);
            self.log_debug(format!(
                "bar_close: pane {close_idx}, {before} -> {} panes",
                self.terminal_tabs[tab_idx].panes.len()
            ));
            // Cancel rename if it was for the closed pane
            if self.renaming_pane == Some((tab_idx, close_idx)) {
                self.renaming_pane = None;
            }
        }

        // Reserve the full grid area so egui knows it's used
        let grid_rect = egui::Rect::from_min_size(
            origin,
            egui::vec2(total_width, rows as f32 * (pane_height + gap) - gap),
        );
        ui.allocate_rect(grid_rect, egui::Sense::hover());
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
                        .add_enabled(self.terminal_tabs.len() > 1, egui::Button::new("Close"))
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
            let plus_btn = Self::tab_plus_button(ui, palette);
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

        // ── App-level paste detection (runs regardless of pane focus state) ──
        {
            // CMD_V_PRESSED is set by the low-level NSEvent monitor in install_paste_monitor().
            // It fires even when macOS suppresses the Cmd+V key event from reaching egui
            // (which happens when the clipboard contains only image data).
            let nsevent_cmd_v = CMD_V_PRESSED.swap(false, std::sync::atomic::Ordering::Relaxed);

            // Also watch for egui-level paste events (text paste path).
            let all_events = ctx.input(|i| i.events.clone());
            for e in &all_events {
                if let egui::Event::Paste(t) = e {
                    self.log_debug(format!("app_paste: Event::Paste text.len()={}", t.len()));
                }
            }
            if nsevent_cmd_v {
                self.log_debug("app_paste: NSEvent Cmd+V detected".to_owned());
            }

            // Only attempt image paste when Cmd+V came from the low-level monitor
            // AND there is no text in the clipboard (image-only case).
            if nsevent_cmd_v {
                let has_text = read_clipboard().map(|t| !t.is_empty()).unwrap_or(false);
                self.log_debug(format!("app_paste: has_text={has_text}"));

                if !has_text {
                    self.log_debug("app_paste: no text → save_clipboard_image".to_owned());
                    let mut img_logs: Vec<String> = Vec::new();
                    let ti = self.active_terminal;
                    let pi = self.terminal_tabs[ti].active_pane;
                    if let Some(img_path) = save_clipboard_image(&mut img_logs) {
                        let filename = img_path
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("image")
                            .to_owned();
                        // Shorten path for terminal display: replace $HOME prefix with ~
                        let path_str = if let Some(home) = std::env::var_os("HOME") {
                            let abs = img_path.to_string_lossy();
                            let home_str = home.to_string_lossy();
                            if abs.starts_with(home_str.as_ref()) {
                                format!("~{}", &abs[home_str.len()..])
                            } else {
                                abs.into_owned()
                            }
                        } else {
                            img_path.to_string_lossy().into_owned()
                        };
                        self.log_debug(format!("app_paste: saved → {path_str}"));
                        self.terminal_tabs[ti].panes[pi].paste_chip = Some(filename);
                        self.terminal_tabs[ti].panes[pi].paste_text(&path_str);
                    } else {
                        self.log_debug("app_paste: save_clipboard_image returned None".to_owned());
                    }
                    for msg in img_logs {
                        self.log_debug(msg);
                    }
                }
            }
        }

        let open_new_tab =
            ctx.input(|input| input.modifiers.command && input.key_pressed(egui::Key::T));
        let insert_checkbox =
            ctx.input(|input| input.modifiers.command && input.key_pressed(egui::Key::L));
        let split_pane =
            ctx.input(|input| input.modifiers.command && input.key_pressed(egui::Key::D));
        let close_pane = ctx.input(|input| {
            input.modifiers.command && input.modifiers.shift && input.key_pressed(egui::Key::D)
        });
        let next_pane = ctx
            .input(|input| input.modifiers.command && input.key_pressed(egui::Key::CloseBracket));
        let prev_pane =
            ctx.input(|input| input.modifiers.command && input.key_pressed(egui::Key::OpenBracket));
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
            input.modifiers.command && input.modifiers.shift && input.key_pressed(egui::Key::L)
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

        // Autosave notes after 1.5 s of inactivity
        let ti = self.active_terminal;
        if self.terminal_tabs[ti].notes_dirty {
            if let Some(t) = self.terminal_tabs[ti].last_type_time {
                if t.elapsed() > Duration::from_millis(1500) {
                    self.save_current_note_silent();
                }
            }
        }

        if open_new_tab {
            self.add_terminal_tab();
        }

        if split_pane && !close_pane {
            let uid = self.alloc_pane_uid();
            self.active_tab_mut().split_pane(uid);
            self.log_debug(format!(
                "split_pane: new pane_uid={uid}, total_panes={}",
                self.active_tab().panes.len()
            ));
        }

        if close_pane {
            let before = self.active_tab().panes.len();
            self.active_tab_mut().close_active_pane();
            self.log_debug(format!(
                "close_pane: {before} -> {} panes",
                self.active_tab().panes.len()
            ));
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
            let ti = self.active_terminal;
            Self::insert_checkbox_line(&mut self.terminal_tabs[ti].notes_markdown);
            self.terminal_tabs[ti].editing_notes = true;
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

                // Draw "StickyTerminal" centered over the bar (painted before layout so it's behind controls)
                ui.painter().text(
                    ui.max_rect().center(),
                    egui::Align2::CENTER_CENTER,
                    "StickyTerminal",
                    egui::FontId::proportional(13.0),
                    palette.muted_text,
                );

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
                        Self::toggle_fullscreen(ctx);
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
                                egui::RichText::new("|||")
                                    .size(11.0)
                                    .color(sidebar_icon_color),
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
                            egui::RichText::new("Help")
                                .size(12.0)
                                .color(palette.muted_text),
                            |ui| {
                                let logs_label = if self.show_debug {
                                    "Hide Logs"
                                } else {
                                    "View Logs"
                                };
                                if ui.selectable_label(self.show_debug, logs_label).clicked() {
                                    self.show_debug = !self.show_debug;
                                    ui.close();
                                }
                            },
                        );

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
                let (switch_to, close_tab, rename_tab, move_tab) = self.render_tab_bar(ui, palette);

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
                    let ti = self.active_terminal;
                    let mut choose_folder = false;
                    let mut open_note = false;
                    let mut new_note = false;
                    let mut save_note = false;
                    let mut open_recent_note: Option<PathBuf> = None;
                    let mut text_edit_changed = false;
                    let note_text = self.terminal_tabs[ti]
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
                        if self.terminal_tabs[ti].notes_dirty {
                            ui.label(
                                egui::RichText::new("\u{25cf}")
                                    .small()
                                    .color(palette.accent.linear_multiply(0.8)),
                            );
                        }

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let toggle_label = if self.terminal_tabs[ti].editing_notes {
                                "Preview"
                            } else {
                                "Edit"
                            };
                            if ui
                                .add(Self::note_action_button(toggle_label, palette))
                                .clicked()
                            {
                                self.terminal_tabs[ti].editing_notes =
                                    !self.terminal_tabs[ti].editing_notes;
                            }
                        });
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
                                    ui.spacing_mut().item_spacing.x = 6.0;

                                    if ui.add(Self::note_action_button("Save", palette)).clicked() {
                                        save_note = true;
                                    }
                                    if ui.add(Self::note_action_button("New", palette)).clicked() {
                                        new_note = true;
                                    }
                                    if ui.add(Self::note_action_button("Open", palette)).clicked() {
                                        open_note = true;
                                    }
                                    {
                                        let recent_copy = self.recent_notes.clone();
                                        if !recent_copy.is_empty() {
                                            egui::menu::menu_custom_button(
                                                ui,
                                                Self::note_action_button("Recent", palette),
                                                |ui| {
                                                    for path in &recent_copy {
                                                        let name = path
                                                            .file_name()
                                                            .and_then(|n| n.to_str())
                                                            .unwrap_or("?");
                                                        if ui
                                                            .selectable_label(false, name)
                                                            .clicked()
                                                        {
                                                            open_recent_note = Some(path.clone());
                                                            ui.close();
                                                        }
                                                    }
                                                },
                                            );
                                        }
                                    }
                                    if ui
                                        .add(Self::note_action_button("Folder", palette))
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
                        if self.terminal_tabs[ti].editing_notes {
                            egui::ScrollArea::vertical()
                                .max_height(content_height)
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    let r = ui.add(
                                        egui::TextEdit::multiline(
                                            &mut self.terminal_tabs[ti].notes_markdown,
                                        )
                                        .desired_width(ui.available_width())
                                        .desired_rows(40)
                                        .hint_text("Write markdown here. Cmd+L to add a checkbox."),
                                    );
                                    if r.changed() {
                                        text_edit_changed = true;
                                    }
                                });
                        } else {
                            let preview_changed = Self::render_markdown_preview(
                                ui,
                                &mut self.terminal_tabs[ti].notes_markdown,
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
                        egui::RichText::new(&self.terminal_tabs[ti].note_status)
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

                    if let Some(path) = open_recent_note {
                        let ti = self.active_terminal;
                        self.terminal_tabs[ti].current_note_file = Some(path);
                        self.load_current_note();
                    }

                    if text_edit_changed {
                        let ti = self.active_terminal;
                        self.terminal_tabs[ti].notes_dirty = true;
                        self.terminal_tabs[ti].last_type_time = Some(std::time::Instant::now());
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
