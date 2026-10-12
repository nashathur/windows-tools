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

/// Defers a widget-page request until the registry worker has resolved its entry.
fn initial_page(initial: Page) -> (Page, Option<Page>) {
    if matches!(initial, Page::Widget(_)) {
        (Page::Widgets, Some(initial))
    } else {
        (initial, None)
    }
}

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
    /// A requested widget page held until the registry has loaded.
    pending_page: Option<Page>,
    /// Registered entries with descriptions.
    entries: Vec<work::LoadedEntry>,
    /// Whether the registry worker has delivered the current entries.
    registry_loaded: bool,
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
    /// High surrogate waiting for its WM_CHAR low surrogate.
    pending_high_surrogate: Option<u16>,
    /// Numeric slider currently held, with its uncommitted value.
    slider_drag: Option<(usize, f64, Rect)>,
    /// Loaded entry list shared with the status worker.
    status_entries: Arc<RwLock<Vec<registry::Entry>>>,
    /// Serialized writer job sender.
    writer: Sender<WriteJob>,
    /// Latest write sequence.
    sequence: u64,
    /// Latest badge generation.
    badge_sequence: u64,
    /// Badge generation attached to each pending write.
    write_badge_sequences: HashMap<u64, u64>,
    /// Latest accepted status round.
    status_round: u64,
    /// Whether the repeating status worker is already running.
    status_started: bool,
    /// Requests an immediate status round on the status worker.
    status_refresh: Option<Sender<()>>,
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
    /// Developer diagnostics refreshed by background workers.
    developer_details: Vec<Value>,
    /// Masked pretty-printed settings snapshot.
    developer_raw: String,
    /// Scroll offset inside the Developer raw-settings box.
    developer_raw_scroll: i32,
    /// Deferred registry action replies, keyed by the action name.
    pending_registry: HashMap<String, (HWND, HWND, u64)>,
    /// Pointer position used for hover highlights.
    hover: Option<(i32, i32)>,
}

impl App {
    /// Creates the UI model and starts background loading after the window exists.
    fn new(hwnd: HWND, initial: Page) -> Self {
        let (page, pending_page) = initial_page(initial);
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
            page,
            pending_page,
            entries: Vec::new(),
            registry_loaded: false,
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
            pending_high_surrogate: None,
            slider_drag: None,
            status_entries: Arc::new(RwLock::new(Vec::new())),
            writer,
            sequence: 0,
            badge_sequence: 0,
            write_badge_sequences: HashMap::new(),
            status_round: 0,
            status_started: false,
            status_refresh: None,
            mtime: taskbar_dock::settings::mtime(),
            nav: Vec::new(),
            pending_remote: HashMap::new(),
            pending_actions: HashMap::new(),
            busy_actions: HashMap::new(),
            developer_details: Vec::new(),
            developer_raw: String::new(),
            developer_raw_scroll: 0,
            pending_registry: HashMap::new(),
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
        self.prepare_for_row_rebuild();
        self.rows.clear();
        match &self.page {
            Page::Widget(name) => {
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
            Page::Dock => {
                let current = self
                    .settings
                    .get("dock")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                self.rows = model::dock_rows(&current, &taskbar_dock::style::dock_defaults());
            }
            Page::Developer => {
                self.rows = model::advanced_rows(&self.entries, &self.settings);
            }
            Page::Widgets => {}
        }
        self.scroll = 0;
    }

    /// Resolves transient controls before replacing the row model they refer to.
    fn prepare_for_row_rebuild(&mut self) {
        if self.edit.is_some() {
            self.commit_edit();
            if self.edit.is_some() {
                self.cancel_edit();
            }
        }
        self.dropdown = None;
        self.cancel_slider();
        self.reveal_secret = false;
        unsafe {
            ReleaseCapture();
        }
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
                    self.status_refresh =
                        Some(work::status_loop(self.hwnd, self.status_entries.clone()));
                    self.status_started = true;
                }
                self.entries = entries;
                self.registry_loaded = true;
                if let Some(page) = self.pending_page.take() {
                    self.page = resolve_pending_page(page, &self.entries);
                }
                self.rebuild_nav();
                self.rebuild_rows();
                work::read_developer(
                    self.hwnd,
                    self.entries.clone(),
                    self.last_status.clone(),
                    true,
                );
            }
            WorkResult::Status { round, statuses } if round > self.status_round => {
                self.status_round = round;
                self.last_status = statuses.clone();
                self.rebuild_widgets();
                work::read_developer(self.hwnd, self.entries.clone(), statuses, false);
            }
            WorkResult::Status { .. } => {}
            WorkResult::File { mtime, .. } if file_snapshot_is_stale(mtime, self.mtime) => {
                work::read_file(self.hwnd);
            }
            WorkResult::File {
                result: Ok(value),
                mtime,
            } => {
                let registry_changed = self.settings.get("widgets") != value.get("widgets");
                self.banner = None;
                self.settings = value;
                self.mtime = mtime;
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
                if matches!(self.page, Page::Dock) {
                    let fixed = self
                        .settings
                        .get("dock")
                        .and_then(|dock| dock.get("tint_fixed"))
                        .is_some_and(Value::is_number);
                    for row in &mut self.rows {
                        if row.key.starts_with("tint_dark.") || row.key.starts_with("tint_light.") {
                            row.enabled = !fixed;
                        }
                    }
                }
                self.rebuild_widgets();
                work::read_developer(
                    self.hwnd,
                    self.entries.clone(),
                    self.last_status.clone(),
                    self.registry_loaded,
                );
            }
            WorkResult::File {
                result: Err(_),
                mtime,
            } => {
                self.mtime = mtime;
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
                let write_badge_sequence = self.write_badge_sequences.remove(&sequence);
                let error = result.as_ref().err().cloned();
                if let Some(row) = self
                    .rows
                    .iter_mut()
                    .find(|row| row.section == section && row.key == key)
                {
                    let value = result.as_ref().ok().cloned();
                    row.apply_result(sequence, value, error.clone());
                }
                if sequence == self.sequence
                    && badge_result_is_current(write_badge_sequence, self.badge_sequence)
                {
                    let title = self
                        .entries
                        .iter()
                        .find(|item| item.entry.name == section)
                        .and_then(|item| item.describe.as_ref().ok())
                        .map(|desc| desc.title.as_str())
                        .unwrap_or(&section);
                    let follow_error = followed.err();
                    self.badge = Some(error.map_or_else(
                        || {
                            follow_error.map_or("Saved".into(), |reason| {
                                let names = reason
                                    .strip_suffix(" hasn't confirmed")
                                    .unwrap_or(&reason)
                                    .split(", ");
                                let titles = names
                                    .map(|name| {
                                        self.entries
                                            .iter()
                                            .find(|item| item.entry.name == name)
                                            .and_then(|item| item.describe.as_ref().ok())
                                            .map(|desc| desc.title.as_str())
                                            .unwrap_or(name)
                                    })
                                    .collect::<Vec<_>>()
                                    .join(", ");
                                if section == "dock" {
                                    format!("Saved; {titles} haven't confirmed")
                                } else {
                                    format!("Saved; {title} hasn't confirmed")
                                }
                            })
                        },
                        |error| format!("Not saved: {error}"),
                    ));
                    self.badge_until = self
                        .badge
                        .as_deref()
                        .filter(|text| *text == "Saved")
                        .map(|_| Instant::now() + Duration::from_secs(2));
                }
                work::read_file(self.hwnd);
                if let Some((sender, source, id)) = self.pending_remote.remove(&sequence) {
                    invalidate(self.hwnd);
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
                warning,
            } => {
                let success = result.is_ok();
                let error = result.as_ref().err().cloned();
                self.busy_actions.remove(&name);
                self.bump_badge_sequence();
                self.badge = Some(result.map_or_else(
                    |e| format!("Couldn't {action}: {e}"),
                    |_| match action.as_str() {
                        "start" => warning.clone().map_or_else(
                            || "Started".into(),
                            |warning| format!("Started; {warning}"),
                        ),
                        "quit" => "Quit".into(),
                        _ => "Restarted".into(),
                    },
                ));
                self.badge_until = Some(Instant::now() + Duration::from_secs(2));
                if let Some(refresh) = &self.status_refresh {
                    let _ = refresh.send(());
                }
                if let Some((sender, source, id)) = self.pending_actions.remove(&(name, action)) {
                    invalidate(self.hwnd);
                    unsafe {
                        UpdateWindow(self.hwnd);
                    }
                    remote::reply(
                        sender,
                        source,
                        id,
                        &json!({"ok":success,"error":error,"warning":warning}),
                    );
                }
            }
            WorkResult::Developer { details, raw } => {
                self.developer_details = details;
                if let Some(raw) = raw {
                    self.developer_raw = raw;
                    self.developer_raw_scroll = 0;
                }
            }
            WorkResult::RegistryMutation {
                operation,
                requested,
                result,
            } => {
                let succeeded = result.is_ok();
                let name = result.as_ref().ok().cloned().unwrap_or_default();
                self.bump_badge_sequence();
                self.badge = Some(result.map_or_else(
                    |error| {
                        if operation == "register" {
                            format!("Not registered: {error}")
                        } else {
                            format!("Not removed: {error}")
                        }
                    },
                    |name| {
                        if operation == "register" {
                            format!("Registered {name}")
                        } else {
                            format!("Removed {name}")
                        }
                    },
                ));
                self.badge_until = None;
                if succeeded {
                    work::load_registry(self.hwnd);
                }
                let key = format!("{operation}:{requested}");
                if let Some((sender, source, id)) = self.pending_registry.remove(&key) {
                    invalidate(self.hwnd);
                    unsafe {
                        UpdateWindow(self.hwnd);
                    }
                    remote::reply(
                        sender,
                        source,
                        id,
                        &json!({"ok":succeeded,"name":name,"error":self.badge}),
                    );
                }
            }
        }
        invalidate(self.hwnd);
    }

    /// Navigates to a page and rebuilds page-local state.
    fn navigate(&mut self, page: Page) {
        if matches!(page, Page::Widget(_)) && !self.registry_loaded {
            self.pending_page = Some(page);
            return;
        }
        self.pending_page = None;
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

    /// Advances the badge generation so an older async result cannot replace it.
    fn bump_badge_sequence(&mut self) -> u64 {
        self.badge_sequence = self.badge_sequence.wrapping_add(1).max(1);
        self.badge_sequence
    }

    /// Associates a queued write with the current badge generation.
    fn mark_write_badge(&mut self, sequence: u64) {
        let badge_sequence = self.bump_badge_sequence();
        self.write_badge_sequences.insert(sequence, badge_sequence);
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
        let startup_enabled = if key == "startup" {
            let value = match &input {
                control::Input::Text(text) => describe::parse(&row.setting, text),
                control::Input::Value(value) => Ok(value.clone()),
            };
            match value.ok().and_then(|value| value.as_bool()) {
                Some(enabled) => Some(enabled),
                None => {
                    self.rows[index].error = Some("expected a switch value".into());
                    invalidate(self.hwnd);
                    return;
                }
            }
        } else {
            None
        };
        let destination = if section == "dock" {
            Ok((None, Some(control::Target::dock())))
        } else if key == "startup" {
            self.row_entry(&section)
                .map(|item| Ok((Some(item.entry.clone()), None)))
                .unwrap_or_else(|| Err("widget not registered".to_string()))
        } else {
            match self.row_entry(&section) {
                None => Err("widget not registered".to_string()),
                Some(item) => match &item.describe {
                    Ok(desc) => Ok((
                        Some(item.entry.clone()),
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
        self.mark_write_badge(sequence);
        self.rows[index].pending = Some(sequence);
        self.rows[index].error = None;
        if key == "startup" {
            let Some(entry) = entry else {
                return;
            };
            let _ = self.writer.send(WriteJob::Startup {
                sequence,
                name: section,
                exe: entry.exe,
                enabled: startup_enabled.unwrap_or(false),
            });
        } else if let Some(target) = target {
            let _ = self.writer.send(WriteJob::Apply {
                sequence,
                target,
                key,
                input,
                follow: entry.map_or_else(
                    || self.entries.iter().map(|item| item.entry.clone()).collect(),
                    |entry| vec![entry],
                ),
            });
        }
        invalidate(self.hwnd);
    }

    /// Returns one protocol state response with secret values masked.
    fn state(&self) -> Value {
        let rows: Vec<Value> = match self.page {
            Page::Widget(_) | Page::Dock | Page::Developer => self
                .rows
                .iter()
                .map(|row| {
                    let (reported_control, auto) = match &row.setting.kind {
                        Kind::Auto { inner, word, .. } => (
                            model::control_for(inner),
                            Some(row.value.as_str() == Some(word)),
                        ),
                        _ => (row.control(), None),
                    };
                    let kind = match reported_control {
                        model::Control::Switch | model::Control::Auto => "switch",
                        model::Control::Segments => "segments",
                        model::Control::Dropdown => "dropdown",
                        model::Control::Slider => "slider",
                        model::Control::Text => "text",
                        model::Control::Color => "color",
                        model::Control::Secret => "secret",
                    };
                    let secret = taskbar_dock::describe::is_secret_kind(&row.setting.kind)
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
                        "enabled": row.enabled,
                        "auto": auto
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
                        "status_text": widget_status_text(widget),
                        "pid": widget.pid,
                        "reason": widget.reason,
                        "show_on_taskbar": widget.show_on_taskbar
                    })
                })
                .collect(),
        };
        let cards = if matches!(self.page, Page::Developer) {
            let registered = model::registered_widgets(&self.entries)
                .into_iter()
                .map(|widget| {
                    json!({
                        "name": widget.name,
                        "exe": widget.exe,
                        "error": widget.error
                    })
                })
                .collect::<Vec<_>>();
            json!({
                "advanced": rows.clone(),
                "details": self.developer_details,
                "registered": registered,
                "raw": self.developer_raw,
                "commands": model::developer_commands()
            })
        } else {
            Value::Null
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
            crate::settings_app::keys::FocusKind::DeveloperAction => {
                format!("developer_action:{}", focus.index)
            }
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
            "rows": rows,
            "cards": cards
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
        let registry_key = request.args.get(1).and_then(|value| {
            value
                .strip_prefix("remove:")
                .map(|name| (format!("remove:{name}"), name.to_string()))
        });
        let reply = match taskbar_dock::ctl::parse_settings(&request.args) {
            Err(error) => json!({"ok":false,"error":error,"kind":"usage"}),
            Ok(command) => match command {
                taskbar_dock::ctl::SettingsCommand::State => self.state(),
                taskbar_dock::ctl::SettingsCommand::Page(name) => {
                    let page = requested_page(&name, &self.entries, self.registry_loaded);
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
        invalidate(self.hwnd);
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
            if let Some((key, _)) = registry_key {
                self.pending_registry.insert(key, (sender, source, id));
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
        let destination = if section == "dock" {
            Ok((None, Some(control::Target::dock())))
        } else if key == "startup" {
            self.row_entry(&section)
                .map(|item| (Some(item.entry.clone()), None))
                .ok_or_else(|| "widget not registered".to_string())
        } else {
            self.row_entry(&section)
                .ok_or_else(|| "widget not registered".to_string())
                .and_then(|item| {
                    item.describe
                        .as_ref()
                        .map(|desc| {
                            (
                                Some(item.entry.clone()),
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
            let Some(entry) = entry else {
                return Err("widget not registered".into());
            };
            self.sequence = self.sequence.wrapping_add(1).max(1);
            let sequence = self.sequence;
            self.mark_write_badge(sequence);
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
        self.mark_write_badge(self.sequence);
        self.rows[index].pending = Some(self.sequence);
        let _ = self.writer.send(WriteJob::Reset {
            sequence: self.sequence,
            target,
            key,
            follow: entry.map_or_else(
                || self.entries.iter().map(|item| item.entry.clone()).collect(),
                |entry| vec![entry],
            ),
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
        if let Some(index) = name
            .strip_prefix("copy:")
            .and_then(|value| value.parse::<usize>().ok())
        {
            let commands = model::developer_commands();
            if let Some(command) = commands.get(index) {
                taskbar_dock::text_edit::clipboard_write(self.hwnd, command);
                self.bump_badge_sequence();
                self.badge = Some("Copied".into());
                self.badge_until = Some(Instant::now() + Duration::from_secs(2));
                return json!({"ok":true});
            }
            return json!({"ok":false,"error":"command not found"});
        }
        if let Some(widget) = name.strip_prefix("remove:") {
            let _ = self.writer.send(WriteJob::Remove {
                name: widget.into(),
            });
            return json!({"ok":true});
        }
        if name == "notepad" {
            return match self.open_settings_notepad() {
                Ok(()) => json!({"ok":true}),
                Err(error) => json!({"ok":false,"error":error}),
            };
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

    /// Opens settings.json and records a visible badge if Notepad cannot start.
    fn open_settings_notepad(&mut self) -> Result<(), String> {
        match launch_notepad() {
            Ok(()) => Ok(()),
            Err(error) => {
                self.bump_badge_sequence();
                self.badge = Some(format!("Not opened: {error}"));
                self.badge_until = Some(Instant::now() + Duration::from_secs(4));
                invalidate(self.hwnd);
                Err(error)
            }
        }
    }

    /// Opens the standard executable picker and queues registry work off the UI thread.
    fn register_widget_dialog(&mut self) {
        use windows_sys::Win32::UI::Controls::Dialogs::{
            GetOpenFileNameW, OFN_FILEMUSTEXIST, OFN_PATHMUSTEXIST, OPENFILENAMEW,
        };
        let filter = wide("Programs (*.exe)\0*.exe\0\0");
        let mut file = vec![0u16; 32768];
        let mut dialog: OPENFILENAMEW = unsafe { std::mem::zeroed() };
        dialog.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
        dialog.hwndOwner = self.hwnd;
        dialog.lpstrFilter = filter.as_ptr();
        dialog.lpstrFile = file.as_mut_ptr();
        dialog.nMaxFile = file.len() as u32;
        dialog.Flags = OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST;
        if unsafe { GetOpenFileNameW(&mut dialog) } == 0 {
            return;
        }
        let length = file
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(file.len());
        let exe = std::path::PathBuf::from(String::from_utf16_lossy(&file[..length]));
        let _ = self.writer.send(WriteJob::Register { exe });
        self.bump_badge_sequence();
        self.badge = Some("Registering".into());
        self.badge_until = None;
        invalidate(self.hwnd);
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
            if taskbar_dock::describe::is_secret_kind(&row.setting.kind) {
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
        let order = if matches!(self.page, Page::Developer) {
            crate::settings_app::keys::developer_focus_order(
                nav_count,
                &self.rows,
                self.developer_action_rects().len(),
            )
        } else {
            crate::settings_app::keys::focus_order(nav_count, &self.rows, action_count)
        };
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
        } else if target.kind == crate::settings_app::keys::FocusKind::DeveloperAction {
            self.ensure_developer_control_visible(target.index);
        }
    }

    /// Returns Developer action rectangles in keyboard order.
    fn developer_action_rects(&self) -> Vec<Rect> {
        let nav_width = layout::px(theme::app::NAV_WIDTH, self.scale);
        let width = unsafe {
            let mut rect: RECT = std::mem::zeroed();
            GetClientRect(self.hwnd, &mut rect);
            rect.right - nav_width - 2 * layout::px(theme::app::CONTENT_PADDING, self.scale)
        };
        let content_top = layout::px(theme::app::HEADER_HEIGHT, self.scale)
            + self.banner.as_ref().map_or(0, |_| {
                layout::px(theme::space::XL * 2.0 + theme::space::S, self.scale)
            })
            + developer_warning_height(&self.page, self.scale);
        let heights = self
            .rows
            .iter()
            .map(|row| self.measured_card_height(row, width))
            .collect::<Vec<_>>();
        let advanced = layout::page_layout(&self.rows, &heights, width, self.scale, content_top, 0);
        let cards = layout::developer_layout(
            content_top + advanced.content_height + layout::px(theme::app::CARD_GAP, self.scale),
            width,
            self.scale,
            self.scroll,
            self.developer_details.len(),
            self.entries.len(),
            model::developer_commands().len(),
        );
        let mut rects = vec![cards.register_button];
        rects.extend(cards.remove_buttons);
        rects.push(cards.raw_notepad);
        rects.extend(cards.copy_buttons);
        rects
    }

    /// Focuses a Developer action and scrolls it into view.
    fn focus_developer_control(&mut self, index: usize) {
        self.focus_item = Some(crate::settings_app::keys::Focus {
            kind: crate::settings_app::keys::FocusKind::DeveloperAction,
            index,
        });
        self.focus = None;
        self.ensure_developer_control_visible(index);
    }

    /// Activates the currently focused Developer button.
    fn activate_developer_focus(&mut self) {
        let Some(focus) = self
            .focus_item
            .filter(|focus| focus.kind == crate::settings_app::keys::FocusKind::DeveloperAction)
        else {
            return;
        };
        let remove_start = 1;
        let notepad_index = remove_start + self.entries.len();
        let copy_start = notepad_index + 1;
        match focus.index {
            0 => self.register_widget_dialog(),
            index if (remove_start..notepad_index).contains(&index) => {
                if let Some(item) = self.entries.get(index - remove_start) {
                    let _ = self.writer.send(WriteJob::Remove {
                        name: item.entry.name.clone(),
                    });
                }
            }
            index if index == notepad_index => {
                let _ = self.open_settings_notepad();
            }
            index if index >= copy_start => {
                let _ = self.click_remote(&format!("copy:{}", index - copy_start));
            }
            _ => {}
        }
    }

    /// Brings a focused Developer action into the content viewport.
    fn ensure_developer_control_visible(&mut self, index: usize) {
        let Some(rect) = self.developer_action_rects().get(index).copied() else {
            return;
        };
        let visible_top = layout::px(theme::app::HEADER_HEIGHT, self.scale)
            + self.banner.as_ref().map_or(0, |_| {
                layout::px(theme::space::XL * 2.0 + theme::space::S, self.scale)
            })
            + developer_warning_height(&self.page, self.scale);
        let client_bottom = unsafe {
            let mut client: RECT = std::mem::zeroed();
            GetClientRect(self.hwnd, &mut client);
            client.bottom
        };
        if rect.top < visible_top {
            self.scroll = (self.scroll - (visible_top - rect.top)).max(0);
        } else if rect.bottom > client_bottom {
            self.scroll += rect.bottom - client_bottom;
        }
        invalidate(self.hwnd);
    }

    /// Returns the option index under an open dropdown item.
    fn dropdown_option_at_point(&self, index: usize, x: i32, y: i32) -> Option<usize> {
        let row = self.rows.get(index)?;
        let nav_width = layout::px(theme::app::NAV_WIDTH, self.scale);
        let width = unsafe {
            let mut rect: RECT = std::mem::zeroed();
            GetClientRect(self.hwnd, &mut rect);
            rect.right - nav_width - 2 * layout::px(theme::app::CONTENT_PADDING, self.scale)
        };
        let left = nav_width + layout::px(theme::app::CONTENT_PADDING, self.scale);
        let banner_height = self.banner.as_ref().map_or(0, |_| {
            layout::px(theme::space::XL * 2.0 + theme::space::S, self.scale)
        });
        let content_top = layout::px(theme::app::HEADER_HEIGHT, self.scale)
            + banner_height
            + developer_warning_height(&self.page, self.scale);
        let heights = self
            .rows
            .iter()
            .map(|row| self.measured_card_height(row, width))
            .collect::<Vec<_>>();
        let positions = layout::page_layout(
            &self.rows,
            &heights,
            width,
            self.scale,
            content_top,
            self.scroll,
        );
        let top = positions.cards.get(index)?.top;
        let card_height = positions.cards[index].bottom - top;
        let local = layout::row_rects(
            width,
            top,
            card_height,
            layout::px(theme::app::CONTROL_WIDTH, self.scale),
            layout::full_line(row),
            row.reset_visible(),
            self.measured_error_height(row, width),
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
            row,
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
        layout::dropdown_option_at(&control.parts, x, y)
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
        let top = layout::px(theme::app::HEADER_HEIGHT, self.scale)
            + self.banner.as_ref().map_or(0, |_| {
                layout::px(theme::space::XL * 2.0 + theme::space::S, self.scale)
            })
            + developer_warning_height(&self.page, self.scale);
        let heights = self
            .rows
            .iter()
            .map(|row| self.measured_card_height(row, width))
            .collect::<Vec<_>>();
        let positioned = layout::page_layout(&self.rows, &heights, width, self.scale, top, 0);
        if let Some(card) = positioned.cards.get(index) {
            let viewport_top = layout::px(theme::app::HEADER_HEIGHT, self.scale);
            let viewport_bottom = unsafe {
                let mut rect: RECT = std::mem::zeroed();
                GetClientRect(self.hwnd, &mut rect);
                rect.bottom
            };
            if card.top - self.scroll < viewport_top {
                self.scroll = card.top - viewport_top;
            }
            if card.bottom - self.scroll > viewport_bottom {
                self.scroll = card.bottom - viewport_bottom;
            }
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
        }) + developer_warning_height(&self.page, self.scale);
        let content = match &self.page {
            Page::Widgets => {
                (self.widgets.len() as i32)
                    * (layout::px(theme::app::NAV_ROW_HEIGHT * 2.0, self.scale)
                        + layout::px(theme::app::CARD_GAP, self.scale))
            }
            Page::Widget(_) | Page::Dock | Page::Developer => {
                let width = unsafe {
                    let mut rect: RECT = std::mem::zeroed();
                    GetClientRect(self.hwnd, &mut rect);
                    rect.right
                        - layout::px(
                            theme::app::NAV_WIDTH + theme::app::CONTENT_PADDING * 2.0,
                            self.scale,
                        )
                };
                let heights = self
                    .rows
                    .iter()
                    .map(|row| self.measured_card_height(row, width))
                    .collect::<Vec<_>>();
                let advanced = layout::page_layout(&self.rows, &heights, width, self.scale, 0, 0);
                if matches!(self.page, Page::Developer) {
                    let custom = layout::developer_layout(
                        advanced.content_height + layout::px(theme::app::CARD_GAP, self.scale),
                        width,
                        self.scale,
                        0,
                        self.developer_details.len(),
                        self.entries.len(),
                        model::developer_commands().len(),
                    );
                    // The warning line is already in `banner`; the cards start one gap down.
                    advanced.content_height
                        + layout::px(theme::app::CARD_GAP, self.scale)
                        + custom.content_height
                } else {
                    advanced.content_height
                }
            }
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
            Kind::Choice { options, other } => {
                if row.control() == model::Control::Dropdown {
                    // Open on the current choice ("Other…" when the value is no option).
                    let current = dropdown_selection(options, *other, &row.value);
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

    /// Selects an option in an Auto row's inner choice control.
    fn choose_auto_dropdown(&mut self, index: usize, option: usize) {
        let Some(row) = self.rows.get(index) else {
            self.dropdown = None;
            return;
        };
        let Kind::Auto { inner, .. } = &row.setting.kind else {
            self.dropdown = None;
            return;
        };
        let Kind::Choice { options, other } = &**inner else {
            self.dropdown = None;
            return;
        };
        if *other && option == options.len() {
            self.dropdown = None;
            self.rows[index].choice_text_mode = true;
            self.begin_edit_with(index, String::new());
        } else if let Some((value, _)) = options.get(option) {
            self.dropdown = None;
            self.write_row(index, control::Input::Value(value.clone()));
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
            self.bump_badge_sequence();
            self.badge = None;
            self.badge_until = None;
        }
    }

    /// Commits a text edit with the shared description parser.
    fn commit_edit(&mut self) {
        let Some((index, edit)) = self.edit.take() else {
            return;
        };
        let Some(setting) = self.rows.get(index).map(|row| row.setting.clone()) else {
            return;
        };
        if let Err(error) = describe::parse(&setting, edit.text()) {
            if let Some(row) = self.rows.get_mut(index) {
                row.draft = Some(edit.text().into());
                row.error = Some(error);
                self.edit = Some((index, edit));
            }
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
        let top = layout::px(theme::app::HEADER_HEIGHT, self.scale)
            + self.banner.as_ref().map_or(0, |_| {
                layout::px(theme::space::XL * 2.0 + theme::space::S, self.scale)
            })
            + developer_warning_height(&self.page, self.scale);
        let heights = self
            .rows
            .iter()
            .map(|row| self.measured_card_height(row, width))
            .collect::<Vec<_>>();
        let positions =
            layout::page_layout(&self.rows, &heights, width, self.scale, top, self.scroll);
        for (row_index, row) in self.rows.iter().enumerate() {
            let card_top = positions.cards[row_index].top;
            let full_line = layout::full_line(row);
            let card_height = positions.cards[row_index].bottom - card_top;
            if row_index == index {
                let local = layout::row_rects(
                    width,
                    card_top,
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
        invalidate(self.hwnd);
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
                } else if focus.kind == crate::settings_app::keys::FocusKind::DeveloperAction {
                    self.activate_developer_focus();
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
            } else if self.focus_item.is_some_and(|focus| {
                focus.kind == crate::settings_app::keys::FocusKind::DeveloperAction
            }) {
                self.activate_developer_focus();
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
        let kind = match &row.setting.kind {
            Kind::Auto { inner, word, .. } if row.value.as_str() != Some(word) => &**inner,
            kind => kind,
        };
        match kind {
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
                if model::control_for(kind) == model::Control::Segments
                    || model::control_for(kind) == model::Control::Dropdown =>
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
        if let Some((index, _)) = open_dropdown {
            if let Some(row) = self.rows.get(index) {
                if let Some(option) = self.dropdown_option_at_point(index, x, y) {
                    if matches!(row.setting.kind, Kind::Auto { .. }) {
                        self.choose_auto_dropdown(index, option);
                    } else {
                        self.choose_dropdown(index, option);
                    }
                }
            }
            invalidate(self.hwnd);
            return;
        }
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
                let _ = self.open_settings_notepad();
                return;
            }
        }
        if self.banner.is_some() {
            return;
        }
        if matches!(self.page, Page::Widgets) {
            let width = unsafe {
                let mut rect: RECT = std::mem::zeroed();
                GetClientRect(self.hwnd, &mut rect);
                rect.right - nav_width - 2 * layout::px(theme::app::CONTENT_PADDING, self.scale)
            };
            let left = nav_width + layout::px(theme::app::CONTENT_PADDING, self.scale);
            let top = layout::px(theme::app::HEADER_HEIGHT, self.scale) - self.scroll;
            let row_height = layout::px(theme::app::NAV_ROW_HEIGHT * 2.0, self.scale);
            let stride = row_height + layout::px(theme::app::CARD_GAP, self.scale);
            if let Some((idx, widget)) = self.widgets.iter().enumerate().find(|(index, widget)| {
                let running = widget.status == "running" || widget.status == "not responding";
                layout::widget_row_rects(top + *index as i32 * stride, width, self.scale, running)
                    .card
                    .contains(x - left, y)
            }) {
                let running = widget.status == "running" || widget.status == "not responding";
                let rects =
                    layout::widget_row_rects(top + idx as i32 * stride, width, self.scale, running);
                if rects.show_on_taskbar.contains(x - left, y) {
                    self.focus_home_control(idx, 0);
                    self.write_show_on_taskbar(idx);
                } else if rects.action.contains(x - left, y) {
                    self.focus_home_control(idx, 1);
                    self.home_action(idx, if running { "quit" } else { "start" });
                } else if rects.restart.is_some_and(|rect| rect.contains(x - left, y)) {
                    self.focus_home_control(idx, 2);
                    self.home_action(idx, "restart");
                } else {
                    self.navigate(Page::Widget(widget.name.clone()));
                }
            }
        } else if !self.rows.is_empty() || matches!(self.page, Page::Developer) {
            let width = unsafe {
                let mut rect: RECT = std::mem::zeroed();
                GetClientRect(self.hwnd, &mut rect);
                rect.right - nav_width - 2 * layout::px(theme::app::CONTENT_PADDING, self.scale)
            };
            let left = nav_width + layout::px(theme::app::CONTENT_PADDING, self.scale);
            let banner_height = self.banner.as_ref().map_or(0, |_| {
                layout::px(theme::space::XL * 2.0 + theme::space::S, self.scale)
            });
            let content_top = layout::px(theme::app::HEADER_HEIGHT, self.scale)
                + banner_height
                + developer_warning_height(&self.page, self.scale);
            let heights = self
                .rows
                .iter()
                .map(|row| self.measured_card_height(row, width))
                .collect::<Vec<_>>();
            let positions = layout::page_layout(
                &self.rows,
                &heights,
                width,
                self.scale,
                content_top,
                self.scroll,
            );
            for index in 0..self.rows.len() {
                let row = self.rows[index].clone();
                let top = positions.cards[index].top;
                let full_line = layout::full_line(&row);
                let card_height = positions.cards[index].bottom - top;
                if y >= top && y < top + card_height {
                    if !row.enabled {
                        return;
                    }
                    let reset_visible = row.reset_visible();
                    let row_id = format!("{}.{}", row.section, row.key);
                    let numeric = match &row.setting.kind {
                        Kind::Number { min, max, step, .. } => Some((*min, *max, *step)),
                        Kind::Auto { inner, .. } => match &**inner {
                            Kind::Number { min, max, step, .. } => Some((*min, *max, *step)),
                            _ => None,
                        },
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
                    let hit_items =
                        layout::row_hit_targets(index, left, local, &control_model.parts);
                    let hit = layout::hit_test(x, y, &hit_items);
                    if matches!(hit, Some(layout::Hit::Reset(_))) {
                        self.focus_item = Some(crate::settings_app::keys::Focus {
                            kind: crate::settings_app::keys::FocusKind::Reset,
                            index,
                        });
                        self.focus = Some(index);
                        self.ensure_focus_visible(index);
                        let _ = self.reset_remote(&row_id);
                    } else if matches!(hit, Some(layout::Hit::Control(_, _))) {
                        self.focus_item = Some(crate::settings_app::keys::Focus {
                            kind: crate::settings_app::keys::FocusKind::Control,
                            index,
                        });
                        self.ensure_focus_visible(index);
                        if let Kind::Auto { word, .. } = &row.setting.kind {
                            let switch = control_model
                                .parts
                                .iter()
                                .find_map(|(part, rect)| {
                                    (*part == layout::ControlPart::AutoSwitch).then_some(*rect)
                                })
                                .unwrap_or_default();
                            if switch.contains(x, y) {
                                self.toggle(index);
                                break;
                            }
                            if row.value.as_str() == Some(word) {
                                break;
                            }
                            if let Kind::Auto { inner, .. } = &row.setting.kind {
                                if let Kind::Choice { options, .. } = &**inner {
                                    if model::control_for(inner) == model::Control::Segments {
                                        let selected =
                                            control_model.parts.iter().find_map(|(part, rect)| {
                                                if let layout::ControlPart::AutoInnerSegment(
                                                    option,
                                                ) = part
                                                {
                                                    rect.contains(x, y).then_some(*option)
                                                } else {
                                                    None
                                                }
                                            });
                                        if let Some((value, _)) =
                                            selected.and_then(|option| options.get(option))
                                        {
                                            self.write_row(
                                                index,
                                                control::Input::Value(value.clone()),
                                            );
                                        }
                                    } else if control_model.parts.iter().any(|(part, rect)| {
                                        *part == layout::ControlPart::AutoInner
                                            && rect.contains(x, y)
                                    }) {
                                        let selected = options
                                            .iter()
                                            .position(|(value, _)| value == &row.value)
                                            .unwrap_or(0);
                                        self.dropdown = Some((index, selected));
                                    }
                                    break;
                                }
                            }
                            if let Some((min, max, step)) = numeric {
                                let track = control_model
                                    .parts
                                    .iter()
                                    .find_map(|(part, rect)| {
                                        (*part == layout::ControlPart::AutoInnerSliderTrack)
                                            .then_some(*rect)
                                    })
                                    .unwrap_or_default();
                                if !track.contains(x, y) {
                                    let value =
                                        control_model.parts.iter().find_map(|(part, rect)| {
                                            (*part == layout::ControlPart::AutoInnerValue)
                                                .then_some(*rect)
                                        });
                                    if value.is_some_and(|rect| rect.contains(x, y)) {
                                        self.begin_edit_with(index, row.value.to_string());
                                    }
                                    break;
                                }
                                let ratio = ((x - track.left) as f64
                                    / (track.right - track.left).max(1) as f64)
                                    .clamp(0.0, 1.0);
                                let raw = min + ratio * (max - min);
                                let value = (raw / step).round() * step;
                                self.rows[index].draft = Some(value.to_string());
                                self.slider_drag = Some((index, value.clamp(min, max), track));
                                unsafe {
                                    SetCapture(self.hwnd);
                                }
                                break;
                            }
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
                            let track = control_model
                                .parts
                                .iter()
                                .find_map(|(part, rect)| {
                                    (*part == layout::ControlPart::SliderTrack).then_some(*rect)
                                })
                                .unwrap_or(control_rect);
                            let ratio = ((x - track.left) as f64
                                / (track.right - track.left).max(1) as f64)
                                .clamp(0.0, 1.0);
                            let raw = min + ratio * (max - min);
                            let value = (raw / step).round() * step;
                            self.rows[index].draft = Some(value.to_string());
                            self.slider_drag = Some((index, value.clamp(min, max), track));
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
            }
            if matches!(self.page, Page::Developer) {
                let advanced_top = content_top;
                let advanced =
                    layout::page_layout(&self.rows, &heights, width, self.scale, advanced_top, 0);
                let cards = layout::developer_layout(
                    advanced_top
                        + advanced.content_height
                        + layout::px(theme::app::CARD_GAP, self.scale),
                    width,
                    self.scale,
                    self.scroll,
                    self.developer_details.len(),
                    self.entries.len(),
                    model::developer_commands().len(),
                );
                if cards.register_button.contains(x - left, y) {
                    self.focus_developer_control(0);
                    self.register_widget_dialog();
                } else if cards.raw_notepad.contains(x - left, y) {
                    self.focus_developer_control(self.entries.len() + 1);
                    let _ = self.open_settings_notepad();
                } else if let Some(index) = cards
                    .remove_buttons
                    .iter()
                    .position(|rect| rect.contains(x - left, y))
                {
                    self.focus_developer_control(index + 1);
                    if let Some(item) = self.entries.get(index) {
                        let name = item.entry.name.clone();
                        let _ = self.writer.send(WriteJob::Remove { name });
                    }
                } else if let Some(index) = cards
                    .copy_buttons
                    .iter()
                    .position(|rect| rect.contains(x - left, y))
                {
                    self.focus_developer_control(self.entries.len() + 2 + index);
                    let _ = self.click_remote(&format!("copy:{index}"));
                }
            }
        }
    }

    /// Scrolls the raw settings text when the pointer is inside its viewport.
    fn scroll_raw_at(&mut self, x: i32, y: i32, delta: i32) -> bool {
        if !matches!(self.page, Page::Developer) {
            return false;
        }
        let nav_width = layout::px(theme::app::NAV_WIDTH, self.scale);
        let left = nav_width + layout::px(theme::app::CONTENT_PADDING, self.scale);
        let width = unsafe {
            let mut rect: RECT = std::mem::zeroed();
            GetClientRect(self.hwnd, &mut rect);
            rect.right - left - layout::px(theme::app::CONTENT_PADDING, self.scale)
        };
        let heights = self
            .rows
            .iter()
            .map(|row| self.measured_card_height(row, width))
            .collect::<Vec<_>>();
        let advanced = layout::page_layout(
            &self.rows,
            &heights,
            width,
            self.scale,
            layout::px(theme::app::HEADER_HEIGHT, self.scale)
                + developer_warning_height(&self.page, self.scale),
            0,
        );
        let cards = layout::developer_layout(
            layout::px(theme::app::HEADER_HEIGHT, self.scale)
                + developer_warning_height(&self.page, self.scale)
                + advanced.content_height
                + layout::px(theme::app::CARD_GAP, self.scale),
            width,
            self.scale,
            self.scroll,
            self.developer_details.len(),
            self.entries.len(),
            model::developer_commands().len(),
        );
        if !cards.raw_view.contains(x - left, y) {
            return false;
        }
        // Same font, inset and wrapping as paint.rs draws the raw view with.
        let inset = layout::px(theme::space::S, self.scale);
        let content_height = self.fonts.measure_height(
            &self.developer_raw,
            cards.raw_view.right - cards.raw_view.left - 2 * inset,
            true,
        ) + 2 * inset;
        let viewport_height = cards.raw_view.bottom - cards.raw_view.top;
        self.developer_raw_scroll = (self.developer_raw_scroll + delta / 2)
            .clamp(0, (content_height - viewport_height).max(0));
        invalidate(self.hwnd);
        true
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
            let range = match &row.setting.kind {
                Kind::Number { min, max, step, .. } => Some((*min, *max, *step)),
                Kind::Auto { inner, .. } => match &**inner {
                    Kind::Number { min, max, step, .. } => Some((*min, *max, *step)),
                    _ => None,
                },
                _ => None,
            };
            if let Some((min, max, step)) = range {
                let ratio = ((x - rect.left) as f64 / (rect.right - rect.left).max(1) as f64)
                    .clamp(0.0, 1.0);
                let value = ((min + ratio * (max - min)) / step).round() * step;
                self.slider_drag = Some((index, value.clamp(min, max), rect));
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
        let Some((widget_name, show_on_taskbar)) = self
            .widgets
            .get(index)
            .map(|widget| (widget.name.clone(), widget.show_on_taskbar))
        else {
            return;
        };
        let Some(item) = self
            .entries
            .iter()
            .find(|item| item.entry.name == widget_name)
        else {
            return;
        };
        let Ok(desc) = &item.describe else {
            return;
        };
        let target = control::Target::from_describe(desc);
        let follow_entry = item.entry.clone();
        self.sequence = self.sequence.wrapping_add(1).max(1);
        self.mark_write_badge(self.sequence);
        let input = control::Input::Value(Value::Bool(!show_on_taskbar));
        let _ = self.writer.send(WriteJob::Apply {
            sequence: self.sequence,
            target,
            key: "show_on_taskbar".into(),
            input,
            follow: vec![follow_entry],
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
                let target = requested_page(&page, &state.entries, state.registry_loaded);
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
            let delta = -(((wparam >> 16) as u16 as i16) as i32);
            let mut point = POINT {
                x: (lparam as i16) as i32,
                y: ((lparam >> 16) as i16) as i32,
            };
            ScreenToClient(hwnd, &mut point);
            if !state.scroll_raw_at(point.x, point.y, delta) {
                state.scroll_by(delta / 2);
            }
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
        WM_KILLFOCUS => {
            state.commit_edit();
            0
        }
        WM_ACTIVATE if (wparam & 0xFFFF) as u32 == WA_INACTIVE => {
            state.commit_edit();
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
                let c = wparam as u16;
                if let Some(ch) = printable_char(c, &mut state.pending_high_surrogate) {
                    edit.insert(&ch.to_string());
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
                model::KeyTarget::Text => {
                    state.edit_key(wparam as u32);
                    if let Some((index, edit)) = state.edit.as_ref() {
                        if let Some(row) = state.rows.get_mut(*index) {
                            row.draft = Some(edit.text().to_string());
                        }
                    }
                    invalidate(hwnd);
                }
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
                developer_details: &state.developer_details,
                registered: &state.entries,
                developer_raw: &state.developer_raw,
                developer_raw_scroll: state.developer_raw_scroll,
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
                focus_developer: state.focus_item.and_then(|focus| {
                    (focus.kind == crate::settings_app::keys::FocusKind::DeveloperAction)
                        .then_some(focus.index)
                }),
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

/// Keeps an unresolved widget page pending until the registry is available.
fn requested_page(name: &str, entries: &[work::LoadedEntry], registry_loaded: bool) -> Page {
    let page = parse_page(name, entries);
    if !registry_loaded
        && page == Page::Widgets
        && !matches!(name, "widgets" | "dock" | "developer")
    {
        Page::Widget(name.into())
    } else {
        page
    }
}

/// Resolves a pending widget page after the registry snapshot arrives.
fn resolve_pending_page(page: Page, entries: &[work::LoadedEntry]) -> Page {
    match page {
        Page::Widget(name) if entries.iter().any(|item| item.entry.name == name) => {
            Page::Widget(name)
        }
        Page::Widget(_) => Page::Widgets,
        page => page,
    }
}

/// Converts the library process state to its stable wire and display label.
fn state_name(state: control::State) -> &'static str {
    match state {
        control::State::Running => "running",
        control::State::Stopped => "stopped",
        control::State::NotResponding => "not responding",
        control::State::KeepsCrashing => "keeps crashing",
        control::State::Unknown => "unknown",
    }
}

/// Formats the status shown in the overview and returned in its state JSON.
fn widget_status_text(widget: &WidgetRow) -> String {
    if widget.status == "unknown" {
        return format!(
            "unknown: {}",
            widget.reason.as_deref().unwrap_or("status unavailable")
        );
    }
    widget.reason.as_ref().map_or_else(
        || widget.status.clone(),
        |reason| format!("{}: {reason}", widget.status),
    )
}

/// Returns the warning-line height included before Developer advanced settings.
fn developer_warning_height(page: &Page, scale: f32) -> i32 {
    if matches!(page, Page::Developer) {
        layout::px(theme::app::CONTROL_HEIGHT, scale)
    } else {
        0
    }
}

/// Identifies file snapshots older than the newest timestamp already observed.
fn file_snapshot_is_stale(
    snapshot: Option<std::time::SystemTime>,
    current: Option<std::time::SystemTime>,
) -> bool {
    matches!((snapshot, current), (Some(snapshot), Some(current)) if snapshot < current)
}

/// Checks whether a write may still update the current badge.
fn badge_result_is_current(write_sequence: Option<u64>, current_sequence: u64) -> bool {
    write_sequence == Some(current_sequence)
}

/// Selects the stored choice or falls back to the first option or Other entry.
fn dropdown_selection(options: &[(Value, String)], other: bool, value: &Value) -> usize {
    options
        .iter()
        .position(|(option, _)| option == value)
        .unwrap_or_else(|| if other { options.len() } else { 0 })
}

/// Decodes printable WM_CHAR units, including paired UTF-16 surrogates.
fn printable_char(unit: u16, pending_high: &mut Option<u16>) -> Option<char> {
    match unit {
        0xd800..=0xdbff => {
            *pending_high = Some(unit);
            None
        }
        0xdc00..=0xdfff => {
            let high = pending_high.take()?;
            let codepoint = 0x1_0000 + (((high as u32 - 0xd800) << 10) | (unit as u32 - 0xdc00));
            char::from_u32(codepoint)
        }
        _ => {
            pending_high.take();
            if unit < 32 || unit == 127 {
                None
            } else {
                char::from_u32(unit as u32)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        badge_result_is_current, dropdown_selection, file_snapshot_is_stale, initial_page,
        printable_char, quote_windows_argument, requested_page, resolve_pending_page,
    };
    use crate::settings_app::model::Page;
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn older_file_snapshot_is_rejected() {
        let old = UNIX_EPOCH + Duration::from_secs(10);
        let new = UNIX_EPOCH + Duration::from_secs(20);
        assert!(file_snapshot_is_stale(Some(old), Some(new)));
        assert!(!file_snapshot_is_stale(Some(new), Some(old)));
        assert!(!file_snapshot_is_stale(None, Some(old)));
    }

    #[test]
    fn older_write_cannot_replace_a_newer_badge() {
        assert!(badge_result_is_current(Some(8), 8));
        assert!(!badge_result_is_current(Some(7), 8));
        assert!(!badge_result_is_current(None, 8));
    }

    #[test]
    fn wm_char_decoder_keeps_printable_unicode_and_ignores_controls() {
        let mut pending = None;
        assert_eq!(printable_char(8, &mut pending), None);
        assert_eq!(printable_char(127, &mut pending), None);
        assert_eq!(printable_char('é' as u16, &mut pending), Some('é'));
        assert_eq!(printable_char(0xd83d, &mut pending), None);
        assert_eq!(printable_char(0xde00, &mut pending), Some('😀'));
    }

    #[test]
    fn dropdown_without_other_highlights_first_option_for_unknown_value() {
        let options = vec![(serde_json::json!("first"), "First".into())];
        assert_eq!(
            dropdown_selection(&options, false, &serde_json::json!("unknown")),
            0
        );
        assert_eq!(
            dropdown_selection(&options, true, &serde_json::json!("unknown")),
            1
        );
    }

    #[test]
    fn widget_page_waits_for_registry_resolution() {
        assert_eq!(
            initial_page(Page::Widget("weather".into())),
            (Page::Widgets, Some(Page::Widget("weather".into())))
        );
        assert_eq!(initial_page(Page::Dock), (Page::Dock, None));
        let pending = requested_page("weather", &[], false);
        assert_eq!(pending, Page::Widget("weather".into()));
        assert_eq!(resolve_pending_page(pending, &[]), Page::Widgets);
    }

    #[test]
    fn notepad_path_argument_is_quoted() {
        assert_eq!(
            quote_windows_argument(r"C:\Taskbar Dock\settings.json"),
            r#""C:\Taskbar Dock\settings.json""#
        );
        assert_eq!(quote_windows_argument("a\\\"b"), r#""a\\\"b""#);
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

/// Opens the current settings.json path in Notepad using a quoted command line.
fn launch_notepad() -> Result<(), String> {
    use windows_sys::Win32::System::Threading::*;
    let path = taskbar_dock::settings::path();
    let command_line = format!(
        "{} {}",
        quote_windows_argument("notepad.exe"),
        quote_windows_argument(&path.to_string_lossy())
    );
    let mut command = wide(&command_line);
    unsafe {
        let mut startup: STARTUPINFOW = std::mem::zeroed();
        startup.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        let mut process: PROCESS_INFORMATION = std::mem::zeroed();
        if CreateProcessW(
            // No application name: Windows searches PATH for the quoted command's program.
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
            Ok(())
        } else {
            Err(std::io::Error::last_os_error().to_string())
        }
    }
}

/// Quotes one Windows command-line argument, including embedded quotes and trailing slashes.
fn quote_windows_argument(argument: &str) -> String {
    let mut quoted = String::from("\"");
    let mut slashes = 0;
    for character in argument.chars() {
        match character {
            '\\' => slashes += 1,
            '"' => {
                quoted.push_str(&"\\".repeat(slashes * 2 + 1));
                quoted.push('"');
                slashes = 0;
            }
            _ => {
                quoted.push_str(&"\\".repeat(slashes));
                slashes = 0;
                quoted.push(character);
            }
        }
    }
    quoted.push_str(&"\\".repeat(slashes * 2));
    quoted.push('"');
    quoted
}
