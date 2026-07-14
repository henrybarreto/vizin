//! Decompiler pane: renders rz-ghidra (Ghidra decompiler) output with syntax
//! colors from annotations, and maps the cursor to addresses via `offset`
//! annotations — enabling follow/xrefs/rename directly in pseudo-C.

use super::{search_lines, Scroller};
use crate::backend::{Annotation, DecompResult};
use crate::ts::TsParser;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};

#[derive(Default)]
// `Decomp` + module `decomp` is intentional: matches the sibling
// `ListingView`/`HexView`/... naming used throughout app.rs/ui.rs.
#[allow(clippy::module_name_repetitions)]
pub struct DecompView {
    pub result: Option<DecompResult>,
    pub lines: Vec<String>,
    /// byte offset in `code` where each line starts
    line_starts: Vec<usize>,
    /// `lines` joined with '\n', cached since it's rebuilt on every
    /// tree-sitter query and functions can be thousands of lines long
    code_cache: String,
    pub scroller: Scroller,
    pub col: usize,
    /// horizontal scroll offset (columns off-screen to the left)
    pub hscroll: usize,
    /// viewport width (columns), updated on each render
    viewport_width: usize,
    /// address of the function currently decompiled
    pub fcn_addr: u64,
    /// status text shown (centered) while there is no code — decompiling/error
    pub notice: Option<String>,
    pub search_highlight: Option<String>,
    /// Tree-sitter C parser for AST-aware operations
    ts: TsParser,
}

/// What sits under the cursor in the decompiled code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Symbol {
    Function { name: String, addr: u64 },
    Global { addr: u64 },
    Local { name: String },
    Param { name: String },
    None,
}

impl DecompView {
    pub fn set(&mut self, result: DecompResult, fcn_addr: u64) {
        self.lines = result.code.lines().map(ToString::to_string).collect();
        self.line_starts = Vec::with_capacity(self.lines.len());
        let mut pos = 0;
        for l in &self.lines {
            self.line_starts.push(pos);
            pos += l.len() + 1; // '\n'
        }
        self.code_cache = self.lines.join("\n");
        self.result = Some(result);
        self.fcn_addr = fcn_addr;
        self.scroller.cursor = 0;
        self.scroller.scroll = 0;
        self.col = 0;
        self.hscroll = 0;
        self.notice = None;
    }

    pub fn clear(&mut self) {
        self.result = None;
        self.lines.clear();
        self.line_starts.clear();
        self.code_cache.clear();
        self.scroller.cursor = 0;
        self.scroller.scroll = 0;
        self.col = 0;
        self.hscroll = 0;
    }

    fn byte_pos(&self) -> Option<usize> {
        let line = self.lines.get(self.scroller.cursor)?;
        let col = self.col.min(line.len().saturating_sub(1));
        Some(self.line_starts.get(self.scroller.cursor)? + col)
    }

    fn annotations(&self) -> &[Annotation] {
        self.result.as_ref().map_or(&[], |r| r.annotations.as_slice())
    }

    /// Full decompiled source text (needed for tree-sitter parsing).
    fn full_code(&self) -> &str {
        &self.code_cache
    }

    /// Address associated with the cursor position (nearest enclosing offset annotation).
    pub fn addr_at_cursor(&self) -> Option<u64> {
        let pos = self.byte_pos()?;
        for a in self.annotations() {
            if a.atype == "offset" && a.start <= pos && pos < a.end {
                if let Some(o) = a.offset {
                    return Some(o);
                }
            }
        }
        // fall back: offset annotation whose start is nearest to cursor byte position
        self.annotations()
            .iter()
            .filter(|a| a.atype == "offset")
            .filter_map(|a| a.offset.map(|o| (a.start, o)))
            .min_by_key(|(s, _)| s.abs_diff(pos))
            .map(|(_, o)| o)
    }

    /// Semantic symbol under the cursor (for follow/rename).
    /// Uses Ghidra annotations first (they carry address info), then
    /// falls back to tree-sitter AST for symbols annotations miss.
    pub fn symbol_at_cursor(&mut self) -> Symbol {
        let Some(pos) = self.byte_pos() else {
            return Symbol::None;
        };
        for a in self.annotations() {
            if a.start <= pos && pos < a.end {
                match a.atype.as_str() {
                    "function_name" => {
                        if let (Some(n), Some(o)) = (&a.name, a.offset) {
                            return Symbol::Function {
                                name: n.clone(),
                                addr: o,
                            };
                        }
                    }
                    "global_variable" | "constant_variable" => {
                        if let Some(o) = a.offset {
                            return Symbol::Global { addr: o };
                        }
                    }
                    "local_variable" => {
                        if let Some(n) = &a.name {
                            return Symbol::Local { name: n.clone() };
                        }
                    }
                    "function_parameter" => {
                        if let Some(n) = &a.name {
                            return Symbol::Param { name: n.clone() };
                        }
                    }
                    _ => {}
                }
            }
        }
        // Fallback: use tree-sitter AST (no address info available)
        match self.ts.symbol_at(&self.code_cache, pos) {
            crate::ts::TsSymbol::Function { name, .. } => Symbol::Function { name, addr: 0 },
            crate::ts::TsSymbol::Global { .. } => Symbol::Global { addr: 0 },
            crate::ts::TsSymbol::Local { name } => Symbol::Local { name },
            crate::ts::TsSymbol::Param { name } => Symbol::Param { name },
            crate::ts::TsSymbol::None => Symbol::None,
        }
    }

    /// Move the cursor to the first line whose code maps to `addr`.
    pub fn cursor_to_addr(&mut self, addr: u64) {
        let mut best: Option<(u64, usize)> = None; // (distance, byte_start)
        for a in self.annotations() {
            if a.atype == "offset" {
                if let Some(o) = a.offset {
                    let dist = o.abs_diff(addr);
                    if best.is_none_or(|(d, _)| dist < d) {
                        best = Some((dist, a.start));
                    }
                }
            }
        }
        if let Some((dist, start)) = best {
            if dist < 0x1000 {
                let line = self.line_of_byte(start);
                let col = start.saturating_sub(self.line_starts.get(line).copied().unwrap_or(0));
                self.scroller.set_cursor(line);
                self.col = col;
            }
        }
    }

    fn line_of_byte(&self, byte: usize) -> usize {
        match self.line_starts.binary_search(&byte) {
            Ok(i) => i,
            Err(0) => 0,
            Err(i) => i - 1,
        }
    }

    pub fn move_col(&mut self, delta: i64) {
        let line_len = self.lines.get(self.scroller.cursor).map_or(0, String::len);
        let max = line_len.saturating_sub(1);
        if delta < 0 {
            let step = usize::try_from(-delta).unwrap_or(usize::MAX);
            self.col = self.col.saturating_sub(step);
        } else {
            let step = usize::try_from(delta).unwrap_or(usize::MAX);
            self.col = self.col.saturating_add(step).min(max);
        }
        self.sync_hscroll();
    }

    /// Adjust `hscroll` so that `self.col` is always visible.
    pub const fn sync_hscroll(&mut self) {
        let w = self.viewport_width;
        if w == 0 {
            return;
        }
        if self.col < self.hscroll {
            self.hscroll = self.col;
        } else if self.col >= self.hscroll + w {
            self.hscroll = self.col + 1 - w;
        }
    }

    const fn is_word_char(c: u8) -> bool {
        c.is_ascii_alphanumeric() || c == b'_'
    }

    /// Byte at `i`, or `0` (neither word nor whitespace) past the end — callers
    /// always pair this with a `i < len`/`i > 0` loop guard.
    fn byte_at(line: &[u8], i: usize) -> u8 {
        line.get(i).copied().unwrap_or(0)
    }

    /// Move `col` to the start of the next word.
    pub fn move_word_next(&mut self) {
        let line = match self.lines.get(self.scroller.cursor) {
            Some(l) => l.as_bytes(),
            None => return,
        };
        let len = line.len();
        let mut i = self.col.saturating_add(1).min(len);
        // skip current word
        while i < len && Self::is_word_char(Self::byte_at(line, i)) {
            i += 1;
        }
        // skip whitespace
        while i < len && Self::byte_at(line, i).is_ascii_whitespace() {
            i += 1;
        }
        self.col = i.min(len.saturating_sub(1));
    }

    /// Move `col` to the start of the previous word.
    pub fn move_word_prev(&mut self) {
        let line = match self.lines.get(self.scroller.cursor) {
            Some(l) => l.as_bytes(),
            None => return,
        };
        let len = line.len();
        let mut i = self.col.saturating_sub(1);
        // skip whitespace before cursor
        while i > 0 && Self::byte_at(line, i).is_ascii_whitespace() {
            i -= 1;
        }
        // skip the word
        while i > 0 && Self::is_word_char(Self::byte_at(line, i)) {
            i -= 1;
        }
        let stepped_on_whitespace = i > 0 && Self::byte_at(line, i).is_ascii_whitespace();
        let stepped_onto_word = i + 1 < len && Self::is_word_char(Self::byte_at(line, i + 1));
        if stepped_on_whitespace || stepped_onto_word {
            i += 1;
        }
        self.col = i;
    }

    /// Move `col` to the end of the current/next word.
    pub fn move_word_end(&mut self) {
        let line = match self.lines.get(self.scroller.cursor) {
            Some(l) => l.as_bytes(),
            None => return,
        };
        let len = line.len();
        let mut i = self.col;
        // skip current word forward
        while i < len && Self::is_word_char(Self::byte_at(line, i)) {
            i += 1;
        }
        // skip whitespace
        while i < len && Self::byte_at(line, i).is_ascii_whitespace() {
            i += 1;
        }
        // now at start of next word; advance to its end
        if i < len && !Self::is_word_char(Self::byte_at(line, i)) {
            // symbol word (single char)
            self.col = i;
            return;
        }
        while i < len && Self::is_word_char(Self::byte_at(line, i)) {
            i += 1;
        }
        self.col = i.saturating_sub(1).min(len.saturating_sub(1));
    }

    /// Move `col` to the next occurrence of `c` on the current line.
    pub fn find_char(&mut self, c: char) {
        let line = match self.lines.get(self.scroller.cursor) {
            Some(l) => l.as_bytes(),
            None => return,
        };
        let target = c as u8;
        let start = self.col + 1;
        if let Some(pos) = line.get(start..).and_then(|s| s.iter().position(|&b| b == target)) {
            self.col = start + pos;
        }
    }

    /// Move `col` to the previous occurrence of `c` on the current line.
    pub fn find_char_back(&mut self, c: char) {
        let line = match self.lines.get(self.scroller.cursor) {
            Some(l) => l.as_bytes(),
            None => return,
        };
        let target = c as u8;
        if self.col == 0 {
            return;
        }
        let end = self.col;
        if let Some(pos) = line.get(..end).and_then(|s| s.iter().rposition(|&b| b == target)) {
            self.col = pos;
        }
    }

    /// Move the cursor to the previous line containing `{`.
    pub fn goto_prev_brace(&mut self) {
        let cur = self.scroller.cursor;
        for i in (0..cur).rev() {
            if let Some(line) = self.lines.get(i) {
                if line.contains('{') {
                    self.scroller.set_cursor(i);
                    self.col = 0;
                    return;
                }
            }
        }
    }

    /// Move the cursor to the next line containing `}`.
    pub fn goto_next_brace(&mut self) {
        let cur = self.scroller.cursor;
        let len = self.lines.len();
        for i in cur + 1..len {
            if let Some(line) = self.lines.get(i) {
                if line.contains('}') {
                    self.scroller.set_cursor(i);
                    self.col = 0;
                    return;
                }
            }
        }
    }

    /// Jump to the matching bracket (`{}`, `()`, `[]`).
    /// Searches forward from cursor for a bracket, then uses tree-sitter
    /// to find its match (skipping strings/comments). Falls back to a
    /// simple byte-level scan when tree-sitter can't parse the code.
    pub fn match_bracket(&mut self) {
        let pairs: &[(u8, u8)] = &[(b'{', b'}'), (b'(', b')'), (b'[', b']')];
        let cursor = self.scroller.cursor;
        // Find the first bracket at or after cursor
        for (li, line) in self.lines.iter().enumerate().skip(cursor) {
            let bytes = line.as_bytes();
            let start = if li == cursor { self.col } else { 0 };
            for (ci, &b) in bytes.iter().enumerate().skip(start) {
                for &(op, cl) in pairs {
                    if b == op || b == cl {
                        // Compute absolute byte position
                        let abs = self.line_starts.get(li).copied().unwrap_or(0) + ci;
                        // Try tree-sitter first
                        if let Some(target) = self.ts.find_match(&self.code_cache, abs) {
                            let line = self.line_of_byte(target);
                            self.scroller.set_cursor(line);
                            self.col = target - self.line_starts.get(line).copied().unwrap_or(0);
                            self.scroller.ensure_visible();
                            return;
                        }
                        // Fallback: byte-level depth scan
                        let target = if b == op {
                            Self::scan_bracket_fwd_static(&self.lines, li, ci, op, cl)
                        } else {
                            Self::scan_bracket_back_static(&self.lines, li, ci, op, cl)
                        };
                        if let Some(target) = target {
                            self.scroller.set_cursor(target.0);
                            self.col = target.1;
                            self.scroller.ensure_visible();
                            return;
                        }
                        return; // bracket found but no match
                    }
                }
            }
        }
    }

    fn scan_bracket_fwd_static(
        lines: &[String],
        start_line: usize, start_col: usize, open: u8, close: u8,
    ) -> Option<(usize, usize)> {
        let mut depth: i64 = 1;
        for (li, line) in lines.iter().enumerate().skip(start_line) {
            let bytes = line.as_bytes();
            let from = if li == start_line { start_col + 1 } else { 0 };
            for (ci, &b) in bytes.iter().enumerate().skip(from) {
                if b == open {
                    depth += 1;
                } else if b == close {
                    depth -= 1;
                    if depth == 0 {
                        return Some((li, ci));
                    }
                }
            }
        }
        None
    }

    fn scan_bracket_back_static(
        lines: &[String],
        start_line: usize, start_col: usize, open: u8, close: u8,
    ) -> Option<(usize, usize)> {
        let mut depth: i64 = 1;
        for idx in (0..=start_line).rev() {
            let Some(line) = lines.get(idx) else {
                continue;
            };
            let bytes = line.as_bytes();
            let to = if idx == start_line {
                let Some(t) = start_col.checked_sub(1) else {
                    continue;
                };
                t
            } else {
                bytes.len().saturating_sub(1)
            };
            for ci in (0..=to).rev() {
                let Some(&b) = bytes.get(ci) else {
                    continue;
                };
                if b == close {
                    depth += 1;
                } else if b == open {
                    depth -= 1;
                    if depth == 0 {
                        return Some((idx, ci));
                    }
                }
            }
        }
        None
    }

    /// Byte range (start, end) of the function enclosing the cursor.
    pub fn scope_at_cursor(&mut self) -> Option<(usize, usize)> {
        let pos = self.byte_pos()?;
        self.ts.enclosing_function(&self.code_cache, pos)
    }

    /// Find all occurrences of the identifier at the cursor (excluding strings/comments).
    pub fn references_at_cursor(&mut self) -> Vec<usize> {
        let Some(word) = self.word_at_cursor_inner() else {
            return vec![];
        };
        self.ts.find_references(&self.code_cache, &word)
    }

    pub fn search(&mut self, pattern: &str, forward: bool) -> bool {
        if self.code_cache.is_empty() {
            return false;
        }
        let cur_byte = self.byte_pos().unwrap_or(0);
        // Try tree-sitter code-only search first
        let ts_result = if forward {
            self.ts.code_search(&self.code_cache, pattern, cur_byte + 1)
        } else {
            self.ts
                .code_search_back(&self.code_cache, pattern, cur_byte.saturating_sub(1))
        };
        if let Some(abs) = ts_result {
            let line = self.line_of_byte(abs);
            self.scroller.set_cursor(line);
            self.col = abs - self.line_starts.get(line).copied().unwrap_or(0);
            self.scroller.ensure_visible();
            return true;
        }
        // Fall back to simple line search (wraps around)
        if let Some(idx) = search_lines(&self.lines, pattern, self.scroller.cursor, forward) {
            self.scroller.set_cursor(idx);
            self.col = 0;
            if let Some(line) = self.lines.get(idx) {
                let low = line.to_lowercase();
                if let Some(pos) = low.find(&pattern.to_lowercase()) {
                    self.col = pos;
                }
            }
            self.scroller.ensure_visible();
            true
        } else {
            false
        }
    }

    /// Extract the word under the cursor for `*` / `#`.
    /// Uses tree-sitter AST to identify the actual token.
    pub fn word_at_cursor(&mut self) -> Option<String> {
        self.word_at_cursor_inner()
    }

    fn word_at_cursor_inner(&mut self) -> Option<String> {
        let pos = self.byte_pos()?;
        // Try tree-sitter first (AST-aware: skips strings, gets exact identifier)
        if let Some(w) = self.ts.word_at(&self.code_cache, pos) {
            if !w.is_empty() {
                return Some(w);
            }
        }
        // Fall back to byte-level extraction
        let line = self.lines.get(self.scroller.cursor)?;
        Self::extract_word_at(line, self.col)
    }

    fn highlight_style(kind: &str) -> Style {
        match kind {
            "keyword" => Style::default().fg(Color::Yellow).bold(),
            "datatype" => Style::default().fg(Color::Cyan),
            "function_name" => Style::default().fg(Color::LightBlue).bold(),
            "comment" => Style::default().fg(Color::DarkGray).italic(),
            "constant_variable" => Style::default().fg(Color::Magenta),
            "global_variable" => Style::default().fg(Color::LightRed),
            "local_variable" => Style::default().fg(Color::White),
            "function_parameter" => Style::default().fg(Color::LightMagenta),
            _ => Style::default().fg(Color::Gray),
        }
    }

    /// Build a per-byte style map once per render (code is ASCII from the decompiler).
    fn style_map(&self, from_byte: usize, to_byte: usize) -> Vec<(usize, usize, Style)> {
        let mut spans = Vec::new();
        for a in self.annotations() {
            if a.atype == "syntax_highlight" && a.end > from_byte && a.start < to_byte {
                if let Some(kind) = &a.syntax_highlight {
                    spans.push((a.start, a.end, Self::highlight_style(kind)));
                }
            }
        }
        spans.sort_by_key(|s| s.0);
        spans
    }

    /// Split `text` into spans, highlighting every occurrence of `pat`.
    fn hl_spans(text: &str, style: Style, pat: Option<&str>) -> Vec<Span<'static>> {
        let Some(pat) = pat else {
            return vec![Span::styled(text.to_string(), style)];
        };
        if pat.is_empty() {
            return vec![Span::styled(text.to_string(), style)];
        }
        let low = text.to_lowercase();
        let plow = pat.to_lowercase();
        let mut out = Vec::new();
        let mut start = 0;
        while let Some(pos) = low[start..].find(&plow) {
            let abs = start + pos;
            if abs > start {
                out.push(Span::styled(text[start..abs].to_string(), style));
            }
            out.push(Span::styled(
                text[abs..abs + plow.len()].to_string(),
                style.bg(Color::Rgb(80, 60, 0)),
            ));
            start = abs + plow.len();
        }
        if start < text.len() {
            out.push(Span::styled(text[start..].to_string(), style));
        }
        out
    }

    /// Build one rendered line: horizontal-scroll clipping + syntax/search highlighting.
    fn render_line(&self, i: usize, styles: &[(usize, usize, Style)], hl: Option<&str>) -> Line<'static> {
        let Some(line) = self.lines.get(i) else {
            return Line::default();
        };
        let lstart = self.line_starts.get(i).copied().unwrap_or(0);
        let selected = i == self.scroller.cursor;
        let base = if selected {
            Style::default().bg(Color::Rgb(40, 40, 60))
        } else {
            Style::default()
        };
        // dim rz-ghidra warning lines
        let default_fg = if line.trim_start().starts_with("// WARNING:") {
            base.fg(Color::DarkGray)
        } else {
            base.fg(Color::Gray)
        };
        // Visible byte range within this line
        let hw = self.hscroll;
        let vw = self.viewport_width;
        let line_len = line.len();
        let vis_start = hw.min(line_len);
        let vis_end = (hw + vw).min(line_len);
        if vis_start >= vis_end {
            return Line::from(Span::styled(String::new(), base));
        }
        let vis = &line[vis_start..vis_end];
        let vis_abs_start = lstart + vis_start;
        let vis_abs_end = lstart + vis_end;
        let mut spans: Vec<Span> = Vec::new();
        let mut cursor_b = vis_abs_start;
        for &(s, e, st) in styles {
            let s = s.max(vis_abs_start);
            let e = e.min(vis_abs_end);
            if s >= e || s < cursor_b {
                continue;
            }
            if s > cursor_b {
                spans.extend(Self::hl_spans(
                    &line[cursor_b - lstart..s - lstart],
                    default_fg,
                    hl,
                ));
            }
            spans.extend(Self::hl_spans(&line[s - lstart..e - lstart], base.patch(st), hl));
            cursor_b = e;
        }
        if cursor_b < vis_abs_end {
            spans.extend(Self::hl_spans(
                &line[cursor_b - lstart..vis_end],
                default_fg,
                hl,
            ));
        }
        if spans.is_empty() {
            spans.push(Span::styled(vis.to_string(), base));
        }
        Line::from(spans)
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect, focused: bool, title_fn: &str) {
        self.scroller.height = area.height.saturating_sub(2) as usize;
        self.viewport_width = area.width.saturating_sub(2) as usize; // minus borders
        self.scroller.ensure_visible();
        self.sync_hscroll();
        let border_style = if focused {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(format!(" Decompiler — {title_fn} "));

        // No code yet: show the status notice (decompiling…, analyzing…, error).
        if self.lines.is_empty() {
            let msg = self.notice.clone().unwrap_or_default();
            let para = Paragraph::new(Line::from(Span::styled(
                msg,
                Style::default().fg(Color::DarkGray).italic(),
            )))
            .centered()
            .block(block);
            frame.render_widget(para, area);
            return;
        }
        let first = self.scroller.scroll;
        let last = (first + self.scroller.height).min(self.lines.len());
        let (from_b, to_b) = if self.lines.is_empty() {
            (0, 0)
        } else {
            let last_len = self.lines.get(last - 1).map_or(0, String::len);
            (
                self.line_starts.get(first).copied().unwrap_or(0),
                self.line_starts.get(last - 1).copied().unwrap_or(0) + last_len,
            )
        };
        let styles = self.style_map(from_b, to_b);

        let hl = self.search_highlight.as_deref();
        let out: Vec<Line> = (first..last)
            .map(|i| self.render_line(i, &styles, hl))
            .collect();
        frame.render_widget(Paragraph::new(out).block(block), area);
        // draw a column cursor when focused
        if focused {
            if let Some(line) = self.lines.get(self.scroller.cursor) {
                let row = u16::try_from(self.scroller.cursor.saturating_sub(self.scroller.scroll))
                    .unwrap_or(u16::MAX);
                let col_abs = self.col.min(line.len().saturating_sub(1));
                let col = i32::try_from(col_abs).unwrap_or(i32::MAX)
                    - i32::try_from(self.hscroll).unwrap_or(i32::MAX);
                let x = area.x + 1 + u16::try_from(col.max(0)).unwrap_or(u16::MAX);
                let y = area.y + 1 + row;
                if col >= 0 && x < area.right() - 1 && y < area.bottom() - 1 {
                    frame.set_cursor_position((x, y));
                }
            }
        }
    }

    /// Extract the contiguous word (alphanumeric + `_` + `.`) at or nearest to byte `col`.
    fn extract_word_at(s: &str, col: usize) -> Option<String> {
        let bytes = s.as_bytes();
        let len = bytes.len();
        if len == 0 {
            return None;
        }
        let col = col.min(len.saturating_sub(1));
        let is_wc = |c: u8| c.is_ascii_alphanumeric() || c == b'_' || c == b'.';
        let at = |i: usize| bytes.get(i).copied().is_some_and(is_wc);
        let start_col = (col..len)
            .find(|&i| at(i))
            .or_else(|| (0..col).rev().find(|&i| at(i)))?;
        let mut start = start_col;
        while start > 0 && at(start - 1) {
            start -= 1;
        }
        let mut end = start_col;
        while end < len && at(end) {
            end += 1;
        }
        s.get(start..end).map(ToString::to_string)
    }
}
