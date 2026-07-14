//! Function list sidebar with live filtering.



use super::Scroller;
use crate::backend::FunctionInfo;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, List, ListItem};

#[derive(Default)]
#[allow(clippy::module_name_repetitions)] // matches sibling `*View` naming
pub struct FunctionsView {
    pub all: Vec<FunctionInfo>,
    pub filtered: Vec<usize>,
    pub filter: String,
    pub scroller: Scroller,
}

impl FunctionsView {
    pub fn set_functions(&mut self, mut fns: Vec<FunctionInfo>) {
        fns.sort_by_key(|f| f.offset);
        self.all = fns;
        self.apply_filter();
    }

    pub fn set_filter(&mut self, filter: &str) {
        self.filter = filter.to_string();
        self.apply_filter();
    }

    fn apply_filter(&mut self) {
        let pat = self.filter.to_lowercase();
        self.filtered = self
            .all
            .iter()
            .enumerate()
            .filter(|(_, f)| pat.is_empty() || f.name.to_lowercase().contains(&pat))
            .map(|(i, _)| i)
            .collect();
        self.scroller.cursor = self.scroller.cursor.min(self.filtered.len().saturating_sub(1));
        self.scroller.ensure_visible();
    }

    pub const fn len(&self) -> usize {
        self.filtered.len()
    }

    pub fn selected(&self) -> Option<&FunctionInfo> {
        self.filtered
            .get(self.scroller.cursor)
            .and_then(|&i| self.all.get(i))
    }

    /// Move the selection to the function containing `addr`, if any.
    pub fn select_addr(&mut self, addr: u64) {
        if let Some(pos) = self.filtered.iter().position(|&i| {
            self.all
                .get(i)
                .is_some_and(|f| addr >= f.offset && addr < f.offset + f.size.max(1))
        }) {
            self.scroller.set_cursor(pos);
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
        let title = if self.filter.is_empty() {
            format!(" Functions ({}) ", self.filtered.len())
        } else {
            format!(" Functions /{} ({}) ", self.filter, self.filtered.len())
        };
        let items: Vec<ListItem> = self
            .filtered
            .iter()
            .enumerate()
            .skip(self.scroller.scroll)
            .take(self.scroller.height)
            .filter_map(|(pos, &i)| {
                let f = self.all.get(i)?;
                let style = if pos == self.scroller.cursor {
                    Style::default().bg(Color::Blue).fg(Color::White).bold()
                } else if focused {
                    Style::default().fg(Color::Gray)
                } else {
                    Style::default().fg(Color::DarkGray)
                };
                Some(ListItem::new(Line::from(vec![
                    Span::styled(format!("{:>10x} ", f.offset), style.fg(Color::DarkGray)),
                    Span::styled(f.name.clone(), style),
                ])))
            })
            .collect();
        let list = List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(border_style)
                .title(title),
        );
        frame.render_widget(list, area);
    }
}
