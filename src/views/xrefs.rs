//! Centered xrefs popup (xrefs-to and xrefs-from).



use super::Scroller;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear};

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
        let w = (area.width * 3 / 4).clamp(40, 100);
        let rows_u16: u16 = self.rows.len().try_into().unwrap_or(u16::MAX);
        let h = rows_u16.saturating_add(2).clamp(3, 20);
        let popup = Scroller::centered_rect(area, w, h);
        frame.render_widget(Clear, popup);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan))
            .title(format!(" {} — Enter: goto, q: close ", self.title));
        let rows = &self.rows;
        self.scroller.render_list(frame, popup, block, rows.len(), |pos, selected| {
            let style = Scroller::row_style(selected);
            Line::from(Span::styled(rows.get(pos).map_or_else(String::new, |r| r.text.clone()), style))
        });
    }
}
