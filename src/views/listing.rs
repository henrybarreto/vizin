//! Disassembly listing: virtualized rows built from `pdj` output, with flag
//! headers, comments, and per-instruction-type coloring.

use super::{search_lines, Scroller};
use crate::backend::{Backend, Instr};
use anyhow::Result;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};

const CHUNK: usize = 256;

#[derive(Debug, Clone)]
pub enum Row {
    Flag(String, u64),
    Instr(usize), // index into `instrs`
}

#[derive(Default)]
#[allow(clippy::module_name_repetitions)] // matches sibling `*View` naming
pub struct ListingView {
    pub instrs: Vec<Instr>,
    pub rows: Vec<Row>,
    pub scroller: Scroller,
    pub search_highlight: Option<String>,
}

impl ListingView {
    /// Load a fresh window of disassembly starting at `addr`.
    pub fn load(&mut self, backend: &mut Backend, addr: u64) -> Result<()> {
        self.instrs = backend.disasm(addr, CHUNK)?;
        self.rebuild_rows();
        self.scroller = Scroller {
            height: self.scroller.height,
            ..Default::default()
        };
        self.cursor_to_addr(addr);
        Ok(())
    }

    pub fn reload(&mut self, backend: &mut Backend) -> Result<()> {
        let addr = self.addr_at_cursor();
        let start = self.instrs.first().map(|i| i.offset);
        let count = self.instrs.len().max(CHUNK);
        if let Some(start) = start {
            self.instrs = backend.disasm(start, count)?;
            self.rebuild_rows();
            if let Some(a) = addr {
                self.cursor_to_addr(a);
            }
        }
        Ok(())
    }

    fn rebuild_rows(&mut self) {
        self.rows.clear();
        for (i, ins) in self.instrs.iter().enumerate() {
            for fl in &ins.flags {
                self.rows.push(Row::Flag(fl.clone(), ins.offset));
            }
            self.rows.push(Row::Instr(i));
        }
    }

    pub fn addr_at_cursor(&self) -> Option<u64> {
        match self.rows.get(self.scroller.cursor)? {
            Row::Flag(_, a) => Some(*a),
            Row::Instr(i) => self.instrs.get(*i).map(|ins| ins.offset),
        }
    }

    pub fn instr_at_cursor(&self) -> Option<&Instr> {
        match self.rows.get(self.scroller.cursor)? {
            Row::Instr(i) => self.instrs.get(*i),
            Row::Flag(_, a) => self.instrs.iter().find(|ins| ins.offset == *a),
        }
    }

    pub fn contains(&self, addr: u64) -> bool {
        match (self.instrs.first(), self.instrs.last()) {
            (Some(f), Some(l)) => addr >= f.offset && addr <= l.offset,
            _ => false,
        }
    }

    pub fn cursor_to_addr(&mut self, addr: u64) {
        if let Some(pos) = self.rows.iter().position(|r| match r {
            Row::Instr(i) => self.instrs.get(*i).is_some_and(|ins| ins.offset >= addr),
            Row::Flag(_, a) => *a >= addr,
        }) {
            self.scroller.set_cursor(pos);
        }
    }

    /// Extend the buffer when the cursor nears either edge.
    pub fn extend_if_needed(&mut self, backend: &mut Backend) {
        if self.rows.is_empty() {
            return;
        }
        // capture BEFORE any instrs mutations so that old Row::Instr indices stay valid
        let cur_addr = self.addr_at_cursor();
        let old_len = self.instrs.len();
        // near the end: append forward
        if self.scroller.cursor + 16 >= self.rows.len() {
            if let Some(last) = self.instrs.last() {
                let next = last.offset + last.size.max(1);
                if let Ok(more) = backend.disasm(next, CHUNK) {
                    let known = self.instrs.last().map_or(0, |i| i.offset);
                    self.instrs.extend(more.into_iter().filter(|i| i.offset > known));
                }
            }
        }
        // near the start: prepend backwards (best effort)
        if self.scroller.cursor < 4 {
            if let Some(first) = self.instrs.first().cloned() {
                if let Ok(mut before) = backend.disasm_back(first.offset, 64) {
                    if !before.is_empty() {
                        before.retain(|i| i.offset < first.offset);
                        before.extend(std::mem::take(&mut self.instrs));
                        self.instrs = before;
                    }
                }
            }
        }
        if self.instrs.len() != old_len {
            self.rebuild_rows();
            if let Some(a) = cur_addr {
                self.cursor_to_addr(a);
            }
        }
    }

    pub fn search_texts(&self) -> Vec<String> {
        self.rows
            .iter()
            .map(|r| match r {
                Row::Flag(name, _) => name.clone(),
                Row::Instr(i) => self
                    .instrs
                    .get(*i)
                    .map_or_else(String::new, |ins| format!("{:#x} {}", ins.offset, ins.disasm)),
            })
            .collect()
    }

    pub fn search(&mut self, pattern: &str, forward: bool) -> bool {
        let lines = self.search_texts();
        if let Some(idx) = search_lines(&lines, pattern, self.scroller.cursor, forward) {
            self.scroller.set_cursor(idx);
            true
        } else {
            false
        }
    }

    /// Extract a word from the current row's display text for `*` / `#`.
    pub fn word_at_cursor(&self) -> Option<String> {
        let text = match self.rows.get(self.scroller.cursor)? {
            Row::Flag(name, _) => name.clone(),
            Row::Instr(i) => self.instrs.get(*i).map_or_else(String::new, |ins| ins.disasm.clone()),
        };
        if text.is_empty() {
            return None;
        }
        // take first alphanumeric/underscore/dot token (skip leading punctuation)
        text.split(|c: char| !c.is_alphanumeric() && c != '_' && c != '.')
            .find(|s| !s.is_empty())
            .map(ToString::to_string)
    }

    fn instr_style(itype: &str) -> Style {
        match itype {
            "call" | "ucall" | "rcall" | "icall" => Style::default().fg(Color::Yellow),
            "jmp" | "ujmp" | "rjmp" | "ijmp" => Style::default().fg(Color::Green),
            "cjmp" | "ucjmp" => Style::default().fg(Color::LightGreen),
            "ret" => Style::default().fg(Color::Red),
            "cmp" | "acmp" | "test" => Style::default().fg(Color::Magenta),
            "push" | "pop" | "upush" | "rpush" => Style::default().fg(Color::Cyan),
            "invalid" | "ill" | "nop" => Style::default().fg(Color::DarkGray),
            _ => Style::default().fg(Color::Gray),
        }
    }

    /// Wrap a text string in spans, optionally highlighting every occurrence of `pat`.
    fn highlight_spans(text: &str, base: Style, pat: Option<&str>) -> Vec<Span<'static>> {
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

    pub fn render(&mut self, frame: &mut Frame, area: Rect, focused: bool, title_fn: &str) {
        self.scroller.height = area.height.saturating_sub(2) as usize;
        self.scroller.ensure_visible();
        let border_style = if focused {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let hl = self.search_highlight.as_deref();
        let mut lines: Vec<Line> = Vec::new();
        for (pos, row) in self
            .rows
            .iter()
            .enumerate()
            .skip(self.scroller.scroll)
            .take(self.scroller.height)
        {
            let selected = pos == self.scroller.cursor;
            let base = if selected {
                Style::default().bg(Color::Rgb(40, 40, 60))
            } else {
                Style::default()
            };
            let line = match row {
                Row::Flag(name, _) => {
                    // first span = fixed-width padding, never highlighted
                    let mut spans = vec![Span::styled("           ", base)];
                    spans.extend(Self::highlight_spans(
                        &format!("{name}:"),
                        base.fg(Color::LightBlue).bold(),
                        hl,
                    ));
                    Line::from(spans)
                }
                Row::Instr(i) => {
                    let Some(ins) = self.instrs.get(*i) else {
                        continue;
                    };
                    let mut spans = Self::highlight_spans(
                        &format!("{:>10x}", ins.offset),
                        base.fg(Color::DarkGray),
                        hl,
                    );
                    spans.push(Span::styled(" ", base));
                    spans.extend(Self::highlight_spans(
                        &format!("{:<16}", ins.bytes),
                        base.fg(Color::DarkGray),
                        hl,
                    ));
                    spans.push(Span::styled(" ", base));
                    spans.extend(Self::highlight_spans(
                        &ins.disasm,
                        base.patch(Self::instr_style(&ins.itype)),
                        hl,
                    ));
                    if let Some(c) = ins.comment_text() {
                        let c = c.replace('\n', " ");
                        spans.push(Span::styled("  ", base));
                        spans.extend(Self::highlight_spans(
                            &format!("; {c}"),
                            base.fg(Color::Rgb(120, 160, 120)).italic(),
                            hl,
                        ));
                    }
                    Line::from(spans)
                }
            };
            lines.push(line);
        }
        let para = Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(border_style)
                .title(format!(" Listing — {title_fn} ")),
        );
        frame.render_widget(para, area);
    }
}
