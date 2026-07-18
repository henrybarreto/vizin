//! `:`-command interpreter and the panel/hex overlay openers it drives.

use crate::app::{App, Focus, MainView};
use crate::views::hex::HexView;
use crate::views::panels::{PanelKind, PanelView};

impl App {
    fn open_panel(&mut self, kind: PanelKind) {
        let r = PanelView::load(kind, &mut self.backend);
        if let Some(p) = self.report(r) {
            self.panel = Some(p);
            self.enter_overlay(MainView::Panel);
        }
    }

    fn open_hex(&mut self) {
        let r = HexView::load(&mut self.backend, self.seek);
        if let Some(h) = self.report(r) {
            self.hex = Some(h);
            self.enter_overlay(MainView::Hex);
        }
    }

    /// Jump to `addr` and focus the main pane — the shared tail of the `:s`
    /// family and the bare-address/symbol fallback below.
    fn goto_resolved(&mut self, addr: u64) {
        self.goto(addr, true);
        self.focus = Focus::Main;
    }

    pub fn run_command(&mut self, cmd: &str) {
        if cmd.is_empty() {
            return;
        }
        let (name, arg) = match cmd.split_once(char::is_whitespace) {
            Some((n, a)) => (n, a.trim()),
            None => (cmd, ""),
        };
        match name {
            "q" => {
                if self.dirty {
                    self.error("unsaved changes (:w to save, :q! to discard)");
                } else {
                    self.quit = true;
                }
            }
            "q!" => self.quit = true,
            "w" | "wq" => {
                let path = if arg.is_empty() {
                    self.project_path.clone()
                } else {
                    Some(arg.to_string())
                };
                let Some(path) = path else {
                    self.error("no project file (use :w <file.rzdb>)");
                    return;
                };
                let r = self.backend.save_project(&path);
                if self.report(r).is_some() {
                    self.project_path = Some(path.clone());
                    self.dirty = false;
                    self.info(format!("project saved: {path}"));
                    if name == "wq" {
                        self.quit = true;
                    }
                }
            }
            "s" | "seek" | "goto" => {
                if arg.is_empty() {
                    self.error("usage: :s <addr|symbol>");
                } else {
                    let r = self.backend.resolve(arg);
                    if let Some(a) = self.report(r) {
                        self.goto_resolved(a);
                    }
                }
            }
            "fn" | "functions" => {
                self.focus = Focus::Sidebar;
            }
            "str" | "strings" => self.open_panel(PanelKind::Strings),
            "imp" | "imports" => self.open_panel(PanelKind::Imports),
            "exp" | "exports" => self.open_panel(PanelKind::Exports),
            "seg" | "segments" => self.open_panel(PanelKind::Segments),
            "hex" => self.open_hex(),
            "oo+" => {
                let r = self.backend.reopen_writable();
                if self.report(r).is_some() {
                    self.info("reopened in write mode — patches now hit the file");
                }
            }
            _ => {
                // bare address / symbol, like :0x401234 or :main
                match self.backend.resolve(cmd) {
                    Ok(a) => self.goto_resolved(a),
                    Err(_) => self.error(format!("unknown command: {cmd}")),
                }
            }
        }
    }
}
