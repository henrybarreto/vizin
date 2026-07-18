//! Command-line completion popup (nvim-style).



use super::Scroller;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear};

/// Ex-command names completed by `:` + Tab, in `:h wildmode`-style `full` order.
pub const COMMANDS: &[&str] = &[
    "q", "q!", "w", "wq",
    "s", "seek", "goto",
    "fn", "functions",
    "str", "strings",
    "imp", "imports",
    "exp", "exports",
    "seg", "segments",
    "hex", "oo+",
];

// `Completion` + module `completion` is intentional: this type is re-exported
// and used by name (`CompletionPopup`) throughout app.rs/ui.rs.
#[allow(clippy::module_name_repetitions)]
pub struct CompletionPopup {
    pub all: Vec<String>,
    pub filtered: Vec<String>,
    pub scroller: Scroller,
}

impl CompletionPopup {
    pub fn new(commands: &[&str]) -> Self {
        let all: Vec<String> = commands.iter().map(ToString::to_string).collect();
        Self {
            filtered: all.clone(),
            all,
            scroller: Scroller::default(),
        }
    }

    pub fn filter(&mut self, prefix: &str) {
        self.filtered = self
            .all
            .iter()
            .filter(|c| c.starts_with(prefix))
            .cloned()
            .collect();
        self.scroller.cursor = 0;
        self.scroller.scroll = 0;
    }

    pub fn selected(&self) -> Option<&str> {
        self.filtered.get(self.scroller.cursor).map(String::as_str)
    }

    pub const fn is_empty(&self) -> bool {
        self.filtered.is_empty()
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect, cmd_line_y: u16) {
        if self.filtered.is_empty() {
            return;
        }
        let max_visible: u16 = self.filtered.len().min(15).try_into().unwrap_or(15);
        let w = (area.width * 2 / 3).clamp(30, 80).min(area.width);
        let h = (max_visible + 2).min(cmd_line_y);
        if h < 3 {
            return;
        }
        let popup = Rect {
            x: area.x,
            y: cmd_line_y.saturating_sub(h),
            width: w,
            height: h,
        };
        frame.render_widget(Clear, popup);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan));
        let filtered = &self.filtered;
        self.scroller.render_list(frame, popup, block, filtered.len(), |pos, selected| {
            let style = if selected {
                Style::default().bg(Color::DarkGray).fg(Color::White)
            } else {
                Style::default().fg(Color::Gray)
            };
            Line::from(Span::styled(filtered.get(pos).cloned().unwrap_or_default(), style))
        });
    }
}
