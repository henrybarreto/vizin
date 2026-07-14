//! Normal-mode key parser: a pure state machine turning key events into Actions.
//! Counts (`12j`) and multi-key sequences (`gg`, `gd`) are handled here.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Up(usize),
    Down(usize),
    Left(usize),
    Right(usize),
    Top,
    Bottom,
    HalfPageDown,
    HalfPageUp,
    PageDown,
    PageUp,
    ScreenTop,    // H
    ScreenMiddle, // M
    ScreenBottom, // L
    ScrollCursorTop,    // zt (scroll so cursor is at top of viewport)
    ScrollCursorMiddle, // zz (scroll so cursor is at middle)
    ScrollCursorBottom, // zb (scroll so cursor is at bottom)
    Follow,       // Enter / gd
    JumpBack,     // ctrl-o
    JumpForward,  // ctrl-i
    XrefsTo,      // x
    XrefsFrom,    // X
    ToggleView,   // Space: listing <-> decompiler
    CycleFocus,   // Tab / Ctrl-w w
    FocusSidebar, // Ctrl-w h
    FocusMain,    // Ctrl-w l
    Rename,       // r
    Comment,      // ;
    Insert,       // i (hex edit)
    SearchNext,   // n
    SearchPrev,   // N
    WordNext,     // w
    WordPrev,     // b
    WordEnd,      // e
    LineStart,    // 0
    LineEnd,      // $
    FindNext(char),  // f{char}
    FindPrev(char),  // F{char}
    ToPrevBrace,  // [{
    ToNextBrace,  // ]}
    MatchBracket, // %
    SearchWordNext,  // *
    SearchWordPrev,  // #
    Close,        // q / Esc
    CommandMode,  // :
    SearchMode,   // /
    ShowValue,    // K: show word-under-cursor as hex/dec/bin/oct
    Help,         // ?: keybinding cheatsheet
}

/// Outcome of [`NormalParser::feed_pending`]: whether a pending multi-key
/// sequence consumed this key, and if so, what it resolved to.
enum PendingResult {
    None,
    Resolved(Option<Action>),
}

// These `pending_*` fields are mutually exclusive single-key lookahead
// states of the parser and would be cleaner as one `Pending` enum; left as
// bools for now since `feed()` below is a well-tested hot path — see task #6.
#[derive(Default)]
#[allow(clippy::struct_excessive_bools)]
pub struct NormalParser {
    count: Option<usize>,
    pending_g: bool,
    pending_z: bool,
    pending_ctrl_w: bool,
    /// true when waiting for the target char after `f`
    pending_f: bool,
    /// true when waiting for the target char after `F`
    pending_upper_f: bool,
    /// true when waiting for the second key after `[`
    pending_open_bracket: bool,
    /// true when waiting for the second key after `]`
    pending_close_bracket: bool,
}

impl NormalParser {
    pub const fn reset(&mut self) {
        self.count = None;
        self.pending_g = false;
        self.pending_z = false;
        self.pending_ctrl_w = false;
        self.pending_f = false;
        self.pending_upper_f = false;
        self.pending_open_bracket = false;
        self.pending_close_bracket = false;
    }

    /// Feed one key event; returns an Action when a complete sequence is recognized.
    pub fn feed(&mut self, key: KeyEvent) -> Option<Action> {
        let n = self.count.unwrap_or(1).max(1);
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            return self.feed_ctrl(key);
        }
        if let PendingResult::Resolved(action) = self.feed_pending(key) {
            return action;
        }
        self.feed_normal(key, n)
    }

    /// Handle a Ctrl-modified key (Ctrl-d/u/f/b/o/i, and the Ctrl-w prefix).
    fn feed_ctrl(&mut self, key: KeyEvent) -> Option<Action> {
        self.reset();
        // Ctrl-w is a prefix for window navigation (like vim)
        if key.code == KeyCode::Char('w') {
            self.pending_ctrl_w = true;
            return None;
        }
        match key.code {
            KeyCode::Char('d') => Some(Action::HalfPageDown),
            KeyCode::Char('u') => Some(Action::HalfPageUp),
            KeyCode::Char('f') => Some(Action::PageDown),
            KeyCode::Char('b') => Some(Action::PageUp),
            KeyCode::Char('o') => Some(Action::JumpBack),
            KeyCode::Char('i') => Some(Action::JumpForward),
            _ => None,
        }
    }

    /// If a multi-key sequence (`g?`, `z?`, `Ctrl-w ?`, `f?`, `F?`, `[?`, `]?`) is
    /// pending, consume this key as its second half.
    const fn feed_pending(&mut self, key: KeyEvent) -> PendingResult {
        if self.pending_g {
            self.pending_g = false;
            self.count = None;
            return PendingResult::Resolved(match key.code {
                KeyCode::Char('g') => Some(Action::Top),
                KeyCode::Char('d') => Some(Action::Follow),
                _ => None,
            });
        }

        if self.pending_z {
            self.pending_z = false;
            self.count = None;
            return PendingResult::Resolved(match key.code {
                KeyCode::Char('z') => Some(Action::ScrollCursorMiddle),
                KeyCode::Char('t') => Some(Action::ScrollCursorTop),
                KeyCode::Char('b') => Some(Action::ScrollCursorBottom),
                _ => None,
            });
        }

        if self.pending_ctrl_w {
            self.pending_ctrl_w = false;
            self.count = None;
            return PendingResult::Resolved(match key.code {
                KeyCode::Char('h') => Some(Action::FocusSidebar),
                KeyCode::Char('l') => Some(Action::FocusMain),
                KeyCode::Char('w') => Some(Action::CycleFocus),
                _ => None,
            });
        }

        if self.pending_f {
            self.pending_f = false;
            self.count = None;
            return PendingResult::Resolved(match key.code {
                KeyCode::Char(c) => Some(Action::FindNext(c)),
                _ => None,
            });
        }

        if self.pending_upper_f {
            self.pending_upper_f = false;
            self.count = None;
            return PendingResult::Resolved(match key.code {
                KeyCode::Char(c) => Some(Action::FindPrev(c)),
                _ => None,
            });
        }

        if self.pending_open_bracket {
            self.pending_open_bracket = false;
            self.count = None;
            return PendingResult::Resolved(match key.code {
                KeyCode::Char('{') => Some(Action::ToPrevBrace),
                _ => None,
            });
        }

        if self.pending_close_bracket {
            self.pending_close_bracket = false;
            self.count = None;
            return PendingResult::Resolved(match key.code {
                KeyCode::Char('}') => Some(Action::ToNextBrace),
                _ => None,
            });
        }

        PendingResult::None
    }

    /// Dispatch a key with no pending sequence and no Ctrl modifier: counts,
    /// sequence-prefix keys (`g`, `z`, `f`, `F`, `[`, `]`), and plain motions.
    fn feed_normal(&mut self, key: KeyEvent, n: usize) -> Option<Action> {
        match key.code {
            KeyCode::Char(c @ '0'..='9') => {
                // A leading 0 is not a count (vim: start of line; unused here).
                let d = c as usize - '0' as usize;
                if c == '0' && self.count.is_none() {
                    self.reset();
                    return Some(Action::LineStart);
                }
                self.count = Some(self.count.unwrap_or(0).saturating_mul(10) + d);
                None
            }
            KeyCode::Char('g') => {
                self.pending_g = true;
                None
            }
            KeyCode::Char('z') => {
                self.pending_z = true;
                None
            }
            KeyCode::Char('f') => {
                self.pending_f = true;
                None
            }
            KeyCode::Char('F') => {
                self.pending_upper_f = true;
                None
            }
            KeyCode::Char('[') => {
                self.pending_open_bracket = true;
                None
            }
            KeyCode::Char(']') => {
                self.pending_close_bracket = true;
                None
            }
            code => {
                self.reset();
                match code {
                    KeyCode::Char('j') | KeyCode::Down => Some(Action::Down(n)),
                    KeyCode::Char('k') | KeyCode::Up => Some(Action::Up(n)),
                    KeyCode::Char('h') | KeyCode::Left => Some(Action::Left(n)),
                    KeyCode::Char('l') | KeyCode::Right => Some(Action::Right(n)),
                    KeyCode::Char('G') => Some(Action::Bottom),
                    KeyCode::Char('H') => Some(Action::ScreenTop),
                    KeyCode::Char('M') => Some(Action::ScreenMiddle),
                    KeyCode::Char('L') => Some(Action::ScreenBottom),
                    KeyCode::PageDown => Some(Action::PageDown),
                    KeyCode::PageUp => Some(Action::PageUp),
                    KeyCode::Enter => Some(Action::Follow),
                    KeyCode::Char('x') => Some(Action::XrefsTo),
                    KeyCode::Char('X') => Some(Action::XrefsFrom),


                    KeyCode::Char('w') => Some(Action::WordNext),
                    KeyCode::Char('b') => Some(Action::WordPrev),
                    KeyCode::Char('e') => Some(Action::WordEnd),
                    KeyCode::Char('%') => Some(Action::MatchBracket),
                    KeyCode::Char('r') => Some(Action::Rename),
                    KeyCode::Char(';') => Some(Action::Comment),
                    KeyCode::Char('i') => Some(Action::Insert),
                    KeyCode::Char('*') => Some(Action::SearchWordNext),
                    KeyCode::Char('#') => Some(Action::SearchWordPrev),
                    KeyCode::Char('$') => Some(Action::LineEnd),
                    KeyCode::Char('n') => Some(Action::SearchNext),
                    KeyCode::Char('N') => Some(Action::SearchPrev),
                    KeyCode::Char('K') => Some(Action::ShowValue),
                    KeyCode::Char('q') | KeyCode::Esc => Some(Action::Close),
                    KeyCode::Char(':') => Some(Action::CommandMode),
                    KeyCode::Char('/') => Some(Action::SearchMode),
                    KeyCode::Tab => Some(Action::CycleFocus),
                    KeyCode::Char(' ') => Some(Action::ToggleView),
                    KeyCode::Char('?') => Some(Action::Help),
                    _ => None,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }
    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    #[test]
    fn plain_motions() {
        let mut p = NormalParser::default();
        assert_eq!(p.feed(key('j')), Some(Action::Down(1)));
        assert_eq!(p.feed(key('k')), Some(Action::Up(1)));
        assert_eq!(p.feed(key('G')), Some(Action::Bottom));
    }

    #[test]
    fn space_and_tab_toggle_view_and_focus() {
        let mut p = NormalParser::default();
        assert_eq!(
            p.feed(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE)),
            Some(Action::ToggleView)
        );
        assert_eq!(
            p.feed(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            Some(Action::CycleFocus)
        );
    }

    #[test]
    fn counts_apply_to_motions() {
        let mut p = NormalParser::default();
        assert_eq!(p.feed(key('1')), None);
        assert_eq!(p.feed(key('2')), None);
        assert_eq!(p.feed(key('j')), Some(Action::Down(12)));
        // count is consumed
        assert_eq!(p.feed(key('j')), Some(Action::Down(1)));
    }

    #[test]
    fn gg_and_gd_sequences() {
        let mut p = NormalParser::default();
        assert_eq!(p.feed(key('g')), None);
        assert_eq!(p.feed(key('g')), Some(Action::Top));
        assert_eq!(p.feed(key('g')), None);
        assert_eq!(p.feed(key('d')), Some(Action::Follow));
        // unknown g-sequence aborts cleanly
        assert_eq!(p.feed(key('g')), None);
        assert_eq!(p.feed(key('z')), None);
        assert_eq!(p.feed(key('j')), Some(Action::Down(1)));
    }

    #[test]
    fn ctrl_keys() {
        let mut p = NormalParser::default();
        assert_eq!(p.feed(ctrl('d')), Some(Action::HalfPageDown));
        assert_eq!(p.feed(ctrl('o')), Some(Action::JumpBack));
        assert_eq!(p.feed(ctrl('i')), Some(Action::JumpForward));
    }

    #[test]
    fn z_sequences() {
        let mut p = NormalParser::default();
        assert_eq!(p.feed(key('z')), None);
        assert_eq!(p.feed(key('z')), Some(Action::ScrollCursorMiddle));
        assert_eq!(p.feed(key('z')), None);
        assert_eq!(p.feed(key('t')), Some(Action::ScrollCursorTop));
        assert_eq!(p.feed(key('z')), None);
        assert_eq!(p.feed(key('b')), Some(Action::ScrollCursorBottom));
        // unknown z-sequence aborts cleanly
        assert_eq!(p.feed(key('z')), None);
        assert_eq!(p.feed(key('x')), None);
        assert_eq!(p.feed(key('j')), Some(Action::Down(1)));
    }

    #[test]
    fn ctrl_w_sequences() {
        let mut p = NormalParser::default();
        assert_eq!(p.feed(ctrl('w')), None);
        assert_eq!(p.feed(key('h')), Some(Action::FocusSidebar));
        assert_eq!(p.feed(ctrl('w')), None);
        assert_eq!(p.feed(key('l')), Some(Action::FocusMain));
        assert_eq!(p.feed(ctrl('w')), None);
        assert_eq!(p.feed(key('w')), Some(Action::CycleFocus));
        // unknown Ctrl-w sequence aborts cleanly
        assert_eq!(p.feed(ctrl('w')), None);
        assert_eq!(p.feed(key('x')), None);
        assert_eq!(p.feed(key('j')), Some(Action::Down(1)));
        // plain Ctrl still works
        assert_eq!(p.feed(ctrl('d')), Some(Action::HalfPageDown));
    }

    #[test]
    fn zero_alone_is_line_start() {
        let mut p = NormalParser::default();
        assert_eq!(p.feed(key('0')), Some(Action::LineStart));
        assert_eq!(p.feed(key('j')), Some(Action::Down(1)));
        // but 10j works
        assert_eq!(p.feed(key('1')), None);
        assert_eq!(p.feed(key('0')), None);
        assert_eq!(p.feed(key('j')), Some(Action::Down(10)));
    }
}
