//! Main settings window, UI-thread state, messages, timers, and interactions.

use crate::settings_app::{
    layout::{self, Rect},
    model::{self, Page, Row, WidgetRow},
    paint, remote,
    work::{self, WorkResult, WriteJob},
};
use serde_json::{json, Value};
use std::cell::Cell;
use std::collections::HashMap;
use std::sync::{mpsc::Sender, Arc, RwLock};
use std::time::{Duration, Instant};
use taskbar_dock::{
    control,
    describe::{self, Kind},
    registry,
    text_edit::TextEdit,
    theme::{self, Theme},
    wide,
};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::{Dwm::*, Gdi::*};
use windows_sys::Win32::System::DataExchange::COPYDATASTRUCT;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::{CreateMutexW, ReleaseMutex};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

/// Worker result message.
pub const WM_WORK: u32 = WM_APP + 11;
/// Copied dockctl command message.
pub const WM_REMOTE: u32 = WM_APP + 12;
const WM_LOAD_LOOK: u32 = WM_APP + 13;
const TIMER_FILE: usize = 1;
const DATA_TAG: usize = 0x444B_5350;
const WM_MOUSE_LEAVE: u32 = 0x02A3;
thread_local! { static MAIN: Cell<HWND> = const { Cell::new(0) }; }

/// Starts the settings application, forwarding to an existing instance when present.
pub fn run(initial: Page) -> i32 {
    unsafe {
        let mutex = CreateMutexW(
            std::ptr::null(),
            1,
            wide("Local\\TaskbarDock.SettingsApp").as_ptr(),
        );
        if mutex == 0 {
            return 1;
        }
        if GetLastError() == ERROR_ALREADY_EXISTS {
            let result = forward(&page_name(&initial));
            CloseHandle(mutex);
            return if result { 0 } else { 1 };
        }
        let instance = GetModuleHandleW(std::ptr::null());
        let class = wide("TaskbarDockSettings");
        let title = wide("Taskbar widgets settings");
        let mut wc: WNDCLASSW = std::mem::zeroed();
        wc.style = CS_HREDRAW | CS_VREDRAW | CS_DBLCLKS;
        wc.lpfnWndProc = Some(window_proc);
        wc.hInstance = instance;
        wc.hCursor = LoadCursorW(0, IDC_ARROW);
        wc.lpszClassName = class.as_ptr();
        if RegisterClassW(&wc) == 0 && GetLastError() != ERROR_CLASS_ALREADY_EXISTS {
            CloseHandle(mutex);
            return 1;
        }
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            0,
            0,
            instance,
            Box::into_raw(Box::new(initial)) as _,
        );
        if hwnd == 0 {
            CloseHandle(mutex);
            return 1;
        }
        let dpi = windows_sys::Win32::UI::HiDpi::GetDpiForWindow(hwnd).max(96);
        let scale = dpi as f32 / 96.0;
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: layout::px(880.0, scale),
            bottom: layout::px(640.0, scale),
        };
        windows_sys::Win32::UI::HiDpi::AdjustWindowRectExForDpi(
            &mut rect,
            WS_OVERLAPPEDWINDOW,
            0,
            0,
            dpi,
        );
        SetWindowPos(
            hwnd,
            0,
            0,
            0,
            rect.right - rect.left,
            rect.bottom - rect.top,
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
        MAIN.with(|main| main.set(hwnd));
        if remote::create(hwnd) == 0 {
            DestroyWindow(hwnd);
            CloseHandle(mutex);
            return 1;
        }
        PostMessageW(hwnd, WM_LOAD_LOOK, 0, 0);
        ShowWindow(hwnd, SW_SHOW);
        UpdateWindow(hwnd);
        let mut message: MSG = std::mem::zeroed();
        while GetMessageW(&mut message, 0, 0, 0) > 0 {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        ReleaseMutex(mutex);
        CloseHandle(mutex);
        message.wParam as i32
    }
}

/// Finds the first instance and forwards its page request.
unsafe fn forward(page: &str) -> bool {
    let class = wide("TaskbarDockSettings");
    let until = Instant::now() + Duration::from_secs(3);
    let target = loop {
        let target = FindWindowW(class.as_ptr(), std::ptr::null());
        if target != 0 {
            break target;
        }
        if Instant::now() >= until {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    AllowSetForegroundWindow(u32::MAX);
    let bytes = page.as_bytes();
    let data = COPYDATASTRUCT {
        dwData: DATA_TAG,
        cbData: bytes.len() as u32,
        lpData: bytes.as_ptr() as *mut _,
    };
    let mut result = 0;
    SendMessageTimeoutW(
        target,
        WM_COPYDATA,
        0,
        &data as *const _ as LPARAM,
        SMTO_ABORTIFHUNG,
        2000,
        &mut result,
    );
    result == 1
}

/// Converts a selected page to its handover string.
fn page_name(page: &Page) -> String {
    match page {
        Page::Widgets => "widgets".into(),
        Page::Dock => "dock".into(),
        Page::Developer => "developer".into(),
        Page::Widget(name) => name.clone(),
    }
}

/// All mutable settings-window state; accessed only by the UI thread.
struct App {
    /// Main window handle.
    hwnd: HWND,
    /// Current page.
    page: Page,
    /// Registered entries with descriptions.
    entries: Vec<work::LoadedEntry>,
    /// Current widget rows.
    rows: Vec<Row>,
    /// Home page rows.
    widgets: Vec<WidgetRow>,
    /// Last status values posted to this UI thread.
    last_status: Vec<control::Status>,
    /// Parsed settings file snapshot.
    settings: Value,
    /// Current appearance theme.
    theme: Theme,
    /// Shared GDI fonts for the current appearance and DPI.
    fonts: paint::Fonts,
    /// Display scaling factor.
    scale: f32,
    /// Settings file error banner.
    banner: Option<String>,
    /// Save or action result badge.
    badge: Option<String>,
    /// Badge expiration, when transient.
    badge_until: Option<Instant>,
    /// Page scroll offset.
    scroll: i32,
    /// Focused row control index.
    focus: Option<usize>,
    /// Current navigation, row, or reset focus target.
    focus_item: Option<crate::settings_app::keys::Focus>,
    /// Open choice list and its highlighted option index.
    dropdown: Option<(usize, usize)>,
    /// Whether a secret value is temporarily visible under the pointer.
    reveal_secret: bool,
    /// Active single-line editor.
    edit: Option<(usize, TextEdit)>,
    /// Numeric slider currently held, with its uncommitted value.
    slider_drag: Option<(usize, f64, Rect)>,
    /// Loaded entry list shared with the status worker.
    status_entries: Arc<RwLock<Vec<registry::Entry>>>,
    /// Serialized writer job sender.
    writer: Sender<WriteJob>,
    /// Latest write sequence.
    sequence: u64,
    /// Latest accepted status round.
    status_round: u64,
    /// Whether the repeating status worker is already running.
    status_started: bool,
    /// Settings file modification time.
    mtime: Option<std::time::SystemTime>,
    /// Current page's nav labels and targets.
    nav: Vec<(String, Page)>,
    /// Deferred replies sent after their associated setting writes finish.
    pending_remote: HashMap<u64, (HWND, HWND, u64)>,
    /// Deferred process-action replies keyed by widget and action.
    pending_actions: HashMap<(String, String), (HWND, HWND, u64)>,
    /// Process actions currently running, by widget name.
    busy_actions: HashMap<String, String>,
    /// Pointer position used for hover highlights.
    hover: Option<(i32, i32)>,
}

impl App {
    /// Creates the UI model and starts background loading after the window exists.
    fn new(hwnd: HWND, initial: Page) -> Self {
        let fallback = taskbar_dock::style::Style::default();
        let theme = Theme::with(false, &fallback.font, None);
        let scale = taskbar_dock::system::scale_for(hwnd);
        let fonts = paint::Fonts::new(&theme, scale);
        let writer = work::writer(hwnd);
        work::load_registry(hwnd);
        work::read_file(hwnd);
        unsafe {
            SetTimer(hwnd, TIMER_FILE, 1000, None);
        }
        Self {
            hwnd,
            page: initial,
            entries: Vec::new(),
            rows: Vec::new(),
            widgets: Vec::new(),
            last_status: Vec::new(),
            settings: json!({}),
            theme,
            fonts,
            scale,
            banner: None,
            badge: None,
            badge_until: None,
            scroll: 0,
            focus: None,
            focus_item: None,
            dropdown: None,
            reveal_secret: false,
            edit: None,
            slider_drag: None,
            status_entries: Arc::new(RwLock::new(Vec::new())),
            writer,
            sequence: 0,
            status_round: 0,
            status_started: false,
            mtime: taskbar_dock::settings::mtime(),
            nav: Vec::new(),
            pending_remote: HashMap::new(),
            pending_actions: HashMap::new(),
            busy_actions: HashMap::new(),
            hover: None,
        }
    }

    /// Rebuilds nav items, using describe titles or registry names on failure.
    fn rebuild_nav(&mut self) {
        self.nav.clear();
        self.nav.push(("Widgets".into(), Page::Widgets));
        self.nav.push(("Dock".into(), Page::Dock));
        for item in &self.entries {
            let title = item
                .describe
                .as_ref()
                .map(|desc| desc.title.clone())
                .unwrap_or_else(|_| item.entry.name.clone());
            self.nav
                .push((title, Page::Widget(item.entry.name.clone())));
        }
        self.nav.push(("Developer".into(), Page::Developer));
    }

    /// Measures one card's wrapped help and error text with the cached UI fonts.
    fn measured_card_height(&self, row: &Row, width: i32) -> i32 {
        let pad = layout::px(theme::app::CARD_PADDING, self.scale);
        let mut label_width = width - 2 * pad;
        if !layout::full_line(row) {
            label_width -= layout::px(
                theme::app::CONTROL_WIDTH + theme::app::LABEL_GAP,
                self.scale,
            );
            label_width -= layout::px(
                theme::app::RESET_WIDTH + theme::app::CONTROL_GAP,
                self.scale,
            );
        }
        let help_height = self
            .fonts
            .measure_height(&row.setting.help, label_width, false);
        let error_height = row.error.as_deref().map_or(0, |error| {
            self.fonts.measure_height(error, width - 2 * pad, true)
        });
        layout::card_height_measured(row, self.scale, help_height, error_height)
    }

    /// Measures a row's error line with the cached small font.
    fn measured_error_height(&self, row: &Row, width: i32) -> i32 {
        let pad = layout::px(theme::app::CARD_PADDING, self.scale);
        row.error.as_deref().map_or(0, |error| {
            self.fonts.measure_height(error, width - 2 * pad, true)
        })
    }

    /// Rebuilds rows from current file values and the current page's description.
    fn rebuild_rows(&mut self) {
        self.rows.clear();
        if let Page::Widget(name) = &self.page {
            if let Some(item) = self.entries.iter().find(|item| item.entry.name == *name) {
                if let Ok(desc) = &item.describe {
                    let current = self
                        .settings
                        .get(name)
                        .cloned()
                        .unwrap_or_else(|| json!({}));
                    self.rows = model::widget_rows(
                        name,
                        &desc.settings,
                        &current,
                        &desc.defaults,
                        taskbar_dock::startup::is_on(name),
                    );
                }
            }
        }
        self.scroll = 0;
    }

    /// Rebuilds the home rows from registry, current file values, and process status.
    fn rebuild_widgets(&mut self) {
        self.widgets = self
            .entries
            .iter()
            .map(|item| {
                let desc = item.describe.as_ref().ok();
                let status = self
                    .last_status
                    .iter()
                    .find(|status| status.name == item.entry.name);
                WidgetRow {
                    name: item.entry.name.clone(),
                    title: desc
                        .map(|d| d.title.clone())
                        .unwrap_or_else(|| item.entry.name.clone()),
                    description: desc.map(|d| d.description.clone()).unwrap_or_default(),
                    status: status
                        .map(|s| state_name(s.state).to_string())
                        .unwrap_or_else(|| "stopped".into()),
                    pid: status.and_then(|s| s.pid),
                    reason: desc
                        .is_none()
                        .then(|| item.describe.as_ref().err().cloned().unwrap_or_default())
                        .or_else(|| status.and_then(|s| s.reason.clone())),
                    show_on_taskbar: self
                        .settings
                        .get(&item.entry.name)
                        .and_then(|v| v.get("show_on_taskbar"))
                        .and_then(Value::as_bool)
                        .unwrap_or(true),
                }
            })
            .collect();
    }

    /// Applies one worker result and repaints after updating UI state.
    fn apply_work(&mut self, result: WorkResult) {
        match result {
            WorkResult::Registry(entries) => {
                if let Ok(mut shared) = self.status_entries.write() {
                    *shared = entries.iter().map(|item| item.entry.clone()).collect();
                }
                if !self.status_started {
                    work::status_loop(self.hwnd, self.status_entries.clone());
                    self.status_started = true;
                }
                self.entries = entries;
                self.rebuild_nav();
                self.rebuild_rows();
            }
            WorkResult::Status { round, statuses } if round > self.status_round => {
                self.status_round = round;
                self.last_status = statuses;
                self.rebuild_widgets();
            }
            WorkResult::Status { .. } => {}
            WorkResult::File(Ok(value)) => {
                let registry_changed = self.settings.get("widgets") != value.get("widgets");
                self.banner = None;
                self.settings = value;
                self.mtime = taskbar_dock::settings::mtime();
                if registry_changed {
                    work::load_registry(self.hwnd);
                }
                if self.rows.is_empty() {
                    self.rebuild_rows();
                } else {
                    for row in &mut self.rows {
                        row.enabled = true;
                        if row.key == "startup" {
                            let _ =
                                row.reload(Value::Bool(taskbar_dock::startup::is_on(&row.section)));
                        } else {
                            let current = self
                                .settings
                                .get(&row.section)
                                .and_then(|section| describe::value_at(section, &row.key))
                                .cloned()
                                .unwrap_or(Value::Null);
                            row.reload(current);
                        }
                    }
                }
                self.rebuild_widgets();
            }
            WorkResult::File(Err(_)) => {
                self.banner = Some("settings.json has an error; fix it in Notepad".into());
                for row in &mut self.rows {
                    row.enabled = false;
                }
            }
            WorkResult::Write {
                sequence,
                section,
                key,
                result,
                followed,
            } => {
                if let Some(row) = self
                    .rows
                    .iter_mut()
                    .find(|row| row.section == section && row.key == key)
                {
                    let value = result.as_ref().ok().cloned();
                    let error = result.as_ref().err().cloned();
                    if row.apply_result(sequence, value, error.clone()) {
                        let title = self
                            .entries
                            .iter()
                            .find(|item| item.entry.name == section)
                            .and_then(|item| item.describe.as_ref().ok())
                            .map(|desc| desc.title.as_str())
                            .unwrap_or(&section);
                        self.badge = Some(error.map_or_else(
                            || {
                                followed.err().map_or("Saved".into(), |_| {
                                    format!("Saved; {title} hasn't confirmed")
                                })
                            },
                            |e| format!("Not saved: {e}"),
                        ));
                        self.badge_until = self
                            .badge
                            .as_deref()
                            .filter(|text| *text == "Saved")
                            .map(|_| Instant::now() + Duration::from_secs(2));
                    }
                }
                work::read_file(self.hwnd);
                if let Some((sender, source, id)) = self.pending_remote.remove(&sequence) {
                    unsafe {
                        UpdateWindow(self.hwnd);
                    }
                    remote::reply(
                        sender,
                        source,
                        id,
                        &json!({"ok":result.is_ok(),"key":key,"error":result.err()}),
                    );
                }
            }
            WorkResult::Action {
                name,
                action,
                result,
            } => {
                let success = result.is_ok();
                let error = result.as_ref().err().cloned();
                self.busy_actions.remove(&name);
                self.badge = Some(result.map_or_else(
                    |e| format!("Couldn't {action}: {e}"),
                    |_| match action.as_str() {
                        "start" => "Started".into(),
                        "quit" => "Quit".into(),
                        _ => "Restarted".into(),
                    },
                ));
                self.badge_until = Some(Instant::now() + Duration::from_secs(2));
                let entries = self.status_entries.clone();
                let hwnd = self.hwnd;
                let round = self.status_round.wrapping_add(1);
                std::thread::spawn(move || {
                    let statuses =
                        control::status_all(&entries.read().map(|e| e.clone()).unwrap_or_default());
                    post_result(hwnd, WorkResult::Status { round, statuses });
                });
                if let Some((sender, source, id)) = self.pending_actions.remove(&(name, action)) {
                    unsafe {
                        UpdateWindow(self.hwnd);
                    }
                    remote::reply(sender, source, id, &json!({"ok":success,"error":error}));
                }
            }
        }
        invalidate(self.hwnd);
    }

    /// Navigates to a page and rebuilds page-local state.
    fn navigate(&mut self, page: Page) {
        self.page = page;
        self.rebuild_rows();
        if self
            .focus_item
            .is_none_or(|focus| focus.kind != crate::settings_app::keys::FocusKind::Navigation)
        {
            self.focus = None;
        }
        invalidate(self.hwnd);
    }

    /// Finds a loaded widget entry for a row.
    fn row_entry(&self, section: &str) -> Option<&work::LoadedEntry> {
        self.entries.iter().find(|item| item.entry.name == section)
    }

    /// Queues a validated write from a setting row.
    fn write_row(&mut self, index: usize, input: control::Input) {
        let Some(row) = self.rows.get(index) else {
            return;
        };
        if !row.enabled || self.banner.is_some() {
            return;
        }
        if let control::Input::Text(text) = &input {
            if let Err(error) = describe::parse(&row.setting, text) {
                self.rows[index].error = Some(error);
                invalidate(self.hwnd);
                return;
            }
        }
        let section = row.section.clone();
        let key = row.key.clone();
        let destination = if key == "startup" {
            self.row_entry(&section)
                .map(|item| Ok((item.entry.clone(), None)))
                .unwrap_or_else(|| Err("widget not registered".to_string()))
        } else {
            match self.row_entry(&section) {
                None => Err("widget not registered".to_string()),
                Some(item) => match &item.describe {
                    Ok(desc) => Ok((
                        item.entry.clone(),
                        Some(control::Target::from_describe(desc)),
                    )),
                    Err(error) => Err(error.clone()),
                },
            }
        };
        let (entry, target) = match destination {
            Ok(value) => value,
            Err(error) => {
                self.rows[index].pending = None;
                self.rows[index].error = Some(error);
                self.focus = Some(index);
                self.focus_item = Some(crate::settings_app::keys::Focus {
                    kind: crate::settings_app::keys::FocusKind::Control,
                    index,
                });
                self.ensure_focus_visible(index);
                invalidate(self.hwnd);
                return;
            }
        };
        self.sequence = self.sequence.wrapping_add(1).max(1);
        let sequence = self.sequence;
        self.rows[index].pending = Some(sequence);
        self.rows[index].error = None;
        if key == "startup" {
            let _ = self.writer.send(WriteJob::Startup {
                sequence,
                name: section,
                exe: entry.exe,
                enabled: matches!(input, control::Input::Value(Value::Bool(true))),
            });
        } else if let Some(target) = target {
            let _ = self.writer.send(WriteJob::Apply {
                sequence,
                target,
                key,
                input,
                follow: vec![entry],
            });
        }
        invalidate(self.hwnd);
    }

    /// Returns one protocol state response with secret values masked.
    fn state(&self) -> Value {
        let rows: Vec<Value> = match self.page {
            Page::Widget(_) => self
                .rows
                .iter()
                .map(|row| {
                    let kind = match row.control() {
                        model::Control::Switch | model::Control::Auto => "switch",
                        model::Control::Segments => "segments",
                        model::Control::Dropdown => "dropdown",
                        model::Control::Slider => "slider",
                        model::Control::Text => "text",
                        model::Control::Color => "color",
                        model::Control::Secret => "secret",
                    };
                    let secret = matches!(row.setting.kind, Kind::Secret)
                        || taskbar_dock::ctl::is_secret(&row.key);
                    let value =
                        if secret && row.value.as_str().is_some_and(|value| !value.is_empty()) {
                            json!("***")
                        } else {
                            row.value.clone()
                        };
                    let draft = row.draft.as_deref().map(|draft| {
                        if secret && !draft.is_empty() {
                            "***".to_string()
                        } else {
                            draft.to_string()
                        }
                    });
                    let default =
                        if secret && row.default.as_str().is_some_and(|value| !value.is_empty()) {
                            json!("***")
                        } else {
                            row.default.clone()
                        };
                    json!({
                        "id": format!("{}.{}", row.section, row.key),
                        "label": row.setting.label,
                        "kind": kind,
                        "value": value,
                        "default": default,
                        "draft": draft,
                        "error": row.error,
                        "enabled": row.enabled
                    })
                })
                .collect(),
            Page::Widgets => self
                .widgets
                .iter()
                .map(|widget| {
                    json!({
                        "name": widget.name,
                        "title": widget.title,
                        "description": widget.description,
                        "status": widget.status,
                        "pid": widget.pid,
                        "reason": widget.reason,
                        "show_on_taskbar": widget.show_on_taskbar
                    })
                })
                .collect(),
            _ => Vec::new(),
        };
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        unsafe {
            GetWindowRect(self.hwnd, &mut rect);
        }
        let focus = self.focus_item.map(|focus| match focus.kind {
            crate::settings_app::keys::FocusKind::Navigation => format!("nav:{}", focus.index),
            crate::settings_app::keys::FocusKind::Control => self
                .rows
                .get(focus.index)
                .map_or(String::new(), |row| format!("{}.{}", row.section, row.key)),
            crate::settings_app::keys::FocusKind::Reset => {
                self.rows.get(focus.index).map_or(String::new(), |row| {
                    format!("{}.{}.reset", row.section, row.key)
                })
            }
            crate::settings_app::keys::FocusKind::Action => format!("action:{}", focus.index),
        });
        json!({
            "ok": true,
            "page": page_name(&self.page),
            "rect": {
                "left": rect.left,
                "top": rect.top,
                "right": rect.right,
                "bottom": rect.bottom
            },
            "scale": self.scale,
            "focus": focus,
            "banner": self.banner,
            "badge": self.badge,
            "rows": rows
        })
    }

    /// Receives a copied remote command and performs it on the UI thread.
    fn remote(&mut self, request: remote::Request) {
        let sender = request.sender;
        let source = request.reply_source;
        let id = request.id;
        let sequence_before = self.sequence;
        let action_key = request
            .args
            .get(1)
            .and_then(|value| value.split_once(':'))
            .filter(|(action, _)| ["start", "quit", "restart"].contains(action))
            .map(|(action, name)| (name.to_string(), action.to_string()));
        let reply = match taskbar_dock::ctl::parse_settings(&request.args) {
            Err(error) => json!({"ok":false,"error":error,"kind":"usage"}),
            Ok(command) => match command {
                taskbar_dock::ctl::SettingsCommand::State => self.state(),
                taskbar_dock::ctl::SettingsCommand::Page(name) => {
                    let page = parse_page(&name, &self.entries);
                    self.navigate(page);
                    json!({"ok":true})
                }
                taskbar_dock::ctl::SettingsCommand::RowSet(id, text) => self.remote_set(&id, &text),
                taskbar_dock::ctl::SettingsCommand::RowReset(id) => match self.reset_remote(&id) {
                    Ok(()) => json!({"ok":true}),
                    Err(error) => json!({"ok":false,"error":error}),
                },
                taskbar_dock::ctl::SettingsCommand::RowToggle(id) => {
                    match self.toggle_remote(&id) {
                        Ok(()) => json!({"ok":true}),
                        Err(error) => json!({"ok":false,"error":error}),
                    }
                }
                taskbar_dock::ctl::SettingsCommand::Click(name) => self.click_remote(&name),
                taskbar_dock::ctl::SettingsCommand::Key(name) => {
                    self.key_name(&name);
                    json!({"ok":true})
                }
                taskbar_dock::ctl::SettingsCommand::Type(text) => {
                    self.type_text(&text);
                    json!({"ok":true})
                }
                taskbar_dock::ctl::SettingsCommand::Quit => {
                    post_close(self.hwnd);
                    json!({"ok":true})
                }
            },
        };
        unsafe {
            UpdateWindow(self.hwnd);
        }
        if self.sequence != sequence_before {
            self.pending_remote
                .insert(self.sequence, (sender, source, id));
            return;
        }
        if reply.get("ok").and_then(Value::as_bool) == Some(true) {
            if let Some(key) = action_key {
                self.pending_actions.insert(key, (sender, source, id));
                return;
            }
        }
        remote::reply(sender, source, id, &reply);
    }

    /// Sets one row through its regular editor validation path.
    fn remote_set(&mut self, id: &str, text: &str) -> Value {
        let Some(index) = self
            .rows
            .iter()
            .position(|row| format!("{}.{}", row.section, row.key) == id)
        else {
            return json!({"ok":false,"error":"row not found"});
        };
        self.focus = Some(index);
        self.focus_item = Some(crate::settings_app::keys::Focus {
            kind: crate::settings_app::keys::FocusKind::Control,
            index,
        });
        self.ensure_focus_visible(index);
        let parsed = match describe::parse(&self.rows[index].setting, text) {
            Ok(value) => value,
            Err(error) => {
                self.rows[index].draft = Some(text.to_string());
                self.rows[index].error = Some(error.clone());
                self.edit = Some((index, TextEdit::new(text)));
                self.focus_item = Some(crate::settings_app::keys::Focus {
                    kind: crate::settings_app::keys::FocusKind::Control,
                    index,
                });
                return json!({"ok":false,"error":error});
            }
        };
        self.rows[index].choice_text_mode =
            model::control_for_value(&self.rows[index].setting.kind, &parsed)
                == model::Control::Text;
        self.rows[index].draft = Some(text.to_string());
        let sequence = self.sequence;
        self.write_row(index, control::Input::Text(text.into()));
        if self.sequence == sequence {
            return json!({"ok":false,"error":self.rows[index].error});
        }
        json!({"ok":true})
    }

    /// Resets the row matching a protocol id.
    fn reset_remote(&mut self, id: &str) -> Result<(), String> {
        let Some(index) = self
            .rows
            .iter()
            .position(|row| format!("{}.{}", row.section, row.key) == id)
        else {
            return Err("row not found".into());
        };
        if self.banner.is_some() || !self.rows[index].enabled {
            return Err("row is disabled".into());
        }
        let section = self.rows[index].section.clone();
        let key = self.rows[index].key.clone();
        let destination = if key == "startup" {
            self.row_entry(&section)
                .map(|item| (item.entry.clone(), None))
                .ok_or_else(|| "widget not registered".to_string())
        } else {
            self.row_entry(&section)
                .ok_or_else(|| "widget not registered".to_string())
                .and_then(|item| {
                    item.describe
                        .as_ref()
                        .map(|desc| {
                            (
                                item.entry.clone(),
                                Some(control::Target::from_describe(desc)),
                            )
                        })
                        .map_err(Clone::clone)
                })
        };
        let (entry, target) = match destination {
            Ok(value) => value,
            Err(error) => {
                self.rows[index].pending = None;
                self.rows[index].error = Some(error.clone());
                return Err(error);
            }
        };
        if key == "startup" {
            self.sequence = self.sequence.wrapping_add(1).max(1);
            let sequence = self.sequence;
            self.rows[index].pending = Some(sequence);
            let _ = self.writer.send(WriteJob::Startup {
                sequence,
                name: section,
                exe: entry.exe,
                enabled: false,
            });
            return Ok(());
        }
        let target = target.unwrap();
        self.sequence = self.sequence.wrapping_add(1).max(1);
        self.rows[index].pending = Some(self.sequence);
        let _ = self.writer.send(WriteJob::Reset {
            sequence: self.sequence,
            target,
            key,
            follow: vec![entry],
        });
        Ok(())
    }

    /// Toggles the row matching a protocol id.
    fn toggle_remote(&mut self, id: &str) -> Result<(), String> {
        let Some(index) = self
            .rows
            .iter()
            .position(|row| format!("{}.{}", row.section, row.key) == id)
        else {
            return Err("row not found".into());
        };
        let sequence = self.sequence;
        self.toggle(index);
        if self.sequence == sequence {
            return Err(self.rows[index]
                .error
                .clone()
                .unwrap_or_else(|| "row is not a switch".into()));
        }
        Ok(())
    }

    /// Activates a protocol action name.
    fn click_remote(&mut self, name: &str) -> Value {
        if name == "notepad" {
            open_notepad();
            return json!({"ok":true});
        }
        if self.banner.is_some() {
            return json!({"ok":false,"error":"settings.json has an error"});
        }
        if let Some((action, widget)) = name.split_once(':') {
            if let Some(item) = self.entries.iter().find(|item| item.entry.name == widget) {
                if self.busy_actions.contains_key(widget) {
                    return json!({"ok": false, "error": "an action is already running"});
                }
                self.busy_actions
                    .insert(widget.to_string(), action.to_string());
                work::action(self.hwnd, item.entry.clone(), action.into());
                return json!({"ok":true});
            }
        }
        json!({"ok":false,"error":"unknown action"})
    }

    /// Routes a named keyboard operation through current input precedence.
    fn key_name(&mut self, key: &str) {
        let operation = match key {
            "tab" => crate::settings_app::keys::Key::Tab,
            "shift+tab" => crate::settings_app::keys::Key::ShiftTab,
            "enter" => crate::settings_app::keys::Key::Enter,
            "esc" => crate::settings_app::keys::Key::Escape,
            "space" => crate::settings_app::keys::Key::Space,
            "up" => crate::settings_app::keys::Key::Up,
            "down" => crate::settings_app::keys::Key::Down,
            "left" => crate::settings_app::keys::Key::Left,
            "right" => crate::settings_app::keys::Key::Right,
            "home" => crate::settings_app::keys::Key::Home,
            "end" => crate::settings_app::keys::Key::End,
            "f2" => crate::settings_app::keys::Key::F2,
            _ => return,
        };
        let code = match operation {
            crate::settings_app::keys::Key::Tab | crate::settings_app::keys::Key::ShiftTab => {
                VK_TAB as u32
            }
            crate::settings_app::keys::Key::Enter => VK_RETURN as u32,
            crate::settings_app::keys::Key::Escape => VK_ESCAPE as u32,
            crate::settings_app::keys::Key::Space => VK_SPACE as u32,
            crate::settings_app::keys::Key::Up => VK_UP as u32,
            crate::settings_app::keys::Key::Down => VK_DOWN as u32,
            crate::settings_app::keys::Key::Left => VK_LEFT as u32,
            crate::settings_app::keys::Key::Right => VK_RIGHT as u32,
            crate::settings_app::keys::Key::Home => VK_HOME as u32,
            crate::settings_app::keys::Key::End => VK_END as u32,
            crate::settings_app::keys::Key::F2 => VK_F2 as u32,
            crate::settings_app::keys::Key::PageUp => VK_PRIOR as u32,
            crate::settings_app::keys::Key::PageDown => VK_NEXT as u32,
        };
        if operation == crate::settings_app::keys::Key::ShiftTab && self.edit.is_none() {
            self.move_focus(true);
            invalidate(self.hwnd);
            return;
        }
        match crate::settings_app::keys::target(self.edit.is_some(), self.dropdown.is_some()) {
            model::KeyTarget::Text => self.edit_key(code),
            model::KeyTarget::Dropdown | model::KeyTarget::Page => self.page_key(code),
        }
        invalidate(self.hwnd);
    }

    /// Inserts protocol text into an active editor.
    fn type_text(&mut self, text: &str) {
        if let Some((index, edit)) = self.edit.as_mut() {
            edit.insert(text);
            let draft = edit.text().to_string();
            if let Some(row) = self.rows.get_mut(*index) {
                row.draft = Some(draft);
            }
        }
    }

    /// Starts editing the focused row with its current value.
    fn begin_edit(&mut self) {
        if let Some(index) = self.focus {
            if let Some(initial) = self.rows.get(index).map(|row| {
                if row.value.is_null() && matches!(row.setting.kind, Kind::Optional(_)) {
                    String::new()
                } else {
                    row.value
                        .as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| row.value.to_string())
                }
            }) {
                self.begin_edit_with(index, initial);
            }
        }
    }

    /// Starts editing a row with an explicit initial value.
    fn begin_edit_with(&mut self, index: usize, initial: String) {
        if self.banner.is_some() || !self.rows.get(index).is_some_and(|row| row.enabled) {
            return;
        }
        if let Some(row) = self.rows.get_mut(index) {
            row.draft = Some(initial.clone());
            let mut edit = TextEdit::new(initial);
            if matches!(row.setting.kind, Kind::Secret) {
                edit.select_all();
            }
            self.edit = Some((index, edit));
        }
    }

    /// Moves focus across page navigation and row controls.
    fn move_focus(&mut self, reverse: bool) {
        let nav_count = self.nav.len();
        let action_count = if matches!(self.page, Page::Widgets) {
            self.home_focus_controls().len()
        } else {
            0
        };
        let order = crate::settings_app::keys::focus_order(nav_count, &self.rows, action_count);
        if order.is_empty() {
            return;
        }
        let old = self
            .focus_item
            .and_then(|current| order.iter().position(|item| *item == current));
        let target =
            crate::settings_app::keys::next_focus(&order, old.map(|index| order[index]), reverse)
                .unwrap();
        self.focus_item = Some(target);
        self.focus = match target.kind {
            crate::settings_app::keys::FocusKind::Control
            | crate::settings_app::keys::FocusKind::Reset => Some(target.index),
            _ => None,
        };
        if target.kind == crate::settings_app::keys::FocusKind::Navigation {
            self.scroll = 0;
        } else if target.kind == crate::settings_app::keys::FocusKind::Control
            || target.kind == crate::settings_app::keys::FocusKind::Reset
        {
            self.ensure_focus_visible(target.index);
        }
    }

    /// Lists the visible home-page switches and process buttons in display order.
    fn home_focus_controls(&self) -> Vec<(usize, u8)> {
        self.widgets
            .iter()
            .enumerate()
            .flat_map(|(index, widget)| {
                let running = widget.status == "running" || widget.status == "not responding";
                let mut controls = vec![(index, 0), (index, 1)];
                if running {
                    controls.push((index, 2));
                }
                controls
            })
            .collect()
    }

    /// Returns the focused home-page target as `(widget index, button kind)`.
    fn home_focus(&self) -> Option<(usize, u8)> {
        let focus = self.focus_item?;
        if focus.kind != crate::settings_app::keys::FocusKind::Action {
            return None;
        }
        self.home_focus_controls().get(focus.index).copied()
    }

    /// Activates a focused home-page switch or process button.
    fn activate_home_focus(&mut self) {
        let Some((index, kind)) = self.home_focus() else {
            return;
        };
        match kind {
            0 => self.write_show_on_taskbar(index),
            1 => {
                let action = self.widgets.get(index).map(|widget| {
                    if widget.status == "running" || widget.status == "not responding" {
                        "quit"
                    } else {
                        "start"
                    }
                });
                if let Some(action) = action {
                    self.home_action(index, action);
                }
            }
            2 => self.home_action(index, "restart"),
            _ => {}
        }
    }

    /// Focuses one of the currently visible controls on a home row.
    fn focus_home_control(&mut self, widget: usize, kind: u8) {
        if let Some(index) = self
            .home_focus_controls()
            .iter()
            .position(|target| *target == (widget, kind))
        {
            self.focus_item = Some(crate::settings_app::keys::Focus {
                kind: crate::settings_app::keys::FocusKind::Action,
                index,
            });
            self.focus = None;
        }
    }

    /// Scrolls a focused row into the visible content viewport.
    fn ensure_focus_visible(&mut self, index: usize) {
        let nav_width = layout::px(theme::app::NAV_WIDTH, self.scale);
        let width = unsafe {
            let mut rect: RECT = std::mem::zeroed();
            GetClientRect(self.hwnd, &mut rect);
            rect.right - nav_width - 2 * layout::px(theme::app::CONTENT_PADDING, self.scale)
        };
        let mut top = layout::px(theme::app::HEADER_HEIGHT, self.scale)
            + self.banner.as_ref().map_or(0, |_| {
                layout::px(theme::space::XL * 2.0 + theme::space::S, self.scale)
            });
        for (row_index, row) in self.rows.iter().enumerate() {
            if crate::settings_app::model::group_heading(&self.rows, row_index).is_some() {
                top += layout::px(theme::app::GROUP_HEIGHT, self.scale);
            }
            let height = self.measured_card_height(row, width)
                + layout::px(theme::app::CARD_GAP, self.scale);
            if row_index == index {
                if top - self.scroll < layout::px(theme::app::HEADER_HEIGHT, self.scale) {
                    self.scroll = top - layout::px(theme::app::HEADER_HEIGHT, self.scale);
                }
                if top + height - self.scroll
                    > unsafe {
                        let mut rect: RECT = std::mem::zeroed();
                        GetClientRect(self.hwnd, &mut rect);
                        rect.bottom
                    }
                {
                    self.scroll = top + height
                        - unsafe {
                            let mut rect: RECT = std::mem::zeroed();
                            GetClientRect(self.hwnd, &mut rect);
                            rect.bottom
                        };
                }
                break;
            }
            top += height;
        }
        self.scroll = self.scroll.max(0);
        self.clamp_page_scroll();
    }

    /// Clamps the current page scroll to its measured content and viewport.
    fn clamp_page_scroll(&mut self) {
        let client_height = unsafe {
            let mut rect: RECT = std::mem::zeroed();
            GetClientRect(self.hwnd, &mut rect);
            rect.bottom
        };
        let header = layout::px(theme::app::HEADER_HEIGHT, self.scale);
        let banner = self.banner.as_ref().map_or(0, |_| {
            layout::px(theme::space::XL * 2.0 + theme::space::S, self.scale)
        });
        let content = match &self.page {
            Page::Widgets => {
                (self.widgets.len() as i32)
                    * (layout::px(theme::app::NAV_ROW_HEIGHT * 2.0, self.scale)
                        + layout::px(theme::app::CARD_GAP, self.scale))
            }
            Page::Widget(_) => {
                let width = unsafe {
                    let mut rect: RECT = std::mem::zeroed();
                    GetClientRect(self.hwnd, &mut rect);
                    rect.right
                        - layout::px(
                            theme::app::NAV_WIDTH + theme::app::CONTENT_PADDING * 2.0,
                            self.scale,
                        )
                };
                let mut height = 0;
                for (index, row) in self.rows.iter().enumerate() {
                    if model::group_heading(&self.rows, index).is_some() {
                        height += layout::px(theme::app::GROUP_HEIGHT, self.scale);
                    }
                    height += self.measured_card_height(row, width)
                        + layout::px(theme::app::CARD_GAP, self.scale);
                }
                height
            }
            _ => 0,
        };
        self.scroll = layout::clamp_scroll(
            self.scroll,
            content + banner,
            (client_height - header).max(0),
        );
    }

    /// Scrolls by a delta and keeps the offset within this page's content.
    fn scroll_by(&mut self, delta: i32) {
        self.scroll = self.scroll.saturating_add(delta);
        self.clamp_page_scroll();
    }

    /// Toggles a switch or advances a choice on the current page.
    fn toggle(&mut self, index: usize) {
        let Some(row) = self.rows.get(index) else {
            return;
        };
        if self.banner.is_some() || !row.enabled {
            return;
        }
        match &row.setting.kind {
            Kind::Switch => self.write_row(
                index,
                control::Input::Value(Value::Bool(row.value.as_bool() != Some(true))),
            ),
            Kind::Choice { options, .. } => {
                if row.control() == model::Control::Dropdown {
                    // Open on the current choice ("Other…" when the value is no option).
                    let current = options
                        .iter()
                        .position(|(value, _)| *value == row.value)
                        .unwrap_or(options.len());
                    self.dropdown = Some((index, current));
                    invalidate(self.hwnd);
                    return;
                }
                let at = options
                    .iter()
                    .position(|(value, _)| value == &row.value)
                    .unwrap_or(0);
                if let Some((value, _)) = options.get((at + 1) % options.len().max(1)) {
                    self.write_row(index, control::Input::Value(value.clone()));
                }
            }
            Kind::Auto { word, .. } => {
                let value = if row.value.as_str() == Some(word) {
                    model::auto_off_value(&row.setting.kind, word, &row.default)
                } else {
                    Some(json!(word))
                };
                if let Some(value) = value {
                    self.write_row(index, control::Input::Value(value));
                }
            }
            _ => self.begin_edit(),
        }
    }

    /// Selects an option in the currently open choice list.
    fn choose_dropdown(&mut self, index: usize, option: usize) {
        let Some(row) = self.rows.get(index) else {
            self.dropdown = None;
            return;
        };
        let Kind::Choice { options, other } = &row.setting.kind else {
            self.dropdown = None;
            return;
        };
        if *other && option == options.len() {
            self.dropdown = None;
            self.focus = Some(index);
            self.rows[index].choice_text_mode = true;
            self.begin_edit_with(index, String::new());
        } else if let Some((value, _)) = options.get(option) {
            let value = value.clone();
            self.dropdown = None;
            self.write_row(index, control::Input::Value(value));
        }
        invalidate(self.hwnd);
    }

    /// Re-reads Look and updates scaled drawing after Windows settings change.
    fn look_changed(&mut self) {
        self.theme = Theme::new(&taskbar_dock::style::Look::current());
        self.scale = taskbar_dock::system::scale_for(self.hwnd);
        self.fonts = paint::Fonts::new(&self.theme, self.scale);
        apply_dwm(self.hwnd, self.theme.light);
        invalidate(self.hwnd);
    }

    /// Checks the settings file timestamp and expires transient badges.
    fn watch_file(&mut self) {
        let mtime = taskbar_dock::settings::mtime();
        if mtime != self.mtime {
            self.mtime = mtime;
            work::read_file(self.hwnd);
        }
        if self
            .badge_until
            .is_some_and(|until| Instant::now() >= until)
        {
            self.badge = None;
            self.badge_until = None;
        }
    }

    /// Commits a text edit with the shared description parser.
    fn commit_edit(&mut self) {
        let Some((index, edit)) = self.edit.take() else {
            return;
        };
        if let Err(error) = describe::parse(&self.rows[index].setting, edit.text()) {
            self.rows[index].draft = Some(edit.text().into());
            self.rows[index].error = Some(error);
            self.edit = Some((index, edit));
            invalidate(self.hwnd);
            return;
        }
        if let Some(row) = self.rows.get_mut(index) {
            row.draft = Some(edit.text().to_string());
        }
        self.write_row(index, control::Input::Text(edit.text().into()));
    }

    /// Cancels an edit and restores its saved value.
    fn cancel_edit(&mut self) {
        if let Some((index, _)) = self.edit.take() {
            if let Some(row) = self.rows.get_mut(index) {
                row.draft = None;
                row.error = None;
                row.choice_text_mode =
                    model::control_for_value(&row.setting.kind, &row.value) == model::Control::Text;
            }
        }
    }

    /// Sets the active editor's caret from measured GDI text extents.
    fn caret_click(&mut self, x: i32, y: i32) -> bool {
        let Some((index, _)) = self.edit.as_ref() else {
            return false;
        };
        let index = *index;
        let content_left = layout::px(
            theme::app::NAV_WIDTH + theme::app::CONTENT_PADDING,
            self.scale,
        );
        let width = unsafe {
            let mut rect: RECT = std::mem::zeroed();
            GetClientRect(self.hwnd, &mut rect);
            rect.right - content_left - layout::px(theme::app::CONTENT_PADDING, self.scale)
        };
        let mut top = layout::px(theme::app::HEADER_HEIGHT, self.scale)
            + self.banner.as_ref().map_or(0, |_| {
                layout::px(theme::space::XL * 2.0 + theme::space::S, self.scale)
            })
            - self.scroll;
        for (row_index, row) in self.rows.iter().enumerate() {
            if crate::settings_app::model::group_heading(&self.rows, row_index).is_some() {
                top += layout::px(theme::app::GROUP_HEIGHT, self.scale);
            }
            let full_line = layout::full_line(row);
            let card_height = self.measured_card_height(row, width);
            if row_index == index {
                let local = layout::row_rects(
                    width,
                    top,
                    card_height,
                    layout::px(theme::app::CONTROL_WIDTH, self.scale),
                    full_line,
                    row.reset_visible(),
                    self.measured_error_height(row, width),
                    self.scale,
                );
                let control = Rect {
                    left: content_left + local.control.left,
                    top: local.control.top,
                    right: content_left + local.control.right,
                    bottom: local.control.bottom,
                };
                if !control.contains(x, y) {
                    return false;
                }
                if let Some((_, edit)) = self.edit.as_mut() {
                    let target =
                        (x - control.left - layout::px(theme::space::S, self.scale)).max(0);
                    let mut previous = 0;
                    let mut caret = 0;
                    for (char_index, _) in edit
                        .text()
                        .char_indices()
                        .chain(std::iter::once((edit.text().len(), '\0')))
                        .enumerate()
                    {
                        let prefix: String = edit.text().chars().take(char_index).collect();
                        let width = self.fonts.measure_width(&prefix);
                        if target < (previous + width) / 2 {
                            caret = char_index;
                            break;
                        }
                        caret = char_index;
                        previous = width;
                    }
                    edit.set_caret(caret);
                }
                return true;
            }
            top += card_height + layout::px(theme::app::CARD_GAP, self.scale);
        }
        false
    }

    /// Handles navigation, clipboard, and deletion keys while editing.
    fn edit_key(&mut self, key: u32) {
        if key == VK_RETURN as u32 {
            self.commit_edit();
            return;
        }
        if key == VK_ESCAPE as u32 {
            self.cancel_edit();
            return;
        }
        if key == VK_TAB as u32 {
            self.commit_edit();
            if self.edit.is_none() {
                self.move_focus(unsafe { GetKeyState(VK_SHIFT as i32) < 0 });
            }
            return;
        }
        let Some((_, edit)) = self.edit.as_mut() else {
            return;
        };
        let shift = unsafe { GetKeyState(VK_SHIFT as i32) < 0 };
        let ctrl = unsafe { GetKeyState(VK_CONTROL as i32) < 0 };
        if key == VK_LEFT as u32 && shift {
            edit.shift_left();
        } else if key == VK_RIGHT as u32 && shift {
            edit.shift_right();
        } else if key == VK_HOME as u32 && shift {
            edit.shift_home();
        } else if key == VK_END as u32 && shift {
            edit.shift_end();
        } else if key == VK_LEFT as u32 {
            edit.left();
        } else if key == VK_RIGHT as u32 {
            edit.right();
        } else if key == VK_HOME as u32 {
            edit.home();
        } else if key == VK_END as u32 {
            edit.end();
        } else if key == VK_BACK as u32 && ctrl {
            edit.ctrl_backspace();
        } else if key == VK_BACK as u32 {
            edit.backspace();
        } else if key == VK_DELETE as u32 {
            edit.delete();
        } else if key == b'A' as u32 && ctrl {
            edit.select_all();
        } else if key == b'C' as u32 && ctrl {
            taskbar_dock::text_edit::clipboard_write(self.hwnd, &edit.copy());
        } else if key == b'X' as u32 && ctrl {
            let text = edit.cut();
            taskbar_dock::text_edit::clipboard_write(self.hwnd, &text);
        } else if key == b'V' as u32 && ctrl {
            if let Some(text) = taskbar_dock::text_edit::clipboard_read() {
                edit.paste(&text);
            }
        }
        if let Some((index, edit)) = self.edit.as_ref() {
            if let Some(row) = self.rows.get_mut(*index) {
                row.draft = Some(edit.text().to_string());
            }
        }
    }

    /// Dispatches normal page navigation and control actions.
    fn page_key(&mut self, key: u32) {
        if let Some((index, selected)) = self.dropdown {
            match key {
                key if key == VK_ESCAPE as u32 => self.dropdown = None,
                key if key == VK_UP as u32 || key == VK_DOWN as u32 => {
                    let count = self
                        .rows
                        .get(index)
                        .map_or(0, |row| match &row.setting.kind {
                            Kind::Choice { options, other } => options.len() + usize::from(*other),
                            _ => 0,
                        });
                    if count > 0 {
                        let next = if key == VK_UP as u32 {
                            selected.saturating_sub(1)
                        } else {
                            (selected + 1).min(count - 1)
                        };
                        self.dropdown = Some((index, next));
                    }
                }
                key if key == VK_RETURN as u32 => self.choose_dropdown(index, selected),
                _ => {}
            }
            invalidate(self.hwnd);
            return;
        }
        let operation = match key {
            value if value == VK_PRIOR as u32 => Some(crate::settings_app::keys::Key::PageUp),
            value if value == VK_NEXT as u32 => Some(crate::settings_app::keys::Key::PageDown),
            _ => None,
        };
        if let Some(operation) = operation {
            self.scroll_by(if operation == crate::settings_app::keys::Key::PageUp {
                -300
            } else {
                300
            });
            invalidate(self.hwnd);
            return;
        }
        if key == VK_ESCAPE as u32 {
            post_close(self.hwnd);
        } else if key == VK_TAB as u32 {
            self.move_focus(unsafe { GetKeyState(VK_SHIFT as i32) < 0 });
        } else if key == VK_RETURN as u32 || key == VK_F2 as u32 {
            if let Some(focus) = self.focus_item {
                if focus.kind == crate::settings_app::keys::FocusKind::Reset {
                    self.reset_focused(focus.index);
                } else if focus.kind == crate::settings_app::keys::FocusKind::Control {
                    self.activate_row(focus.index);
                } else if focus.kind == crate::settings_app::keys::FocusKind::Navigation {
                    if let Some((_, page)) = self.nav.get(focus.index) {
                        self.navigate(page.clone());
                    }
                } else if focus.kind == crate::settings_app::keys::FocusKind::Action {
                    self.activate_home_focus();
                }
            } else {
                self.begin_edit();
            }
        } else if key == VK_SPACE as u32 {
            if self
                .focus_item
                .is_some_and(|focus| focus.kind == crate::settings_app::keys::FocusKind::Action)
            {
                self.activate_home_focus();
            } else if let Some(index) = self.focus {
                if matches!(
                    self.rows.get(index).map(Row::control),
                    Some(model::Control::Switch | model::Control::Auto)
                ) {
                    self.toggle(index);
                }
            }
        } else if key == VK_UP as u32 {
            self.nav_move(-1);
        } else if key == VK_DOWN as u32 {
            self.nav_move(1);
        } else if key == VK_PRIOR as u32 {
            self.scroll_by(-300);
        } else if key == VK_NEXT as u32 {
            self.scroll_by(300);
        } else if key == VK_HOME as u32 {
            self.scroll = 0;
        } else if key == VK_END as u32 {
            self.scroll = i32::MAX;
            self.clamp_page_scroll();
        } else if key == VK_LEFT as u32 || key == VK_RIGHT as u32 {
            if let Some(index) = self.focus {
                self.adjust_focused(index, key == VK_RIGHT as u32);
            }
        }
        invalidate(self.hwnd);
    }

    /// Applies one keyboard step to a slider or a closed choice control.
    fn adjust_focused(&mut self, index: usize, forward: bool) {
        let Some(row) = self.rows.get(index) else {
            return;
        };
        match &row.setting.kind {
            Kind::Number { min, max, step, .. } => {
                let current = row.value.as_f64().unwrap_or(*min);
                let value = if forward {
                    current + step
                } else {
                    current - step
                }
                .clamp(*min, *max);
                self.write_row(index, control::Input::Value(json!(value)));
            }
            Kind::Choice { options, .. }
                if row.control() == model::Control::Segments
                    || row.control() == model::Control::Dropdown =>
            {
                if options.is_empty() {
                    return;
                }
                let current = options
                    .iter()
                    .position(|(value, _)| value == &row.value)
                    .unwrap_or(0);
                let next = if forward {
                    (current + 1) % options.len()
                } else {
                    current.saturating_sub(1)
                };
                self.write_row(index, control::Input::Value(options[next].0.clone()));
            }
            _ => {}
        }
    }

    /// Moves the selected navigation page vertically.
    fn nav_move(&mut self, delta: isize) {
        if self
            .focus_item
            .is_some_and(|focus| focus.kind != crate::settings_app::keys::FocusKind::Navigation)
        {
            return;
        }
        let at = self
            .nav
            .iter()
            .position(|(_, page)| page == &self.page)
            .unwrap_or(0) as isize;
        let next = (at + delta).clamp(0, self.nav.len().saturating_sub(1) as isize) as usize;
        if let Some((_, page)) = self.nav.get(next) {
            self.navigate(page.clone());
            self.focus_item = Some(crate::settings_app::keys::Focus {
                kind: crate::settings_app::keys::FocusKind::Navigation,
                index: next,
            });
        }
    }

    /// Handles page and row mouse interactions.
    fn click(&mut self, x: i32, y: i32) {
        let nav_width = layout::px(theme::app::NAV_WIDTH, self.scale);
        if self.edit.is_some() {
            if self.caret_click(x, y) {
                invalidate(self.hwnd);
                return;
            }
            self.commit_edit();
            if self.edit.is_some() {
                return;
            }
        }
        let open_dropdown = self.dropdown.take();
        if x < nav_width {
            let client_height = unsafe {
                let mut rect: RECT = std::mem::zeroed();
                GetClientRect(self.hwnd, &mut rect);
                rect.bottom
            };
            let row_height = layout::px(theme::app::NAV_ROW_HEIGHT, self.scale);
            let mut top = layout::px(theme::app::CONTENT_PADDING, self.scale);
            let mut nav_items = Vec::new();
            for (index, (_, page)) in self.nav.iter().enumerate() {
                if matches!(page, Page::Developer) {
                    top = client_height
                        - row_height
                        - layout::px(theme::app::CONTENT_PADDING, self.scale);
                }
                nav_items.push((
                    Rect {
                        left: layout::px(theme::space::S, self.scale),
                        top,
                        right: nav_width - layout::px(theme::space::S, self.scale),
                        bottom: top + row_height,
                    },
                    layout::Hit::Nav(index),
                ));
                if !matches!(page, Page::Developer) {
                    top += row_height;
                }
            }
            if let Some(layout::Hit::Nav(index)) = layout::hit_test(x, y, &nav_items) {
                if let Some((_, page)) = self.nav.get(index) {
                    self.navigate(page.clone());
                    self.focus_item = Some(crate::settings_app::keys::Focus {
                        kind: crate::settings_app::keys::FocusKind::Navigation,
                        index,
                    });
                }
            }
            return;
        }
        if self.banner.is_some()
            && x > nav_width + layout::px(theme::app::CONTROL_WIDTH, self.scale)
            && y < layout::px(
                theme::app::HEADER_HEIGHT + theme::space::XL * 2.0,
                self.scale,
            )
        {
            if layout::hit_test(
                x,
                y,
                &[(
                    Rect {
                        left: nav_width + layout::px(theme::app::CONTROL_WIDTH, self.scale),
                        top: 0,
                        right: i32::MAX,
                        bottom: layout::px(
                            theme::app::HEADER_HEIGHT + theme::space::XL * 2.0,
                            self.scale,
                        ),
                    },
                    layout::Hit::Notepad,
                )],
            )
            .is_some()
            {
                open_notepad();
                return;
            }
        }
        if self.banner.is_some() {
            return;
        }
        if matches!(self.page, Page::Widgets) {
            let row_height = layout::px(theme::app::NAV_ROW_HEIGHT * 2.0, self.scale);
            let idx = ((y - layout::px(theme::app::HEADER_HEIGHT, self.scale) + self.scroll)
                / (row_height + layout::px(theme::app::CARD_GAP, self.scale)))
            .max(0) as usize;
            if let Some(widget) = self.widgets.get(idx).cloned() {
                let width = unsafe {
                    let mut rect: RECT = std::mem::zeroed();
                    GetClientRect(self.hwnd, &mut rect);
                    rect.right - nav_width - 2 * layout::px(theme::app::CONTENT_PADDING, self.scale)
                };
                let left = nav_width + layout::px(theme::app::CONTENT_PADDING, self.scale);
                let action_width = layout::px(theme::app::HOME_ACTION_WIDTH, self.scale);
                let switch_width = layout::px(theme::app::HOME_SWITCH_WIDTH, self.scale);
                let pad = layout::px(theme::app::CARD_PADDING, self.scale);
                let button_y = y
                    >= layout::px(theme::app::HEADER_HEIGHT, self.scale) - self.scroll + row_height
                        - pad
                        - layout::px(theme::app::CONTROL_HEIGHT, self.scale)
                    && y < layout::px(theme::app::HEADER_HEIGHT, self.scale) - self.scroll
                        + row_height
                        - pad;
                let switch_left = left + width - pad - switch_width - action_width * 2 - pad;
                let action_left = left + width - action_width * 2 - pad;
                let actions = [
                    (
                        Rect {
                            left: action_left,
                            top: y + row_height
                                - pad
                                - layout::px(theme::app::CONTROL_HEIGHT, self.scale),
                            right: action_left + action_width,
                            bottom: y + row_height - pad,
                        },
                        layout::Hit::Action(0),
                    ),
                    (
                        Rect {
                            left: action_left + action_width + pad / 2,
                            top: y + row_height
                                - pad
                                - layout::px(theme::app::CONTROL_HEIGHT, self.scale),
                            right: left + width - pad,
                            bottom: y + row_height - pad,
                        },
                        layout::Hit::Action(1),
                    ),
                ];
                let action_hit = layout::hit_test(x, y, &actions);
                if button_y && x >= switch_left && x < action_left - pad / 2 {
                    self.focus_home_control(idx, 0);
                    self.write_show_on_taskbar(idx);
                } else if button_y && matches!(action_hit, Some(layout::Hit::Action(0))) {
                    self.focus_home_control(idx, 1);
                    let action = if widget.status == "running" || widget.status == "not responding"
                    {
                        "quit"
                    } else {
                        "start"
                    };
                    self.home_action(idx, action);
                } else if button_y && matches!(action_hit, Some(layout::Hit::Action(1))) {
                    self.focus_home_control(idx, 2);
                    self.home_action(idx, "restart");
                } else {
                    self.navigate(Page::Widget(widget.name.clone()));
                }
            }
        } else if !self.rows.is_empty() {
            let width = unsafe {
                let mut rect: RECT = std::mem::zeroed();
                GetClientRect(self.hwnd, &mut rect);
                rect.right - nav_width - 2 * layout::px(theme::app::CONTENT_PADDING, self.scale)
            };
            let left = nav_width + layout::px(theme::app::CONTENT_PADDING, self.scale);
            let banner_height = self.banner.as_ref().map_or(0, |_| {
                layout::px(theme::space::XL * 2.0 + theme::space::S, self.scale)
            });
            let mut top =
                layout::px(theme::app::HEADER_HEIGHT, self.scale) + banner_height - self.scroll;
            for index in 0..self.rows.len() {
                let row = self.rows[index].clone();
                if crate::settings_app::model::group_heading(&self.rows, index).is_some() {
                    top += layout::px(theme::app::GROUP_HEIGHT, self.scale);
                }
                let full_line = layout::full_line(&row);
                let card_height = self.measured_card_height(&row, width);
                if let Some((open_index, _)) = open_dropdown {
                    if open_index == index {
                        let local = layout::row_rects(
                            width,
                            top,
                            card_height,
                            layout::px(theme::app::CONTROL_WIDTH, self.scale),
                            full_line,
                            row.reset_visible(),
                            self.measured_error_height(&row, width),
                            self.scale,
                        );
                        let trigger = Rect {
                            left: left + local.control.left,
                            top: local.control.top,
                            right: left + local.control.right,
                            bottom: local.control.bottom,
                        };
                        let client_height = unsafe {
                            let mut rect: RECT = std::mem::zeroed();
                            GetClientRect(self.hwnd, &mut rect);
                            rect.bottom
                        };
                        let control = layout::control_model(
                            &row,
                            trigger,
                            self.scale,
                            Rect {
                                left,
                                top: layout::px(theme::app::HEADER_HEIGHT, self.scale),
                                right: left + width,
                                bottom: client_height,
                            },
                            true,
                        );
                        if let Some(option) = control.parts.iter().find_map(|(part, rect)| {
                            if let layout::ControlPart::DropdownItem(index) = part {
                                rect.contains(x, y).then_some(*index)
                            } else {
                                None
                            }
                        }) {
                            self.choose_dropdown(index, option);
                            return;
                        }
                    }
                }
                if y >= top && y < top + card_height {
                    if !row.enabled {
                        return;
                    }
                    let reset_visible = row.reset_visible();
                    let row_id = format!("{}.{}", row.section, row.key);
                    let numeric = match &row.setting.kind {
                        Kind::Number { min, max, step, .. } => Some((*min, *max, *step)),
                        _ => None,
                    };
                    self.focus = Some(index);
                    let local = layout::row_rects(
                        width,
                        top,
                        card_height,
                        layout::px(theme::app::CONTROL_WIDTH, self.scale),
                        full_line,
                        reset_visible,
                        self.measured_error_height(&row, width),
                        self.scale,
                    );
                    let control_rect = Rect {
                        left: left + local.control.left,
                        top: local.control.top,
                        right: left + local.control.right,
                        bottom: local.control.bottom,
                    };
                    let client_height = unsafe {
                        let mut rect: RECT = std::mem::zeroed();
                        GetClientRect(self.hwnd, &mut rect);
                        rect.bottom
                    };
                    let control_model = layout::control_model(
                        &row,
                        control_rect,
                        self.scale,
                        Rect {
                            left,
                            top: layout::px(theme::app::HEADER_HEIGHT, self.scale),
                            right: left + width,
                            bottom: client_height,
                        },
                        false,
                    );
                    let card_hit = Rect {
                        left: 0,
                        top,
                        right: width,
                        bottom: top + card_height,
                    };
                    let mut hit_items = vec![
                        (card_hit, layout::Hit::Row(index)),
                        (local.control, layout::Hit::Control(index)),
                    ];
                    if let Some(reset) = local.reset.filter(|_| reset_visible) {
                        hit_items.push((reset, layout::Hit::Reset(index)));
                    }
                    let hit = layout::hit_test(x - left, y, &hit_items);
                    if matches!(hit, Some(layout::Hit::Reset(_))) {
                        self.focus_item = Some(crate::settings_app::keys::Focus {
                            kind: crate::settings_app::keys::FocusKind::Reset,
                            index,
                        });
                        self.focus = Some(index);
                        self.ensure_focus_visible(index);
                        let _ = self.reset_remote(&row_id);
                    } else if matches!(hit, Some(layout::Hit::Control(_))) {
                        self.focus_item = Some(crate::settings_app::keys::Focus {
                            kind: crate::settings_app::keys::FocusKind::Control,
                            index,
                        });
                        self.ensure_focus_visible(index);
                        if let Kind::Auto { word, .. } = &row.setting.kind {
                            let split = control_model
                                .parts
                                .iter()
                                .find_map(|(part, rect)| {
                                    (*part == layout::ControlPart::AutoInner).then_some(rect.left)
                                })
                                .unwrap_or(control_rect.left);
                            if x >= split && row.value.as_str() != Some(word) {
                                self.begin_edit_with(index, row.value.to_string());
                                break;
                            }
                            self.toggle(index);
                            break;
                        }
                        if row.control() == model::Control::Secret
                            && control_model.parts.iter().any(|(part, rect)| {
                                *part == layout::ControlPart::Eye && rect.contains(x, y)
                            })
                        {
                            self.reveal_secret = true;
                            unsafe {
                                SetCapture(self.hwnd);
                            }
                            break;
                        }
                        if row.control() == model::Control::Segments {
                            if let Kind::Choice { options, .. } = &row.setting.kind {
                                let selected = control_model
                                    .parts
                                    .iter()
                                    .find_map(|(part, rect)| {
                                        if let layout::ControlPart::Segment(option) = part {
                                            rect.contains(x, y).then_some(*option)
                                        } else {
                                            None
                                        }
                                    })
                                    .unwrap_or(0);
                                if let Some((value, _)) = options.get(selected) {
                                    self.write_row(index, control::Input::Value(value.clone()));
                                }
                                break;
                            }
                        }
                        if let Some((min, max, step)) = numeric {
                            let ratio = ((x - left - local.control.left) as f64
                                / (local.control.right - local.control.left).max(1) as f64)
                                .clamp(0.0, 1.0);
                            let raw = min + ratio * (max - min);
                            let value = (raw / step).round() * step;
                            self.rows[index].draft = Some(value.to_string());
                            self.slider_drag = Some((
                                index,
                                value.clamp(min, max),
                                Rect {
                                    left: left + local.control.left,
                                    top: local.control.top,
                                    right: left + local.control.right,
                                    bottom: local.control.bottom,
                                },
                            ));
                            unsafe {
                                SetCapture(self.hwnd);
                            }
                        } else {
                            self.activate_row(index);
                        }
                    } else {
                        self.activate_row(index);
                    }
                    break;
                }
                top += card_height + layout::px(theme::app::CARD_GAP, self.scale);
            }
        }
    }

    /// Activates a row according to its described setting kind.
    fn activate_row(&mut self, index: usize) {
        let Some(row) = self.rows.get(index) else {
            return;
        };
        let control = if row.control() == model::Control::Text {
            model::Control::Text
        } else {
            crate::settings_app::keys::control(&row.setting.kind)
        };
        match control {
            model::Control::Switch
            | model::Control::Segments
            | model::Control::Dropdown
            | model::Control::Auto => self.toggle(index),
            _ => self.begin_edit(),
        }
    }

    /// Activates a focused Reset button.
    fn reset_focused(&mut self, index: usize) {
        if let Some(row) = self.rows.get(index) {
            let id = format!("{}.{}", row.section, row.key);
            let _ = self.reset_remote(&id);
        }
    }

    /// Updates a captured slider's preview without writing the setting.
    fn move_slider(&mut self, x: i32) {
        let Some((index, _, rect)) = self.slider_drag else {
            return;
        };
        if let Some(row) = self.rows.get(index) {
            if let Kind::Number { min, max, step, .. } = &row.setting.kind {
                let ratio = ((x - rect.left) as f64 / (rect.right - rect.left).max(1) as f64)
                    .clamp(0.0, 1.0);
                let value = ((min + ratio * (max - min)) / step).round() * step;
                self.slider_drag = Some((index, value.clamp(*min, *max), rect));
                self.rows[index].draft = Some(value.to_string());
                invalidate(self.hwnd);
            }
        }
    }

    /// Commits a slider value on release.
    fn finish_slider(&mut self) {
        if let Some((index, value, _)) = self.slider_drag.take() {
            unsafe {
                ReleaseCapture();
            }
            if let Some(row) = self.rows.get_mut(index) {
                row.draft = None;
            }
            self.write_row(index, control::Input::Value(json!(value)));
        }
    }

    /// Restores the saved value when slider capture is lost.
    fn cancel_slider(&mut self) {
        if let Some((index, _, _)) = self.slider_drag.take() {
            if let Some(row) = self.rows.get_mut(index) {
                row.draft = None;
            }
            invalidate(self.hwnd);
        }
    }

    /// Writes the overview's Show on taskbar switch through control::apply.
    fn write_show_on_taskbar(&mut self, index: usize) {
        let Some(widget) = self.widgets.get(index) else {
            return;
        };
        let Some(item) = self
            .entries
            .iter()
            .find(|item| item.entry.name == widget.name)
        else {
            return;
        };
        let Ok(desc) = &item.describe else {
            return;
        };
        let target = control::Target::from_describe(desc);
        self.sequence = self.sequence.wrapping_add(1).max(1);
        let input = control::Input::Value(Value::Bool(!widget.show_on_taskbar));
        let _ = self.writer.send(WriteJob::Apply {
            sequence: self.sequence,
            target,
            key: "show_on_taskbar".into(),
            input,
            follow: vec![item.entry.clone()],
        });
    }

    /// Starts, quits, or restarts the overview widget at the requested action button.
    fn home_action(&mut self, index: usize, action: &str) {
        if let Some(widget) = self.widgets.get(index) {
            if let Some(item) = self
                .entries
                .iter()
                .find(|item| item.entry.name == widget.name)
            {
                if self.busy_actions.contains_key(&widget.name) {
                    return;
                }
                self.busy_actions
                    .insert(widget.name.clone(), action.to_string());
                work::action(self.hwnd, item.entry.clone(), action.into());
            }
        }
    }
}

/// Applies title bar mode and Mica when the running Windows version supports them.
fn apply_dwm(hwnd: HWND, light: bool) {
    unsafe {
        let dark: BOOL = (!light).into();
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
            &dark as *const _ as _,
            std::mem::size_of_val(&dark) as u32,
        );
        let backdrop = DWMSBT_MAINWINDOW;
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE as u32,
            &backdrop as *const _ as _,
            std::mem::size_of_val(&backdrop) as u32,
        );
    }
}

/// Handles main settings window messages on the UI thread.
unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let mut app = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut App;
    if message == WM_NCCREATE {
        let create = &*(lparam as *const CREATESTRUCTW);
        let initial = Box::from_raw(create.lpCreateParams as *mut Page);
        app = Box::into_raw(Box::new(App::new(hwnd, *initial)));
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, app as isize);
    }
    if app.is_null() {
        return DefWindowProcW(hwnd, message, wparam, lparam);
    }
    let state = &mut *app;
    match message {
        WM_CREATE => {
            apply_dwm(hwnd, state.theme.light);
            0
        }
        WM_WORK => {
            state.apply_work(*Box::from_raw(lparam as *mut WorkResult));
            0
        }
        WM_REMOTE => {
            state.remote(*Box::from_raw(lparam as *mut remote::Request));
            0
        }
        WM_LOAD_LOOK => {
            state.look_changed();
            0
        }
        WM_COPYDATA => {
            let data = &*(lparam as *const COPYDATASTRUCT);
            if data.dwData == DATA_TAG && !data.lpData.is_null() {
                let page = String::from_utf8_lossy(std::slice::from_raw_parts(
                    data.lpData as *const u8,
                    data.cbData as usize,
                ))
                .to_string();
                let target = parse_page(&page, &state.entries);
                state.navigate(target);
                ShowWindow(hwnd, SW_RESTORE);
                SetForegroundWindow(hwnd);
                InvalidateRect(hwnd, std::ptr::null(), 0);
                1
            } else {
                0
            }
        }
        WM_TIMER if wparam == TIMER_FILE => {
            state.watch_file();
            0
        }
        WM_SETTINGCHANGE | WM_THEMECHANGED => {
            state.look_changed();
            0
        }
        WM_DPICHANGED => {
            let suggested = &*(lparam as *const RECT);
            SetWindowPos(
                hwnd,
                0,
                suggested.left,
                suggested.top,
                suggested.right - suggested.left,
                suggested.bottom - suggested.top,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            state.look_changed();
            0
        }
        WM_GETMINMAXINFO => {
            let info = &mut *(lparam as *mut MINMAXINFO);
            let scale = taskbar_dock::system::scale_for(hwnd);
            info.ptMinTrackSize.x = layout::px(640.0, scale);
            info.ptMinTrackSize.y = layout::px(420.0, scale);
            0
        }
        WM_SIZE => {
            InvalidateRect(hwnd, std::ptr::null(), 0);
            0
        }
        WM_MOUSEWHEEL => {
            state.scroll_by(-(((wparam >> 16) as u16 as i16) as i32) / 2);
            InvalidateRect(hwnd, std::ptr::null(), 0);
            0
        }
        WM_LBUTTONDOWN => {
            state.click((lparam as i16) as i32, ((lparam >> 16) as i16) as i32);
            InvalidateRect(hwnd, std::ptr::null(), 0);
            0
        }
        WM_LBUTTONDBLCLK => {
            state.click((lparam as i16) as i32, ((lparam >> 16) as i16) as i32);
            if state.focus.is_some_and(|index| {
                matches!(
                    state.rows.get(index).map(|row| &row.setting.kind),
                    Some(Kind::Number { .. })
                )
            }) {
                state.cancel_slider();
                state.begin_edit();
            }
            InvalidateRect(hwnd, std::ptr::null(), 0);
            0
        }
        WM_MOUSEMOVE => {
            state.move_slider((lparam as i16) as i32);
            state.hover = Some(((lparam as i16) as i32, ((lparam >> 16) as i16) as i32));
            let mut tracking = TRACKMOUSEEVENT {
                cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: hwnd,
                dwHoverTime: 0,
            };
            TrackMouseEvent(&mut tracking);
            InvalidateRect(hwnd, std::ptr::null(), 0);
            0
        }
        WM_MOUSE_LEAVE => {
            state.hover = None;
            InvalidateRect(hwnd, std::ptr::null(), 0);
            0
        }
        WM_LBUTTONUP => {
            state.finish_slider();
            state.reveal_secret = false;
            0
        }
        WM_CAPTURECHANGED => {
            state.cancel_slider();
            state.reveal_secret = false;
            0
        }
        WM_CHAR => {
            if let Some((index, edit)) = state.edit.as_mut() {
                match wparam as u16 {
                    8 => edit.backspace(),
                    127 => edit.delete(),
                    c if c >= 32 => {
                        if let Some(ch) = char::from_u32(c as u32) {
                            edit.insert(&ch.to_string());
                        }
                    }
                    _ => {}
                }
                if let Some(row) = state.rows.get_mut(*index) {
                    row.draft = Some(edit.text().to_string());
                }
                InvalidateRect(hwnd, std::ptr::null(), 0);
            }
            0
        }
        WM_KEYDOWN => {
            match crate::settings_app::keys::target(state.edit.is_some(), state.dropdown.is_some())
            {
                model::KeyTarget::Text => state.edit_key(wparam as u32),
                model::KeyTarget::Dropdown | model::KeyTarget::Page => {
                    state.page_key(wparam as u32)
                }
            }
            0
        }
        WM_PAINT => {
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            let dc = BeginPaint(hwnd, &mut ps);
            let mut client: RECT = std::mem::zeroed();
            GetClientRect(hwnd, &mut client);
            let memory = CreateCompatibleDC(dc);
            let bitmap = CreateCompatibleBitmap(dc, client.right, client.bottom);
            let old = SelectObject(memory, bitmap);
            let content_width = client.right
                - layout::px(
                    theme::app::NAV_WIDTH + theme::app::CONTENT_PADDING * 2.0,
                    state.scale,
                );
            let card_heights = state
                .rows
                .iter()
                .map(|row| state.measured_card_height(row, content_width))
                .collect::<Vec<_>>();
            let error_heights = state
                .rows
                .iter()
                .map(|row| state.measured_error_height(row, content_width))
                .collect::<Vec<_>>();
            let frame = paint::Frame {
                page: &state.page,
                theme: &state.theme,
                fonts: &state.fonts,
                scale: state.scale,
                nav: &state.nav,
                widgets: &state.widgets,
                busy_actions: &state.busy_actions,
                rows: &state.rows,
                edit: state.edit.as_ref().map(|(index, edit)| (*index, edit)),
                hover: state.hover,
                card_heights: &card_heights,
                error_heights: &error_heights,
                page_error: match &state.page {
                    Page::Widget(name) => state
                        .entries
                        .iter()
                        .find(|item| item.entry.name == *name)
                        .and_then(|item| item.describe.as_ref().err())
                        .map(String::as_str),
                    _ => None,
                },
                badge: state.badge.as_deref(),
                banner: state.banner.as_deref(),
                scroll: state.scroll,
                focus: state.focus,
                focus_reset: state.focus_item.and_then(|focus| {
                    (focus.kind == crate::settings_app::keys::FocusKind::Reset)
                        .then_some(focus.index)
                }),
                focus_nav: state.focus_item.and_then(|focus| {
                    (focus.kind == crate::settings_app::keys::FocusKind::Navigation)
                        .then_some(focus.index)
                }),
                focus_home: state.home_focus(),
                dropdown: state.dropdown,
                reveal_secret: state.reveal_secret,
                height: client.bottom,
            };
            paint::draw(memory, client.right, client.bottom, &frame);
            BitBlt(dc, 0, 0, client.right, client.bottom, memory, 0, 0, SRCCOPY);
            SelectObject(memory, old);
            DeleteObject(bitmap);
            DeleteDC(memory);
            EndPaint(hwnd, &ps);
            0
        }
        WM_CLOSE => {
            DestroyWindow(hwnd);
            0
        }
        WM_DESTROY => {
            KillTimer(hwnd, TIMER_FILE);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            drop(Box::from_raw(app));
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

/// Converts a page name to a known page, defaulting unknown names to the overview.
fn parse_page(name: &str, entries: &[work::LoadedEntry]) -> Page {
    match name {
        "widgets" => Page::Widgets,
        "dock" => Page::Dock,
        "developer" => Page::Developer,
        name if entries.iter().any(|item| item.entry.name == name) => Page::Widget(name.into()),
        _ => Page::Widgets,
    }
}

/// Converts the library process state to its stable wire and display label.
fn state_name(state: control::State) -> &'static str {
    match state {
        control::State::Running => "running",
        control::State::Stopped => "stopped",
        control::State::NotResponding => "not responding",
        control::State::KeepsCrashing => "keeps crashing",
    }
}

/// Posts an already constructed worker result to the UI thread.
fn post_result(hwnd: HWND, result: WorkResult) {
    let data = Box::into_raw(Box::new(result));
    if unsafe { PostMessageW(hwnd, WM_WORK, 0, data as LPARAM) } == 0 {
        unsafe {
            drop(Box::from_raw(data));
        }
    }
}

/// Invalidates the client area for a later paint.
fn invalidate(hwnd: HWND) {
    unsafe {
        InvalidateRect(hwnd, std::ptr::null(), 0);
    }
}

/// Posts a close request without blocking the window procedure.
fn post_close(hwnd: HWND) {
    unsafe {
        PostMessageW(hwnd, WM_CLOSE, 0, 0);
    }
}

/// Opens the current settings.json path in Notepad using CreateProcessW.
fn open_notepad() {
    use windows_sys::Win32::System::Threading::*;
    let exe = wide("notepad.exe");
    let file = wide(&taskbar_dock::settings::path().to_string_lossy());
    unsafe {
        let mut startup: STARTUPINFOW = std::mem::zeroed();
        startup.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        let mut process: PROCESS_INFORMATION = std::mem::zeroed();
        let mut command = [
            exe[..exe.len() - 1].to_vec(),
            file[..file.len() - 1].to_vec(),
        ]
        .concat();
        command.push(0);
        if CreateProcessW(
            std::ptr::null(),
            command.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            0,
            0,
            std::ptr::null(),
            std::ptr::null(),
            &startup,
            &mut process,
        ) != 0
        {
            CloseHandle(process.hThread);
            CloseHandle(process.hProcess);
        }
    }
}
