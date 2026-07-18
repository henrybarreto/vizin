//! Command-line/search input editing: the `:`/`/` input box, its Tab
//! completion popup, and submitting the finished line (command dispatch,
//! search, rename, comment).

use crate::app::{App, Focus, InputKind, InputState, RenameTarget};
use crate::views::completion::CompletionPopup;
use crate::vim::Action;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

impl App {
    /// Move the completion-popup selection and mirror it into the command buffer.
    fn cycle_completion(&mut self, dir: Action) {
        let Some(popup) = self.completion_popup.as_mut() else {
            return;
        };
        popup.scroller.handle(dir, popup.filtered.len());
        let Some(sel) = popup.selected().map(str::to_string) else {
            return;
        };
        if let Some(input) = self.input.as_mut() {
            input.buffer = sel;
        }
    }

    pub fn on_input_key(&mut self, key: KeyEvent) {
        // Handle completion popup navigation first
        if let Some(popup) = self.completion_popup.as_mut() {
            if popup.is_empty() {
                self.completion_popup = None;
            } else {
                match key.code {
                    KeyCode::Up | KeyCode::Char('k')
                        if key.modifiers.contains(KeyModifiers::CONTROL) =>
                    {
                        self.cycle_completion(Action::Up(1));
                        return;
                    }
                    KeyCode::Down | KeyCode::Char('j')
                        if key.modifiers.contains(KeyModifiers::CONTROL) =>
                    {
                        self.cycle_completion(Action::Down(1));
                        return;
                    }
                    KeyCode::Tab => {
                        self.cycle_completion(Action::Down(1));
                        return;
                    }
                    KeyCode::BackTab => {
                        self.cycle_completion(Action::Up(1));
                        return;
                    }
                    KeyCode::Esc => {
                        self.completion_popup = None;
                        return;
                    }
                    KeyCode::Enter => {
                        self.completion_popup = None;
                        let Some(input) = self.input.take() else {
                            return;
                        };
                        self.submit_input(input);
                        return;
                    }
                    _ => {}
                }
            }
        }

        let Some(input) = self.input.as_mut() else {
            return;
        };
        match key.code {
            KeyCode::Esc => {
                if input.kind == InputKind::Search && self.focus == Focus::Sidebar {
                    self.funcs.set_filter("");
                }
                self.completion_popup = None;
                self.input = None;
            }
            KeyCode::Backspace => {
                input.buffer.pop();
                self.completion_popup = None;
                if input.kind == InputKind::Search && self.focus == Focus::Sidebar {
                    let f = input.buffer.clone();
                    self.funcs.set_filter(&f);
                }
            }
            KeyCode::Enter => {
                if let Some(input) = self.input.take() {
                    self.submit_input(input);
                }
            }
            KeyCode::Tab if input.kind == InputKind::Command => {
                self.tab_complete_command();
            }
            KeyCode::Char(c)
                if !key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                input.buffer.push(c);
                self.completion_popup = None;
                if input.kind == InputKind::Search && self.focus == Focus::Sidebar {
                    let f = input.buffer.clone();
                    self.funcs.set_filter(&f);
                }
            }
            _ => {}
        }
    }

    fn tab_complete_command(&mut self) {
        let Some(input) = self.input.as_ref() else {
            return;
        };
        // Like nvim's cmdline completion: only the command name (the first
        // token) is completed. Once a space follows it, the user is typing
        // arguments we don't have completions for — leave the line alone
        // rather than reporting bogus "no matches" against the whole line.
        if input.buffer.contains(' ') {
            return;
        }
        let prefix = input.buffer.trim().to_string();
        let matches: Vec<&str> = crate::views::completion::COMMANDS
            .iter()
            .copied()
            .filter(|c| c.starts_with(&prefix))
            .collect();
        if matches.is_empty() {
            self.info(format!("no matches for: {prefix}"));
        } else if let [only] = matches.as_slice() {
            if let Some(input) = self.input.as_mut() {
                input.buffer = (*only).to_string();
            }
            self.completion_popup = None;
        } else {
            let mut popup = CompletionPopup::new(crate::views::completion::COMMANDS);
            popup.filter(&prefix);
            if let Some(sel) = popup.selected() {
                let sel = sel.to_string();
                if let Some(input) = self.input.as_mut() {
                    input.buffer = sel;
                }
            }
            self.completion_popup = Some(popup);
        }
    }

    fn submit_input(&mut self, input: InputState) {
        match input.kind {
            InputKind::Command => self.run_command(input.buffer.trim()),
            InputKind::Search => {
                if self.focus == Focus::Sidebar {
                    // filter already applied live
                    return;
                }
                self.search_pattern = input.buffer;
                self.repeat_search(true);
            }
            InputKind::Rename(target) => {
                let new = input.buffer.trim().to_string();
                if new.is_empty() {
                    self.error("rename cancelled: empty name");
                    return;
                }
                // Ghidra-only decompiler temporaries (pcVar8, iVar4…) have no
                // backing rizin variable, so a real `afvn` rename can't apply —
                // fall back to a vizin-only display alias instead of erroring.
                if let RenameTarget::Var { fcn, old } = &target {
                    let r = self.backend.is_real_variable(*fcn, old);
                    let Some(is_real) = self.report(r) else {
                        return;
                    };
                    if !is_real {
                        self.local_aliases
                            .entry(*fcn)
                            .or_default()
                            .insert(old.clone(), new.clone());
                        self.save_aliases();
                        self.info(format!(
                            "'{old}' is Ghidra-only — aliased to '{new}' (display hint, not a real rizin rename)"
                        ));
                        return;
                    }
                }
                let res = match &target {
                    RenameTarget::Function(addr) => {
                        self.backend.rename_function(*addr, &new)
                    }
                    RenameTarget::Flag(addr) => self.backend.rename_flag(*addr, &new),
                    RenameTarget::Var { fcn, old } => {
                        self.backend.rename_variable(*fcn, old, &new)
                    }
                };
                if let Some(cmd) = self.report(res) {
                    self.forward_edit(cmd);
                    self.dirty = true;
                    self.refresh_after_edit();
                    self.info(format!("renamed to {new}"));
                }
            }
            InputKind::Comment(addr) => {
                let text = input.buffer.trim().to_string();
                let r = self.backend.set_comment(addr, &text);
                if let Some(cmd) = self.report(r) {
                    self.forward_edit(cmd);
                    self.dirty = true;
                    let r = self.listing.reload(&mut self.backend);
                    self.report(r);
                    // comment appears in the decompiler too — refresh it
                    self.invalidate_decomp(false);
                    self.info(if text.is_empty() {
                        "comment removed".to_string()
                    } else {
                        "comment set".to_string()
                    });
                }
            }
        }
    }
}
