//! Hex view with vim navigation and insert-mode byte patching.

use crate::backend::Backend;
use crate::vim::Action;
use anyhow::Result;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};
use std::collections::BTreeMap;

const COLS: usize = 16;
const WINDOW: usize = 4096;

#[allow(clippy::module_name_repetitions)] // matches sibling `*View` naming
pub struct HexView {
    pub base: u64,
    pub bytes: Vec<u8>,
    /// staged (unsaved) edits: addr -> new byte
    pub edits: BTreeMap<u64, u8>,
    pub cursor: usize, // byte index in window
    pub nibble: bool,  // false = high nibble
    pub scroll: usize, // row offset
    pub height: usize,
    pub editing: bool,
}

impl HexView {
    pub fn load(backend: &mut Backend, addr: u64) -> Result<Self> {
        let base = addr & !0xf;
        let bytes = backend.read_bytes(base, WINDOW)?;
        Ok(Self {
            base,
            bytes,
            edits: BTreeMap::new(),
            cursor: usize::try_from(addr - base).unwrap_or(0),
            nibble: false,
            scroll: 0,
            height: 0,
            editing: false,
        })
    }

    pub fn addr_at_cursor(&self) -> u64 {
        self.base + u64::try_from(self.cursor).unwrap_or(u64::MAX)
    }

    /// Row index of the cursor within the currently visible viewport.
    pub const fn cursor_row_in_view(&self) -> usize {
        (self.cursor / COLS).saturating_sub(self.scroll)
    }

    pub fn seek(&mut self, backend: &mut Backend, addr: u64) -> Result<()> {
        if addr < self.base || addr >= self.base + u64::try_from(self.bytes.len()).unwrap_or(u64::MAX) {
            let base = addr & !0xf;
            self.bytes = backend.read_bytes(base, WINDOW)?;
            self.base = base;
            self.scroll = 0;
        }
        self.cursor = usize::try_from(addr - self.base).unwrap_or(0);
        self.nibble = false;
        Ok(())
    }

    /// Slide the window when the cursor crosses its edges.
    fn reanchor(&mut self, backend: &mut Backend, target: i64) -> Result<()> {
        let base = i64::try_from(self.base).unwrap_or(i64::MAX);
        let addr = u64::try_from((base + target).max(0)).unwrap_or(0);
        self.seek(backend, addr)
    }

    /// Byte-offset step (in the current WINDOW-sized buffer) for a scroll/motion action.
    fn step_for(&self, action: Action) -> Option<i64> {
        let to_i64 = |n: usize| i64::try_from(n).unwrap_or(i64::MAX);
        let cols = to_i64(COLS);
        Some(match action {
            Action::Left(n) => -to_i64(n),
            Action::Right(n) => to_i64(n),
            Action::Up(n) => -(to_i64(n) * cols),
            Action::Down(n) => to_i64(n) * cols,
            Action::HalfPageUp => -(to_i64(self.height.max(2) / 2) * cols),
            Action::HalfPageDown => to_i64(self.height.max(2) / 2) * cols,
            Action::PageUp => -(to_i64(self.height.max(1)) * cols),
            Action::PageDown => to_i64(self.height.max(1)) * cols,
            _ => return None,
        })
    }

    pub fn handle(&mut self, action: Action, backend: &mut Backend) -> Result<bool> {
        match action {
            Action::Top => {
                self.cursor = 0;
                self.nibble = false;
                self.ensure_visible();
                return Ok(true);
            }
            Action::Bottom => {
                self.cursor = self.bytes.len().saturating_sub(1);
                self.nibble = false;
                self.ensure_visible();
                return Ok(true);
            }
            Action::ScrollCursorMiddle => {
                let row = self.cursor / COLS;
                self.scroll = row.saturating_sub(self.height.max(1) / 2);
                return Ok(true);
            }
            _ => {}
        }
        let Some(step) = self.step_for(action) else {
            return Ok(false);
        };
        let cursor = i64::try_from(self.cursor).unwrap_or(i64::MAX);
        let len = i64::try_from(self.bytes.len()).unwrap_or(i64::MAX);
        let target = cursor + step;
        if target < 0 || target >= len {
            self.reanchor(backend, target)?;
        } else {
            self.cursor = usize::try_from(target).unwrap_or(0);
        }
        self.nibble = false;
        self.ensure_visible();
        Ok(true)
    }

    fn ensure_visible(&mut self) {
        let row = self.cursor / COLS;
        let h = self.height.max(1);
        if row < self.scroll {
            self.scroll = row;
        } else if row >= self.scroll + h {
            self.scroll = row + 1 - h;
        }
    }

    /// Type one hex digit in insert mode.
    pub fn input_nibble(&mut self, c: char) -> bool {
        let Some(d) = c.to_digit(16) else {
            return false;
        };
        let addr = self.addr_at_cursor();
        let d = u8::try_from(d).unwrap_or(0);
        let cur = self
            .edits
            .get(&addr)
            .copied()
            .unwrap_or_else(|| self.bytes.get(self.cursor).copied().unwrap_or(0));
        let new = if self.nibble {
            (cur & 0xf0) | d
        } else {
            (d << 4) | (cur & 0x0f)
        };
        self.edits.insert(addr, new);
        if self.nibble {
            self.nibble = false;
            if self.cursor + 1 < self.bytes.len() {
                self.cursor += 1;
            }
            self.ensure_visible();
        } else {
            self.nibble = true;
        }
        true
    }

    /// Commit staged edits as contiguous `wx` writes. Returns bytes written.
    pub fn commit(&mut self, backend: &mut Backend) -> Result<usize> {
        if self.edits.is_empty() {
            return Ok(0);
        }
        let mut runs: Vec<(u64, Vec<u8>)> = Vec::new();
        for (&addr, &b) in &self.edits {
            match runs.last_mut() {
                Some((start, buf)) if *start + buf.len() as u64 == addr => buf.push(b),
                _ => runs.push((addr, vec![b])),
            }
        }
        let mut written = 0;
        for (addr, buf) in &runs {
            backend.write_bytes(*addr, buf)?;
            written += buf.len();
        }
        // refresh window from disk and drop staging
        self.bytes = backend.read_bytes(self.base, self.bytes.len().max(WINDOW))?;
        self.edits.clear();
        Ok(written)
    }

    pub fn discard(&mut self) {
        self.edits.clear();
        self.nibble = false;
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect, focused: bool) {
        self.height = area.height.saturating_sub(2) as usize;
        self.ensure_visible();
        let border_style = if self.editing {
            Style::default().fg(Color::LightRed)
        } else if focused {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let rows = self.bytes.len().div_ceil(COLS);
        let mut lines: Vec<Line> = Vec::new();
        for row in self.scroll..(self.scroll + self.height).min(rows) {
            let addr = self.base + u64::try_from(row * COLS).unwrap_or(u64::MAX);
            let mut spans = vec![Span::styled(
                format!("{addr:>10x}  "),
                Style::default().fg(Color::DarkGray),
            )];
            let mut ascii = String::new();
            for col in 0..COLS {
                let idx = row * COLS + col;
                if idx >= self.bytes.len() {
                    spans.push(Span::raw("   "));
                    continue;
                }
                let a = self.base + u64::try_from(idx).unwrap_or(u64::MAX);
                let edited = self.edits.contains_key(&a);
                let b = self
                    .edits
                    .get(&a)
                    .copied()
                    .unwrap_or_else(|| self.bytes.get(idx).copied().unwrap_or(0));
                let mut st = if edited {
                    Style::default().fg(Color::LightRed).bold()
                } else if b == 0 {
                    Style::default().fg(Color::DarkGray)
                } else {
                    Style::default().fg(Color::Gray)
                };
                if idx == self.cursor {
                    st = st.bg(if self.editing {
                        Color::Rgb(90, 40, 40)
                    } else {
                        Color::Rgb(40, 40, 60)
                    });
                    if focused {
                        st = st.add_modifier(Modifier::REVERSED);
                    }
                }
                spans.push(Span::styled(format!("{b:02x}"), st));
                spans.push(Span::raw(if col == 7 { "  " } else { " " }));
                ascii.push(if (0x20..0x7f).contains(&b) {
                    b as char
                } else {
                    '.'
                });
            }
            spans.push(Span::styled(ascii, Style::default().fg(Color::Rgb(140, 140, 100))));
            lines.push(Line::from(spans));
        }
        let mode = if self.editing {
            " Hex [INSERT — type hex digits, Esc commits] "
        } else {
            " Hex — i: edit, Enter/q: back "
        };
        let para = Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(border_style)
                .title(mode),
        );
        frame.render_widget(para, area);
    }
}
