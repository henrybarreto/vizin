//! Generic list panels: strings, imports, exports, segments.

use super::{search_lines, Scroller};
use crate::backend::Backend;
use anyhow::Result;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelKind {
    Strings,
    Imports,
    Exports,
    Segments,
}

impl PanelKind {
    pub const fn title(self) -> &'static str {
        match self {
            Self::Strings => "Strings",
            Self::Imports => "Imports",
            Self::Exports => "Exports",
            Self::Segments => "Segments",
        }
    }
}

pub struct PanelRow {
    pub addr: Option<u64>,
    pub text: String,
}

pub struct PanelView {
    pub kind: PanelKind,
    pub rows: Vec<PanelRow>,
    pub scroller: Scroller,
}

impl PanelView {
    pub fn load(kind: PanelKind, backend: &mut Backend) -> Result<Self> {
        let rows = match kind {
            PanelKind::Strings => backend
                .strings()?
                .into_iter()
                .map(|s| PanelRow {
                    addr: Some(s.vaddr),
                    text: format!(
                        "{:>10x}  {:<18} {:<8} {}",
                        s.vaddr,
                        s.section,
                        s.stype,
                        s.string.replace('\n', "\\n")
                    ),
                })
                .collect(),
            PanelKind::Imports => backend
                .imports()?
                .into_iter()
                .map(|i| PanelRow {
                    addr: i.plt.filter(|&a| a != 0),
                    text: format!(
                        "{:>10}  {:<6} {:<8} {}",
                        i.plt.map(|a| format!("{a:x}")).unwrap_or_default(),
                        i.bind,
                        i.itype,
                        i.name
                    ),
                })
                .collect(),
            PanelKind::Exports => backend
                .exports()?
                .into_iter()
                .map(|e| PanelRow {
                    addr: Some(e.vaddr),
                    text: format!("{:>10x}  {:<8} {:<6} {}", e.vaddr, e.etype, e.size, e.name),
                })
                .collect(),
            PanelKind::Segments => backend
                .segments()?
                .into_iter()
                .map(|s| PanelRow {
                    addr: Some(s.vaddr),
                    text: format!(
                        "{:>10x}  {:<10} size {:<10x} {}",
                        s.vaddr, s.perm, s.size, s.name
                    ),
                })
                .collect(),
        };
        Ok(Self {
            kind,
            rows,
            scroller: Scroller::default(),
        })
    }

    pub fn selected_addr(&self) -> Option<u64> {
        self.rows.get(self.scroller.cursor)?.addr
    }

    pub fn search(&mut self, pattern: &str, forward: bool) -> bool {
        let lines: Vec<String> = self.rows.iter().map(|r| r.text.clone()).collect();
        if let Some(idx) = search_lines(&lines, pattern, self.scroller.cursor, forward) {
            self.scroller.set_cursor(idx);
            true
        } else {
            false
        }
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect, focused: bool) {
        self.scroller.height = area.height.saturating_sub(2) as usize;
        self.scroller.ensure_visible();
        let border_style = if focused {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        };
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
        let para = Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(border_style)
                .title(format!(" {} ({}) — Enter: goto, q: close ", self.kind.title(), self.rows.len())),
        );
        frame.render_widget(para, area);
    }
}
