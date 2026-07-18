//! Application state and event dispatch: focus, modes, jumplist, editing.

use crate::backend::{Backend, Instr};
use crate::decompiler::{DecompCache, DecompEvent, Decompiler};
use crate::ts::Symbol;
use crate::views::completion::CompletionPopup;
use crate::views::decomp::DecompView;
use crate::views::functions::FunctionsView;
use crate::views::help::HelpPopup;
use crate::views::hex::HexView;
use crate::views::listing::ListingView;
use crate::views::panels::PanelView;
use crate::views::xrefs::{XrefRow, XrefsPopup};
use crate::vim::{Action, NormalParser};
use anyhow::Result;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// How long the cursor must rest on a new function before we spend a decompile.
const DECOMP_DEBOUNCE: Duration = Duration::from_millis(120);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Sidebar,
    Main,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MainView {
    Listing,
    Decomp,
    Hex,
    Panel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenameTarget {
    Function(u64),
    Flag(u64),
    Var { fcn: u64, old: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputKind {
    Command,
    Search,
    Rename(RenameTarget),
    Comment(u64),
}

/// LSP-hover-style floating box shown by `K`, anchored at the cursor's row.
pub struct ValuePopup {
    pub lines: Vec<String>,
    /// visible row within the pane's content area (0 = first line under the border)
    pub row: usize,
}

pub struct InputState {
    pub kind: InputKind,
    pub prompt: String,
    pub buffer: String,
}

/// Background-decompiler orchestration state: the worker handle and its
/// result cache, which function the pane should show, what's currently
/// shown/in-flight, and cursor-sync bookkeeping.
struct DecompState {
    decompiler: Option<Decompiler>,
    cache: DecompCache,
    /// function we want the decomp pane to show (None = no function at cursor)
    desired_fn: Option<u64>,
    /// when `desired_fn` last changed (debounce timer)
    desired_since: Instant,
    /// function currently populated in the decomp view
    shown: Option<u64>,
    /// function whose decompile request is in flight
    inflight: Option<u64>,
    /// seek address last synced into the decompiler cursor position
    synced_seek: u64,
}

impl DecompState {
    fn new(decompiler: Option<Decompiler>) -> Self {
        Self {
            decompiler,
            cache: DecompCache::new(8),
            desired_fn: None,
            desired_since: Instant::now(),
            shown: None,
            inflight: None,
            synced_seek: 0,
        }
    }
}

// Each bool below is an independent, orthogonal flag (quit vs. dirty vs.
// message severity vs. layout mode) rather than combinable state that would
// read better as an enum — see task #6 if that changes.
#[allow(clippy::struct_excessive_bools)]
pub struct App {
    pub backend: Backend,
    pub funcs: FunctionsView,
    pub listing: ListingView,
    pub decomp: DecompView,
    pub hex: Option<HexView>,
    pub panel: Option<PanelView>,
    pub popup: Option<XrefsPopup>,
    pub completion_popup: Option<CompletionPopup>,
    pub help_popup: Option<HelpPopup>,
    pub focus: Focus,
    pub main_view: MainView,
    prev_view: MainView,
    pub seek: u64,
    back_stack: Vec<u64>,
    fwd_stack: Vec<u64>,
    pub input: Option<InputState>,
    pub message: String,
    pub message_is_error: bool,
    pub dirty: bool,
    pub quit: bool,
    pub arch: String,
    parser: NormalParser,
    pub search_pattern: String,
    pub project_path: Option<String>,
    pub dual_pane: bool,
    // ----- background decompiler -----
    decomp_state: DecompState,
    /// spinner animation frame
    pub spinner: usize,
    /// Display-only names for Ghidra-only decompiler temporaries (pcVar8, iVar4…)
    /// that have no backing rizin variable and so can't be renamed for real.
    /// Keyed by function entry address, then the Ghidra-invented name.
    pub local_aliases: HashMap<u64, HashMap<String, String>>,
    aliases_path: PathBuf,
    /// floating hover box shown by `K`; cleared on the next keypress
    pub value_popup: Option<ValuePopup>,
}

impl App {
    pub fn new(mut backend: Backend, project: Option<String>) -> Self {
        let info = backend.bin_info().unwrap_or_default();
        let mut funcs = FunctionsView::default();
        funcs.set_functions(backend.functions().unwrap_or_default());

        let start = ["main", "sym.main", "entry0"]
            .iter()
            .find_map(|s| backend.resolve(s).ok())
            .or_else(|| funcs.all.first().map(|f| f.offset))
            .unwrap_or(0);

        // Spawn the background decompiler (its own rizin instance) so pdgj never
        // blocks the UI. It analyzes independently and signals Ready when done.
        let decompiler = if backend.has_ghidra {
            Some(Decompiler::spawn(backend.file.clone(), project.clone()))
        } else {
            None
        };

        let aliases_path = PathBuf::from(format!("{}.vizin-aliases.json", backend.file));
        let local_aliases = std::fs::read_to_string(&aliases_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();

        let mut app = Self {
            backend,
            funcs,
            listing: ListingView::default(),
            decomp: DecompView::default(),
            hex: None,
            panel: None,
            popup: None,
            completion_popup: None,
            help_popup: None,
            focus: Focus::Main,
            main_view: MainView::Listing,
            prev_view: MainView::Listing,
            seek: start,
            back_stack: Vec::new(),
            fwd_stack: Vec::new(),
            input: None,
            message: String::new(),
            message_is_error: false,
            dirty: false,
            quit: false,
            arch: format!("{}/{}", info.arch, info.bits),
            parser: NormalParser::default(),
            search_pattern: String::new(),
            project_path: project,
            dual_pane: false,
            decomp_state: DecompState::new(decompiler),
            spinner: 0,
            local_aliases,
            aliases_path,
            value_popup: None,
        };
        if !app.backend.has_ghidra {
            app.error("rz-ghidra not found: decompiler disabled (install rz-ghidra)");
        }
        app.goto(start, false);
        app
    }

    /// First `"..."` quoted string literal appearing on a decompiled line, if any.
    fn string_literal_on_line(line: &str) -> Option<String> {
        let start = line.find('"')?;
        let rest = line.get(start + 1..)?;
        let end = rest.find('"')?;
        rest.get(..end).map(ToString::to_string)
    }

    /// Parse a word under the cursor as a number: `0x..`/`0b..`/`0o..` literals,
    /// asm/decompiler-style hex with an `h` suffix (`1Ah`, `var_38h`), or plain decimal.
    fn parse_numeric(word: &str) -> Option<u64> {
        let w = word.trim();
        if let Some(h) = w.strip_prefix("0x").or_else(|| w.strip_prefix("0X")) {
            return u64::from_str_radix(h, 16).ok();
        }
        if let Some(b) = w.strip_prefix("0b").or_else(|| w.strip_prefix("0B")) {
            return u64::from_str_radix(b, 2).ok();
        }
        if let Some(o) = w.strip_prefix("0o").or_else(|| w.strip_prefix("0O")) {
            return u64::from_str_radix(o, 8).ok();
        }
        if let Some(h) = w.strip_suffix('h').or_else(|| w.strip_suffix('H')) {
            let h = h
                .trim_start_matches("var_")
                .trim_start_matches("arg_")
                .trim_start_matches("local_");
            if let Ok(v) = u64::from_str_radix(h, 16) {
                return Some(v);
            }
        }
        w.parse::<u64>().ok()
    }

    /// Open the `K` hover popup showing `label`'s value in dec/hex/oct/bin
    /// (plus a char/string decoding when the bytes look printable), anchored
    /// at `row` (the cursor's visible row within the current pane).
    fn open_value_popup(&mut self, label: &str, v: u64, row: usize) {
        self.open_value_popup_str(label, v, row, None);
    }

    /// Same as [`Self::open_value_popup`], but `forced_str` (when given) is
    /// shown as-is instead of guessing a string from `v`'s own bytes — used
    /// when rizin already resolved a real string at a referenced address
    /// (e.g. a `lea reg, str.Foo` operand's target).
    fn open_value_popup_str(&mut self, label: &str, v: u64, row: usize, forced_str: Option<String>) {
        let mut lines = vec![
            label.to_string(),
            format!("dec {v}"),
            format!("hex 0x{v:x}"),
            format!("oct 0o{v:o}"),
            format!("bin 0b{v:b}"),
        ];
        if let Some(s) = forced_str {
            lines.push(format!("str \"{s}\""));
        } else {
            lines.extend(Self::char_repr_lines(v));
        }
        self.value_popup = Some(ValuePopup { lines, row });
    }

    /// Append a `; <comment>` line to the currently-open value popup, if any.
    fn append_comment_line(&mut self, comment: Option<String>) {
        let Some(c) = comment.filter(|c| !c.is_empty()) else {
            return;
        };
        if let Some(vp) = self.value_popup.as_mut() {
            vp.lines.push(format!("; {c}"));
        }
    }

    /// `chr` for a single printable byte, `str` for a printable little-endian
    /// byte run (as it'd be laid out in memory) — e.g. a packed string constant.
    fn char_repr_lines(v: u64) -> Vec<String> {
        let is_printable = |b: u8| (0x20..=0x7e).contains(&b);
        let mut lines = Vec::new();
        if let Ok(b) = u8::try_from(v) {
            if is_printable(b) {
                lines.push(format!("chr '{}'", b as char));
            }
        }
        let mut bytes: Vec<u8> = v.to_le_bytes().into_iter().collect();
        while bytes.last() == Some(&0) {
            bytes.pop();
        }
        if bytes.len() > 1 && bytes.iter().all(|&b| is_printable(b)) {
            let s: String = bytes.iter().map(|&b| b as char).collect();
            lines.push(format!("str \"{s}\""));
        }
        lines
    }

    pub fn info(&mut self, msg: impl Into<String>) {
        self.message = msg.into();
        self.message_is_error = false;
    }

    pub fn error(&mut self, msg: impl Into<String>) {
        self.message = msg.into();
        self.message_is_error = true;
    }

    pub fn report<T>(&mut self, r: Result<T>) -> Option<T> {
        match r {
            Ok(v) => Some(v),
            Err(e) => {
                self.error(format!("{e:#}"));
                None
            }
        }
    }

    pub fn current_fn_name(&self) -> String {
        // rizin's `size` can span a wide range for functions with scattered chunks,
        // so an [offset, offset+size) test yields false overlaps. Attribute the
        // address to the nearest preceding function start instead.
        self.funcs
            .all
            .iter()
            .filter(|f| f.offset <= self.seek)
            .max_by_key(|f| f.offset)
            .filter(|f| self.seek < f.offset + f.size.max(1))
            .map_or_else(|| "?".to_string(), |f| f.name.clone())
    }

    // ---------- navigation ----------

    pub fn goto(&mut self, addr: u64, push: bool) {
        if push && addr != self.seek {
            self.back_stack.push(self.seek);
            self.fwd_stack.clear();
        }
        self.sync_seek_to_listing(addr);
        if self.main_view == MainView::Hex {
            if let Some(mut hex) = self.hex.take() {
                let r = hex.seek(&mut self.backend, addr);
                self.report(r);
                self.hex = Some(hex);
            }
        }
    }

    /// Point `seek` at `addr`, and load/scroll the listing pane + sidebar
    /// selection to match. Shared by `goto` and the decomp-pane cursor sync.
    fn sync_seek_to_listing(&mut self, addr: u64) {
        self.seek = addr;
        if self.listing.contains(addr) {
            self.listing.cursor_to_addr(addr);
        } else {
            let r = self.listing.load(&mut self.backend, addr);
            self.report(r);
        }
        self.funcs.select_addr(addr);
    }

    fn jump_back(&mut self) {
        if let Some(a) = self.back_stack.pop() {
            self.fwd_stack.push(self.seek);
            self.goto(a, false);
        } else {
            self.info("jumplist: at oldest entry");
        }
    }

    fn jump_forward(&mut self) {
        if let Some(a) = self.fwd_stack.pop() {
            self.back_stack.push(self.seek);
            self.goto(a, false);
        } else {
            self.info("jumplist: at newest entry");
        }
    }

    /// Decide what the decompiler pane should show and, if needed, request a
    /// background decompile. Never blocks — pdgj runs on the worker thread.
    /// Called before each render.
    pub fn prepare(&mut self, width: u16) {
        self.dual_pane = width >= 160
            && matches!(self.main_view, MainView::Listing | MainView::Decomp);
        let decomp_visible = self.main_view == MainView::Decomp || self.dual_pane;
        if !decomp_visible || self.decomp_state.decompiler.is_none() {
            return;
        }

        // Which function should be shown? (afij is ~0.2ms, cheap per frame.)
        let target = self
            .backend
            .function_at(self.seek)
            .ok()
            .flatten()
            .map(|f| f.offset);

        // (Re)start the debounce timer whenever the target changes.
        if self.decomp_state.desired_fn != target {
            self.decomp_state.desired_fn = target;
            self.decomp_state.desired_since = Instant::now();
        }

        let Some(f) = target else {
            if self.decomp_state.shown.is_some() || !self.decomp.lines.is_empty() {
                self.decomp.clear();
            }
            self.decomp.notice = Some("(no function at cursor)".into());
            self.decomp_state.shown = None;
            return;
        };

        if self.decomp_state.shown == Some(f) {
            self.decomp.notice = None;
            if self.decomp_state.synced_seek != self.seek {
                self.decomp.cursor_to_addr(self.seek);
                self.decomp_state.synced_seek = self.seek;
            }
            return;
        }
        if let Some(res) = self.decomp_state.cache.get(f) {
            self.decomp.set(res, f);
            self.decomp_state.shown = Some(f);
            self.decomp.cursor_to_addr(self.seek);
            self.decomp_state.synced_seek = self.seek;
            return;
        }

        // Not cached: clear stale code, show a status line, and (after the
        // debounce) fire the request.
        if !self.decomp.lines.is_empty() {
            self.decomp.clear();
        }
        self.decomp_state.shown = None;
        let ready = self.decomp_state.decompiler.as_ref().is_some_and(|d| d.ready);
        if !ready {
            self.decomp.notice = Some("decompiler: analyzing binary…".into());
            return;
        }
        self.decomp.notice = Some(format!("{} decompiling {}…", self.spinner_char(), self.fn_label(f)));
        if self.decomp_state.desired_since.elapsed() >= DECOMP_DEBOUNCE && self.decomp_state.inflight != Some(f) {
            if let Some(d) = &self.decomp_state.decompiler {
                d.request(f);
            }
            self.decomp_state.inflight = Some(f);
        }
    }

    /// True while the decomp pane is visible but not yet showing the target.
    pub fn decomp_waiting(&self) -> bool {
        self.decomp_state.decompiler.is_some()
            && (self.main_view == MainView::Decomp || self.dual_pane)
            && self.decomp_state.desired_fn.is_some()
            && self.decomp_state.shown != self.decomp_state.desired_fn
    }

    /// Drain results from the background decompiler. Returns true if anything changed.
    pub fn poll_decomp(&mut self) -> bool {
        let msgs = match &mut self.decomp_state.decompiler {
            Some(d) => d.poll(),
            None => return false,
        };
        let mut changed = false;
        for m in msgs {
            changed = true;
            match m {
                DecompEvent::Ready => {}
                DecompEvent::Done { addr, result } => {
                    self.decomp_state.cache.put(addr, *result);
                    if self.decomp_state.inflight == Some(addr) {
                        self.decomp_state.inflight = None;
                    }
                    if self.decomp_state.desired_fn == Some(addr) {
                        if let Some(res) = self.decomp_state.cache.get(addr) {
                            self.decomp.set(res, addr);
                            self.decomp_state.shown = Some(addr);
                            self.decomp.cursor_to_addr(self.seek);
                            self.decomp_state.synced_seek = self.seek;
                        }
                    }
                }
                DecompEvent::Failed { addr, error } => {
                    if self.decomp_state.inflight == Some(addr) {
                        self.decomp_state.inflight = None;
                    }
                    if self.decomp_state.desired_fn == Some(addr) {
                        self.decomp.clear();
                        self.decomp.notice = Some(format!("decompile failed: {error}"));
                        self.decomp_state.shown = None;
                    }
                }
            }
        }
        changed
    }

    fn spinner_char(&self) -> char {
        const FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
        FRAMES
            .get(self.spinner % FRAMES.len())
            .copied()
            .unwrap_or('⠋')
    }

    fn fn_label(&self, addr: u64) -> String {
        self.funcs
            .all
            .iter()
            .find(|f| f.offset == addr)
            .map_or_else(|| format!("{addr:#x}"), |f| f.name.clone())
    }

    /// Replay an edit command on the background decompiler instance so its
    /// decompiled output shows the same names/comments.
    pub fn forward_edit(&self, cmd: String) {
        if let Some(d) = &self.decomp_state.decompiler {
            d.forward(cmd);
        }
    }

    /// Best-effort persist of `local_aliases` to a sidecar JSON file next to
    /// the binary — these are vizin-only display hints, not part of any rizin
    /// project, so they don't round-trip through `Ps`/`-p`.
    pub fn save_aliases(&self) {
        if let Ok(s) = serde_json::to_string_pretty(&self.local_aliases) {
            let _ = std::fs::write(&self.aliases_path, s);
        }
    }

    pub fn refresh_after_edit(&mut self) {
        let fns = self.backend.functions().unwrap_or_default();
        let filter = self.funcs.filter.clone();
        self.funcs.set_functions(fns);
        self.funcs.set_filter(&filter);
        let r = self.listing.reload(&mut self.backend);
        self.report(r);
        // Names changed — drop cached decompilations (callers may show the new
        // name too) and re-show the current function from scratch.
        self.invalidate_decomp(true);
        self.funcs.select_addr(self.seek);
    }

    /// Drop cached/shown decompilations so the pane re-decompiles from
    /// scratch — used whenever an edit could change the decompiled output.
    ///
    /// `drop_inflight` decides what happens to a decompile request already
    /// queued on the background worker for the current function:
    /// - `true` (renames, byte patches): the edit can change what that
    ///   in-flight request produces, so treat it as stale — clearing
    ///   `inflight` makes `prepare()` queue a fresh request instead of
    ///   waiting on one that may reflect pre-edit state.
    /// - `false` (comments): the decompiled code structure is unaffected,
    ///   so the in-flight request (if any) is still good — leaving it alone
    ///   avoids firing a redundant duplicate request for the same function.
    pub fn invalidate_decomp(&mut self, drop_inflight: bool) {
        self.decomp_state.cache.clear();
        self.decomp_state.shown = None;
        if drop_inflight {
            self.decomp_state.inflight = None;
        }
    }

    // ---------- key handling ----------

    pub fn on_key(&mut self, key: KeyEvent) {
        if key.kind != KeyEventKind::Press {
            return;
        }
        if self.input.is_some() {
            self.on_input_key(key);
            return;
        }
        if let Some(hex) = self.hex.as_mut() {
            if hex.editing {
                self.on_hex_edit_key(key);
                return;
            }
        }
        let Some(action) = self.parser.feed(key) else {
            return;
        };
        self.message.clear();
        self.value_popup = None;
        if self.help_popup.is_some() {
            self.on_help_popup_action(action);
            return;
        }
        if self.popup.is_some() {
            self.on_popup_action(action);
            return;
        }
        if action == Action::Help {
            self.help_popup = Some(HelpPopup::new());
            return;
        }
        match action {
            // Three-panel navigation: Sidebar ←→ Listing ←→ Decompiler
            Action::FocusSidebar => {
                // move left: Decomp → Listing → Sidebar
                if self.main_view == MainView::Decomp {
                    self.main_view = MainView::Listing;
                } else {
                    self.focus = Focus::Sidebar;
                }
            }
            Action::FocusMain => {
                // move right: Sidebar → Listing → Decomp
                if self.focus == Focus::Sidebar {
                    self.focus = Focus::Main;
                    self.main_view = MainView::Listing;
                } else if self.main_view == MainView::Listing {
                    self.main_view = MainView::Decomp;
                }
            }
            Action::CycleFocus => {
                // cycle forward: Sidebar → Listing → Decomp → Sidebar
                match (self.focus, &self.main_view) {
                    (Focus::Sidebar, _) => {
                        self.focus = Focus::Main;
                        self.main_view = MainView::Listing;
                    }
                    (Focus::Main, MainView::Listing) => {
                        self.main_view = MainView::Decomp;
                    }
                    (Focus::Main, _) => {
                        self.focus = Focus::Sidebar;
                    }
                }
            }
            Action::CommandMode => {
                self.input = Some(InputState {
                    kind: InputKind::Command,
                    prompt: ":".into(),
                    buffer: String::new(),
                });
            }
            Action::SearchMode => {
                if self.focus == Focus::Sidebar {
                    self.funcs.set_filter("");
                }
                self.input = Some(InputState {
                    kind: InputKind::Search,
                    prompt: "/".into(),
                    buffer: String::new(),
                });
            }
            Action::JumpBack => self.jump_back(),
            Action::JumpForward => self.jump_forward(),
            _ => match self.focus {
                Focus::Sidebar => self.on_sidebar_action(action),
                Focus::Main => self.on_main_action(action),
            },
        }
    }

    fn on_help_popup_action(&mut self, action: Action) {
        let Some(popup) = self.help_popup.as_mut() else {
            return;
        };
        let len = popup.len();
        if popup.scroller.handle(action, len) {
            return;
        }
        if matches!(action, Action::Close | Action::Help) {
            self.help_popup = None;
        }
    }

    fn on_popup_action(&mut self, action: Action) {
        let Some(popup) = self.popup.as_mut() else {
            return;
        };
        let len = popup.rows.len();
        if popup.scroller.handle(action, len) {
            return;
        }
        match action {
            Action::Follow => {
                if let Some(addr) = popup.selected_addr() {
                    self.popup = None;
                    self.goto(addr, true);
                    self.focus = Focus::Main;
                }
            }
            Action::Close | Action::XrefsTo | Action::XrefsFrom => self.popup = None,
            _ => {}
        }
    }

    fn on_sidebar_action(&mut self, action: Action) {
        let len = self.funcs.len();
        if self.funcs.scroller.handle(action, len) {
            return;
        }
        match action {
            Action::Follow => {
                if let Some(f) = self.funcs.selected() {
                    let addr = f.offset;
                    self.goto(addr, true);
                    self.focus = Focus::Main;
                }
            }
            Action::XrefsTo => {
                if let Some(f) = self.funcs.selected() {
                    let addr = f.offset;
                    self.show_xrefs_to(addr);
                }
            }
            Action::Rename => {
                if let Some(f) = self.funcs.selected() {
                    let (addr, name) = (f.offset, f.name.clone());
                    self.start_rename(RenameTarget::Function(addr), &name);
                }
            }
            Action::Close => {
                if self.funcs.filter.is_empty() {
                    self.info("use :q to quit");
                } else {
                    self.funcs.set_filter("");
                    self.info("filter cleared");
                }
            }
            _ => {}
        }
    }

    fn on_main_action(&mut self, action: Action) {
        match self.main_view {
            MainView::Listing => self.on_listing_action(action),
            MainView::Decomp => self.on_decomp_action(action),
            MainView::Hex => self.on_hex_action(action),
            MainView::Panel => self.on_panel_action(action),
        }
    }

    fn sync_seek_from_listing(&mut self) {
        if let Some(a) = self.listing.addr_at_cursor() {
            self.seek = a;
            self.funcs.select_addr(a);
        }
    }

    fn on_listing_action(&mut self, action: Action) {
        let len = self.listing.rows.len();
        if self.listing.scroller.handle(action, len) {
            self.listing.extend_if_needed(&mut self.backend);
            self.sync_seek_from_listing();
            return;
        }
        match action {
            Action::Follow => self.follow_from_listing(),
            Action::XrefsTo => self.show_xrefs_to(self.seek),
            Action::Rename => self.rename_at_listing_cursor(),
            Action::Comment => self.start_comment(self.seek),
            Action::ShowValue => self.show_value_at_listing_cursor(),
            _ => self.handle_common_action(action),
        }
    }

    fn show_value_at_listing_cursor(&mut self) {
        let row = self.listing.scroller.visible_row();
        let comment = self.listing.instr_at_cursor().and_then(Instr::comment_text);
        // Prefer resolving the instruction's actual data reference (e.g. a
        // `lea reg, str.Foo` operand) to a real string over guessing from bytes.
        if let Some(ptr) = self.listing.instr_at_cursor().and_then(|i| i.ptr) {
            if let Ok(s) = self.backend.string_at(ptr) {
                self.open_value_popup_str(&format!("{ptr:#x}"), ptr, row, Some(s));
                self.append_comment_line(comment);
                return;
            }
        }
        let word = self.listing.word_at_cursor();
        self.show_value_or_word(word, self.seek, row, comment);
    }

    /// Shared tail of `show_value_at_*_cursor`: try the word under the
    /// cursor as a number, else fall back to `addr` itself.
    fn show_value_or_word(&mut self, word: Option<String>, addr: u64, row: usize, comment: Option<String>) {
        if let Some(word) = word {
            if let Some(v) = Self::parse_numeric(&word) {
                self.open_value_popup(&word, v, row);
                self.append_comment_line(comment);
                return;
            }
        }
        self.open_value_popup(&format!("{addr:#x}"), addr, row);
        self.append_comment_line(comment);
    }

    fn on_decomp_action(&mut self, action: Action) {
        match action {
            Action::Left(n) => {
                self.decomp.move_col(-i64::try_from(n).unwrap_or(i64::MAX));
                return;
            }
            Action::Right(n) => {
                self.decomp.move_col(i64::try_from(n).unwrap_or(i64::MAX));
                return;
            }
            Action::WordNext => {
                self.decomp.move_word_next();
                return;
            }
            Action::WordPrev => {
                self.decomp.move_word_prev();
                return;
            }
            Action::WordEnd => {
                self.decomp.move_word_end();
                return;
            }
            Action::LineStart => {
                self.decomp.col = 0;
                return;
            }
            Action::LineEnd => {
                if let Some(line) = self.decomp.lines.get(self.decomp.scroller.cursor) {
                    self.decomp.col = line.len().saturating_sub(1);
                }
                return;
            }
            Action::FindNext(c) => {
                self.decomp.find_char(c);
                return;
            }
            Action::FindPrev(c) => {
                self.decomp.find_char_back(c);
                return;
            }
            Action::ToPrevBrace => {
                self.decomp.goto_prev_brace();
                if let Some(a) = self.decomp.addr_at_cursor() {
                    self.seek = a;
                }
                return;
            }
            Action::ToNextBrace => {
                self.decomp.goto_next_brace();
                if let Some(a) = self.decomp.addr_at_cursor() {
                    self.seek = a;
                }
                return;
            }
            Action::MatchBracket => {
                self.decomp.match_bracket();
                if let Some(a) = self.decomp.addr_at_cursor() {
                    self.seek = a;
                }
                return;
            }
            _ => {}
        }
        let len = self.decomp.lines.len();
        if self.decomp.scroller.handle(action, len) {
            if let Some(a) = self.decomp.addr_at_cursor() {
                self.sync_seek_to_listing(a);
            }
            return;
        }
        match action {
            Action::Follow => self.follow_from_decomp(),
            Action::XrefsTo => {
                let addr = match self.decomp.symbol_at_cursor() {
                    Symbol::Function { addr, .. } | Symbol::Global { addr, .. } => addr,
                    _ => self.seek,
                };
                self.show_xrefs_to(addr);
            }
            Action::Rename => self.rename_at_decomp_cursor(),
            Action::Comment => {
                if let Some(a) = self.decomp.addr_at_cursor() {
                    self.start_comment(a);
                }
            }
            Action::ShowValue => self.show_value_at_decomp_cursor(),
            _ => self.handle_common_action(action),
        }
    }

    /// Actions handled identically by the listing and decomp panes: search
    /// (both text and word-under-cursor), the listing↔decomp toggle, xrefs
    /// from the current seek, and the "use :q to quit" hint on close.
    fn handle_common_action(&mut self, action: Action) {
        match action {
            Action::XrefsFrom => self.show_xrefs_from(self.seek),
            Action::ToggleView => self.toggle_decomp(),
            Action::SearchNext => self.repeat_search(true),
            Action::SearchPrev => self.repeat_search(false),
            Action::SearchWordNext | Action::SearchWordPrev => self.search_word(action),
            Action::Close => self.info("use :q to quit"),
            _ => {}
        }
    }

    fn show_value_at_decomp_cursor(&mut self) {
        let row = self.decomp.scroller.visible_row();
        let addr_here = self.decomp.addr_at_cursor();
        let comment = addr_here.and_then(|a| self.backend.comment_at(a).ok().flatten());
        if let Symbol::Global { addr, .. } = self.decomp.symbol_at_cursor() {
            if let Ok(s) = self.backend.string_at(addr) {
                self.open_value_popup_str(&format!("{addr:#x}"), addr, row, Some(s));
                self.append_comment_line(comment);
                return;
            }
        }
        // Ghidra often assigns a string literal straight into a local
        // (`pcVar8 = "EasyPassword";`) with no data-reference annotation at
        // all — the cursor sitting on `pcVar8` rather than the literal itself
        // still means "show me this line's string". Read it off the raw text.
        if let Some(s) = self
            .decomp
            .lines
            .get(self.decomp.scroller.cursor)
            .and_then(|l| Self::string_literal_on_line(l))
        {
            let a = addr_here.unwrap_or(self.seek);
            self.open_value_popup_str(&format!("{a:#x}"), a, row, Some(s));
            self.append_comment_line(comment);
            return;
        }
        let word = self.decomp.word_at_cursor();
        self.show_value_or_word(word, addr_here.unwrap_or(self.seek), row, comment);
    }

    fn on_hex_action(&mut self, action: Action) {
        let Some(hex) = self.hex.as_mut() else {
            self.exit_overlay();
            return;
        };
        match action {
            Action::Insert => {
                if self.backend.writable {
                    hex.editing = true;
                    self.info("-- INSERT -- type hex digits, Esc to commit");
                } else {
                    self.error("read-only: start with -w or use :oo+ to enable patching");
                }
            }
            Action::Close | Action::ToggleView => {
                let addr = hex.addr_at_cursor();
                self.close_hex(addr, false);
            }
            Action::Follow => {
                let addr = hex.addr_at_cursor();
                self.close_hex(addr, true);
            }
            Action::ShowValue => {
                let addr = hex.addr_at_cursor();
                let v = hex
                    .edits
                    .get(&addr)
                    .copied()
                    .or_else(|| hex.bytes.get(hex.cursor).copied())
                    .unwrap_or(0);
                let row = hex.cursor_row_in_view();
                self.open_value_popup(&format!("{addr:#x}"), u64::from(v), row);
            }
            other => {
                let r = hex.handle(other, &mut self.backend);
                self.seek = hex.addr_at_cursor();
                self.report(r);
            }
        }
    }

    /// Leave the hex view, jumping to `addr` in the restored view (`follow`
    /// pushes a jumplist entry; a plain close does not).
    fn close_hex(&mut self, addr: u64, follow: bool) {
        self.exit_overlay();
        self.hex = None;
        self.goto(addr, follow);
    }

    fn on_hex_edit_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Enter => {
                let Some(hx) = self.hex.as_mut() else { return };
                hx.editing = false;
                let n = hx.commit(&mut self.backend);
                match n {
                    Ok(0) => self.info("no changes"),
                    Ok(n) => self.info(format!("wrote {n} byte(s)")),
                    Err(e) => {
                        hx.discard();
                        self.error(format!("{e:#}"));
                    }
                }
                // patched bytes change disassembly and decompilation
                let r = self.listing.reload(&mut self.backend);
                self.report(r);
                self.invalidate_decomp(true);
                // tell the worker to reload the (now-patched) file from disk
                if let Some(d) = &self.decomp_state.decompiler {
                    d.forward("oo".into());
                }
            }
            KeyCode::Char(c)
                if key.modifiers == KeyModifiers::NONE
                    || key.modifiers == KeyModifiers::SHIFT =>
            {
                // only hex digits are accepted in insert mode; other chars are ignored
                if let Some(hex) = self.hex.as_mut() {
                    let _ = hex.input_nibble(c);
                }
            }
            _ => {}
        }
    }

    fn on_panel_action(&mut self, action: Action) {
        let Some(panel) = self.panel.as_mut() else {
            self.exit_overlay();
            return;
        };
        let len = panel.rows.len();
        if panel.scroller.handle(action, len) {
            return;
        }
        match action {
            Action::Follow => {
                if let Some(addr) = panel.selected_addr() {
                    self.panel = None;
                    self.main_view = MainView::Listing;
                    self.goto(addr, true);
                } else {
                    self.error("no address for this row");
                }
            }
            Action::Close => {
                self.panel = None;
                self.exit_overlay();
            }
            Action::XrefsTo => {
                if let Some(addr) = panel.selected_addr() {
                    self.show_xrefs_to(addr);
                }
            }
            _ => self.handle_common_action(action),
        }
    }

    // ---------- follow / xrefs ----------

    fn follow_from_listing(&mut self) {
        let Some(ins) = self.listing.instr_at_cursor() else {
            return;
        };
        if let Some(j) = ins.jump {
            self.goto(j, true);
            return;
        }
        let addr = ins.offset;
        match self.backend.xrefs_from(addr) {
            Ok(refs) if refs.len() == 1 => {
                if let Some(r) = refs.first() {
                    self.goto(r.to, true);
                }
            }
            Ok(refs) if !refs.is_empty() => self.popup_xrefs_from(&refs),
            _ => self.info("nothing to follow here"),
        }
    }

    fn follow_from_decomp(&mut self) {
        match self.decomp.symbol_at_cursor() {
            Symbol::Function { addr, .. } | Symbol::Global { addr, .. } => {
                self.goto(addr, true);
            }
            _ => {
                if let Some(a) = self.decomp.addr_at_cursor() {
                    // follow whatever the underlying instruction jumps to
                    match self.backend.disasm(a, 1).ok().and_then(|v| v.first().and_then(|i| i.jump)) {
                        Some(j) => self.goto(j, true),
                        None => self.info("nothing to follow here"),
                    }
                }
            }
        }
    }

    fn show_xrefs_to(&mut self, addr: u64) {
        match self.backend.xrefs_to(addr) {
            Ok(refs) if !refs.is_empty() => {
                let rows = refs
                    .iter()
                    .map(|x| XrefRow {
                        addr: x.from,
                        text: format!(
                            "{:>10x}  {:<6} {:<24} {}",
                            x.from,
                            x.kind,
                            x.fcn_name.clone().unwrap_or_default(),
                            x.opcode
                        ),
                    })
                    .collect();
                self.popup = Some(XrefsPopup::new(format!("Xrefs to {addr:#x}"), rows));
            }
            Ok(_) => self.info(format!("no xrefs to {addr:#x}")),
            Err(e) => self.error(format!("{e:#}")),
        }
    }

    fn show_xrefs_from(&mut self, addr: u64) {
        match self.backend.xrefs_from(addr) {
            Ok(refs) if !refs.is_empty() => self.popup_xrefs_from(&refs),
            Ok(_) => self.info(format!("no xrefs from {addr:#x}")),
            Err(e) => self.error(format!("{e:#}")),
        }
    }

    fn popup_xrefs_from(&mut self, refs: &[crate::backend::XrefFrom]) {
        let rows = refs
            .iter()
            .map(|x| XrefRow {
                addr: x.to,
                text: format!(
                    "{:>10x}  {:<6} {}",
                    x.to,
                    x.kind,
                    x.name.clone().unwrap_or_default()
                ),
            })
            .collect();
        self.popup = Some(XrefsPopup::new(format!("Xrefs from {:#x}", self.seek), rows));
    }

    fn toggle_decomp(&mut self) {
        self.main_view = match self.main_view {
            MainView::Listing => {
                if !self.backend.has_ghidra {
                    self.error("decompiler unavailable (rz-ghidra not installed)");
                    return;
                }
                MainView::Decomp
            }
            MainView::Decomp => MainView::Listing,
            other => other,
        };
    }

    // ---------- rename / comment ----------

    fn start_rename(&mut self, target: RenameTarget, old: &str) {
        self.input = Some(InputState {
            kind: InputKind::Rename(target),
            prompt: format!("rename {old} → "),
            buffer: old.to_string(),
        });
    }

    fn rename_at_listing_cursor(&mut self) {
        let addr = self.seek;
        if let Some(f) = self.funcs.all.iter().find(|f| f.offset == addr) {
            let name = f.name.clone();
            self.start_rename(RenameTarget::Function(addr), &name);
            return;
        }
        // try target of the instruction (rename what we point at, like ghidra)
        let target = self
            .listing
            .instr_at_cursor()
            .and_then(|i| i.jump)
            .unwrap_or(addr);
        if let Some(f) = self.funcs.all.iter().find(|f| f.offset == target) {
            let name = f.name.clone();
            self.start_rename(RenameTarget::Function(target), &name);
            return;
        }
        let flag = self
            .listing
            .instr_at_cursor()
            .and_then(|i| i.flags.first().cloned());
        match flag {
            Some(name) => self.start_rename(RenameTarget::Flag(addr), &name),
            None => self.error("no function or flag here to rename"),
        }
    }

    fn rename_at_decomp_cursor(&mut self) {
        match self.decomp.symbol_at_cursor() {
            Symbol::Function { name, addr } => {
                self.start_rename(RenameTarget::Function(addr), &name);
            }
            Symbol::Global { addr, .. } => self.start_rename(RenameTarget::Flag(addr), ""),
            Symbol::Local { name } | Symbol::Param { name } => {
                let fcn = self.decomp.fcn_addr;
                // Prefill with the existing alias (if any) so re-renaming continues
                // from what's currently shown, not the raw Ghidra-invented name.
                let shown = self
                    .local_aliases
                    .get(&fcn)
                    .and_then(|m| m.get(&name))
                    .cloned()
                    .unwrap_or_else(|| name.clone());
                self.start_rename(RenameTarget::Var { fcn, old: name }, &shown);
            }
            Symbol::None => self.error("no symbol under cursor"),
        }
    }

    fn start_comment(&mut self, addr: u64) {
        let existing = self
            .backend
            .raw_cmd(&format!("CC. @ {addr:#x}"))
            .unwrap_or_default()
            .trim()
            .to_string();
        self.input = Some(InputState {
            kind: InputKind::Comment(addr),
            prompt: format!("comment @ {addr:#x}: "),
            buffer: existing,
        });
    }

    pub fn repeat_search(&mut self, forward: bool) {
        if self.search_pattern.is_empty() {
            self.error("no search pattern (use /)");
            return;
        }
        let pat = self.search_pattern.clone();
        let found = match self.main_view {
            MainView::Decomp => self.decomp.search(&pat, forward),
            MainView::Panel => self.panel.as_mut().is_some_and(|p| p.search(&pat, forward)),
            _ => {
                let found = self.listing.search(&pat, forward);
                if found {
                    self.sync_seek_from_listing();
                }
                found
            }
        };
        if !found {
            self.error(format!("pattern not found: {pat}"));
        }
    }

    fn search_word(&mut self, action: Action) {
        let word = match self.main_view {
            MainView::Listing => self.listing.word_at_cursor(),
            MainView::Decomp => self.decomp.word_at_cursor(),
            _ => None,
        };
        let Some(word) = word else {
            self.error("no word under cursor");
            return;
        };
        if word.is_empty() {
            self.error("no word under cursor");
            return;
        }
        self.search_pattern = word;
        self.repeat_search(action == Action::SearchWordNext);
    }

    // ---------- commands ----------

    /// Enter an overlay view (hex/panel) over whatever's currently shown,
    /// remembering it so `exit_overlay` can restore it.
    pub fn enter_overlay(&mut self, view: MainView) {
        if self.main_view != MainView::Panel && self.main_view != MainView::Hex {
            self.prev_view = self.main_view;
        }
        self.main_view = view;
        self.focus = Focus::Main;
    }

    /// Leave the current overlay view, restoring whatever was shown before it.
    const fn exit_overlay(&mut self) {
        self.main_view = self.prev_view;
    }
}
