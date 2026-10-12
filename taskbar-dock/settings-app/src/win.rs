use crate::{
    model::{self, Control, Page, Row},
    remote,
    work::{self, LoadedEntry, WorkResult, WriteJob},
};
use serde_json::{json, Value};
use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    sync::{mpsc::Sender, Arc, RwLock},
};
use taskbar_dock::{
    control,
    describe::{self, Kind},
    registry, wide,
};
use windows_reactor::*;
use windows_sys::Win32::{
    Foundation::*,
    System::{
        DataExchange::COPYDATASTRUCT,
        LibraryLoader::GetModuleHandleW,
        Threading::{CreateMutexW, ReleaseMutex},
    },
    UI::{Input::KeyboardAndMouse::*, Shell::ShellExecuteW, WindowsAndMessaging::*},
};

pub const WM_WORK: u32 = WM_APP + 11;
pub const WM_REMOTE: u32 = WM_APP + 12;
const WM_COPYDATA_TAG: usize = 0x444B_5350;
thread_local! {
    static SENDER: RefCell<Option<LocalSender<Message>>> = const { RefCell::new(None) };
    static EARLY: RefCell<VecDeque<Message>> = const { RefCell::new(VecDeque::new()) };
    static WINDOW: std::cell::Cell<HWND> = const { std::cell::Cell::new(0) };
    static INITIAL: RefCell<Page> = const { RefCell::new(Page::Widgets) };
    static RUNTIME: RefCell<Value> = const { RefCell::new(Value::Null) };
}

enum Message {
    Navigate(Page),
    Open(String),
    Work(WorkResult),
    Remote(remote::Request),
    Set {
        section: String,
        key: String,
        text: String,
    },
    Action(String, String),
    Restore(String),
    Expand(String, bool),
    RefreshDeveloper,
    Copy(usize),
    RegisterDialog,
    OtherMode(String, String),
    SliderEdit(String, String, String),
    SliderCommit(String, String),
    /// Text typed into a box: kept as the row's draft, nothing written yet.
    Draft(String, String, String),
    /// Enter or focus leaving the box: the draft is checked and written.
    Commit(String, String),
    /// Esc: the draft is dropped and the box shows the saved value again.
    Cancel(String, String),
    SliderSettled(String, String, String),
    /// A dockctl reply, sent after the view has rendered the change.
    ReplyNow(remote::Request, Value),
    /// Injected keys have been processed: reply, or wait for the writes they queued.
    InputSettled(remote::Request, u64, Value),
    /// A Widgets-page switch: one key of a widget written without a row on the page.
    SetWidget(String, String, Value),
    /// The "Saved" badge expires (only if no newer badge replaced it).
    ClearBadge(u64),
    /// Esc outside an editor: close the window.
    CloseWindow,
    Noop,
}
fn send(message: Message) {
    SENDER.with(|slot| {
        if let Some(sender) = slot.borrow().as_ref() {
            let _ = sender.send(message);
        } else {
            EARLY.with(|q| q.borrow_mut().push_back(message));
        }
    });
}
fn install_sender(sender: LocalSender<Message>) {
    let pending = take_early();
    SENDER.with(|slot| *slot.borrow_mut() = Some(sender.clone()));
    for message in pending {
        let _ = sender.send(message);
    }
}
fn take_early() -> VecDeque<Message> {
    EARLY.with(|queue| std::mem::take(&mut *queue.borrow_mut()))
}

pub fn run(page: Page) -> i32 {
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
            let ok = forward(&page_name(&page));
            CloseHandle(mutex);
            return if ok { 0 } else { 1 };
        }
        // The runtime comes before the `TaskbarDockSettings` window: Block's Edit settings…
        // takes that window as "the app has started", so it must not exist when the app is
        // about to exit 3 (then Block falls back to Notepad).
        let package = match App::bootstrap_runtime() {
            Ok(package) => package,
            Err(error) => {
                runtime_missing(&format!("HRESULT 0x{:08X}", error.code().0 as u32));
                CloseHandle(mutex);
                return 3;
            }
        };
        if package
            .family
            .starts_with("Microsoft.WindowsAppRuntime.CBS.")
        {
            if let Err(error) = sync_resources(&package.path) {
                runtime_missing(&format!("resources.pri beside the app: {error}"));
                CloseHandle(mutex);
                return 3;
            }
        }
        RUNTIME.with(|runtime| {
            *runtime.borrow_mut() = json!({"family": package.family, "path": package.path});
        });
        INITIAL.with(|p| *p.borrow_mut() = page.clone());
        let instance = GetModuleHandleW(std::ptr::null());
        let class = wide("TaskbarDockSettings");
        let mut wc: WNDCLASSW = std::mem::zeroed();
        wc.lpfnWndProc = Some(proc);
        wc.hInstance = instance;
        wc.lpszClassName = class.as_ptr();
        if RegisterClassW(&wc) == 0 && GetLastError() != ERROR_CLASS_ALREADY_EXISTS {
            return 1;
        }
        let hidden = CreateWindowExW(
            0,
            class.as_ptr(),
            std::ptr::null(),
            WS_POPUP,
            0,
            0,
            0,
            0,
            0,
            0,
            instance,
            std::ptr::null(),
        );
        if hidden == 0 {
            return 1;
        }
        WINDOW.with(|h| h.set(hidden));
        if remote::create(hidden) == 0 {
            DestroyWindow(hidden);
            return 1;
        }
        let result = match App::run_component::<Model>(page) {
            Ok(()) => 0,
            Err(e) => {
                work::log_line(&format!(
                    "WinUI startup error HRESULT 0x{:08X}: {e}",
                    e.code().0 as u32
                ));
                1
            }
        };
        let _ = ReleaseMutex(mutex);
        CloseHandle(mutex);
        result
    }
}

/// No usable Windows App Runtime: says so, offers Microsoft's download page, logs the reason.
fn runtime_missing(reason: &str) {
    let message = format!(
        "Taskbar Dock settings needs the Windows App Runtime 2. Open Microsoft's download page?\n\n{reason}"
    );
    unsafe {
        let yes = MessageBoxW(
            0,
            wide(&message).as_ptr(),
            wide("Taskbar Dock settings").as_ptr(),
            MB_YESNO | MB_ICONERROR,
        );
        if yes == IDYES {
            ShellExecuteW(
                0,
                wide("open").as_ptr(),
                wide("https://learn.microsoft.com/windows/apps/windows-app-sdk/downloads").as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                SW_SHOWNORMAL,
            );
        }
    }
    work::log_line(&format!("runtime bootstrap failed: {reason}"));
}

/// Windows' own runtime copy needs its `resources.pri` beside the exe (WinUI's resource loader
/// expects a package identity otherwise): copied when missing or different. An error only when
/// the file is still missing afterwards.
fn sync_resources(package: &str) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = exe.parent().ok_or("the app has no folder")?;
    let src = std::path::Path::new(package).join("resources.pri");
    let dst = dir.join("resources.pri");
    let needs = match (std::fs::metadata(&src), std::fs::metadata(&dst)) {
        (Ok(a), Ok(b)) => a.len() != b.len() || a.modified().ok() != b.modified().ok(),
        (Ok(_), Err(_)) => true,
        (Err(e), Err(_)) => return Err(format!("{} unreadable: {e}", src.display())),
        (Err(_), Ok(_)) => false,
    };
    if needs {
        if let Err(e) = std::fs::copy(&src, &dst) {
            work::log_line(&format!("resources.pri copy failed: {e}"));
            if !dst.exists() {
                return Err(format!("copy failed: {e}"));
            }
        }
    }
    Ok(())
}
unsafe fn forward(page: &str) -> bool {
    let class = wide("TaskbarDockSettings");
    let start = std::time::Instant::now();
    let target = loop {
        let h = FindWindowW(class.as_ptr(), std::ptr::null());
        if h != 0 {
            break h;
        }
        if start.elapsed() >= std::time::Duration::from_secs(3) {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    };
    AllowSetForegroundWindow(u32::MAX);
    let bytes = page.as_bytes();
    let data = COPYDATASTRUCT {
        dwData: WM_COPYDATA_TAG,
        cbData: bytes.len() as u32,
        lpData: bytes.as_ptr() as *mut _,
    };
    let mut out = 0;
    SendMessageTimeoutW(
        target,
        WM_COPYDATA,
        0,
        &data as *const _ as LPARAM,
        SMTO_ABORTIFHUNG,
        2000,
        &mut out,
    ) != 0
        && out == 1
}
fn page_name(page: &Page) -> String {
    match page {
        Page::Widgets => "widgets".into(),
        Page::Dock => "dock".into(),
        Page::Developer => "developer".into(),
        Page::Widget(n) => n.clone(),
    }
}
fn page_from_name(name: &str) -> Page {
    match name {
        "widgets" => Page::Widgets,
        "dock" => Page::Dock,
        "developer" => Page::Developer,
        n => Page::Widget(n.into()),
    }
}
unsafe extern "system" fn proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match msg {
        WM_COPYDATA => {
            let d = &*(l as *const COPYDATASTRUCT);
            if d.dwData != WM_COPYDATA_TAG || d.lpData.is_null() {
                return 0;
            }
            let b = std::slice::from_raw_parts(d.lpData as *const u8, d.cbData as usize);
            let name = String::from_utf8_lossy(b);
            let title = wide("Taskbar Dock");
            let visible = FindWindowW(std::ptr::null(), title.as_ptr());
            if visible != 0 {
                ShowWindow(visible, SW_RESTORE);
                SetForegroundWindow(visible);
            }
            send(Message::Navigate(page_from_name(&name)));
            1
        }
        WM_TIMER => {
            work::read_file(hwnd);
            0
        }
        WM_WORK => {
            if l != 0 {
                send(Message::Work(*Box::from_raw(l as *mut WorkResult)));
            }
            0
        }
        WM_REMOTE => {
            if l != 0 {
                send(Message::Remote(*Box::from_raw(l as *mut remote::Request)));
            }
            0
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, w, l),
    }
}

/// A dockctl request that replies once every write it queued has a result.
struct Waiter {
    request: remote::Request,
    sequences: std::collections::HashSet<u64>,
    errors: Vec<String>,
}

struct Model {
    page: Page,
    entries: Vec<LoadedEntry>,
    settings: Value,
    rows: Vec<Row>,
    widgets: Vec<model::WidgetRow>,
    status: Vec<control::Status>,
    banner: Option<String>,
    badge: String,
    writer: Sender<WriteJob>,
    seq: u64,
    status_entries: Arc<RwLock<Vec<registry::Entry>>>,
    status_refresh: Option<Sender<()>>,
    /// dockctl requests waiting for the writes they queued.
    waiters: Vec<Waiter>,
    /// dockctl requests waiting for a Remove from list to finish, by widget name.
    pending_removals: HashMap<String, remote::Request>,
    /// Sequence of the write whose result the badge shows; older results leave it alone.
    badge_seq: u64,
    /// Bumped on every badge change, so an expiry only clears its own "Saved".
    badge_gen: u64,
    /// Modification time of the settings snapshot shown; older reads are dropped.
    file_mtime: Option<std::time::SystemTime>,
    /// Results of recent writes (sequence -> error), so a request that starts waiting after
    /// its writes already finished still gets their outcome.
    finished: HashMap<u64, Option<String>>,
    /// dockctl requests waiting for a Start / Stop / Restart to finish, by widget name.
    pending_actions: HashMap<String, remote::Request>,
    /// Widgets with a Start / Stop / Restart running; a second one is refused meanwhile.
    busy: std::collections::HashSet<String>,
    expanded: HashMap<String, bool>,
    developer_details: Vec<Value>,
    developer_raw: Option<String>,
    developer_badge: String,
    runtime: Value,
}
impl Component for Model {
    type Input = Page;
    type Message = Message;
    fn create(input: &Page, ctx: &ComponentContext<Self>) -> Self {
        install_sender(ctx.sender());
        let hwnd = WINDOW.with(|h| h.get());
        let writer = work::writer(hwnd);
        work::load_registry(hwnd);
        work::read_file(hwnd);
        unsafe {
            let _ = SetTimer(hwnd, 1, 1000, None);
        }
        Self {
            page: input.clone(),
            entries: vec![],
            settings: json!({}),
            rows: vec![],
            widgets: vec![],
            status: vec![],
            banner: None,
            badge: "".into(),
            writer,
            seq: 0,
            status_entries: Arc::new(RwLock::new(vec![])),
            status_refresh: None,
            waiters: Vec::new(),
            pending_removals: HashMap::new(),
            badge_seq: 0,
            badge_gen: 0,
            file_mtime: None,
            finished: HashMap::new(),
            pending_actions: HashMap::new(),
            busy: std::collections::HashSet::new(),
            expanded: HashMap::new(),
            developer_details: vec![],
            developer_raw: Some("Loading settings…".into()),
            developer_badge: String::new(),
            runtime: RUNTIME.with(|runtime| runtime.borrow().clone()),
        }
    }
    fn update(&mut self, m: Message, ctx: &ComponentContext<Self>) {
        match m {
            Message::Navigate(p) => self.go_to(p, ctx),
            Message::Open(n) => self.go_to(Page::Widget(n), ctx),
            Message::Noop => {}
            Message::Set { section, key, text } => {
                if let Err(error) = self.set(&section, &key, &text) {
                    self.show_badge(format!("Not saved: {error}"), false, ctx);
                }
            }
            Message::Restore(group) => self.restore_group(&group),
            Message::Expand(id, expanded) => {
                self.expanded.insert(id, expanded);
            }
            Message::RefreshDeveloper => self.refresh_developer(true),
            Message::Copy(index) => {
                if let Some(command) = model::developer_commands().get(index) {
                    taskbar_dock::text_edit::clipboard_write(WINDOW.with(|h| h.get()), command);
                    self.developer_badge = "Copied".into();
                    self.badge = "Copied".into();
                }
            }
            Message::RegisterDialog => self.register_widget_dialog(),
            Message::OtherMode(section, key) => {
                if let Some(row) = self
                    .rows
                    .iter_mut()
                    .find(|row| row.section == section && row.key == key)
                {
                    row.choice_text_mode = true;
                    row.draft = None;
                    row.error = None;
                }
            }
            Message::SliderEdit(section, key, text) => {
                if let Some(row) = self
                    .rows
                    .iter_mut()
                    .find(|row| row.section == section && row.key == key)
                {
                    row.draft = Some(text.clone());
                    row.error = None;
                }
                let section2 = section.clone();
                let key2 = key.clone();
                let text2 = text.clone();
                // Slider exposes pointer release on its Border wrapper; key-up is not exposed by
                // reactor 0.100, so keyboard changes settle after 400 ms without another value.
                ctx.spawn_background(move |_| {
                    std::thread::sleep(std::time::Duration::from_millis(400));
                    Message::SliderSettled(section2, key2, text2)
                });
            }
            Message::SliderCommit(section, key) => self.commit_slider(&section, &key),
            Message::Draft(section, key, text) => {
                if let Some(row) = self
                    .rows
                    .iter_mut()
                    .find(|row| row.section == section && row.key == key)
                {
                    if row.pending.is_none() && text == model::display_text(&row.value) {
                        row.draft = None;
                    } else {
                        row.draft = Some(text);
                    }
                }
            }
            Message::Commit(section, key) => {
                let draft = self
                    .rows
                    .iter()
                    .find(|row| row.section == section && row.key == key)
                    .filter(|row| row.draft.is_some() && row.draft != row.submitted)
                    .and_then(|row| row.draft.clone());
                if let Some(text) = draft {
                    let _ = self.set(&section, &key, &text);
                }
            }
            Message::Cancel(section, key) => {
                if let Some(row) = self
                    .rows
                    .iter_mut()
                    .find(|row| row.section == section && row.key == key)
                {
                    row.draft = None;
                    row.error = None;
                }
            }
            Message::SliderSettled(section, key, text) => {
                let current = self
                    .rows
                    .iter()
                    .find(|row| row.section == section && row.key == key)
                    .and_then(|row| row.draft.as_deref());
                // A drag still held (left button down) writes on release (capture lost), not
                // after the keyboard's 400 ms pause.
                let dragging = unsafe { GetAsyncKeyState(VK_LBUTTON as i32) } < 0;
                if current == Some(text.as_str()) && !dragging {
                    let _ = self.set(&section, &key, &text);
                }
            }
            Message::Work(WorkResult::Registry(entries)) => {
                if let Ok(mut x) = self.status_entries.write() {
                    *x = entries.iter().map(|i| i.entry.clone()).collect();
                }
                self.status_refresh = Some(work::status_loop(
                    WINDOW.with(|h| h.get()),
                    self.status_entries.clone(),
                ));
                self.entries = entries;
                self.refresh_rows();
                self.refresh_widgets();
                if matches!(self.page, Page::Developer) {
                    self.refresh_developer(true);
                }
            }
            Message::Work(WorkResult::Status { statuses, .. }) => {
                self.status = statuses;
                self.refresh_widgets();
                if matches!(self.page, Page::Developer) {
                    self.refresh_developer(true);
                }
            }
            Message::Work(WorkResult::File { result: Ok(v), mtime }) => {
                // Reads run on their own threads: an older snapshot arriving late is dropped.
                if let (Some(shown), Some(read)) = (self.file_mtime, mtime) {
                    if read < shown {
                        return;
                    }
                }
                self.file_mtime = mtime.or(self.file_mtime);
                let registry_changed = !self.settings.as_object().is_none_or(|o| o.is_empty())
                    && v.get("widgets") != self.settings.get("widgets");
                self.settings = v;
                self.banner = None;
                if registry_changed {
                    work::load_registry(WINDOW.with(|h| h.get()));
                }
                self.refresh_rows();
                self.refresh_widgets();
                if matches!(self.page, Page::Developer) {
                    self.refresh_developer(false);
                }
            }
            Message::Work(WorkResult::File { result: Err(_), .. }) => {
                self.banner = Some("settings.json has an error; fix it in Notepad".into());
                for r in &mut self.rows {
                    r.enabled = false;
                }
            }
            Message::Work(WorkResult::Write {
                sequence,
                section,
                key,
                result,
                followed,
            }) => {
                if let Some(r) = self
                    .rows
                    .iter_mut()
                    .find(|r| r.section == section && r.key == key)
                {
                    r.apply_result(
                        sequence,
                        result.as_ref().ok().cloned(),
                        result.as_ref().err().cloned(),
                    );
                }
                // An older write's result never replaces the badge of a newer one.
                if sequence >= self.badge_seq {
                    self.badge_seq = sequence;
                    let text = match (&result, &followed) {
                        (Err(e), _) => format!("Not saved: {e}"),
                        (Ok(_), Err(_)) => {
                            let title = self.title_of(&section);
                            if section == "dock" {
                                "Saved; a widget hasn't confirmed".to_string()
                            } else {
                                format!("Saved; {title} hasn't confirmed")
                            }
                        }
                        (Ok(_), Ok(())) => "Saved".to_string(),
                    };
                    let expires = text == "Saved";
                    self.show_badge(text, expires, ctx);
                }
                self.finished.insert(
                    sequence,
                    result.as_ref().err().map(|e| format!("{section}.{key}: {e}")),
                );
                self.finished.retain(|seq, _| *seq + 256 > sequence);
                let mut done = Vec::new();
                for (index, waiter) in self.waiters.iter_mut().enumerate() {
                    if waiter.sequences.remove(&sequence) {
                        if let Err(e) = &result {
                            waiter.errors.push(format!("{section}.{key}: {e}"));
                        }
                        if waiter.sequences.is_empty() {
                            done.push(index);
                        }
                    }
                }
                for index in done.into_iter().rev() {
                    let waiter = self.waiters.remove(index);
                    let reply = if waiter.errors.is_empty() {
                        json!({"ok":true})
                    } else {
                        json!({"ok":false,"error":waiter.errors.join("; ")})
                    };
                    reply_later(ctx, waiter.request, reply);
                }
                work::read_file(WINDOW.with(|h| h.get()));
            }
            Message::Work(WorkResult::Action {
                name,
                action,
                result,
                warning,
            }) => {
                self.busy.remove(&name);
                let title = self.title_of(&name);
                self.show_badge(action_badge(&title, &action, &result, warning.as_deref()), false, ctx);
                if let Some(refresh) = &self.status_refresh {
                    let _ = refresh.send(());
                }
                if let Some(request) = self.pending_actions.remove(&name) {
                    let reply = match &result {
                        Ok(pid) => json!({"ok":true,"pid":pid,"warning":warning}),
                        Err(error) => json!({"ok":false,"error":error}),
                    };
                    reply_later(ctx, request, reply);
                }
            }
            Message::Work(WorkResult::Developer { details, raw }) => {
                self.developer_details = details;
                if raw.is_some() {
                    self.developer_raw = raw;
                }
            }
            Message::Work(WorkResult::RegistryMutation {
                operation,
                requested,
                result,
            }) => {
                self.developer_badge = match &result {
                    Ok(name) if operation == "register" => format!("Registered {name}"),
                    Ok(name) => format!("Removed {name}"),
                    Err(error) => format!("Not {operation}ed: {error}"),
                };
                self.show_badge(self.developer_badge.clone(), false, ctx);
                if let Some(request) = self.pending_removals.remove(&requested) {
                    let reply = match &result {
                        Ok(_) => json!({"ok":true}),
                        Err(error) => json!({"ok":false,"error":error}),
                    };
                    reply_later(ctx, request, reply);
                }
                work::load_registry(WINDOW.with(|h| h.get()));
            }
            Message::Action(name, action) => {
                if action == "notepad" {
                    open_notepad();
                    return;
                }
                if action == "remove" {
                    let _ = self.writer.send(WriteJob::Remove { name });
                    self.developer_badge = "Removing".into();
                    self.show_badge("Removing".into(), false, ctx);
                    return;
                }
                if let Err(error) = self.start_action(&name, &action, ctx) {
                    self.show_badge(error, false, ctx);
                }
            }
            Message::ReplyNow(req, reply) => {
                remote::reply(req.sender, req.reply_source, req.id, &reply)
            }
            Message::InputSettled(req, sequence_before, reply) => {
                self.wait_or_reply(req, sequence_before, reply, ctx);
            }
            Message::SetWidget(section, key, value) => {
                if let Err(error) = self.write_value(&section, &key, value) {
                    self.show_badge(format!("Not saved: {error}"), false, ctx);
                }
            }
            Message::ClearBadge(generation) => {
                if generation == self.badge_gen {
                    self.badge.clear();
                }
            }
            Message::CloseWindow => close_window(),
            Message::Remote(req) => {
                use taskbar_dock::ctl::SettingsCommand as C;
                let sequence_before = self.seq;
                let reply = match taskbar_dock::ctl::parse_settings(&req.args) {
                    Ok(C::State) => self.remote_state(),
                    Ok(C::Page(name)) => {
                        self.go_to(page_from_name(&name), ctx);
                        json!({"ok":true})
                    }
                    Ok(C::Quit) => {
                        // Replied before closing: the delayed reply would die with the window.
                        remote::reply(req.sender, req.reply_source, req.id, &json!({"ok":true}));
                        close_window();
                        return;
                    }
                    Ok(C::RowSet(id, text)) => match self.row_key(&id) {
                        Some((section, key)) => match self.set(&section, &key, &text) {
                            Ok(()) => json!({"ok":true}),
                            Err(error) => json!({"ok":false,"error":error}),
                        },
                        None => json!({"ok":false,"error":"row is not on the current page"}),
                    },
                    Ok(C::RowReset(id)) => match self.row_key(&id) {
                        Some((_, key)) if key == "startup" => {
                            json!({"ok":false,"error":"Start with Windows has no default"})
                        }
                        Some((section, key)) => match self.reset_row(&section, &key) {
                            Ok(()) => json!({"ok":true}),
                            Err(error) => json!({"ok":false,"error":error}),
                        },
                        None => json!({"ok":false,"error":"row is not on the current page"}),
                    },
                    Ok(C::RowToggle(id)) => match self.toggle_text(&id) {
                        Some((section, key, text)) => match self.set(&section, &key, &text) {
                            Ok(()) => json!({"ok":true}),
                            Err(error) => json!({"ok":false,"error":error}),
                        },
                        None => json!({"ok":false,"error":"no switch row with that id on this page"}),
                    },
                    Ok(C::Click(id)) => self.remote_click(&id, ctx),
                    Ok(C::Key(key)) => {
                        let reply = self.remote_key(&key);
                        if reply["ok"] == true {
                            settle_input(ctx, req, sequence_before, reply);
                            return;
                        }
                        reply
                    }
                    Ok(C::Type(text)) => {
                        let reply = self.remote_type(&text);
                        if reply["ok"] == true {
                            settle_input(ctx, req, sequence_before, reply);
                            return;
                        }
                        reply
                    }
                    Err(error) => json!({"ok":false,"error":error,"kind":"usage"}),
                };
                if let Some(name) = reply.get("deferred_action").and_then(Value::as_str) {
                    self.pending_actions.insert(name.to_string(), req);
                } else if let Some(name) = reply.get("deferred_remove").and_then(Value::as_str) {
                    self.pending_removals.insert(name.to_string(), req);
                } else if reply.get("ok").and_then(Value::as_bool) == Some(true) {
                    self.wait_or_reply(req, sequence_before, reply, ctx);
                } else {
                    reply_later(ctx, req, reply);
                }
            }
        }
    }
    fn view(&self, _: &Page, ctx: &mut ViewContext<Self>) -> View {
        ctx.window_title("Taskbar Dock");
        ctx.window_visuals(
            WindowVisuals::new()
                .client_size(880.0, 640.0)
                .constraints(WindowConstraints {
                    min_width: Some(640.0),
                    min_height: Some(420.0),
                    max_width: None,
                    max_height: None,
                })
                .backdrop(WindowBackdrop::Mica),
        );
        let content = match &self.page {
            Page::Widgets => self.widgets_view(ctx),
            Page::Widget(n) => self.widget_page_view(n, ctx),
            Page::Dock => self.settings_view(None, ctx),
            Page::Developer => self.developer_view(ctx),
        };
        let badge = TextBlock::new()
            .text(self.badge.clone())
            .font_size(12.0)
            .horizontal_alignment(HorizontalAlignment::Right)
            .automation_id("badge");
        let page_heading: View = match &self.page {
            Page::Widget(name) => {
                let title = self
                    .entries
                    .iter()
                    .find(|entry| entry.entry.name == *name)
                    .and_then(|entry| entry.describe.as_ref().ok())
                    .map(|desc| desc.title.as_str())
                    .unwrap_or(name);
                StackPanel::new()
                    .orientation(Orientation::Horizontal)
                    .spacing(8.0)
                    .children((
                        Button::new()
                            .style(ButtonStyle::Subtle)
                            .automation_id("back:widgets")
                            .margin(Thickness::new(-11.0, 0.0, 0.0, 0.0))
                            .vertical_alignment(VerticalAlignment::Center)
                            .on_click(ctx.callback(|_| Message::Navigate(Page::Widgets)))
                            .content(
                                TextBlock::new()
                                    .text("Widgets")
                                    .font_size(28.0)
                                    .font_weight(FontWeight::SEMI_BOLD)
                                    .opacity(0.72),
                            ),
                        FontIcon::new()
                            .glyph("\u{E76C}")
                            .opacity(0.72)
                            .vertical_alignment(VerticalAlignment::Center),
                        TextBlock::new()
                            .text(title)
                            .font_size(28.0)
                            .vertical_alignment(VerticalAlignment::Center)
                            .font_weight(FontWeight::SEMI_BOLD),
                    ))
                    .into()
            }
            _ => TextBlock::new()
                .text(self.page_title())
                .font_size(28.0)
                .font_weight(FontWeight::SEMI_BOLD)
                .grid_column(0)
                .into(),
        };
        let header = Grid::new()
            .columns([GridLength::STAR, GridLength::Auto])
            .children((page_heading, badge.grid_column(1)));
        let body = StackPanel::new().spacing(20.0).children((header, content));
        let page: View = if let Some(b) = &self.banner {
            StackPanel::new()
                .spacing(12.0)
                .children((
                    Grid::new()
                        .columns([GridLength::STAR, GridLength::Auto])
                        .column_spacing(8.0)
                        .children((
                            InfoBar::new()
                                .severity(InfoBarSeverity::Error)
                                .is_open(true)
                                .is_closable(false)
                                .message(b.clone())
                                .automation_id("banner")
                                .grid_column(0),
                            Button::new()
                                .automation_id("notepad")
                                .vertical_alignment(VerticalAlignment::Center)
                                .on_click(
                                    ctx.callback(|_| Message::Action("".into(), "notepad".into())),
                                )
                                .grid_column(1)
                                .content("Open in Notepad"),
                        )),
                    body,
                ))
                .into()
        } else {
            body.into()
        };
        let page = ScrollViewer::new().content(
            Border::new()
                .padding(Thickness::new(24.0, 8.0, 24.0, 24.0))
                .content(page),
        );
        let nav = NavigationView::new()
            .pane_display_mode(NavigationViewPaneDisplayMode::Left)
            .is_pane_toggle_button_visible(false)
            .is_settings_visible(false)
            .on_selected_tag_changed(ctx.callback(|tag: Option<String>| {
                tag.map(|s| Message::Navigate(page_from_name(&s)))
                    .unwrap_or(Message::Noop)
            }))
            .slots([
                SlotView::new(NavigationViewSlot::Content, page),
                SlotView::collection(
                    NavigationViewSlot::MenuItems,
                    [
                        KeyedView::new(
                            "widgets",
                            nav_item(
                                "widgets",
                                "Widgets",
                                "\u{E71D}",
                                matches!(self.page, Page::Widgets | Page::Widget(_)),
                            ),
                        ),
                        KeyedView::new(
                            "dock",
                            nav_item("dock", "Dock", "\u{E771}", matches!(self.page, Page::Dock)),
                        ),
                        KeyedView::new(
                            "developer",
                            nav_item(
                                "developer",
                                "Developer",
                                "\u{EC7A}",
                                matches!(self.page, Page::Developer),
                            ),
                        ),
                    ],
                ),
            ]);
        Grid::new()
            .rows([GridLength::Auto, GridLength::STAR])
            // Esc outside an editor closes the window (a card's own Esc, nearer the focus, wins).
            .key_accelerators(KeyAccelerators::new([KeyAccelerator::new(
                AcceleratorKey::Escape,
                AcceleratorModifiers::None,
                ctx.callback(|_| Message::CloseWindow),
            )]))
            .children((
                TitleBar::new().title("Taskbar Dock").grid_row(0),
                Border::new().grid_row(1).content(nav),
            ))
    }
    fn input_changed(&mut self, _: &Page, _: &ComponentContext<Self>) {}
}
impl Model {
    fn remote_state(&self) -> Value {
        let title = wide("Taskbar Dock");
        let hwnd = unsafe { FindWindowW(std::ptr::null(), title.as_ptr()) };
        let mut rect: windows_sys::Win32::Foundation::RECT = unsafe { std::mem::zeroed() };
        let has_rect = hwnd != 0 && unsafe { GetWindowRect(hwnd, &mut rect) } != 0;
        let visible = hwnd != 0 && unsafe { IsWindowVisible(hwnd) } != 0;
        let mut title_text = [0u16; 256];
        let title_len = if hwnd != 0 {
            unsafe { GetWindowTextW(hwnd, title_text.as_mut_ptr(), title_text.len() as i32) }
        } else {
            0
        };
        let actual_title = String::from_utf16_lossy(&title_text[..title_len.max(0) as usize]);
        let scale = if hwnd != 0 {
            (unsafe { windows_sys::Win32::UI::HiDpi::GetDpiForWindow(hwnd) }) as f64 / 96.0
        } else {
            1.0
        };
        let rows = self.rows.iter().map(|row| {
            let secret = describe::is_secret_kind(&row.setting.kind);
            let value = if secret && !row.value.as_str().unwrap_or_default().is_empty() { json!("***") } else { row.value.clone() };
            let default = if secret && !row.default.as_str().unwrap_or_default().is_empty() { json!("***") } else { row.default.clone() };
            let kind = match row.control() { Control::Switch=>"switch",Control::Dropdown=>"dropdown",Control::Slider=>"slider",Control::NumberBox=>"numberbox",Control::Text=>"text",Control::Color=>"color",Control::Secret=>"secret",Control::Auto=>"auto" };
            json!({"id":row_id(row),"label":row.setting.label,"kind":kind,"value":value,"default":default,"draft":if secret && row.draft.is_some() { Some("***") } else { row.draft.as_deref() },"error":row.error,"enabled":row.enabled})
        }).collect::<Vec<_>>();
        let widgets = self.widgets.iter().map(|widget| json!({"name":widget.name,"title":widget.title,"status":widget.status,"pid":widget.pid})).collect::<Vec<_>>();
        let rect = has_rect.then(
            || json!({"left":rect.left,"top":rect.top,"right":rect.right,"bottom":rect.bottom}),
        );
        let cards = if matches!(self.page, Page::Developer) {
            let advanced = self.entries.iter().map(|entry| json!({
                "name": entry.entry.name,
                "rows": self.rows.iter().filter(|row| row.section == entry.entry.name).map(|row| {
                    let secret = describe::is_secret_kind(&row.setting.kind);
                    let value = if secret && !row.value.as_str().unwrap_or_default().is_empty() { json!("***") } else { row.value.clone() };
                    let default = if secret && !row.default.as_str().unwrap_or_default().is_empty() { json!("***") } else { row.default.clone() };
                    let draft = if secret && row.draft.is_some() { Some("***") } else { row.draft.as_deref() };
                    json!({"id":row_id(row),"label":row.setting.label,"kind":format!("{:?}",row.control()).to_lowercase(),"value":value,"default":default,"draft":draft,"error":row.error,"enabled":row.enabled})
                }).collect::<Vec<_>>()
            })).collect::<Vec<_>>();
            let registered = model::registered_widgets(&self.entries).into_iter().map(|item| json!({"name":item.name,"title":item.title,"exe":item.exe,"error":item.error})).collect::<Vec<_>>();
            json!({"advanced":advanced,"details":self.developer_details,"registered":registered,"raw":self.developer_raw,"commands":model::developer_commands(),"badge":self.developer_badge})
        } else {
            Value::Null
        };
        json!({"ok":true,"page":page_name(&self.page),"title":actual_title,"visible":visible,"rect":rect,"scale":scale,"banner":self.banner,"badge":self.badge,"rows":rows,"widgets":widgets,"focused":Value::Null,"cards":cards,"runtime":self.runtime})
    }
    fn page_title(&self) -> String {
        match &self.page {
            Page::Widgets => "Widgets".into(),
            Page::Widget(n) => format!(
                "Widgets  \u{203A}  {}",
                self.entries
                    .iter()
                    .find(|e| e.entry.name == *n)
                    .and_then(|e| e.describe.as_ref().ok())
                    .map(|d| d.title.as_str())
                    .unwrap_or(n)
            ),
            Page::Dock => "Dock".into(),
            Page::Developer => "Developer".into(),
        }
    }
    fn refresh_rows(&mut self) {
        let mut next = match &self.page {
            Page::Widget(n) => self
                .entries
                .iter()
                .find(|e| e.entry.name == *n)
                .and_then(|e| {
                    e.describe.as_ref().ok().map(|d| {
                        model::widget_rows(
                            n,
                            &d.settings,
                            self.settings.get(n).unwrap_or(&Value::Null),
                            &d.defaults,
                            taskbar_dock::startup::is_on(n),
                        )
                    })
                })
                .unwrap_or_default(),
            Page::Dock => model::dock_rows(
                self.settings.get("dock").unwrap_or(&Value::Null),
                &taskbar_dock::style::dock_defaults(),
            ),
            Page::Developer => model::advanced_rows(&self.entries, &self.settings),
            _ => vec![],
        };
        for row in &mut next {
            if let Some(old) = self
                .rows
                .iter()
                .find(|old| old.section == row.section && old.key == row.key)
            {
                if old.draft.is_some() || old.pending.is_some() || old.error.is_some() {
                    // Keep the edit, pending write and error; enablement is always derived anew.
                    let enabled = row.enabled;
                    *row = old.clone();
                    row.enabled = enabled;
                }
            }
            if self.banner.is_some() {
                row.enabled = false;
            }
        }
        self.rows = next;
    }
    fn refresh_widgets(&mut self) {
        self.widgets = model::registered_widgets(&self.entries)
            .into_iter()
            .map(|w| {
                let s = self.status.iter().find(|s| s.name == w.name);
                let loaded = self.entries.iter().find(|e| e.entry.name == w.name);
                model::WidgetRow {
                    name: w.name.clone(),
                    title: loaded
                        .and_then(|e| e.describe.as_ref().ok())
                        .map(|d| d.title.clone())
                        .unwrap_or_else(|| w.name.clone()),
                    description: loaded
                        .and_then(|e| e.describe.as_ref().ok())
                        .map(|d| d.description.clone())
                        .unwrap_or_else(|| w.error.clone().unwrap_or_default()),
                    status: s
                        .map(|s| status_label(&s.state).to_string())
                        .unwrap_or_else(|| "stopped".into()),
                    pid: s.and_then(|s| s.pid),
                    reason: s.and_then(|s| s.reason.clone()).or(w.error),
                    show_on_taskbar: self
                        .settings
                        .get(&w.name)
                        .and_then(|v| v.get("show_on_taskbar"))
                        .and_then(Value::as_bool)
                        .unwrap_or(true),
                }
            })
            .collect();
    }
    fn refresh_developer(&mut self, include_raw: bool) {
        if !matches!(self.page, Page::Developer) {
            return;
        }
        work::read_developer(
            WINDOW.with(|h| h.get()),
            self.entries.clone(),
            self.status.clone(),
            include_raw || self.developer_raw.is_none(),
        );
    }
    /// The UI's edit path for one row: parse and check the text, then queue the write. A failed
    /// check leaves the text in the box with the error line, and nothing is written.
    fn set(&mut self, section: &str, key: &str, text: &str) -> Result<(), String> {
        let row = self
            .rows
            .iter_mut()
            .find(|r| r.section == section && r.key == key)
            .ok_or_else(|| "row is not on the current page".to_string())?;
        let value = match describe::parse(&row.setting, text) {
            Ok(v) => v,
            Err(e) => {
                if matches!(
                    row.control(),
                    Control::Text | Control::Color | Control::Secret
                ) || matches!(row.setting.kind, Kind::Optional(_))
                {
                    row.draft = Some(text.to_string());
                }
                row.error = Some(e.clone());
                return Err(format!("{}: {e}", row.setting.label));
            }
        };
        let label = row.setting.label.clone();
        let sequence = self
            .write_value(section, key, value)
            .map_err(|e| format!("{label}: {e}"))?;
        if let Some(row) = self
            .rows
            .iter_mut()
            .find(|r| r.section == section && r.key == key)
        {
            row.pending = Some(sequence);
            row.draft = Some(text.into());
            row.submitted = Some(text.into());
        }
        Ok(())
    }
    /// Queues one checked value on the writer thread and returns its sequence number. A widget's
    /// key is followed by that widget; a dock key by every registered widget.
    fn write_value(&mut self, section: &str, key: &str, value: Value) -> Result<u64, String> {
        let job_sequence = self.seq + 1;
        let job = if key == "startup" {
            let exe = self
                .entries
                .iter()
                .find(|entry| entry.entry.name == section)
                .map(|entry| entry.entry.exe.clone())
                .ok_or_else(|| format!("{section} isn't registered"))?;
            WriteJob::Startup {
                sequence: job_sequence,
                name: section.to_string(),
                exe,
                enabled: value.as_bool().unwrap_or(false),
            }
        } else {
            let (target, follow) = self.target(section)?;
            WriteJob::Apply {
                sequence: job_sequence,
                target,
                key: key.into(),
                input: control::Input::Value(value),
                follow,
            }
        };
        self.writer
            .send(job)
            .map_err(|_| "the writer has stopped".to_string())?;
        self.seq = job_sequence;
        Ok(job_sequence)
    }
    /// The write target of a section and the widgets that reload after a write to it.
    fn target(&self, section: &str) -> Result<(control::Target, Vec<registry::Entry>), String> {
        if section == "dock" {
            let follow = self.entries.iter().map(|e| e.entry.clone()).collect();
            return Ok((control::Target::dock(), follow));
        }
        let entry = self
            .entries
            .iter()
            .find(|e| e.entry.name == section)
            .ok_or_else(|| format!("{section} isn't registered"))?;
        let describe = entry
            .describe
            .as_ref()
            .map_err(|e| format!("{section} can't be described: {e}"))?;
        Ok((
            control::Target::from_describe(describe),
            vec![entry.entry.clone()],
        ))
    }
    fn commit_slider(&mut self, section: &str, key: &str) {
        let text = self
            .rows
            .iter()
            .find(|row| row.section == section && row.key == key)
            .filter(|row| row.draft != row.submitted)
            .and_then(|row| row.draft.clone());
        if let Some(text) = text {
            let _ = self.set(section, key, &text);
        }
    }
    /// Writes a row's described default (dockctl `row <id> reset`).
    fn reset_row(&mut self, section: &str, key: &str) -> Result<(), String> {
        let (target, follow) = self.target(section)?;
        if !self.rows.iter().any(|r| r.section == section && r.key == key) {
            return Err("row is not on the current page".into());
        }
        let sequence = self.seq + 1;
        self.writer
            .send(WriteJob::Reset {
                sequence,
                target,
                key: key.into(),
                follow,
            })
            .map_err(|_| "the writer has stopped".to_string())?;
        self.seq = sequence;
        if let Some(row) = self
            .rows
            .iter_mut()
            .find(|r| r.section == section && r.key == key)
        {
            row.pending = Some(sequence);
            row.draft = None;
            row.submitted = None;
            row.error = None;
        }
        Ok(())
    }
    /// Before the page's rows are replaced, every unsaved edit is committed as Enter would.
    fn commit_drafts(&mut self, ctx: &ComponentContext<Self>) {
        let drafts = self
            .rows
            .iter()
            .filter(|row| row.draft.is_some() && row.draft != row.submitted)
            .map(|row| {
                (
                    row.section.clone(),
                    row.key.clone(),
                    row.draft.clone().unwrap_or_default(),
                )
            })
            .collect::<Vec<_>>();
        for (section, key, text) in drafts {
            if let Err(error) = self.set(&section, &key, &text) {
                self.show_badge(format!("Not saved: {error}"), false, ctx);
            }
        }
    }
    fn go_to(&mut self, page: Page, ctx: &ComponentContext<Self>) {
        self.commit_drafts(ctx);
        self.page = page;
        self.refresh_rows();
        self.refresh_developer(true);
    }
    fn title_of(&self, name: &str) -> String {
        self.entries
            .iter()
            .find(|e| e.entry.name == name)
            .and_then(|e| e.describe.as_ref().ok())
            .map(|d| d.title.clone())
            .unwrap_or_else(|| name.to_string())
    }
    /// Shows a badge text; "Saved" expires after 2 s unless another badge replaced it.
    fn show_badge(&mut self, text: String, expires: bool, ctx: &ComponentContext<Self>) {
        self.badge_gen += 1;
        self.badge = text;
        if expires {
            let generation = self.badge_gen;
            ctx.spawn_background(move |_| {
                std::thread::sleep(std::time::Duration::from_secs(2));
                Message::ClearBadge(generation)
            });
        }
    }
    /// Starts a Start / Stop / Restart; refused while one runs for that widget.
    fn start_action(
        &mut self,
        name: &str,
        action: &str,
        ctx: &ComponentContext<Self>,
    ) -> Result<(), String> {
        if self.busy.contains(name) {
            return Err(format!(
                "{} is busy with another start or stop",
                self.title_of(name)
            ));
        }
        let entry = self
            .entries
            .iter()
            .find(|e| e.entry.name == name)
            .map(|e| e.entry.clone())
            .ok_or_else(|| format!("{name} isn't registered"))?;
        self.busy.insert(name.to_string());
        self.show_badge(format!("{}…", action_word(action)), false, ctx);
        work::action(WINDOW.with(|h| h.get()), entry, action.into());
        Ok(())
    }
    /// Replies now (after the next render), or once the writes queued since `sequence_before`
    /// all have results.
    fn wait_or_reply(
        &mut self,
        request: remote::Request,
        sequence_before: u64,
        reply: Value,
        ctx: &ComponentContext<Self>,
    ) {
        let mut sequences = std::collections::HashSet::new();
        let mut errors = Vec::new();
        for sequence in sequence_before + 1..=self.seq {
            match self.finished.get(&sequence) {
                Some(Some(error)) => errors.push(error.clone()),
                Some(None) => {}
                None => {
                    sequences.insert(sequence);
                }
            }
        }
        if !sequences.is_empty() {
            self.waiters.push(Waiter {
                request,
                sequences,
                errors,
            });
        } else if errors.is_empty() {
            reply_later(ctx, request, reply);
        } else {
            reply_later(ctx, request, json!({"ok":false,"error":errors.join("; ")}));
        }
    }
    /// A row id on the current page as (section, key).
    fn row_key(&self, id: &str) -> Option<(String, String)> {
        self.rows
            .iter()
            .find(|row| row_id(row) == id)
            .map(|row| (row.section.clone(), row.key.clone()))
    }
    /// The text `row <id> toggle` writes: a switch flipped, an Auto row's word or its off value.
    fn toggle_text(&self, id: &str) -> Option<(String, String, String)> {
        let row = self.rows.iter().find(|row| row_id(row) == id)?;
        let text = match &row.setting.kind {
            Kind::Switch => (!row.value.as_bool().unwrap_or(false)).to_string(),
            Kind::Auto { word, .. } => {
                if row.value.as_str() == Some(word.as_str()) {
                    model::auto_off_value(&row.setting.kind, word, &row.default)?.to_string()
                } else {
                    word.clone()
                }
            }
            _ => return None,
        };
        Some((row.section.clone(), row.key.clone(), text))
    }
    fn remote_click(&mut self, id: &str, ctx: &ComponentContext<Self>) -> Value {
        if id == "notepad" {
            open_notepad();
            return json!({"ok":true});
        }
        if id == "register" {
            self.register_widget_dialog();
            return json!({"ok":true});
        }
        if let Some(index) = id
            .strip_prefix("copy:")
            .and_then(|n| n.parse::<usize>().ok())
        {
            if let Some(command) = model::developer_commands().get(index) {
                taskbar_dock::text_edit::clipboard_write(WINDOW.with(|h| h.get()), command);
                self.developer_badge = "Copied".into();
                self.badge = "Copied".into();
                return json!({"ok":true});
            }
        }
        if let Some(name) = id.strip_prefix("refresh-log:") {
            if self.entries.iter().any(|entry| entry.entry.name == name) {
                self.refresh_developer(true);
                return json!({"ok":true});
            }
        }
        if let Some((action, name)) = id.split_once(':') {
            if action == "remove" && self.entries.iter().any(|e| e.entry.name == name) {
                let _ = self.writer.send(WriteJob::Remove { name: name.into() });
                return json!({"ok":true,"deferred_remove":name});
            }
            if ["start", "quit", "restart"].contains(&action) {
                return match self.start_action(name, action, ctx) {
                    Ok(()) => json!({"ok":true,"deferred_action":name}),
                    Err(error) => json!({"ok":false,"error":error}),
                };
            }
            if action == "open" && self.entries.iter().any(|e| e.entry.name == name) {
                self.go_to(Page::Widget(name.into()), ctx);
                return json!({"ok":true});
            }
            if action == "restore" {
                if let Some((section, group)) = name.split_once(':') {
                    if section == "dock" || self.entries.iter().any(|e| e.entry.name == section) {
                        self.restore_group(&format!("{section}|{group}"));
                        return json!({"ok":true});
                    }
                }
            }
        }
        json!({"ok":false,"error":"unknown click target"})
    }
    fn remote_key(&self, key: &str) -> Value {
        let hwnd = unsafe { FindWindowW(std::ptr::null(), wide("Taskbar Dock").as_ptr()) };
        if hwnd == 0 || unsafe { SetForegroundWindow(hwnd) } == 0 {
            return json!({"ok":false,"error":"can't bring the window to the front"});
        }
        let vk = match key {
            "tab" => 0x09,
            "shift+tab" => 0x09,
            "enter" => 0x0D,
            "esc" => 0x1B,
            "space" => 0x20,
            "up" => 0x26,
            "down" => 0x28,
            "left" => 0x25,
            "right" => 0x27,
            "home" => 0x24,
            "end" => 0x23,
            "f2" => 0x71,
            _ => return json!({"ok":false,"error":"unsupported key"}),
        };
        let key_input = |vk: u16, up: bool| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: vk,
                    wScan: 0,
                    dwFlags: if up { KEYEVENTF_KEYUP } else { 0 },
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        let mut inputs = vec![key_input(vk, false), key_input(vk, true)];
        if key == "shift+tab" {
            inputs.insert(0, key_input(VK_SHIFT, false));
            inputs.push(key_input(VK_SHIFT, true));
        }
        unsafe {
            let _ = SendInput(
                inputs.len() as u32,
                inputs.as_mut_ptr(),
                std::mem::size_of::<INPUT>() as i32,
            );
        }
        json!({"ok":true})
    }
    fn remote_type(&self, text: &str) -> Value {
        let hwnd = unsafe { FindWindowW(std::ptr::null(), wide("Taskbar Dock").as_ptr()) };
        if hwnd == 0 || unsafe { SetForegroundWindow(hwnd) } == 0 {
            return json!({"ok":false,"error":"can't bring the window to the front"});
        }
        unsafe {
            for unit in text.encode_utf16() {
                if GetForegroundWindow() != hwnd {
                    return json!({"ok":false,"error":"the window lost the front while typing"});
                }
                let mut pair = [
                    INPUT {
                        r#type: INPUT_KEYBOARD,
                        Anonymous: INPUT_0 {
                            ki: KEYBDINPUT {
                                wVk: 0,
                                wScan: unit,
                                dwFlags: KEYEVENTF_UNICODE,
                                time: 0,
                                dwExtraInfo: 0,
                            },
                        },
                    },
                    INPUT {
                        r#type: INPUT_KEYBOARD,
                        Anonymous: INPUT_0 {
                            ki: KEYBDINPUT {
                                wVk: 0,
                                wScan: unit,
                                dwFlags: KEYEVENTF_UNICODE | KEYEVENTF_KEYUP,
                                time: 0,
                                dwExtraInfo: 0,
                            },
                        },
                    },
                ];
                let _ = SendInput(2, pair.as_mut_ptr(), std::mem::size_of::<INPUT>() as i32);
            }
        }
        json!({"ok":true})
    }
    /// Restore defaults: resets every key of one group ("section|group"), Start with Windows
    /// excepted (it isn't a key).
    fn restore_group(&mut self, group: &str) {
        let (section, group) = group
            .split_once('|')
            .map_or((None, group), |(s, g)| (Some(s), g));
        let keys = self
            .rows
            .iter()
            .filter(|row| {
                row.setting.group == group
                    && row.key != "startup"
                    && section.is_none_or(|section| row.section == section)
            })
            .map(|row| (row.section.clone(), row.key.clone()))
            .collect::<Vec<_>>();
        for (section, key) in keys {
            if let Err(error) = self.reset_row(&section, &key) {
                if let Some(row) = self
                    .rows
                    .iter_mut()
                    .find(|r| r.section == section && r.key == key)
                {
                    row.error = Some(error);
                }
            }
        }
    }
    fn widgets_view(&self, ctx: &mut ViewContext<Self>) -> View {
        if self.widgets.is_empty() {
            return TextBlock::new()
                .text("No widgets are registered.")
                .font_size(14.0)
                .into();
        }
        let rows = self
            .widgets
            .iter()
            .map(|w| {
                let name = w.name.clone();
                // The card's text: icon, title, status and description. On a working widget it is
                // the content of one flat button that fills the whole card (see below), so the
                // whole card highlights and opens the page, except where the buttons and the
                // switch sit on top of it.
                let reserve = if process_present(&w.status) { 236.0 } else { 172.0 };
                let second_line = if w.reason.is_none() {
                    format!("{} · {}", w.status, w.description)
                } else {
                    w.reason.clone().unwrap_or_default()
                };
                let text = StackPanel::new()
                    .orientation(Orientation::Horizontal)
                    .spacing(14.0)
                    .margin(Thickness::new(0.0, 0.0, reserve, 0.0))
                    .vertical_alignment(VerticalAlignment::Center)
                    .children((
                        FontIcon::new()
                            .glyph(icon_for(&w.name))
                            .vertical_alignment(VerticalAlignment::Center),
                        StackPanel::new().spacing(2.0).children((
                            TextBlock::new()
                                .text(w.title.clone())
                                .font_size(14.0)
                                .text_trimming(TextTrimming::CharacterEllipsis),
                            StackPanel::new()
                                .orientation(Orientation::Horizontal)
                                .spacing(6.0)
                                .children((
                                    Border::new()
                                        .width(8.0)
                                        .height(8.0)
                                        .corner_radius(CornerRadius::uniform(4.0))
                                        .vertical_alignment(VerticalAlignment::Center)
                                        .background(status_brush(&w.status)),
                                    TextBlock::new()
                                        .text(second_line)
                                        .font_size(12.0)
                                        .opacity(0.78)
                                        .text_trimming(TextTrimming::CharacterEllipsis),
                                )),
                        )),
                    ));
                let text_area: View = if w.reason.is_none() {
                    Button::new()
                        .style(ButtonStyle::Subtle)
                        .horizontal_alignment(HorizontalAlignment::Stretch)
                        .vertical_alignment(VerticalAlignment::Stretch)
                        .horizontal_content_alignment(HorizontalAlignment::Stretch)
                        .vertical_content_alignment(VerticalAlignment::Center)
                        .is_enabled(self.banner.is_none())
                        .on_click(ctx.callback({
                            let n = name.clone();
                            move |_| Message::Open(n.clone())
                        }))
                        .automation_id(format!("open:{}", w.name))
                        .content(
                            Grid::new()
                                .columns([GridLength::STAR, GridLength::Auto])
                                .margin(Thickness::new(5.0, 0.0, 5.0, 0.0))
                                .children((
                                    Border::new().grid_column(0).content(text),
                                    FontIcon::new()
                                        .glyph("\u{E76C}")
                                        .grid_column(1)
                                        .vertical_alignment(VerticalAlignment::Center),
                                )),
                        )
                        .into()
                } else {
                    Border::new()
                        .padding(Thickness::new(16.0, 0.0, 16.0, 0.0))
                        .vertical_alignment(VerticalAlignment::Center)
                        .content(text)
                        .into()
                };
                let show = ToggleSwitch::new()
                    .min_width(0.0)
                    .min_width(0.0)
                    .is_on(w.show_on_taskbar)
                    .is_enabled(self.banner.is_none())
                    .on_toggled(ctx.callback({
                        let n = name.clone();
                        move |v: bool| {
                            Message::SetWidget(n.clone(), "show_on_taskbar".into(), Value::Bool(v))
                        }
                    }))
                    .automation_id(format!("{}.show_on_taskbar", w.name))
                    .slots([
                        SlotView::new(ToggleSwitchSlot::OnContent, TextBlock::new().text("")),
                        SlotView::new(ToggleSwitchSlot::OffContent, TextBlock::new().text("")),
                    ]);
                let actions = if process_present(&w.status) {
                    StackPanel::new()
                        .orientation(Orientation::Horizontal)
                        .spacing(4.0)
                        .children((
                            Button::new()
                                .is_enabled(self.banner.is_none())
                                .on_click(ctx.callback({
                                    let n = name.clone();
                                    move |_| Message::Action(n.clone(), "quit".into())
                                }))
                                .automation_id(format!("quit:{}", w.name))
                                .content("Stop"),
                            Button::new()
                                .is_enabled(self.banner.is_none())
                                .on_click(ctx.callback({
                                    let n = name.clone();
                                    move |_| Message::Action(n.clone(), "restart".into())
                                }))
                                .automation_id(format!("restart:{}", w.name))
                                .content("Restart"),
                        ))
                } else {
                    StackPanel::new().children((Button::new()
                        .is_enabled(self.banner.is_none())
                        .on_click(ctx.callback({
                            let n = name.clone();
                            move |_| Message::Action(n.clone(), "start".into())
                        }))
                        .automation_id(format!("start:{}", w.name))
                        .content("Start"),))
                };
                // Layers in one cell set: the flat button (text, chevron) spans the whole card;
                // the buttons and the switch sit on top of it, left of the chevron's column.
                Border::new()
                    .min_height(68.0)
                    .corner_radius(CornerRadius::uniform(4.0))
                    .background(ThemeBrush::CardBackground)
                    .border_brush(ThemeBrush::CardStroke)
                    .border_thickness(Thickness::uniform(1.0))
                    .horizontal_alignment(HorizontalAlignment::Stretch)
                    .content(
                        Grid::new()
                            .columns([
                                GridLength::STAR,
                                GridLength::Auto,
                                GridLength::Auto,
                                GridLength::Pixel(52.0),
                            ])
                            .column_spacing(12.0)
                            .children((
                                Border::new()
                                    .grid_column(0)
                                    .grid_column_span(4)
                                    .content(text_area),
                                Border::new()
                                    .grid_column(1)
                                    .vertical_alignment(VerticalAlignment::Center)
                                    .content(actions),
                                Border::new()
                                    .grid_column(2)
                                    .vertical_alignment(VerticalAlignment::Center)
                                    .content(show),
                            )),
                    )
            })
            .enumerate()
            .map(|(i, v)| KeyedView::new(i.to_string(), v))
            .collect::<Vec<_>>();
        StackPanel::new().spacing(4.0).keyed_children(rows)
    }
    fn settings_view(&self, section: Option<&str>, ctx: &mut ViewContext<Self>) -> View {
        let rows = if matches!(self.page, Page::Developer) {
            section
                .map(|section| {
                    self.rows
                        .iter()
                        .filter(|row| row.section == section)
                        .cloned()
                        .collect::<Vec<_>>()
                })
                .unwrap_or_else(|| self.rows.clone())
        } else {
            self.rows.clone()
        };
        let mut cards = Vec::<View>::new();
        for (i, r) in rows.iter().enumerate() {
            if let Some(g) = model::group_heading(&rows, i) {
                let group = g.to_string();
                let head = TextBlock::new()
                    .text(group.clone())
                    .font_size(14.0)
                    .font_weight(FontWeight::SEMI_BOLD)
                    .vertical_alignment(VerticalAlignment::Center);
                // One height for every heading line (the flat Restore defaults button's), so the
                // page doesn't shift when the button appears.
                let restore: View = if model::group_modified(&rows, &group) {
                    let sec = section.unwrap_or("dock").to_string();
                    let gid = format!("restore:{sec}:{group}");
                    Button::new()
                        .style(ButtonStyle::Subtle)
                        .vertical_alignment(VerticalAlignment::Center)
                        .on_click(ctx.callback({
                            let g = group.clone();
                            move |_| Message::Restore(format!("{sec}|{}", g))
                        }))
                        .automation_id(gid)
                        .content("Restore defaults")
                        .into()
                } else {
                    TextBlock::new().text("").into()
                };
                cards.push(
                    Grid::new()
                        .columns([GridLength::STAR, GridLength::Auto])
                        .min_height(32.0)
                        .margin(Thickness::new(0.0, 20.0, 0.0, 4.0))
                        .children((
                            Border::new()
                                .grid_column(0)
                                .vertical_alignment(VerticalAlignment::Center)
                                .content(head),
                            Border::new().grid_column(1).content(restore),
                        ))
                        .into(),
                );
            }
            let id = row_id(r);
            let control: View = match &r.setting.kind {
                Kind::Switch => ToggleSwitch::new()
                    .min_width(0.0)
                    .min_width(0.0)
                    .is_on(r.value.as_bool().unwrap_or(false))
                    .is_enabled(r.enabled)
                    .on_toggled(ctx.callback({
                        let s = r.section.clone();
                        let k = r.key.clone();
                        move |v: bool| Message::Set {
                            section: s.clone(),
                            key: k.clone(),
                            text: v.to_string(),
                        }
                    }))
                    .automation_id(id.clone())
                    .slots([
                        SlotView::new(ToggleSwitchSlot::OnContent, TextBlock::new().text("")),
                        SlotView::new(ToggleSwitchSlot::OffContent, TextBlock::new().text("")),
                    ])
                    .into(),
                Kind::Number { min, max, unit, .. } if r.control() == Control::NumberBox => {
                    number_box_control(r, unit, r.enabled, &id, ctx)
                }
                Kind::Number { min, max, step, .. } => {
                    slider_control(r, *min, *max, *step, r.enabled, &id, ctx)
                }
                Kind::Choice {
                    options,
                    other: true,
                } if r.choice_text_mode => TextBox::new()
                    .text(
                        r.draft
                            .clone()
                            .unwrap_or_else(|| r.value.as_str().unwrap_or_default().to_string()),
                    )
                    .placeholder_text("Other\u{2026}")
                    .is_enabled(r.enabled)
                    .on_text_changed(ctx.callback({
                        let s = r.section.clone();
                        let k = r.key.clone();
                        move |v| Message::Draft(s.clone(), k.clone(), v)
                    }))
                    .on_lost_focus(ctx.callback({
                        let s = r.section.clone();
                        let k = r.key.clone();
                        move |_| Message::Commit(s.clone(), k.clone())
                    }))
                    .width(240.0)
                    .automation_id(id.clone())
                    .into(),
                Kind::Choice {
                    options,
                    other: true,
                } => {
                    let mut items = options
                        .iter()
                        .map(|(_, label)| label.clone())
                        .collect::<Vec<_>>();
                    items.push("Other\u{2026}".into());
                    let selected = options.iter().position(|(value, _)| value == &r.value);
                    ComboBox::new()
                        .items_source(items)
                        .selected_index(selected)
                        .is_enabled(r.enabled)
                        .on_selection_changed(ctx.callback({
                            let s = r.section.clone();
                            let k = r.key.clone();
                            let opts = options.clone();
                            move |index: Option<usize>| match index {
                                Some(i) if i == opts.len() => {
                                    Message::OtherMode(s.clone(), k.clone())
                                }
                                Some(i) => opts
                                    .get(i)
                                    .map(|(value, _)| Message::Set {
                                        section: s.clone(),
                                        key: k.clone(),
                                        text: value.to_string(),
                                    })
                                    .unwrap_or(Message::Noop),
                                None => Message::Noop,
                            }
                        }))
                        .width(180.0)
                        .automation_id(id.clone())
                        .into()
                }
                Kind::Auto {
                    word,
                    label: auto_label,
                    inner,
                } => auto_control(r, word, auto_label, inner, &id, ctx),
                Kind::Choice {
                    options,
                    other: false,
                } => {
                    let items = options.iter().map(|(_, t)| t.clone()).collect::<Vec<_>>();
                    let sel = options.iter().position(|(v, _)| v == &r.value);
                    ComboBox::new()
                        .items_source(items)
                        .selected_index(sel)
                        .is_enabled(r.enabled)
                        .on_selection_changed(ctx.callback({
                            let s = r.section.clone();
                            let k = r.key.clone();
                            let opts = options.clone();
                            move |i: Option<usize>| {
                                i.and_then(|i| opts.get(i))
                                    .map(|(v, _)| Message::Set {
                                        section: s.clone(),
                                        key: k.clone(),
                                        text: v.to_string(),
                                    })
                                    .unwrap_or(Message::Noop)
                            }
                        }))
                        .width(180.0)
                        .automation_id(id.clone())
                        .into()
                }
                Kind::Optional(inner) => match inner.as_ref() {
                    Kind::Number { .. } => {
                        let number = NumberBox::new()
                            .value(r.value.as_f64())
                            .is_enabled(r.enabled)
                            .on_value_changed(ctx.callback({
                                let s = r.section.clone();
                                let k = r.key.clone();
                                move |v: Option<f64>| Message::Set {
                                    section: s.clone(),
                                    key: k.clone(),
                                    text: v.map(|x| x.to_string()).unwrap_or_default(),
                                }
                            }))
                            .automation_id(id.clone())
                            .width(160.0);
                        number.into()
                    }
                    _ => TextBox::new()
                        .text(r.draft.clone().unwrap_or_else(|| {
                            if r.value.is_null() {
                                String::new()
                            } else {
                                r.value.to_string()
                            }
                        }))
                        .placeholder_text("Not set")
                        .is_enabled(r.enabled)
                        .on_text_changed(ctx.callback({
                            let s = r.section.clone();
                            let k = r.key.clone();
                            move |v| Message::Draft(s.clone(), k.clone(), v)
                        }))
                        .on_lost_focus(ctx.callback({
                            let s = r.section.clone();
                            let k = r.key.clone();
                            move |_| Message::Commit(s.clone(), k.clone())
                        }))
                        .width(160.0)
                        .automation_id(id.clone())
                        .into(),
                },
                Kind::Color { .. } => {
                    let text = r
                        .draft
                        .clone()
                        .unwrap_or_else(|| r.value.as_str().unwrap_or_default().to_string());
                    let swatch: View = if let Some(color) = parse_swatch(&text) {
                        Border::new()
                            .width(20.0)
                            .height(20.0)
                            .corner_radius(CornerRadius::uniform(4.0))
                            .background(Brush::Solid(color))
                            .border_brush(ThemeBrush::CardStroke)
                            .border_thickness(Thickness::uniform(1.0))
                            .into()
                    } else {
                        TextBlock::new().text("").into()
                    };
                    StackPanel::new()
                        .orientation(Orientation::Horizontal)
                        .spacing(8.0)
                        .children((
                            swatch,
                            TextBox::new()
                                .placeholder_text("Default")
                                .text(text)
                                .is_enabled(r.enabled)
                                .on_text_changed(ctx.callback({
                                    let s = r.section.clone();
                                    let k = r.key.clone();
                                    move |v| Message::Draft(s.clone(), k.clone(), v)
                                }))
                                .on_lost_focus(ctx.callback({
                                    let s = r.section.clone();
                                    let k = r.key.clone();
                                    move |_| Message::Commit(s.clone(), k.clone())
                                }))
                                .width(120.0)
                                .automation_id(id.clone()),
                        ))
                }
                Kind::Secret => PasswordBox::new()
                    .width(240.0)
                    // The saved secret is never put on screen: the box holds only what is typed,
                    // and typing replaces the whole value.
                    .password(r.draft.as_deref().unwrap_or(""))
                    .placeholder_text(if r.value.as_str().is_some_and(|s| !s.is_empty()) {
                        "Saved; type to replace"
                    } else {
                        "Not set"
                    })
                    .is_enabled(r.enabled)
                    .on_password_changed(ctx.callback({
                        let s = r.section.clone();
                        let k = r.key.clone();
                        move |v| Message::Draft(s.clone(), k.clone(), v)
                    }))
                    .on_lost_focus(ctx.callback({
                        let s = r.section.clone();
                        let k = r.key.clone();
                        move |_| Message::Commit(s.clone(), k.clone())
                    }))
                    .automation_id(id.clone())
                    .into(),
                _ => TextBox::new()
                    .text(r.draft.clone().unwrap_or_else(|| {
                        r.value.as_str().map(str::to_string).unwrap_or_else(|| {
                            if r.value.is_null() {
                                String::new()
                            } else {
                                r.value.to_string()
                            }
                        })
                    }))
                    .placeholder_text(match &r.setting.kind {
                        Kind::Text { placeholder, .. } if !placeholder.is_empty() => placeholder.clone(),
                        _ => "Not set".to_string(),
                    })
                    .is_enabled(r.enabled)
                    .on_text_changed(ctx.callback({
                        let s = r.section.clone();
                        let k = r.key.clone();
                        move |v| Message::Draft(s.clone(), k.clone(), v)
                    }))
                    .on_lost_focus(ctx.callback({
                        let s = r.section.clone();
                        let k = r.key.clone();
                        move |_| Message::Commit(s.clone(), k.clone())
                    }))
                    .width(240.0)
                    .automation_id(id.clone())
                    .into(),
            };
            let mut lines: Vec<KeyedView> = vec![KeyedView::new(
                "label",
                TextBlock::new()
                    .text(r.setting.label.clone())
                    .font_size(14.0)
                    .text_wrapping(TextWrapping::Wrap),
            )];
            if !r.setting.help.is_empty() {
                lines.push(KeyedView::new(
                    "help",
                    TextBlock::new()
                        .text(r.setting.help.clone())
                        .font_size(12.0)
                        .opacity(0.78)
                        .text_wrapping(TextWrapping::Wrap),
                ));
            }
            if let Some(error) = &r.error {
                lines.push(KeyedView::new(
                    "error",
                    TextBlock::new()
                        .text(error.clone())
                        .font_size(12.0)
                        .text_wrapping(TextWrapping::Wrap)
                        .foreground(ThemeBrush::SystemCritical),
                ));
            }
            let labels = StackPanel::new().spacing(2.0).keyed_children(lines);
            let mut row = Grid::new()
                .columns([GridLength::STAR, GridLength::Auto])
                .column_spacing(16.0);
            if matches!(
                r.control(),
                Control::Text | Control::Color | Control::Secret
            ) || matches!(r.setting.kind, Kind::Optional(_))
            {
                let (s, k) = (r.section.clone(), r.key.clone());
                row = row.key_accelerators(KeyAccelerators::new([
                    KeyAccelerator::new(
                        AcceleratorKey::Enter,
                        AcceleratorModifiers::None,
                        ctx.callback({
                            let (s, k) = (s.clone(), k.clone());
                            move |_| Message::Commit(s.clone(), k.clone())
                        }),
                    ),
                    KeyAccelerator::new(
                        AcceleratorKey::Escape,
                        AcceleratorModifiers::None,
                        ctx.callback(move |_| Message::Cancel(s.clone(), k.clone())),
                    ),
                ]));
            }
            let row = row.children((
                Border::new()
                    .grid_column(0)
                    .vertical_alignment(VerticalAlignment::Center)
                    .content(labels),
                Border::new()
                    .grid_column(1)
                    .vertical_alignment(VerticalAlignment::Center)
                    .horizontal_alignment(HorizontalAlignment::Right)
                    .content(control),
            ));
            cards.push(
                Border::new()
                    .padding(Thickness::xy(16.0, 10.0))
                    .min_height(68.0)
                    .horizontal_alignment(HorizontalAlignment::Stretch)
                    .corner_radius(CornerRadius::uniform(4.0))
                    .background(ThemeBrush::CardBackground)
                    .border_brush(ThemeBrush::CardStroke)
                    .border_thickness(Thickness::uniform(1.0))
                    .content(row)
                    .into(),
            );
        }
        let _ = section;
        StackPanel::new().spacing(4.0).keyed_children(
            cards
                .into_iter()
                .enumerate()
                .map(|(i, v)| KeyedView::new(i.to_string(), v)),
        )
    }
    fn developer_view(&self, ctx: &mut ViewContext<Self>) -> View {
        let mut sections = Vec::<View>::new();
        sections.push(InfoBar::new()
            .title("")
            .message("These change how the widget connects and what it shows; wrong values can stop it working")
            .severity(InfoBarSeverity::Warning)
            .is_open(true)
            .is_closable(false)
            .automation_id("developer.warning")
            .into());
        let rows = model::advanced_rows(&self.entries, &self.settings);
        for entry in &self.entries {
            let name = &entry.entry.name;
            let expander_id = format!("expander:{name}");
            let title = entry
                .describe
                .as_ref()
                .map(|d| d.title.clone())
                .unwrap_or_else(|_| name.clone());
            let icon = match name.as_str() {
                "weather" => "\u{E753}",
                "bus" => "\u{E806}",
                "media" => "\u{E8D6}",
                "discord" => "\u{E95B}",
                _ => "\u{E7C1}",
            };
            let widget_rows = rows
                .iter()
                .filter(|r| r.section == *name)
                .cloned()
                .collect::<Vec<_>>();
            let inner: View = if let Err(reason) = &entry.describe {
                StackPanel::new()
                    .spacing(8.0)
                    .children((
                        TextBlock::new()
                            .text(format!("Describe failed: {reason}"))
                            .text_wrapping(TextWrapping::Wrap)
                            .foreground(ThemeBrush::SystemCritical),
                        TextBlock::new()
                            .text(format!("Exe: {}", entry.entry.exe.display()))
                            .text_wrapping(TextWrapping::Wrap),
                    ))
                    .into()
            } else {
                let advanced = self.settings_view(Some(name), ctx);
                let detail = self
                    .developer_details
                    .iter()
                    .find(|d| d["name"] == *name)
                    .cloned()
                    .unwrap_or(Value::Null);
                let logs = detail["log_lines"]
                    .as_array()
                    .map(|lines| {
                        lines
                            .iter()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .unwrap_or_else(|| "no log".into());
                let detail_card = StackPanel::new().spacing(8.0).children((
                    TextBlock::new()
                        .text("Details")
                        .font_size(14.0)
                        .font_weight(FontWeight::SEMI_BOLD),
                    TextBlock::new()
                        .text(format!(
                            "Exe: {}",
                            detail["exe"].as_str().unwrap_or_else(|| entry
                                .entry
                                .exe
                                .to_str()
                                .unwrap_or(""))
                        ))
                        .text_wrapping(TextWrapping::Wrap),
                    TextBlock::new().text(format!(
                        "Build: {}",
                        detail["build"].as_str().unwrap_or("loading")
                    )),
                    TextBlock::new().text(format!(
                        "PID: {}",
                        detail["pid"].as_str().unwrap_or("not running")
                    )),
                    Grid::new()
                        .columns([GridLength::STAR, GridLength::Auto])
                        .children((
                            TextBlock::new()
                                .text("Recent log lines")
                                .font_weight(FontWeight::SEMI_BOLD)
                                .grid_column(0),
                            Button::new()
                                .automation_id(format!("refresh-log:{name}"))
                                .on_click(ctx.callback(|_| Message::RefreshDeveloper))
                                .grid_column(1)
                                .content("Refresh"),
                        )),
                    Border::new()
                        .height(300.0)
                        .horizontal_alignment(HorizontalAlignment::Stretch)
                        .border_brush(ThemeBrush::CardStroke)
                        .border_thickness(Thickness::uniform(1.0))
                        .padding(Thickness::uniform(8.0))
                        .content(
                            ScrollViewer::new().content(
                                TextBlock::new()
                                    .text(logs)
                                    .font_size(12.0)
                                    .text_wrapping(TextWrapping::NoWrap),
                            ),
                        ),
                ));
                StackPanel::new()
                    .spacing(20.0)
                    .children((advanced, detail_card))
                    .into()
            };
            let header = StackPanel::new()
                .orientation(Orientation::Horizontal)
                .spacing(10.0)
                .children((
                    FontIcon::new().glyph(icon),
                    TextBlock::new()
                        .text(title)
                        .font_size(14.0),
                ));
            let mut expander = Expander::new()
                .horizontal_alignment(HorizontalAlignment::Stretch)
                .is_expanded(self.expanded.get(&expander_id).copied().unwrap_or(false))
                .on_is_expanded_changed(ctx.callback({
                    let id = expander_id.clone();
                    move |open| Message::Expand(id.clone(), open)
                }))
                .automation_id(expander_id.clone())
                .slots([
                    SlotView::new(ExpanderSlot::Header, header),
                    SlotView::new(ExpanderSlot::Content, inner),
                ]);
            let _ = &mut expander;
            sections.push(expander.into());
            let _ = widget_rows;
        }
        let registered = model::registered_widgets(&self.entries);
        let mut registry_lines = Vec::<View>::new();
        for item in registered {
            let name = item.name.clone();
            let detail = match item.error {
                Some(error) => format!("{}
{error}", item.exe),
                None => item.exe.to_string(),
            };
            let summary = StackPanel::new().spacing(2.0).children((
                TextBlock::new().text(item.title.clone()).font_size(14.0),
                TextBlock::new()
                    .text(detail)
                    .font_size(12.0)
                    .opacity(0.78)
                    .text_wrapping(TextWrapping::Wrap),
            ));
            registry_lines.push(
                Grid::new()
                    .columns([GridLength::STAR, GridLength::Auto])
                    .column_spacing(8.0)
                    .children((
                        Border::new().grid_column(0).content(summary),
                        Button::new()
                            .vertical_alignment(VerticalAlignment::Center)
                            .automation_id(format!("remove:{name}"))
                            .on_click(ctx.callback({
                                let name = name.clone();
                                move |_| Message::Action(name.clone(), "remove".into())
                            }))
                            .grid_column(1)
                            .content("Remove from list"),
                    ))
                    .into(),
            );
        }
        registry_lines.push(
            Button::new()
                .automation_id("register")
                .on_click(ctx.callback(|_| Message::RegisterDialog))
                .content("Register widget…")
                .into(),
        );
        let registered_view = StackPanel::new().spacing(8.0).keyed_children(
            registry_lines
                .into_iter()
                .enumerate()
                .map(|(i, v)| KeyedView::new(i.to_string(), v)),
        );
        let mut diagnostics = Vec::<View>::new();
        diagnostics.push(
            TextBlock::new()
                .text("Registered widgets")
                .font_size(14.0)
                .font_weight(FontWeight::SEMI_BOLD)
                .into(),
        );
        diagnostics.push(registered_view.into());
        diagnostics.push(
            TextBlock::new()
                .text("Raw settings")
                .font_size(14.0)
                .font_weight(FontWeight::SEMI_BOLD)
                .margin(Thickness::new(0.0, 20.0, 0.0, 0.0))
                .into(),
        );
        diagnostics.push(
            Grid::new()
                .columns([GridLength::STAR, GridLength::Auto])
                .children((
                    TextBlock::new()
                        .text("Secrets are masked")
                        .font_size(12.0)
                        .grid_column(0),
                    Button::new()
                        .automation_id("notepad")
                        .on_click(
                            ctx.callback(|_| Message::Action(String::new(), "notepad".into())),
                        )
                        .grid_column(1)
                        .content("Open in Notepad"),
                ))
                .into(),
        );
        diagnostics.push(
            Border::new()
                .height(280.0)
                .border_brush(ThemeBrush::CardStroke)
                .border_thickness(Thickness::uniform(1.0))
                .padding(Thickness::uniform(8.0))
                .content(
                    ScrollViewer::new().content(
                        TextBlock::new()
                            .text(
                                self.developer_raw
                                    .clone()
                                    .unwrap_or_else(|| "Loading settings…".into()),
                            )
                            .font_size(12.0)
                            .text_wrapping(TextWrapping::NoWrap),
                    ),
                )
                .into(),
        );
        diagnostics.push(
            TextBlock::new()
                .text("Console commands")
                .font_size(14.0)
                .font_weight(FontWeight::SEMI_BOLD)
                .margin(Thickness::new(0.0, 20.0, 0.0, 0.0))
                .into(),
        );
        for (index, command) in model::developer_commands().iter().enumerate() {
            diagnostics.push(
                Grid::new()
                    .columns([GridLength::STAR, GridLength::Auto])
                    .children((
                        TextBlock::new()
                            .text(*command)
                            .font_size(12.0)
                            .vertical_alignment(VerticalAlignment::Center)
                            .grid_column(0),
                        Button::new()
                            .automation_id(format!("copy:{index}"))
                            .on_click(ctx.callback(move |_| Message::Copy(index)))
                            .grid_column(1)
                            .content("Copy"),
                    ))
                    .into(),
            );
        }
        let diag_id = "expander:diagnostics".to_string();
        sections.push(
            Expander::new()
                .horizontal_alignment(HorizontalAlignment::Stretch)
                .automation_id(diag_id.clone())
                .is_expanded(self.expanded.get(&diag_id).copied().unwrap_or(false))
                .on_is_expanded_changed(ctx.callback({
                    let id = diag_id.clone();
                    move |open| Message::Expand(id.clone(), open)
                }))
                .slots([
                    SlotView::new(
                        ExpanderSlot::Header,
                        StackPanel::new()
                            .orientation(Orientation::Horizontal)
                            .spacing(10.0)
                            .children((
                                FontIcon::new().glyph("\u{E9D9}"),
                                TextBlock::new()
                                    .text("Diagnostics")
                                    .font_size(14.0),
                            )),
                    ),
                    SlotView::new(
                        ExpanderSlot::Content,
                        StackPanel::new().spacing(8.0).keyed_children(
                            diagnostics
                                .into_iter()
                                .enumerate()
                                .map(|(i, v)| KeyedView::new(i.to_string(), v)),
                        ),
                    ),
                ])
                .into(),
        );
        let title = if self.developer_badge.is_empty() {
            self.badge.clone()
        } else {
            self.developer_badge.clone()
        };
        let _ = title;
        StackPanel::new().spacing(12.0).keyed_children(
            sections
                .into_iter()
                .enumerate()
                .map(|(i, v)| KeyedView::new(i.to_string(), v)),
        )
    }
    fn register_widget_dialog(&mut self) {
        use windows_sys::Win32::UI::Controls::Dialogs::{
            GetOpenFileNameW, OFN_FILEMUSTEXIST, OFN_PATHMUSTEXIST, OPENFILENAMEW,
        };
        let filter = wide("Programs (*.exe)\0*.exe\0\0");
        let mut file = vec![0u16; 32768];
        let mut dialog: OPENFILENAMEW = unsafe { std::mem::zeroed() };
        dialog.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
        dialog.hwndOwner = WINDOW.with(|h| h.get());
        dialog.lpstrFilter = filter.as_ptr();
        dialog.lpstrFile = file.as_mut_ptr();
        dialog.nMaxFile = file.len() as u32;
        dialog.Flags = OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST;
        if unsafe { GetOpenFileNameW(&mut dialog) } != 0 {
            let length = file
                .iter()
                .position(|unit| *unit == 0)
                .unwrap_or(file.len());
            let exe = std::path::PathBuf::from(String::from_utf16_lossy(&file[..length]));
            let _ = self.writer.send(WriteJob::Register { exe });
            self.developer_badge = "Registering".into();
        }
    }
    fn widget_page_view(&self, name: &str, ctx: &mut ViewContext<Self>) -> View {
        let Some(widget) = self.widgets.iter().find(|widget| widget.name == name) else {
            return self.settings_view(Some(name), ctx);
        };
        let status_text = StackPanel::new()
            .orientation(Orientation::Horizontal)
            .spacing(14.0)
            .vertical_alignment(VerticalAlignment::Center)
            .children((
                FontIcon::new()
                    .glyph(icon_for(name))
                    .vertical_alignment(VerticalAlignment::Center),
                StackPanel::new().spacing(2.0).children((
                    TextBlock::new()
                        .text(widget.description.clone())
                        .font_size(14.0)
                        .text_wrapping(TextWrapping::Wrap),
                    StackPanel::new()
                        .orientation(Orientation::Horizontal)
                        .spacing(6.0)
                        .children((
                            Border::new()
                                .width(8.0)
                                .height(8.0)
                                .corner_radius(CornerRadius::uniform(4.0))
                                .vertical_alignment(VerticalAlignment::Center)
                                .background(status_brush(&widget.status)),
                            TextBlock::new()
                                .text(format!(
                                    "{}{}",
                                    widget.status,
                                    widget
                                        .pid
                                        .map(|pid| format!(" · pid {pid}"))
                                        .unwrap_or_default()
                                ))
                                .font_size(12.0)
                                .opacity(0.78),
                        )),
                )),
            ));
        let action_buttons = if process_present(&widget.status) {
            StackPanel::new()
                .orientation(Orientation::Horizontal)
                .spacing(4.0)
                .children((
                    Button::new()
                        .on_click(ctx.callback({
                            let n = name.to_string();
                            move |_| Message::Action(n.clone(), "quit".into())
                        }))
                        .automation_id(format!("quit:{name}"))
                        .content("Stop"),
                    Button::new()
                        .on_click(ctx.callback({
                            let n = name.to_string();
                            move |_| Message::Action(n.clone(), "restart".into())
                        }))
                        .automation_id(format!("restart:{name}"))
                        .content("Restart"),
                ))
        } else {
            StackPanel::new().children((Button::new()
                .on_click(ctx.callback({
                    let n = name.to_string();
                    move |_| Message::Action(n.clone(), "start".into())
                }))
                .automation_id(format!("start:{name}"))
                .content("Start"),))
        };
        // The widget's own card at the top, as Windows Settings heads a device page: icon,
        // description, status; Stop / Restart (or Start) on the right.
        let status = Border::new()
            .min_height(68.0)
            .padding(Thickness::xy(16.0, 10.0))
            .corner_radius(CornerRadius::uniform(4.0))
            .background(ThemeBrush::CardBackground)
            .border_brush(ThemeBrush::CardStroke)
            .border_thickness(Thickness::uniform(1.0))
            .content(
                Grid::new()
                    .columns([GridLength::STAR, GridLength::Auto])
                    .column_spacing(16.0)
                    .children((
                        Border::new()
                            .grid_column(0)
                            .vertical_alignment(VerticalAlignment::Center)
                            .content(status_text),
                        Border::new()
                            .grid_column(1)
                            .vertical_alignment(VerticalAlignment::Center)
                            .content(action_buttons),
                    )),
            );
        StackPanel::new()
            .spacing(4.0)
            .children((status, self.settings_view(Some(name), ctx)))
    }
}
fn nav_item(tag: &str, label: &str, glyph: &str, selected: bool) -> View {
    NavigationViewItem::new()
        .tag(tag)
        .is_selected(selected)
        .automation_id(format!("nav.{tag}"))
        .slots([
            SlotView::new(NavigationViewItemSlot::Icon, FontIcon::new().glyph(glyph)),
            SlotView::new(
                NavigationViewItemSlot::Content,
                TextBlock::new().text(label),
            ),
        ])
        .into()
}
fn row_id(row: &Row) -> String {
    if row.key == "startup" {
        format!("startup.{}", row.section)
    } else {
        format!("{}.{}", row.section, row.key)
    }
}
/// Whether a status means a process exists (Stop / Restart), not Start: a hung widget still
/// runs and needs Stop or Restart to recover.
fn process_present(status: &str) -> bool {
    matches!(status, "Running" | "Not responding")
}
fn status_label(state: &control::State) -> &'static str {
    match state {
        control::State::Running => "Running",
        control::State::NotResponding => "Not responding",
        control::State::Stopped => "Stopped",
        control::State::KeepsCrashing => "Keeps crashing",
        control::State::Unknown => "Unknown",
    }
}
fn status_brush(status: &str) -> ThemeBrush {
    match status {
        "Running" => ThemeBrush::SystemSuccess,
        "Stopped" => ThemeBrush::SystemNeutral,
        "Not responding" => ThemeBrush::SystemCaution,
        "Keeps crashing" => ThemeBrush::SystemCritical,
        _ => ThemeBrush::SystemNeutral,
    }
}
fn icon_for(name: &str) -> &'static str {
    match name {
        "weather" => "\u{E753}",
        "bus" => "\u{E806}",
        "media" => "\u{E8D6}",
        "discord" => "\u{E95B}",
        _ => "\u{E71D}",
    }
}

/// A slider with its value to the left (no box behind it). It writes on release; keyboard
/// changes settle after 400 ms (reactor has no key-up event for sliders).
fn slider_control(
    r: &Row,
    min: f64,
    max: f64,
    step: f64,
    enabled: bool,
    id: &str,
    ctx: &mut ViewContext<Model>,
) -> View {
    let value = r
        .draft
        .as_deref()
        .and_then(|value| value.parse::<f64>().ok())
        .or_else(|| r.value.as_f64())
        .unwrap_or(min);
    let slider = Slider::new()
        .minimum(min)
        .maximum(max)
        .step_frequency(step)
        .value(value)
        .is_enabled(enabled)
        .on_value_changed(ctx.callback({
            let s = r.section.clone();
            let k = r.key.clone();
            move |x: f64| Message::SliderEdit(s.clone(), k.clone(), x.to_string())
        }))
        .width(200.0)
        .vertical_alignment(VerticalAlignment::Center)
        .automation_id(id.to_string());
    Border::new()
        .on_pointer_released(ctx.callback({
            let s = r.section.clone();
            let k = r.key.clone();
            move |_| Message::SliderCommit(s.clone(), k.clone())
        }))
        .on_pointer_capture_lost(ctx.callback({
            let s = r.section.clone();
            let k = r.key.clone();
            move |_| Message::SliderCommit(s.clone(), k.clone())
        }))
        .content(
            StackPanel::new()
                .orientation(Orientation::Horizontal)
                .spacing(12.0)
                .children((
                    TextBlock::new()
                        .text(model::number_text(value, step))
                        .font_size(14.0)
                        .opacity(if enabled { 1.0 } else { 0.5 })
                        .vertical_alignment(VerticalAlignment::Center),
                    slider,
                )),
        )
}

/// An Auto row's control: "<label> [switch]" on the first line (on = the word), the inner slider
/// or number box under it, greyed while the switch is on and showing the value turning it off
/// would write.
fn auto_control(
    r: &Row,
    word: &str,
    auto_label: &str,
    inner: &Kind,
    id: &str,
    ctx: &mut ViewContext<Model>,
) -> View {
    let auto = r.value.as_str() == Some(word);
    let off_value = model::auto_off_value(&r.setting.kind, word, &r.default).unwrap_or(Value::Null);
    let switch = ToggleSwitch::new()
        .min_width(0.0)
        .is_on(auto)
        .is_enabled(r.enabled)
        .on_toggled(ctx.callback({
            let (s, k, word) = (r.section.clone(), r.key.clone(), word.to_string());
            let off = off_value.to_string();
            move |on: bool| Message::Set {
                section: s.clone(),
                key: k.clone(),
                text: if on { word.clone() } else { off.clone() },
            }
        }))
        .automation_id(id.to_string())
        .slots([
            SlotView::new(ToggleSwitchSlot::OnContent, TextBlock::new().text("")),
            SlotView::new(ToggleSwitchSlot::OffContent, TextBlock::new().text("")),
        ]);
    let first = StackPanel::new()
        .orientation(Orientation::Horizontal)
        .spacing(12.0)
        .horizontal_alignment(HorizontalAlignment::Right)
        .children((
            TextBlock::new()
                .text(auto_label.to_string())
                .font_size(14.0)
                .vertical_alignment(VerticalAlignment::Center),
            switch,
        ));
    let mut shown = r.clone();
    if auto {
        shown.value = off_value;
        shown.draft = None;
    }
    let inner_id = format!("{id}.value");
    let second: View = match inner {
        Kind::Number {
            min,
            max,
            unit,
            integer,
            ..
        } if model::number_control(unit, *integer) == Control::NumberBox => {
            number_box_control(&shown, unit, r.enabled && !auto, &inner_id, ctx)
        }
        Kind::Number { min, max, step, .. } => slider_control(
            &shown,
            *min,
            *max,
            *step,
            r.enabled && !auto,
            &inner_id,
            ctx,
        ),
        _ => TextBlock::new()
            .text(model::display_text(&shown.value))
            .into(),
    };
    StackPanel::new().spacing(6.0).children((
        first,
        Border::new()
            .horizontal_alignment(HorizontalAlignment::Right)
            .content(second),
    ))
}

/// A number box with its unit as separate text to the right (never inside the edited text).
fn number_box_control(
    r: &Row,
    unit: &str,
    enabled: bool,
    id: &str,
    ctx: &mut ViewContext<Model>,
) -> View {
    // No Minimum/Maximum on the box: WinUI would clamp an out-of-range entry before the app
    // sees it; the description's check refuses it instead (red line, nothing written).
    let number = NumberBox::new()
        .value(r.value.as_f64())
        .is_enabled(enabled)
        .on_value_changed(ctx.callback({
            let (s, k) = (r.section.clone(), r.key.clone());
            move |v: Option<f64>| {
                v.map(|x| Message::Set {
                    section: s.clone(),
                    key: k.clone(),
                    text: x.to_string(),
                })
                .unwrap_or(Message::Noop)
            }
        }))
        .automation_id(id.to_string())
        .width(160.0);
    if unit.is_empty() {
        number.into()
    } else {
        StackPanel::new()
            .orientation(Orientation::Horizontal)
            .spacing(8.0)
            .children((
                number,
                TextBlock::new()
                    .text(unit.to_string())
                    .font_size(14.0)
                    .opacity(0.78)
                    .vertical_alignment(VerticalAlignment::Center),
            ))
    }
}

/// Sends a dockctl reply once the update that produced it has been rendered: reactor drains
/// queued messages before it composes the view, so the reply takes a short trip through a
/// background task.
fn reply_later(ctx: &ComponentContext<Model>, request: remote::Request, reply: Value) {
    ctx.spawn_background(move |_| {
        std::thread::sleep(std::time::Duration::from_millis(30));
        Message::ReplyNow(request, reply)
    });
}

/// `key` / `type`: SendInput only queues the keys; the reply waits until WinUI has handled them
/// (and, if they queued writes, until those have results).
fn settle_input(
    ctx: &ComponentContext<Model>,
    request: remote::Request,
    sequence_before: u64,
    reply: Value,
) {
    ctx.spawn_background(move |_| {
        std::thread::sleep(std::time::Duration::from_millis(250));
        Message::InputSettled(request, sequence_before, reply)
    });
}

/// Closes the WinUI window (the app exits with it).
fn close_window() {
    let hwnd = unsafe { FindWindowW(std::ptr::null(), wide("Taskbar Dock").as_ptr()) };
    if hwnd != 0 {
        unsafe {
            PostMessageW(hwnd, WM_CLOSE, 0, 0);
        }
    }
}

/// Opens settings.json in Notepad.
fn open_notepad() {
    let path = taskbar_dock::settings::path().display().to_string();
    unsafe {
        ShellExecuteW(
            0,
            wide("open").as_ptr(),
            wide("notepad.exe").as_ptr(),
            wide(&path).as_ptr(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        );
    }
}

/// The word shown while a process action runs: "Starting", "Stopping", "Restarting".
fn action_word(action: &str) -> &'static str {
    match action {
        "start" => "Starting",
        "quit" => "Stopping",
        _ => "Restarting",
    }
}

/// The badge after a process action: "Weather started", "Not stopped: <reason>", ...
fn action_badge(
    title: &str,
    action: &str,
    result: &Result<u32, String>,
    warning: Option<&str>,
) -> String {
    let (done, verb) = match action {
        "start" => ("started", "started"),
        "quit" => ("stopped", "stopped"),
        _ => ("restarted", "restarted"),
    };
    match (result, warning) {
        (Ok(_), Some(warning)) => format!("{title} {done}; {warning}"),
        (Ok(_), None) => format!("{title} {done}"),
        (Err(error), _) => format!("Not {verb}: {error}"),
    }
}

fn parse_swatch(text: &str) -> Option<Color> {
    let value = text.strip_prefix('#').unwrap_or(text);
    if value.len() != 6 || !value.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let raw = u32::from_str_radix(value, 16).ok()?;
    Some(Color::rgb((raw >> 16) as u8, (raw >> 8) as u8, raw as u8))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handover_page_names_parse_for_root_and_widget_pages() {
        assert_eq!(page_from_name("widgets"), Page::Widgets);
        assert_eq!(page_from_name("dock"), Page::Dock);
        assert_eq!(page_from_name("developer"), Page::Developer);
        assert_eq!(page_from_name("weather"), Page::Widget("weather".into()));
    }

    #[test]
    fn messages_before_component_creation_are_queued_in_order() {
        EARLY.with(|queue| queue.borrow_mut().clear());
        send(Message::Navigate(Page::Dock));
        send(Message::Open("weather".into()));
        let mut messages = take_early().into_iter();
        assert!(matches!(
            messages.next(),
            Some(Message::Navigate(Page::Dock))
        ));
        assert!(matches!(messages.next(), Some(Message::Open(name)) if name == "weather"));
        assert!(messages.next().is_none());
    }
}
