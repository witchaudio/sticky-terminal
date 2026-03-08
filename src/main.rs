#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use pulldown_cmark::{
    CodeBlockKind, Event, HeadingLevel, Options, Parser as MarkdownParser, Tag, TagEnd,
};
use rfd::FileDialog;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;
use vt100::Parser;

#[cfg(target_os = "macos")]
use objc::runtime::Object;
#[cfg(target_os = "macos")]
use objc::{class, msg_send, sel, sel_impl};

const WINDOW_WIDTH: f32 = 1180.0;
const WINDOW_HEIGHT: f32 = 760.0;
const TOP_BAR_HEIGHT: f32 = 34.0;
const MINIMIZED_HEIGHT: f32 = 40.0;
const SIDEBAR_DEFAULT_WIDTH: f32 = 340.0;
const TERMINAL_SCROLLBACK: usize = 5_000;

#[derive(Clone, Copy)]
struct ThemePalette {
    bg: egui::Color32,
    bar_bg: egui::Color32,
    border: egui::Color32,
    text: egui::Color32,
    muted_text: egui::Color32,
    selection: egui::Color32,
    terminal_bg: egui::Color32,
    sidebar_bg: egui::Color32,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum ThemePreset {
    Terminal,
    Black,
    Blue,
    Red,
}

#[derive(Clone, Copy)]
struct TerminalPoint {
    row: u16,
    col: u16,
}

struct TerminalTab {
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
}

struct GhostStickiesApp {
    notes_markdown: String,
    notes_root: Option<PathBuf>,
    current_note_file: Option<PathBuf>,
    note_status: String,
    notes_preview_mode: bool,
    theme: ThemePreset,
    minimized: bool,
    sidebar_open: bool,
    privacy_mode: bool,
    startup_tasks_run: bool,
    applied_privacy_mode: Option<bool>,
    next_tab_number: usize,
    terminal_tabs: Vec<TerminalTab>,
    active_terminal: usize,
    renaming_tab: Option<usize>,
    rename_buffer: String,
}

#[derive(Serialize, Deserialize, Default)]
struct AppConfig {
    notes_root: Option<PathBuf>,
    current_note_file: Option<PathBuf>,
    theme: ThemePreset,
}

#[derive(Clone, Copy)]
enum AppSymbol {
    Quit,
    Minimize,
    Privacy,
    NotesBack,
    NotesForward,
}

impl Default for ThemePreset {
    fn default() -> Self {
        Self::Black
    }
}

impl ThemePreset {
    const ALL: [Self; 4] = [Self::Terminal, Self::Black, Self::Blue, Self::Red];

    fn label(self) -> &'static str {
        match self {
            Self::Terminal => "Terminal",
            Self::Black => "Black",
            Self::Blue => "Blue",
            Self::Red => "Red",
        }
    }

    fn palette(self) -> ThemePalette {
        match self {
            Self::Terminal => ThemePalette {
                bg: egui::Color32::from_rgba_premultiplied(9, 13, 10, 214),
                terminal_bg: egui::Color32::from_rgba_premultiplied(10, 13, 11, 242),
                sidebar_bg: egui::Color32::from_rgba_premultiplied(16, 22, 18, 228),
                bar_bg: egui::Color32::from_rgba_premultiplied(17, 22, 18, 240),
                border: egui::Color32::from_rgba_premultiplied(90, 220, 150, 82),
                text: egui::Color32::from_rgb(168, 255, 196),
                muted_text: egui::Color32::from_rgb(108, 170, 126),
                selection: egui::Color32::from_rgba_premultiplied(44, 104, 70, 150),
            },
            Self::Black => ThemePalette {
                bg: egui::Color32::from_rgba_premultiplied(18, 18, 20, 224),
                terminal_bg: egui::Color32::from_rgba_premultiplied(15, 15, 17, 244),
                sidebar_bg: egui::Color32::from_rgba_premultiplied(24, 24, 28, 232),
                bar_bg: egui::Color32::from_rgba_premultiplied(24, 24, 28, 244),
                border: egui::Color32::from_rgba_premultiplied(230, 230, 236, 66),
                text: egui::Color32::from_rgb(242, 242, 246),
                muted_text: egui::Color32::from_rgb(160, 160, 168),
                selection: egui::Color32::from_rgba_premultiplied(100, 110, 130, 130),
            },
            Self::Blue => ThemePalette {
                bg: egui::Color32::from_rgba_premultiplied(14, 24, 42, 224),
                terminal_bg: egui::Color32::from_rgba_premultiplied(13, 18, 34, 244),
                sidebar_bg: egui::Color32::from_rgba_premultiplied(20, 32, 54, 232),
                bar_bg: egui::Color32::from_rgba_premultiplied(20, 30, 50, 244),
                border: egui::Color32::from_rgba_premultiplied(120, 175, 255, 84),
                text: egui::Color32::from_rgb(215, 234, 255),
                muted_text: egui::Color32::from_rgb(143, 173, 214),
                selection: egui::Color32::from_rgba_premultiplied(66, 110, 185, 148),
            },
            Self::Red => ThemePalette {
                bg: egui::Color32::from_rgba_premultiplied(40, 14, 16, 224),
                terminal_bg: egui::Color32::from_rgba_premultiplied(28, 10, 12, 244),
                sidebar_bg: egui::Color32::from_rgba_premultiplied(48, 18, 20, 232),
                bar_bg: egui::Color32::from_rgba_premultiplied(46, 18, 20, 244),
                border: egui::Color32::from_rgba_premultiplied(255, 130, 130, 92),
                text: egui::Color32::from_rgb(255, 220, 220),
                muted_text: egui::Color32::from_rgb(208, 144, 144),
                selection: egui::Color32::from_rgba_premultiplied(170, 70, 70, 148),
            },
        }
    }
}

impl TerminalTab {
    fn new(number: usize, cwd: PathBuf) -> Self {
        let rows = 28;
        let cols = 120;

        Self {
            title: format!("Tab {number}"),
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
            command.arg("-i");
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

impl Drop for TerminalTab {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
        }
    }
}

impl Default for GhostStickiesApp {
    fn default() -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

        Self {
            notes_markdown: "# TODO\n- [ ] Keep this shell usable for Codex CLI\n- [ ] Add quick project tabs\n- [ ] Save notes between sessions\n\n## Notes\nWrite markdown on the left.\nUse the right side like a real terminal.".to_owned(),
            notes_root: None,
            current_note_file: None,
            note_status: "Set your StickyTerminal notes folder to start saving notes.".to_owned(),
            notes_preview_mode: false,
            theme: ThemePreset::default(),
            minimized: false,
            sidebar_open: false,
            privacy_mode: false,
            startup_tasks_run: false,
            applied_privacy_mode: None,
            next_tab_number: 2,
            terminal_tabs: vec![TerminalTab::new(1, cwd)],
            active_terminal: 0,
            renaming_tab: None,
            rename_buffer: String::new(),
        }
    }
}

impl GhostStickiesApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        egui_extras::install_image_loaders(&cc.egui_ctx);
        let mut app = Self::default();
        app.load_saved_config();
        app
    }

    fn symbol_image(symbol: AppSymbol) -> egui::Image<'static> {
        let image = match symbol {
            AppSymbol::Quit => egui::include_image!("../assets/x.square.fill.png"),
            AppSymbol::Minimize => {
                egui::include_image!("../assets/arrow.down.right.and.arrow.up.left.png")
            }
            AppSymbol::Privacy => egui::include_image!("../assets/eye.circle.png"),
            AppSymbol::NotesBack => egui::include_image!("../assets/arrow.backward.png"),
            AppSymbol::NotesForward => egui::include_image!("../assets/arrow.right.png"),
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
            self.note_status = "Choose your StickyTerminal folder first.".to_owned();
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
            self.note_status = "Pick a note inside your StickyTerminal folder.".to_owned();
            return;
        }

        self.current_note_file = Some(file);
        self.load_current_note();
    }

    fn save_current_note(&mut self) {
        let Some(path) = self.note_file_path() else {
            self.note_status = "Choose your StickyTerminal folder and a note first.".to_owned();
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
            self.note_status = "Choose your StickyTerminal folder first.".to_owned();
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
            self.note_status = "Save the note inside your StickyTerminal folder.".to_owned();
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

    fn markdown_text_format(
        ui: &egui::Ui,
        color: egui::Color32,
        size: f32,
        monospace: bool,
        _strong: bool,
        italic: bool,
        underline: bool,
    ) -> egui::TextFormat {
        let mut font_id = if monospace {
            egui::TextStyle::Monospace.resolve(ui.style())
        } else {
            egui::TextStyle::Body.resolve(ui.style())
        };
        font_id.size = size;

        egui::TextFormat {
            font_id,
            color,
            italics: italic,
            underline: if underline {
                egui::Stroke::new(1.0, color)
            } else {
                egui::Stroke::NONE
            },
            ..Default::default()
        }
    }

    fn append_markdown_text(
        ui: &egui::Ui,
        job: &mut egui::text::LayoutJob,
        text: &str,
        color: egui::Color32,
        heading: Option<HeadingLevel>,
        strong: bool,
        italic: bool,
        monospace: bool,
        underline: bool,
    ) {
        let size = match heading {
            Some(HeadingLevel::H1) => 28.0,
            Some(HeadingLevel::H2) => 24.0,
            Some(HeadingLevel::H3) => 21.0,
            Some(HeadingLevel::H4) => 19.0,
            Some(HeadingLevel::H5) => 17.0,
            Some(HeadingLevel::H6) => 16.0,
            None => {
                if monospace {
                    egui::TextStyle::Monospace.resolve(ui.style()).size
                } else {
                    egui::TextStyle::Body.resolve(ui.style()).size
                }
            }
        };

        let mut format =
            Self::markdown_text_format(ui, color, size, monospace, strong, italic, underline);
        if strong {
            format.font_id.size += 1.0;
        }

        job.append(text, 0.0, format);
    }

    fn flush_markdown_job(ui: &mut egui::Ui, job: &mut egui::text::LayoutJob) {
        if !job.text.is_empty() {
            let output = std::mem::take(job);
            ui.label(output);
        }
    }

    fn render_markdown(ui: &mut egui::Ui, markdown: &str, color: egui::Color32) {
        let mut options = Options::empty();
        options.insert(Options::ENABLE_TABLES);
        options.insert(Options::ENABLE_TASKLISTS);
        options.insert(Options::ENABLE_STRIKETHROUGH);

        let parser = MarkdownParser::new_ext(markdown, options);
        let mut job = egui::text::LayoutJob::default();
        let mut heading = None;
        let mut strong = false;
        let mut italic = false;
        let mut in_code_block = false;
        let mut current_link: Option<String> = None;
        let mut list_depth: usize = 0;

        for event in parser {
            match event {
                Event::Start(tag) => match tag {
                    Tag::Paragraph => {}
                    Tag::Heading { level, .. } => {
                        Self::flush_markdown_job(ui, &mut job);
                        heading = Some(level);
                    }
                    Tag::BlockQuote(_) => {
                        Self::flush_markdown_job(ui, &mut job);
                        Self::append_markdown_text(
                            ui, &mut job, "│ ", color, None, false, false, false, false,
                        );
                    }
                    Tag::CodeBlock(kind) => {
                        Self::flush_markdown_job(ui, &mut job);
                        in_code_block = true;
                        let label = match kind {
                            CodeBlockKind::Fenced(name) if !name.is_empty() => {
                                format!("```{name}\n")
                            }
                            _ => "```\n".to_owned(),
                        };
                        Self::append_markdown_text(
                            ui, &mut job, &label, color, None, false, false, true, false,
                        );
                    }
                    Tag::List(_) => {
                        list_depth += 1;
                    }
                    Tag::Item => {
                        if !job.text.is_empty() {
                            Self::flush_markdown_job(ui, &mut job);
                        }
                        let indent = "  ".repeat(list_depth.saturating_sub(1));
                        let bullet = format!("{indent}• ");
                        Self::append_markdown_text(
                            ui, &mut job, &bullet, color, None, false, false, false, false,
                        );
                    }
                    Tag::Emphasis => italic = true,
                    Tag::Strong => strong = true,
                    Tag::Link { dest_url, .. } => current_link = Some(dest_url.to_string()),
                    _ => {}
                },
                Event::End(tag) => match tag {
                    TagEnd::Paragraph => {
                        Self::flush_markdown_job(ui, &mut job);
                        ui.add_space(6.0);
                    }
                    TagEnd::Heading(_) => {
                        Self::flush_markdown_job(ui, &mut job);
                        ui.add_space(6.0);
                        heading = None;
                    }
                    TagEnd::BlockQuote(_) => {
                        Self::flush_markdown_job(ui, &mut job);
                        ui.add_space(4.0);
                    }
                    TagEnd::CodeBlock => {
                        Self::append_markdown_text(
                            ui, &mut job, "\n```", color, None, false, false, true, false,
                        );
                        Self::flush_markdown_job(ui, &mut job);
                        ui.add_space(6.0);
                        in_code_block = false;
                    }
                    TagEnd::List(_) => {
                        list_depth = list_depth.saturating_sub(1);
                        if !job.text.is_empty() {
                            Self::flush_markdown_job(ui, &mut job);
                        }
                    }
                    TagEnd::Item => {
                        Self::flush_markdown_job(ui, &mut job);
                    }
                    TagEnd::Emphasis => italic = false,
                    TagEnd::Strong => strong = false,
                    TagEnd::Link => current_link = None,
                    _ => {}
                },
                Event::Text(text) => {
                    Self::append_markdown_text(
                        ui,
                        &mut job,
                        &text,
                        color,
                        heading,
                        strong,
                        italic,
                        in_code_block,
                        current_link.is_some(),
                    );
                }
                Event::Code(text) => {
                    Self::append_markdown_text(
                        ui, &mut job, &text, color, heading, strong, italic, true, false,
                    );
                }
                Event::SoftBreak | Event::HardBreak => {
                    Self::append_markdown_text(
                        ui,
                        &mut job,
                        "\n",
                        color,
                        heading,
                        strong,
                        italic,
                        in_code_block,
                        false,
                    );
                }
                Event::Rule => {
                    Self::flush_markdown_job(ui, &mut job);
                    ui.separator();
                    ui.add_space(6.0);
                }
                Event::Html(html) | Event::InlineHtml(html) => {
                    Self::append_markdown_text(
                        ui, &mut job, &html, color, heading, false, false, true, false,
                    );
                }
                Event::TaskListMarker(checked) => {
                    let marker = if checked { "[x] " } else { "[ ] " };
                    Self::append_markdown_text(
                        ui, &mut job, marker, color, heading, false, false, true, false,
                    );
                }
                _ => {}
            }
        }

        Self::flush_markdown_job(ui, &mut job);
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

                // NSWindowSharingNone = 0, NSWindowSharingReadOnly = 1.
                let sharing_type = if enabled { 0isize } else { 1isize };
                let _: () = msg_send![window, setSharingType: sharing_type];
            }
        }
    }

    #[cfg(not(target_os = "macos"))]
    fn apply_macos_share_privacy(&self, _enabled: bool) {}

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

    fn add_terminal_tab(&mut self) {
        let cwd = self.active_tab().cwd.clone();
        let number = self.next_tab_number;
        self.next_tab_number += 1;
        self.terminal_tabs.push(TerminalTab::new(number, cwd));
        self.active_terminal = self.terminal_tabs.len().saturating_sub(1);
        self.renaming_tab = None;
        self.rename_buffer.clear();
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

    fn render_terminal(&mut self, ui: &mut egui::Ui, palette: ThemePalette, ctx: &egui::Context) {
        let frame = egui::Frame::NONE
            .fill(palette.terminal_bg)
            .stroke(egui::Stroke::new(1.0, palette.border))
            .corner_radius(egui::CornerRadius::same(12))
            .inner_margin(egui::Margin::same(12));

        frame.show(ui, |ui| {
            let terminal_id =
                ui.make_persistent_id(("live_terminal_surface", self.active_terminal));
            let size = ui.available_size();
            let (rect, _response) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
            let response = ui.interact(rect, terminal_id, egui::Sense::click_and_drag());

            let font_id = egui::TextStyle::Monospace.resolve(ui.style());
            let measure =
                ui.painter()
                    .layout_no_wrap("W".to_owned(), font_id.clone(), palette.text);
            let char_width = measure.size().x.max(8.0);
            let row_height = measure.size().y.max(16.0) + 2.0;
            let inner_padding = 4.0;

            let rows = ((rect.height() - inner_padding * 2.0) / row_height).floor() as u16;
            let cols = ((rect.width() - inner_padding * 2.0) / char_width).floor() as u16;

            {
                let tab = self.active_tab_mut();
                tab.resize(rows, cols);

                if response.clicked() {
                    response.request_focus();
                    tab.selection = None;
                }

                if response.drag_started() {
                    response.request_focus();
                    if let Some(pointer_pos) = response.interact_pointer_pos() {
                        let point = tab.cell_from_pos(
                            rect,
                            pointer_pos,
                            char_width,
                            row_height,
                            inner_padding,
                        );
                        tab.selection = Some((point, point));
                    }
                }

                if response.dragged() {
                    if let Some(pointer_pos) = response.interact_pointer_pos() {
                        let point = tab.cell_from_pos(
                            rect,
                            pointer_pos,
                            char_width,
                            row_height,
                            inner_padding,
                        );
                        if let Some((anchor, _)) = tab.selection {
                            tab.selection = Some((anchor, point));
                        }
                    }
                }

                if response.hovered() {
                    let scroll_delta = ctx.input(|input| input.smooth_scroll_delta.y);
                    if scroll_delta.abs() > f32::EPSILON {
                        let rows_delta = (scroll_delta / row_height).round() as i32;
                        if rows_delta != 0 {
                            tab.adjust_scrollback(rows_delta);
                        }
                    }
                }

                tab.has_focus = ui.memory(|memory| memory.has_focus(terminal_id));
                if tab.has_focus {
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
                tab.handle_input(ctx);
            }

            let painter = ui.painter_at(rect);
            let tab = self.active_tab();
            let screen = tab.parser.screen();
            for row in 0..tab.rows {
                for col in 0..tab.cols {
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

                    if tab.cell_selected(row, col) {
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
                }
            }

            let (cursor_row, cursor_col) = screen.cursor_position();
            if tab.has_focus {
                let x = rect.left() + inner_padding + cursor_col as f32 * char_width;
                let y = rect.top() + inner_padding + cursor_row as f32 * row_height;
                let cursor_rect = egui::Rect::from_min_size(
                    egui::pos2(x, y),
                    egui::vec2(2.0, (row_height - 2.0).max(12.0)),
                );
                painter.rect_filled(cursor_rect, egui::CornerRadius::same(1), palette.text);
            } else {
                painter.text(
                    rect.right_top() + egui::vec2(-10.0, 6.0),
                    egui::Align2::RIGHT_TOP,
                    "click to type",
                    egui::TextStyle::Small.resolve(ui.style()),
                    palette.muted_text,
                );
            }
        });
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
        let mut received_output = false;
        for tab in &mut self.terminal_tabs {
            tab.ensure_started();
            if tab.drain_output() {
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

        let palette = self.theme.palette();

        let mut style = (*ctx.style()).clone();
        style.visuals.window_fill = palette.bg;
        style.visuals.panel_fill = palette.bg;
        style.visuals.extreme_bg_color = palette.bg;
        style.visuals.selection.bg_fill = palette.selection;
        style.visuals.widgets.active.bg_fill = palette.bar_bg;
        style.visuals.widgets.hovered.bg_fill = palette.bar_bg;
        style.visuals.widgets.inactive.bg_fill = palette.sidebar_bg;
        style.visuals.widgets.noninteractive.bg_fill = palette.sidebar_bg;
        style.visuals.widgets.active.fg_stroke.color = palette.text;
        style.visuals.widgets.hovered.fg_stroke.color = palette.text;
        style.visuals.widgets.inactive.fg_stroke.color = palette.text;
        style.visuals.override_text_color = Some(palette.text);
        style.visuals.window_corner_radius = egui::CornerRadius::same(14);
        ctx.set_style(style);

        let mut start_drag = false;
        let mut quit_requested = false;
        let mut restart_clicked = false;
        let mut privacy_toggled = false;
        let mut theme_changed = None;

        egui::TopBottomPanel::top("top_bar")
            .exact_height(TOP_BAR_HEIGHT)
            .frame(
                egui::Frame::NONE
                    .fill(palette.bar_bg)
                    .stroke(egui::Stroke::new(1.0, palette.border))
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
                    if Self::symbol_button(ui, AppSymbol::Quit, "Quit", false).clicked() {
                        quit_requested = true;
                    }

                    if Self::symbol_button(ui, AppSymbol::Minimize, "Minimize", self.minimized)
                        .clicked()
                    {
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

                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new("StickyTerminal")
                            .strong()
                            .color(palette.text),
                    );

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("↻").on_hover_text("Restart shell").clicked() {
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

                        ui.menu_button("Theme", |ui| {
                            for preset in ThemePreset::ALL {
                                if ui
                                    .selectable_label(self.theme == preset, preset.label())
                                    .clicked()
                                {
                                    theme_changed = Some(preset);
                                    ui.close();
                                }
                            }
                        });
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
            self.active_tab_mut().restart();
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

        if self.sidebar_open {
            egui::SidePanel::left("notes_sidebar")
                .resizable(true)
                .default_width(SIDEBAR_DEFAULT_WIDTH)
                .width_range(250.0..=560.0)
                .frame(
                    egui::Frame::NONE
                        .fill(palette.sidebar_bg)
                        .stroke(egui::Stroke::new(1.0, palette.border))
                        .inner_margin(egui::Margin::same(10)),
                )
                .show(ctx, |ui| {
                    let mut choose_folder = false;
                    let mut open_note = false;
                    let mut save_note = false;
                    let mut new_note = false;

                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("Notes")
                                .strong()
                                .size(18.0)
                                .color(palette.text),
                        );

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if Self::symbol_button(ui, AppSymbol::NotesBack, "Hide notes", false)
                                .clicked()
                            {
                                self.sidebar_open = false;
                            }
                        });
                    });
                    ui.label(
                        egui::RichText::new(
                            "One markdown note, saved inside your StickyTerminal folder",
                        )
                        .small()
                        .color(palette.muted_text),
                    );
                    ui.add_space(8.0);

                    ui.label(
                        egui::RichText::new("Notes folder")
                            .small()
                            .color(palette.muted_text),
                    );
                    ui.horizontal(|ui| {
                        let folder_text = self
                            .notes_root
                            .as_ref()
                            .map(|path| path.display().to_string())
                            .unwrap_or_else(|| "No folder selected".to_owned());
                        ui.label(egui::RichText::new(folder_text).small().color(palette.text));
                        if ui.button("Choose").clicked() {
                            choose_folder = true;
                        }
                    });

                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new("Current note")
                            .small()
                            .color(palette.muted_text),
                    );
                    ui.horizontal(|ui| {
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
                        ui.label(egui::RichText::new(note_text).small().color(palette.text));
                        if ui.button("Open").clicked() {
                            open_note = true;
                        }
                        if ui.button("New").clicked() {
                            new_note = true;
                        }
                        if ui.button("Save").clicked() {
                            save_note = true;
                        }
                    });

                    if let Some(path) = self.note_file_path() {
                        ui.label(
                            egui::RichText::new(path.display().to_string())
                                .small()
                                .color(palette.muted_text),
                        );
                    }

                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new(&self.note_status)
                            .small()
                            .color(palette.muted_text),
                    );
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut self.notes_preview_mode, false, "Write");
                        ui.selectable_value(&mut self.notes_preview_mode, true, "Preview");
                    });
                    ui.add_space(8.0);

                    if self.notes_preview_mode {
                        egui::Frame::NONE
                            .fill(palette.terminal_bg)
                            .stroke(egui::Stroke::new(1.0, palette.border))
                            .corner_radius(egui::CornerRadius::same(10))
                            .inner_margin(egui::Margin::same(10))
                            .show(ui, |ui| {
                                egui::ScrollArea::vertical()
                                    .auto_shrink([false, false])
                                    .show(ui, |ui| {
                                        Self::render_markdown(
                                            ui,
                                            &self.notes_markdown,
                                            palette.text,
                                        );
                                    });
                            });
                    } else {
                        let editor = egui::TextEdit::multiline(&mut self.notes_markdown)
                            .font(egui::TextStyle::Monospace)
                            .desired_width(f32::INFINITY)
                            .desired_rows(24)
                            .hint_text("Write markdown here, then press Save.");
                        ui.add_sized([ui.available_width(), ui.available_height()], editor);
                    }

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
        } else {
            egui::SidePanel::left("notes_tab_collapsed")
                .exact_width(28.0)
                .frame(
                    egui::Frame::NONE
                        .fill(palette.bar_bg)
                        .stroke(egui::Stroke::new(1.0, palette.border)),
                )
                .show(ctx, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.add_space(8.0);
                        if Self::symbol_button(ui, AppSymbol::NotesForward, "Show notes", false)
                            .clicked()
                        {
                            self.sidebar_open = true;
                        }
                    });
                });
        }

        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(palette.bg)
                    .stroke(egui::Stroke::new(1.0, palette.border))
                    .inner_margin(egui::Margin::same(10)),
            )
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    let mut switch_to = None;
                    let mut move_tab = None;
                    let mut close_tab = None;
                    let mut rename_tab = None;

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

                        let response = ui.add(
                            egui::Button::new(&self.terminal_tabs[index].title)
                                .selected(selected)
                                .sense(egui::Sense::click_and_drag()),
                        );

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

                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if self.active_tab().parser.screen().scrollback() > 0 {
                        ui.label(
                            egui::RichText::new(format!(
                                "History {}",
                                self.active_tab().parser.screen().scrollback()
                            ))
                            .small()
                            .color(palette.muted_text),
                        );
                    }
                    if self.active_tab().selection_exists() && ui.button("Copy").clicked() {
                        let _ = self.active_tab().copy_selection(ctx);
                    }
                });
                ui.add_space(10.0);

                self.render_terminal(ui, palette, ctx);
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
