

pub mod completion;
pub mod decomp;
pub mod functions;
pub mod help;
pub mod hex;
pub mod listing;
pub mod panels;
pub mod xrefs;

use crate::vim::Action;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Paragraph};

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

    /// Per-frame prologue: viewport height is `area`'s height minus `chrome`
    /// (border/title rows), then clamp scroll to keep the cursor visible.
    pub fn begin_frame(&mut self, area: Rect, chrome: u16) {
        self.height = area.height.saturating_sub(chrome) as usize;
        self.ensure_visible();
    }

    /// Case-insensitive forward/backward search over `lines`, moving the
    /// cursor to the match. Returns whether a match was found.
    pub fn search(&mut self, lines: &[String], pattern: &str, forward: bool) -> bool {
        search_lines(lines, pattern, self.cursor, forward).is_some_and(|idx| {
            self.set_cursor(idx);
            true
        })
    }

    /// Cyan border when focused, dark gray otherwise — the default chrome for
    /// every bordered view.
    pub fn border_style(focused: bool) -> Style {
        if focused {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        }
    }

    /// Selected-row style (blue background) vs the default row style (gray).
    pub fn row_style(selected: bool) -> Style {
        if selected {
            Style::default().bg(Color::Blue).fg(Color::White)
        } else {
            Style::default().fg(Color::Gray)
        }
    }

    /// Rect of `width` x `height` centered in `area`, clamped to fit.
    pub fn centered_rect(area: Rect, width: u16, height: u16) -> Rect {
        let w = width.min(area.width);
        let h = height.min(area.height);
        Rect {
            x: area.x + area.width.saturating_sub(w) / 2,
            y: area.y + area.height.saturating_sub(h) / 2,
            width: w,
            height: h,
        }
    }

    /// Split `text` into spans, highlighting every case-insensitive
    /// occurrence of `pat` with a dim-yellow background.
    pub fn highlight_spans(text: &str, base: Style, pat: Option<&str>) -> Vec<Span<'static>> {
        let Some(pat) = pat else {
            return vec![Span::styled(text.to_string(), base)];
        };
        if pat.is_empty() {
            return vec![Span::styled(text.to_string(), base)];
        }
        let low = text.to_lowercase();
        let plow = pat.to_lowercase();
        let mut spans = Vec::new();
        let mut start = 0;
        while let Some(pos) = low[start..].find(&plow) {
            let abs = start + pos;
            if abs > start {
                spans.push(Span::styled(text[start..abs].to_string(), base));
            }
            spans.push(Span::styled(
                text[abs..abs + plow.len()].to_string(),
                base.bg(Color::Rgb(80, 60, 0)),
            ));
            start = abs + plow.len();
        }
        if start < text.len() {
            spans.push(Span::styled(text[start..].to_string(), base));
        }
        spans
    }

    /// Render a simple bordered list: `len` rows, each built by `line_for(row,
    /// selected)` — callers derive their own style from `selected` (e.g. via
    /// [`Self::row_style`]), so views that need more than a plain 2-state
    /// selected/default look (focus-dimming, etc.) can still use this for
    /// the viewport prologue and the skip/take visible-window slice.
    pub fn render_list(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        block: Block<'_>,
        len: usize,
        mut line_for: impl FnMut(usize, bool) -> Line<'static>,
    ) {
        self.begin_frame(area, 2);
        let lines: Vec<Line> = (self.scroll..(self.scroll + self.height).min(len))
            .map(|pos| line_for(pos, pos == self.cursor))
            .collect();
        frame.render_widget(Paragraph::new(lines).block(block), area);
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

    #[test]
    fn centered_rect_clamps_to_area() {
        let area = Rect { x: 0, y: 0, width: 100, height: 40 };
        let r = Scroller::centered_rect(area, 40, 10);
        assert_eq!(r, Rect { x: 30, y: 15, width: 40, height: 10 });
        // requested size larger than area: clamp to area's bounds
        let r = Scroller::centered_rect(area, 200, 200);
        assert_eq!(r, Rect { x: 0, y: 0, width: 100, height: 40 });
    }

    #[test]
    fn highlight_spans_splits_on_match() {
        let spans = Scroller::highlight_spans("call sym.foo", Style::default(), Some("sym"));
        let texts: Vec<&str> = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(texts, vec!["call ", "sym", ".foo"]);
        // no pattern: single unhighlighted span
        let spans = Scroller::highlight_spans("call sym.foo", Style::default(), None);
        assert_eq!(spans.len(), 1);
    }

    #[test]
    fn scroller_search_moves_cursor() {
        let lines: Vec<String> = ["mov eax, 1", "call sym.foo", "ret"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let mut s = Scroller { height: 10, ..Default::default() };
        assert!(s.search(&lines, "call", true));
        assert_eq!(s.cursor, 1);
        assert!(!s.search(&lines, "nope", true));
    }
}
