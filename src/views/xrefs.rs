//! Centered xrefs popup (xrefs-to and xrefs-from).



use super::Scroller;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

pub struct XrefRow {
    pub addr: u64,
    pub text: String,
}

#[allow(clippy::module_name_repetitions)] // matches sibling `CompletionPopup` naming
pub struct XrefsPopup {
    pub title: String,
    pub rows: Vec<XrefRow>,
    pub scroller: Scroller,
}

impl XrefsPopup {
    pub fn new(title: String, rows: Vec<XrefRow>) -> Self {
        Self {
            title,
            rows,
            scroller: Scroller::default(),
        }
    }

    pub fn selected_addr(&self) -> Option<u64> {
        self.rows.get(self.scroller.cursor).map(|r| r.addr)
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        let w = (area.width * 3 / 4).clamp(40, 100).min(area.width);
        let rows_u16: u16 = self.rows.len().try_into().unwrap_or(u16::MAX);
        let h = rows_u16.saturating_add(2).clamp(3, 20).min(area.height);
        let popup = Rect {
            x: area.x + (area.width - w) / 2,
            y: area.y + (area.height.saturating_sub(h)) / 2,
            width: w,
            height: h,
        };
        self.scroller.height = popup.height.saturating_sub(2) as usize;
        self.scroller.ensure_visible();
        let lines: Vec<Line> = self
            .rows
            .iter()
            .enumerate()
            .skip(self.scroller.scroll)
            .take(self.scroller.height)
            .map(|(pos, r)| {
                let style = if pos == self.scroller.cursor {
                    Style::default().bg(Color::Blue).fg(Color::White)
                } else {
                    Style::default().fg(Color::Gray)
                };
                Line::from(Span::styled(r.text.clone(), style))
            })
            .collect();
        frame.render_widget(Clear, popup);
        let para = Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan))
                .title(format!(" {} — Enter: goto, q: close ", self.title)),
        );
        frame.render_widget(para, popup);
    }
}
