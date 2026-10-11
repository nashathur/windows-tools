//! The bus stop and line picker, drawn inside Block's focused panel.

use crate::{data, data_dir};
use crate::ttqueue;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;
use taskbar_dock::block::{Canvas, Ctx, PanelFrame, PanelKey, PanelKeyCode, PanelMouse};
use taskbar_dock::text_edit::{self, TextEdit};
use taskbar_dock::{draw, theme};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::Threading::INFINITE;
use windows_sys::Win32::UI::WindowsAndMessaging::{GetCaretBlinkTime, PostMessageW, WM_APP};

pub(crate) const WM_PICKER: u32 = WM_APP + 3;
const MAX_SHOWN: usize = 300;
const MAX_VISIBLE: usize = 10;
const EVICT_AFTER: u64 = 60;
const TIMER_EVICT: usize = 2;
const TIMER_CARET: usize = 3;

/// The picker is either searching stop platforms or choosing a line at the chosen platform.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    Stops,
    Lines,
}

/// A worker result posted back to the bus widget's UI thread.
pub(crate) enum ResultData {
    Stops(Result<Vec<data::StopHit>, String>),
    Lines(Result<Vec<data::LineOpt>, String>),
}

/// A worker result tagged with the generation of the job that produced it.
pub(crate) struct Posted {
    pub(crate) generation: usize,
    pub(crate) data: ResultData,
}

/// The rectangles and row count shared by the picker's drawing and hit testing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Layout {
    back: (i32, i32, i32, i32),
    search: (i32, i32, i32, i32),
    list_top: i32,
    row_height: i32,
    visible_rows: usize,
    status: (i32, i32, i32, i32),
    cancel: (i32, i32, i32, i32),
    select: (i32, i32, i32, i32),
}

/// A target that can be hovered or pressed in the picker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    Back,
    Row(usize),
    Cancel,
    Select,
}

/// A background lookup needed when the picker panel is reopened.
#[derive(Clone, Debug, PartialEq, Eq)]
enum OpenJob {
    Stops,
    Lines(String),
}

/// Computes every picker control rectangle and the number of rows that fit.
fn layout(width: i32, height: i32, scale: f32) -> Layout {
    let px = |v: f32| (v * scale).round() as i32;
    let pad = px(theme::space::PANEL_PADDING);
    let row_height = px(theme::space::ROW_HEIGHT_LARGE).max(1);
    let button_bottom = height - pad;
    let button_top = button_bottom - px(40.0);
    let status_top = button_top - px(42.0);
    let list_top = px(136.0);
    let visible_rows = ((status_top - list_top) / row_height).clamp(0, MAX_VISIBLE as i32) as usize;
    Layout {
        back: (pad, pad, pad + px(28.0), pad + px(28.0)),
        search: (pad, px(76.0), width - pad, px(116.0)),
        list_top,
        row_height,
        visible_rows,
        status: (pad, status_top, width - pad, status_top + px(36.0)),
        cancel: (width - pad - px(176.0), button_top, width - pad - px(88.0), button_bottom),
        select: (width - pad - px(80.0), button_top, width - pad, button_bottom),
    }
}

/// The persistent state and UI behavior for the bus stop picker panel.
pub(crate) struct Picker {
    mode: Mode,
    edit: TextEdit,
    stops: Vec<data::StopHit>,
    filtered: Vec<usize>,
    chosen: Option<(String, String)>,
    lines: Vec<data::LineOpt>,
    highlight: usize,
    first: usize,
    status: String,
    loading: bool,
    loading_lines: bool,
    generation: usize,
    live_generation: Arc<AtomicUsize>,
    closed_at: Option<Instant>,
    pressed: Option<Target>,
    double_clicked: bool,
    hovered: Option<Target>,
    panel_width: i32,
    panel_height: i32,
    scale: f32,
    opened: bool,
    caret_epoch: Instant,
}

impl Default for Picker {
    /// Creates an empty picker ready to search for a stop.
    fn default() -> Self {
        Self {
            mode: Mode::Stops,
            edit: TextEdit::default(),
            stops: Vec::new(),
            filtered: Vec::new(),
            chosen: None,
            lines: Vec::new(),
            highlight: 0,
            first: 0,
            status: "Type part of the stop name.".into(),
            loading: false,
            loading_lines: false,
            generation: 0,
            live_generation: Arc::new(AtomicUsize::new(0)),
            closed_at: None,
            pressed: None,
            double_clicked: false,
            hovered: None,
            panel_width: theme::size::PANEL_WIDE as i32,
            panel_height: theme::size::PANEL_PICKER_HEIGHT as i32,
            scale: 1.0,
            opened: false,
            caret_epoch: Instant::now(),
        }
    }
}

impl Picker {
    /// Advances the picker generation and publishes it to queued worker jobs.
    fn next_generation(&mut self) -> usize {
        self.generation = self.generation.wrapping_add(1);
        self.live_generation.store(self.generation, Ordering::Release);
        self.generation
    }

    /// Reports the picker's current step, search, rows, status and job state to dockctl.
    pub(crate) fn state(&self) -> serde_json::Value {
        serde_json::json!({
            "step": if self.mode == Mode::Stops && !self.loading_lines { "stops" } else { "lines" },
            "search": self.edit.text(),
            "row_count": self.rows_len(),
            "highlighted_row": self.highlight,
            "highlighted": self.row_label(self.highlight),
            "first_rows": self.visible_labels().into_iter().take(3).collect::<Vec<_>>(),
            "status": self.status,
            "search_running": self.loading,
        })
    }

    /// Returns the number of rows in the active step.
    fn rows_len(&self) -> usize {
        if self.loading_lines {
            0
        } else if self.mode == Mode::Stops {
            self.filtered.len()
        } else {
            self.lines.len()
        }
    }

    /// Returns the currently visible row labels for the panel state report.
    fn visible_labels(&self) -> Vec<String> {
        let count = self.layout().visible_rows;
        let end = (self.first + count).min(self.rows_len());
        (self.first..end).map(|i| self.row_label(i)).collect()
    }

    /// Formats the label for a stop or line row.
    fn row_label(&self, i: usize) -> String {
        match self.mode {
            Mode::Stops => self
                .filtered
                .get(i)
                .and_then(|&n| self.stops.get(n))
                .map(|s| format!("{} ({})", s.name, s.code))
                .unwrap_or_default(),
            Mode::Lines => self
                .lines
                .get(i)
                .map(|l| {
                    let next = if l.next.is_empty() { "trip ends here".to_string() } else { format!("next stop {}", l.next) };
                    format!("Line {} ({}) · {next} · {}", l.line, l.agency, l.headsign)
                })
                .unwrap_or_default(),
        }
    }

    /// Computes layout from the current panel size and scale.
    fn layout(&self) -> Layout {
        layout(self.panel_width, self.panel_height, self.scale)
    }

    /// Starts loading the stop list on a worker and invalidates older jobs.
    fn start_stops(&mut self, main: isize) {
        let generation = self.next_generation();
        self.loading = true;
        self.loading_lines = false;
        self.status = "Loading stops…".into();
        let live_generation = self.live_generation.clone();
        ttqueue::submit(move || {
            if live_generation.load(Ordering::Acquire) != generation { return; }
            post(main, generation, ResultData::Stops(data::all_stops(data_dir()).map_err(|e| e.to_string())))
        });
    }

    /// Handles opening or closing the picker and manages its timers and lazy loading.
    pub(crate) fn shown(&mut self, open: bool, ctx: &mut Ctx) {
        self.opened = open;
        if open {
            self.closed_at = None;
            ctx.stop(TIMER_EVICT);
            self.sync_caret_timer(ctx);
            match self.open_job() {
                Some(OpenJob::Stops) => self.start_stops(ctx.main_window()),
                Some(OpenJob::Lines(code)) => self.start_lines(ctx.main_window(), code),
                None if self.mode == Mode::Stops && !self.loading_lines => self.refilter_keep_highlight(),
                None => {}
            }
        } else {
            self.closed_at = Some(Instant::now());
            self.pressed = None;
            self.hovered = None;
            ctx.stop(TIMER_CARET);
            ctx.every(TIMER_EVICT, 1000);
        }
        ctx.redraw_panel();
    }

    /// Starts or stops caret blinking according to the active step and Windows setting.
    fn sync_caret_timer(&mut self, ctx: &mut Ctx) {
        if self.opened && self.mode == Mode::Stops && !self.loading_lines {
            let blink = unsafe { GetCaretBlinkTime() };
            if blink == INFINITE {
                ctx.stop(TIMER_CARET);
            } else {
                ctx.every(TIMER_CARET, blink.max(1));
            }
        } else {
            ctx.stop(TIMER_CARET);
        }
    }

    /// Advances the caret or releases the stop list after its idle eviction period.
    pub(crate) fn timer(&mut self, id: usize, ctx: &mut Ctx) {
        if id == TIMER_CARET {
            ctx.redraw_panel();
        } else if id == TIMER_EVICT && !self.loading && self.closed_at.is_some_and(|t| t.elapsed().as_secs() >= EVICT_AFTER) {
            self.evict_stops();
            self.closed_at = None;
            ctx.stop(TIMER_EVICT);
        }
    }

    /// Releases only stop-list memory while retaining the current step and selection state.
    fn evict_stops(&mut self) {
        self.stops.clear();
        self.stops.shrink_to_fit();
        self.filtered.clear();
    }

    /// Reports which background lookup reopening the current step needs, if any.
    fn open_job(&self) -> Option<OpenJob> {
        if self.mode == Mode::Lines {
            if self.lines.is_empty() && !self.loading_lines {
                return self.chosen.as_ref().map(|(code, _)| OpenJob::Lines(code.clone()));
            }
        } else if self.stops.is_empty() && !self.loading {
            return Some(OpenJob::Stops);
        }
        None
    }

    /// Applies a worker result only when its generation is still current.
    pub(crate) fn accept_result(&mut self, generation: usize, data: ResultData, ctx: &mut Ctx) {
        if !result_is_current(self.generation, generation) {
            return;
        }
        self.loading = false;
        match data {
            ResultData::Stops(Ok(stops)) => {
                let count = stops.len();
                self.stops = stops;
                if self.mode == Mode::Lines {
                    if let Some((code, _)) = self.chosen.clone() {
                        self.start_lines(ctx.main_window(), code);
                    }
                } else {
                    self.refilter_keep_highlight();
                    if self.filtered.is_empty() && self.edit.text().is_empty() {
                        self.status = format!("{count} platforms loaded. Type part of the stop name.");
                    }
                }
            }
            ResultData::Stops(Err(e)) => self.status = format!("Couldn't read the timetable: {e}"),
            ResultData::Lines(Ok(lines)) if !lines.is_empty() => {
                self.loading_lines = false;
                self.lines = lines;
                self.mode = Mode::Lines;
                self.highlight = 0;
                self.first = 0;
                self.status = "Pick the line and direction (“next stop” shows which way it goes), then Select.".into();
            }
            ResultData::Lines(Ok(_)) => {
                self.loading_lines = false;
                self.mode = Mode::Stops;
                self.status = "No buses found at this platform. Press Back or search again.".into();
            }
            ResultData::Lines(Err(e)) => {
                self.loading_lines = false;
                self.mode = Mode::Stops;
                self.status = format!("Couldn't read the timetable: {e}");
            }
        }
        if !self.loading && self.closed_at.is_some() {
            self.closed_at = Some(Instant::now());
        }
        self.sync_caret_timer(ctx);
        ctx.redraw_panel();
    }

    /// Rebuilds stop matches after the search text changes.
    fn refilter(&mut self) {
        let query = self.edit.text().trim().to_string();
        let terms: Vec<String> = data::fold(&query).split_whitespace().map(str::to_string).collect();
        self.mode = Mode::Stops;
        self.lines.clear();
        if self.stops.is_empty() && self.loading && !self.loading_lines {
            self.filtered.clear();
            self.status = "Loading stops…".into();
            return;
        }
        if terms.is_empty() {
            self.filtered.clear();
            self.status = "Type part of the stop name.".into();
            self.highlight = 0;
            self.first = 0;
            return;
        }
        let hits = matching_stops(&self.stops, &query);
        let total = hits.len();
        self.filtered = hits.into_iter().take(MAX_SHOWN).collect();
        self.highlight = 0;
        self.first = 0;
        self.status = match total {
            0 => "No stops found.".into(),
            n if n > MAX_SHOWN => format!("{n} platforms; showing the first {MAX_SHOWN}, type more to narrow down."),
            n => format!("{n} platforms (each direction of a stop is a separate platform). Pick one, then Select."),
        };
    }

    /// Ref filters the search while retaining its prior highlighted row when possible.
    fn refilter_keep_highlight(&mut self) {
        let previous = self.highlight;
        self.refilter();
        self.move_highlight(previous);
    }

    /// Starts a line lookup for a chosen stop on a worker thread.
    fn start_lines(&mut self, main: isize, code: String) {
        let generation = self.next_generation();
        self.loading = true;
        self.loading_lines = true;
        let name = self.chosen.as_ref().map_or("", |x| x.1.as_str()).to_string();
        self.status = format!("Finding the lines at {name} ({code})… this reads the whole timetable, up to a minute.");
        let live_generation = self.live_generation.clone();
        ttqueue::submit(move || {
            if live_generation.load(Ordering::Acquire) != generation { return; }
            post(main, generation, ResultData::Lines(data::lines_at_stop(data_dir(), &code).map_err(|e| e.to_string())))
        });
    }

    /// Handles picker keyboard input and reports whether the key was consumed.
    pub(crate) fn key(&mut self, key: PanelKey, ctx: &mut Ctx) -> bool {
        if key.alt && key.key == PanelKeyCode::Left {
            return self.back(ctx);
        }
        if key.key == PanelKeyCode::Escape {
            return if self.mode == Mode::Lines || self.loading_lines {
                self.back(ctx);
                true
            } else {
                false
            };
        }
        if key.key == PanelKeyCode::Tab {
            return true;
        }
        if key.ctrl && matches!(key.key, PanelKeyCode::Home | PanelKeyCode::End) {
            self.move_highlight(if key.key == PanelKeyCode::Home {
                0
            } else {
                self.rows_len().saturating_sub(1)
            });
            ctx.redraw_panel();
            return true;
        }
        if self.mode == Mode::Stops && !self.loading_lines {
            if key.ctrl && key.key == PanelKeyCode::A {
                self.edit.select_all();
            } else if key.ctrl && key.key == PanelKeyCode::C {
                // nothing selected: the clipboard keeps what it has
                if self.edit.all_selected() {
                    text_edit::clipboard_write(ctx.main_window(), &self.edit.copy());
                }
            } else if key.ctrl && key.key == PanelKeyCode::X {
                if self.edit.all_selected() {
                    text_edit::clipboard_write(ctx.main_window(), &self.edit.cut());
                    self.refilter();
                }
            } else if key.ctrl && key.key == PanelKeyCode::V {
                if let Some(text) = text_edit::clipboard_read() {
                    self.edit.paste(&text);
                    self.refilter();
                }
            } else if key.ctrl && key.key == PanelKeyCode::Backspace {
                self.edit.ctrl_backspace();
                self.refilter();
            } else {
                match key.key {
                    PanelKeyCode::Up => self.move_highlight(self.highlight.saturating_sub(1)),
                    PanelKeyCode::Down => self.move_highlight(self.highlight.saturating_add(1)),
                    PanelKeyCode::PageUp => self.move_highlight(self.highlight.saturating_sub(self.layout().visible_rows.max(1))),
                    PanelKeyCode::PageDown => self.move_highlight(self.highlight.saturating_add(self.layout().visible_rows.max(1))),
                    PanelKeyCode::Home => self.edit.home(),
                    PanelKeyCode::End => self.edit.end(),
                    PanelKeyCode::Left => self.edit.left(),
                    PanelKeyCode::Right => self.edit.right(),
                    PanelKeyCode::Backspace => {
                        self.edit.backspace();
                        self.refilter();
                    }
                    PanelKeyCode::Delete => {
                        self.edit.delete();
                        self.refilter();
                    }
                    PanelKeyCode::Enter => self.select(ctx),
                    _ => return false,
                }
            }
        } else {
            match key.key {
                PanelKeyCode::Up => self.move_highlight(self.highlight.saturating_sub(1)),
                PanelKeyCode::Down => self.move_highlight(self.highlight.saturating_add(1)),
                PanelKeyCode::PageUp => self.move_highlight(self.highlight.saturating_sub(self.layout().visible_rows.max(1))),
                PanelKeyCode::PageDown => self.move_highlight(self.highlight.saturating_add(self.layout().visible_rows.max(1))),
                PanelKeyCode::Home => self.move_highlight(0),
                PanelKeyCode::End => self.move_highlight(self.rows_len().saturating_sub(1)),
                PanelKeyCode::Enter => self.select(ctx),
                _ => return false,
            }
        }
        self.sync_caret_timer(ctx);
        ctx.redraw_panel();
        true
    }

    /// Inserts typed text into the stop search and refreshes its results.
    pub(crate) fn character(&mut self, text: &str, ctx: &mut Ctx) {
        if self.mode == Mode::Stops && !self.loading_lines {
            self.edit.insert(text);
            self.refilter();
            ctx.redraw_panel();
        }
    }

    /// Scrolls the visible row window and keeps the highlight within the result list.
    pub(crate) fn wheel(&mut self, delta: i32, ctx: &mut Ctx) {
        self.scroll(delta);
        ctx.redraw_panel();
    }

    /// The wheel's scrolling: by one row per notch, the highlight kept on screen.
    fn scroll(&mut self, delta: i32) {
        let visible = self.layout().visible_rows;
        // no rows (a line search running, nothing found): nothing to scroll; the stop highlight
        // stays for Back
        let total = self.rows_len();
        if total == 0 || visible == 0 {
            return;
        }
        let rows = (delta / 120).unsigned_abs().max(1) as usize;
        let last_first = total.saturating_sub(visible);
        self.first = if delta > 0 { self.first.saturating_sub(rows) } else { self.first + rows }.min(last_first);
        // the highlight stays on screen: first ..= the last visible row
        self.highlight = self.highlight.clamp(self.first, (self.first + visible).min(total) - 1);
    }

    /// Moves the highlighted row and scrolls it into the visible list area.
    fn move_highlight(&mut self, n: usize) {
        let visible = self.layout().visible_rows;
        if self.rows_len() == 0 || visible == 0 {
            self.highlight = 0;
            self.first = 0;
            return;
        }
        self.highlight = n.min(self.rows_len() - 1);
        if self.highlight < self.first {
            self.first = self.highlight;
        }
        if self.highlight >= self.first + visible {
            self.first = self.highlight + 1 - visible;
        }
    }

    /// Returns to stop search, cancelling a line job or loading the stop list if it was evicted.
    fn back(&mut self, ctx: &mut Ctx) -> bool {
        if !self.back_to_stops() {
            return false;
        }
        if self.stops.is_empty() && !self.loading {
            self.start_stops(ctx.main_window());
        } else if self.stops.is_empty() {
            self.status = "Loading stops…".into();
        } else {
            self.status = "Pick a stop, then Select.".into();
            self.refilter_keep_highlight();
        }
        self.sync_caret_timer(ctx);
        ctx.redraw_panel();
        true
    }

    /// Switches back to stop search and invalidates any line lookup in progress.
    fn back_to_stops(&mut self) -> bool {
        if self.mode != Mode::Lines && !self.loading_lines {
            return false;
        }
        self.cancel_line_lookup();
        self.lines.clear();
        self.mode = Mode::Stops;
        true
    }

    /// Invalidates a running line lookup so its eventual result is ignored.
    fn cancel_line_lookup(&mut self) {
        if self.loading_lines {
            self.next_generation();
            self.loading = false;
            self.loading_lines = false;
        }
    }

    /// Selects the highlighted stop or line and resets the picker after a successful choice.
    fn select(&mut self, ctx: &mut Ctx) {
        match self.mode {
            Mode::Stops if !self.loading => {
                if let Some(&i) = self.filtered.get(self.highlight) {
                    let stop = &self.stops[i];
                    let (code, name) = (stop.code.clone(), stop.name.clone());
                    self.chosen = Some((code.clone(), name));
                    self.start_lines(ctx.main_window(), code);
                }
            }
            Mode::Lines => {
                if let (Some(line), Some((code, name))) = (self.lines.get(self.highlight), self.chosen.clone()) {
                    let cfg = chosen_config(&name, &code, line);
                    if let Err(e) = crate::worker::set_config(cfg) {
                        self.status = format!("Saving failed: {e}");
                    } else {
                        self.reset_after_select();
                        ctx.close_panel();
                    }
                }
            }
            _ => {}
        }
        self.sync_caret_timer(ctx);
        ctx.redraw_panel();
    }

    /// Resets picker state while preserving a strictly newer job generation.
    fn reset_after_select(&mut self) {
        let generation = self.generation;
        let live_generation = self.live_generation.clone();
        *self = Self {
            generation,
            live_generation,
            ..Self::default()
        };
        self.next_generation();
    }

    /// Tracks hover, presses, clicks and double clicks against the shared layout.
    pub(crate) fn mouse(&mut self, ev: PanelMouse, ctx: &mut Ctx) {
        let layout = self.layout();
        match ev {
            PanelMouse::Move(x, y) => {
                let next = self.target_at(layout, x, y);
                if next != self.hovered {
                    self.hovered = next;
                    ctx.redraw_panel();
                }
            }
            PanelMouse::Leave => {
                if self.hovered.take().is_some() {
                    ctx.redraw_panel();
                }
            }
            PanelMouse::Down(x, y) => {
                self.pressed = self.target_at(layout, x, y);
                self.double_clicked = false;
            }
            PanelMouse::DoubleClick(x, y) => {
                self.pressed = self.target_at(layout, x, y);
                self.double_clicked = matches!(self.pressed, Some(Target::Row(_)));
            }
            PanelMouse::Up(x, y) => {
                let released = self.target_at(layout, x, y);
                let pressed = self.pressed.take();
                if pressed.is_some() && pressed == released {
                    match released {
                        Some(Target::Back) => {
                            self.back(ctx);
                        }
                        Some(Target::Cancel) => ctx.close_panel(),
                        Some(Target::Select) => self.select(ctx),
                        Some(Target::Row(row)) => {
                            self.move_highlight(row);
                            if self.double_clicked {
                                self.select(ctx);
                            }
                        }
                        None => {}
                    }
                }
                self.double_clicked = false;
                ctx.redraw_panel();
            }
            PanelMouse::Cancel => {
                self.pressed = None;
                self.double_clicked = false;
            }
        }
    }

    /// Returns the interactive target at a panel point.
    fn target_at(&self, layout: Layout, x: i32, y: i32) -> Option<Target> {
        let within = |r: (i32, i32, i32, i32)| x >= r.0 && x < r.2 && y >= r.1 && y < r.3;
        if (self.mode == Mode::Lines || self.loading_lines) && within(layout.back) {
            return Some(Target::Back);
        }
        if within(layout.cancel) {
            return Some(Target::Cancel);
        }
        if within(layout.select) {
            return Some(Target::Select);
        }
        let pad = (theme::space::PANEL_PADDING * self.scale).round() as i32;
        if x >= pad && x < self.panel_width - pad && y >= layout.list_top && layout.row_height > 0 {
            let slot = ((y - layout.list_top) / layout.row_height) as usize;
            if slot < layout.visible_rows {
                let row = self.first + slot;
                if row < self.rows_len() {
                    return Some(Target::Row(row));
                }
            }
        }
        None
    }

    /// Draws the picker header, search box, visible rows, status and action buttons.
    pub(crate) fn draw(&mut self, cv: &mut Canvas, f: &PanelFrame) {
        self.panel_width = cv.w;
        self.panel_height = f.h;
        self.scale = f.scale;
        let layout = self.layout();
        let px = |v: f32| (v * f.scale).round() as i32;
        let theme = f.theme();
        let pad = px(theme::space::PANEL_PADDING);
        let width = cv.w;
        let line_step = self.mode == Mode::Lines || self.loading_lines;
        unsafe {
            let dc = cv.dc;
            let old_font = GetCurrentObject(dc, OBJ_FONT as u32);
            SetBkMode(dc, TRANSPARENT as i32);
            let face = taskbar_dock::wide(&f.look.font);
            let icon_face = taskbar_dock::wide(theme::ICON_FONT);
            let font = |style: theme::TextStyle, family: &[u16]| {
                CreateFontW(
                    -style.px_at(f.scale),
                    0,
                    0,
                    0,
                    style.weight,
                    0,
                    0,
                    0,
                    1,
                    0,
                    0,
                    5,
                    0,
                    family.as_ptr(),
                )
            };
            let title_font = font(theme::TITLE, &face);
            let body_font = font(theme::BODY, &face);
            let caption_font = font(theme::CAPTION, &face);
            let icon_font = font(theme::ICON, &icon_face);
            let draw_text = |font: HFONT, rect: (i32, i32, i32, i32), color: theme::Rgb, value: &str, flags: u32| {
                SelectObject(dc, font);
                SetTextColor(dc, color.colorref());
                let mut rect = RECT {
                    left: rect.0,
                    top: rect.1,
                    right: rect.2,
                    bottom: rect.3,
                };
                let wide = taskbar_dock::wide(value);
                DrawTextW(dc, wide.as_ptr(), -1, &mut rect, flags);
            };
            let title_x = pad + if line_step { px(32.0) } else { 0 };
            draw_text(
                title_font,
                (title_x, pad, width - pad, pad + px(24.0)),
                theme.color.fg,
                "Change stop",
                DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX,
            );
            draw_text(
                caption_font,
                (title_x, pad + px(25.0), width - pad, pad + px(45.0)),
                theme.color.dim,
                if line_step { "Pick line and direction" } else { "Find your stop" },
                DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX,
            );
            if line_step {
                if self.hovered == Some(Target::Back) {
                    draw::list_row(cv.pixels(), width, layout.back, f.look, f.scale, true);
                }
                draw_text(
                    icon_font,
                    layout.back,
                    theme.color.fg,
                    "\u{E72B}",
                    DT_SINGLELINE | DT_CENTER | DT_VCENTER | DT_NOPREFIX,
                );
            }
            if !line_step {
                draw::text_box(cv.pixels(), width, layout.search, f.look, f.scale, true);
                let inner_width = layout.search.2 - layout.search.0 - px(24.0);
                let (shown, caret_x) = visible_text(dc, body_font, self.edit.text(), self.edit.caret(), inner_width);
                if self.edit.all_selected() {
                    let selected_width = measure(dc, body_font, &shown).min(inner_width);
                    draw::fill_round(
                        cv.pixels(),
                        width,
                        (
                            (layout.search.0 + px(12.0)) as f32,
                            (layout.search.1 + px(6.0)) as f32,
                            (layout.search.0 + px(12.0) + selected_width) as f32,
                            (layout.search.3 - px(6.0)) as f32,
                        ),
                        (f.look.control_radius * f.scale).round(),
                        theme.status.accent.tuple(),
                        0.28,
                    );
                }
                draw_text(
                    body_font,
                    (
                        layout.search.0 + px(12.0),
                        layout.search.1 + px(7.0),
                        layout.search.2 - px(12.0),
                        layout.search.3 - px(7.0),
                    ),
                    theme.color.fg,
                    &shown,
                    DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX,
                );
                if caret_visible(self.caret_epoch) {
                    let caret_x = layout.search.0 + px(12.0) + caret_x;
                    draw::fill_round(
                        cv.pixels(),
                        width,
                        (
                            caret_x as f32,
                            (layout.search.1 + px(8.0)) as f32,
                            (caret_x + px(1.5)) as f32,
                            (layout.search.3 - px(8.0)) as f32,
                        ),
                        px(1.0) as f32,
                        theme.status.accent.tuple(),
                        1.0,
                    );
                }
            }
            let end = (self.first + layout.visible_rows).min(self.rows_len());
            for i in self.first..end {
                let slot = i - self.first;
                let top = layout.list_top + slot as i32 * layout.row_height;
                let row_rect = (pad, top, width - pad, top + layout.row_height);
                draw::list_row(cv.pixels(), width, row_rect, f.look, f.scale, i == self.highlight);
                match self.mode {
                    Mode::Stops => {
                        if let Some(row) = self.filtered.get(i).and_then(|&ix| self.stops.get(ix)) {
                            draw_text(
                                body_font,
                                (pad + px(10.0), top + px(3.0), width - pad - px(10.0), top + px(22.0)),
                                theme.color.fg,
                                &row.name,
                                DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX,
                            );
                            draw_text(
                                caption_font,
                                (pad + px(10.0), top + px(21.0), width - pad - px(10.0), top + px(38.0)),
                                theme.color.dim,
                                &row.code,
                                DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX,
                            );
                        }
                    }
                    Mode::Lines => {
                        if let Some(row) = self.lines.get(i) {
                            let badge = crate::line_badge_color(&row.agency, &row.line, &row.color, &row.text_color);
                            let badge_rect = (pad + px(8.0), top + px(6.0), pad + px(52.0), top + layout.row_height - px(6.0));
                            draw::fill_round(
                                cv.pixels(),
                                width,
                                (badge_rect.0 as f32, badge_rect.1 as f32, badge_rect.2 as f32, badge_rect.3 as f32),
                                (f.look.control_radius * f.scale).round(),
                                badge.0.tuple(),
                                1.0,
                            );
                            draw_text(
                                caption_font,
                                badge_rect,
                                badge.1,
                                &row.line,
                                DT_SINGLELINE | DT_CENTER | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX,
                            );
                            // where it goes on top (what tells the directions apart), the next stop below
                            let next = if row.next.is_empty() {
                                "trip ends here".to_string()
                            } else {
                                format!("next stop {}", row.next)
                            };
                            let towards = if row.headsign.is_empty() { next.clone() } else { format!("to {}", row.headsign) };
                            draw_text(
                                body_font,
                                (pad + px(60.0), top + px(3.0), width - pad - px(8.0), top + px(22.0)),
                                theme.color.fg,
                                &towards,
                                DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX,
                            );
                            if !row.headsign.is_empty() {
                                draw_text(
                                    caption_font,
                                    (pad + px(60.0), top + px(21.0), width - pad - px(8.0), top + px(38.0)),
                                    theme.color.dim,
                                    &next,
                                    DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX,
                                );
                            }
                        }
                    }
                }
            }
            draw_text(
                caption_font,
                layout.status,
                theme.color.dim,
                &self.status,
                DT_WORDBREAK | DT_END_ELLIPSIS | DT_NOPREFIX,
            );
            draw::button(
                cv.pixels(),
                width,
                layout.cancel,
                f.look,
                f.scale,
                false,
                self.hovered == Some(Target::Cancel),
            );
            draw::button(
                cv.pixels(),
                width,
                layout.select,
                f.look,
                f.scale,
                true,
                self.hovered == Some(Target::Select),
            );
            draw_text(
                caption_font,
                layout.cancel,
                theme.color.fg,
                "Cancel",
                DT_SINGLELINE | DT_CENTER | DT_VCENTER | DT_NOPREFIX,
            );
            draw_text(
                caption_font,
                layout.select,
                theme.status.on_accent,
                "Select",
                DT_SINGLELINE | DT_CENTER | DT_VCENTER | DT_NOPREFIX,
            );
            // a font still selected into the DC can't be deleted: put the DC's own back first
            SelectObject(dc, old_font);
            DeleteObject(title_font);
            DeleteObject(body_font);
            DeleteObject(caption_font);
            DeleteObject(icon_font);
        }
    }
}

/// Returns text that fits around the caret and its measured horizontal offset.
fn visible_text(dc: HDC, font: HFONT, text: &str, caret: usize, width: i32) -> (String, i32) {
    let chars: Vec<char> = text.chars().collect();
    let caret = caret.min(chars.len());
    let mut start = 0;
    while start < caret && unsafe { measure(dc, font, &chars[start..caret].iter().collect::<String>()) } > width.saturating_sub(8) {
        start += 1;
    }
    let mut end = caret;
    while end < chars.len() && unsafe { measure(dc, font, &chars[start..=end].iter().collect::<String>()) } < width.saturating_sub(8) {
        end += 1;
    }
    let shown = chars[start..end].iter().collect::<String>();
    let caret_x = unsafe { measure(dc, font, &chars[start..caret].iter().collect::<String>()) };
    (shown, caret_x)
}

/// Measures a string in the supplied GDI font.
unsafe fn measure(dc: HDC, font: HFONT, text: &str) -> i32 {
    SelectObject(dc, font);
    let wide = taskbar_dock::wide(text);
    let mut size = SIZE { cx: 0, cy: 0 };
    GetTextExtentPoint32W(dc, wide.as_ptr(), text.encode_utf16().count() as i32, &mut size);
    size.cx
}

/// Whether the caret shows now: Windows' blink time is how long it stays on (and then off), so
/// it alternates every interval; INFINITE is a steady caret.
fn caret_visible(epoch: Instant) -> bool {
    let blink = unsafe { GetCaretBlinkTime() };
    if blink == INFINITE {
        return true;
    }
    (epoch.elapsed().as_millis() as u64 / u64::from(blink.max(1))) % 2 == 0
}

/// Posts a worker result to the Block UI thread and frees it if posting fails.
fn post(main: isize, generation: usize, data: ResultData) {
    let posted = Box::into_raw(Box::new(Posted { generation, data }));
    if unsafe { PostMessageW(main, WM_PICKER, generation, posted as isize) } == 0 {
        unsafe {
            drop(Box::from_raw(posted));
        }
    }
}

/// Builds the saved config for a selected line direction.
pub(crate) fn chosen_config(stop_name: &str, stop_code: &str, line: &data::LineOpt) -> data::Config {
    let short = stop_name.split_once(", ").map_or(stop_name, |x| x.1);
    let towards = if line.headsign.is_empty() { &line.next } else { &line.headsign };
    data::Config {
        agency: line.agency.clone(),
        line: line.line.clone(),
        stop_code: stop_code.to_string(),
        label: format!("{short} → {towards}"),
        dest: String::new(),
        badge_color: String::new(),
        badge_text: String::new(),
    }
}

/// Returns every stop matching the picker's folded word and exact-code search.
pub(crate) fn matching_stops(stops: &[data::StopHit], query: &str) -> Vec<usize> {
    let query = query.trim();
    let terms: Vec<String> = data::fold(query).split_whitespace().map(str::to_string).collect();
    stops.iter().enumerate()
        .filter(|(_, stop)| stop.code == query || (!terms.is_empty() && terms.iter().all(|term| stop.folded.contains(term))))
        .map(|(index, _)| index)
        .collect()
}

/// Checks whether a result still belongs to the current picker job generation.
fn result_is_current(current: usize, result: usize) -> bool {
    current == result
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a stop row suitable for state-only tests.
    fn stop(code: &str, name: &str) -> data::StopHit {
        data::StopHit {
            code: code.into(),
            name: name.into(),
            folded: data::fold(name),
        }
    }

    /// Verifies shared stop matching handles folded words, exact codes and empty queries.
    #[test]
    fn matching_stops_uses_folded_words_and_stop_codes() {
        let stops = vec![stop("123", "Utrecht, Rhonedreef"), stop("456", "Utrecht Central")];
        assert_eq!(matching_stops(&stops, "rhone utrecht"), vec![0]);
        assert_eq!(matching_stops(&stops, "123"), vec![0]);
        assert!(matching_stops(&stops, " ").is_empty());
    }

    /// Verifies the shared config builder retains the picker's display label.
    #[test]
    fn chosen_config_builds_picker_label_and_clears_overrides() {
        let line = data::LineOpt { agency: "UOV".into(), line: "3".into(), headsign: "Utrecht CS".into(), next: "Central".into(), trips: 1, color: "".into(), text_color: "".into() };
        let config = chosen_config("Utrecht, Rhonedreef", "59150850", &line);
        assert_eq!(config.label, "Rhonedreef → Utrecht CS");
        assert_eq!(config.dest, "");
        assert_eq!(config.badge_color, "");
        assert_eq!(config.badge_text, "");
    }

    /// Verifies stop filtering still caps matches at the documented limit.
    #[test]
    fn filters_by_folded_terms_and_caps_rows_at_three_hundred() {
        let mut picker = Picker::default();
        picker.edit.insert("platform");
        picker.stops = (0..350).map(|i| stop(&i.to_string(), &format!("Utrecht platform {i}"))).collect();
        picker.refilter();
        assert_eq!(picker.filtered.len(), MAX_SHOWN);
        assert!(picker.status.starts_with("350 platforms; showing the first 300"));
    }

    /// Verifies a result from a cancelled line lookup cannot replace stop search state.
    #[test]
    fn back_during_line_search_drops_late_result() {
        let mut picker = Picker::default();
        picker.mode = Mode::Lines;
        picker.loading = true;
        picker.loading_lines = true;
        picker.generation = 7;
        let old_generation = picker.generation;
        assert!(picker.back_to_stops());
        assert!(!result_is_current(picker.generation, old_generation));
    }

    /// Verifies resetting after Select advances the generation and rejects earlier results.
    #[test]
    fn result_from_before_select_is_dropped_after_reset() {
        let mut picker = Picker::default();
        picker.generation = 11;
        picker.reset_after_select();
        assert_eq!(picker.generation, 12);
        assert!(!result_is_current(picker.generation, 11));
    }

    /// Verifies eviction releases only stop-list allocations and retains the active choice state.
    #[test]
    fn eviction_keeps_lines_chosen_search_and_step_but_frees_stops() {
        let mut picker = Picker::default();
        picker.mode = Mode::Lines;
        picker.edit.insert("central");
        picker.chosen = Some(("123".into(), "Central".into()));
        picker.lines.push(data::LineOpt {
            agency: "UOV".into(),
            line: "3".into(),
            next: "Utrecht CS".into(),
            headsign: "".into(),
            trips: 1,
            color: "".into(),
            text_color: "".into(),
        });
        picker.stops.push(stop("123", "Central"));
        picker.filtered.push(0);
        picker.evict_stops();
        assert!(picker.stops.is_empty());
        assert!(picker.filtered.is_empty());
        assert_eq!(picker.chosen.as_ref().map(|x| x.0.as_str()), Some("123"));
        assert_eq!(picker.edit.text(), "central");
        assert_eq!(picker.mode, Mode::Lines);
        assert_eq!(picker.lines.len(), 1);
        assert_eq!(picker.lines[0].line, "3");
    }

    /// Verifies reopening with line rows already present does not need a worker job.
    #[test]
    fn reopening_line_step_with_lines_starts_no_job() {
        let mut picker = Picker {
            mode: Mode::Lines,
            ..Picker::default()
        };
        picker.lines.push(data::LineOpt {
            agency: String::new(),
            line: "3".into(),
            next: "Utrecht CS".into(),
            headsign: String::new(),
            trips: 1,
            color: String::new(),
            text_color: String::new(),
        });
        assert_eq!(picker.mode, Mode::Lines);
        assert_eq!(picker.lines.len(), 1);
        assert!(!picker.loading);
        assert!(!picker.loading_lines);
        assert_eq!(picker.open_job(), None);
    }

    /// Verifies all interactive control rectangles are disjoint and hit their own bounds.
    #[test]
    fn layout_hit_targets_are_disjoint_and_button_hits_match_their_rects() {
        let layout = layout(360, 640, 1.0);
        let rects = [layout.search, layout.status, layout.cancel, layout.select];
        for (i, a) in rects.iter().enumerate() {
            for b in rects.iter().skip(i + 1) {
                assert!(a.2 <= b.0 || b.2 <= a.0 || a.3 <= b.1 || b.3 <= a.1, "overlap: {a:?} {b:?}");
            }
        }
        let mut picker = Picker::default();
        picker.panel_width = 360;
        picker.panel_height = 640;
        assert_eq!(
            picker.target_at(layout, layout.cancel.0 + 1, layout.cancel.1 + 1),
            Some(Target::Cancel)
        );
        assert_eq!(
            picker.target_at(layout, layout.select.0 + 1, layout.select.1 + 1),
            Some(Target::Select)
        );
        assert_ne!(picker.target_at(layout, layout.cancel.2, layout.cancel.1 + 1), Some(Target::Cancel));
        assert_eq!(picker.target_at(layout, 100, layout.cancel.1 + 1), None);
        picker.mode = Mode::Lines;
        assert_eq!(picker.target_at(layout, layout.back.0 + 1, layout.back.1 + 1), Some(Target::Back));
        assert_eq!(picker.target_at(layout, layout.back.2, layout.back.1 + 1), None);
        picker.mode = Mode::Stops;
        picker.filtered = (0..layout.visible_rows).collect();
        for row in 0..layout.visible_rows {
            let top = layout.list_top + row as i32 * layout.row_height;
            assert_eq!(picker.target_at(layout, 30, top + 1), Some(Target::Row(row)));
            assert!(top + layout.row_height <= layout.status.1);
            if row + 1 < layout.visible_rows {
                assert!(top + layout.row_height <= layout.list_top + (row as i32 + 1) * layout.row_height);
            }
        }
    }

    /// Codex review: scrolled down the stop list, a stop chosen (its line search running, no rows),
    /// then the wheel turned: it panicked in `clamp`. Now nothing scrolls and the stop highlight stays.
    #[test]
    fn wheel_during_a_line_search_does_nothing() {
        let mut picker = Picker::default();
        picker.stops = (0..40).map(|i| stop(&i.to_string(), &format!("Stop {i}"))).collect();
        picker.edit.insert("stop");
        picker.refilter();
        picker.move_highlight(30);
        assert!(picker.first > 0);
        picker.loading_lines = true;
        picker.scroll(120);
        picker.scroll(-120);
        assert_eq!(picker.highlight, 30);
        // with rows again, the wheel keeps the highlight on screen
        picker.loading_lines = false;
        for _ in 0..5 {
            picker.scroll(120);
        }
        let visible = picker.layout().visible_rows;
        assert!(picker.highlight >= picker.first && picker.highlight < picker.first + visible);
    }

    /// Verifies a short panel exposes fewer list rows than the normal ten-row panel.
    #[test]
    fn visible_rows_shrink_with_a_short_panel() {
        assert_eq!(layout(360, 640, 1.0).visible_rows, 10);
        assert!(layout(360, 320, 1.0).visible_rows < 10);
    }

    /// Verifies worker generations reject stale results after a later job starts.
    #[test]
    fn stale_result_generations_are_not_current() {
        assert!(result_is_current(8, 8));
        assert!(!result_is_current(9, 8));
    }
}
