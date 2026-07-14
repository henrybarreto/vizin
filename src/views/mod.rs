

pub mod completion;
pub mod decomp;
pub mod functions;
pub mod help;
pub mod hex;
pub mod listing;
pub mod panels;
pub mod xrefs;

use crate::vim::Action;

/// Cursor + viewport state shared by all list-like views.
#[derive(Debug, Default, Clone)]
pub struct Scroller {
    pub cursor: usize,
    pub scroll: usize,
    /// viewport height, updated on each render
    pub height: usize,
}

impl Scroller {
    /// Row index of `cursor` within the currently visible viewport.
    pub const fn visible_row(&self) -> usize {
        self.cursor.saturating_sub(self.scroll)
    }

    /// Handle vertical motion over `len` rows. Returns true if the action was consumed.
    pub fn handle(&mut self, action: Action, len: usize) -> bool {
        if len == 0 {
            self.cursor = 0;
            self.scroll = 0;
            return matches!(
                action,
                Action::Up(_)
                    | Action::Down(_)
                    | Action::Top
                    | Action::Bottom
                    | Action::HalfPageDown
                    | Action::HalfPageUp
                    | Action::PageDown
                    | Action::PageUp
                    | Action::ScreenTop
                    | Action::ScreenMiddle
                    | Action::ScreenBottom
                    | Action::ScrollCursorTop
                    | Action::ScrollCursorMiddle
                    | Action::ScrollCursorBottom
            );
        }
        let h = self.height.max(1);
        match action {
            Action::Down(n) => self.cursor = (self.cursor + n).min(len - 1),
            Action::Up(n) => self.cursor = self.cursor.saturating_sub(n),
            Action::Top => self.cursor = 0,
            Action::Bottom => self.cursor = len - 1,
            Action::HalfPageDown => self.cursor = (self.cursor + h / 2).min(len - 1),
            Action::HalfPageUp => self.cursor = self.cursor.saturating_sub(h / 2),
            Action::PageDown => self.cursor = (self.cursor + h).min(len - 1),
            Action::PageUp => self.cursor = self.cursor.saturating_sub(h),
            Action::ScreenTop => self.cursor = self.scroll.min(len - 1),
            Action::ScreenMiddle => self.cursor = (self.scroll + h / 2).min(len - 1),
            Action::ScreenBottom => self.cursor = (self.scroll + h.saturating_sub(1)).min(len - 1),
            Action::ScrollCursorMiddle => {
                self.scroll = self.cursor.saturating_sub(h / 2);
            }
            Action::ScrollCursorTop => self.scroll = self.cursor,
            Action::ScrollCursorBottom => self.scroll = self.cursor + 1 - h,
            _ => return false,
        }
        self.ensure_visible();
        true
    }

    pub fn ensure_visible(&mut self) {
        let h = self.height.max(1);
        if self.cursor < self.scroll {
            self.scroll = self.cursor;
        } else if self.cursor >= self.scroll + h {
            self.scroll = self.cursor + 1 - h;
        }
    }

    pub fn set_cursor(&mut self, idx: usize) {
        self.cursor = idx;
        self.ensure_visible();
    }
}

/// Case-insensitive forward/backward search over lines; returns matching row index.
pub fn search_lines(
    lines: &[String],
    pattern: &str,
    from: usize,
    forward: bool,
) -> Option<usize> {
    if pattern.is_empty() || lines.is_empty() {
        return None;
    }
    let pat = pattern.to_lowercase();
    let n = lines.len();
    let idx = |step: usize| -> usize {
        if forward {
            (from + 1 + step) % n
        } else {
            (from + n - 1 - step % n) % n
        }
    };
    (0..n)
        .map(idx)
        .find(|&i| lines.get(i).is_some_and(|l| l.to_lowercase().contains(&pat)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vim::Action;

    #[test]
    fn scroller_clamps_and_scrolls() {
        let mut s = Scroller {
            height: 10,
            ..Default::default()
        };
        s.handle(Action::Down(5), 100);
        assert_eq!(s.cursor, 5);
        s.handle(Action::Bottom, 100);
        assert_eq!(s.cursor, 99);
        assert_eq!(s.scroll, 90);
        s.handle(Action::Down(3), 100);
        assert_eq!(s.cursor, 99);
        s.handle(Action::Top, 100);
        assert_eq!((s.cursor, s.scroll), (0, 0));
    }

    #[test]
    fn search_wraps_both_directions() {
        let lines: Vec<String> = ["mov eax, 1", "call sym.foo", "ret", "call sym.bar"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(search_lines(&lines, "call", 1, true), Some(3));
        assert_eq!(search_lines(&lines, "call", 3, true), Some(1));
        assert_eq!(search_lines(&lines, "CALL", 1, false), Some(3));
    }
}
