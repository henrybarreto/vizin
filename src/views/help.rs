//! Static in-app keybinding cheatsheet, opened with `?`.

use super::Scroller;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

/// One row of the cheatsheet: a section heading (rendered bold, no key column)
/// or a `key -> action` pair.
enum Row {
    Heading(&'static str),
    Bind(&'static str, &'static str),
}

#[allow(clippy::module_name_repetitions)] // matches sibling `XrefsPopup` naming
pub struct HelpPopup {
    rows: Vec<Row>,
    pub scroller: Scroller,
}

impl HelpPopup {
    pub fn new() -> Self {
        Self {
            rows: Self::help_rows(),
            scroller: Scroller::default(),
        }
    }

    pub const fn len(&self) -> usize {
        self.rows.len()
    }

    pub const fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        let w = (area.width * 2 / 3).clamp(50, 70).min(area.width);
        let h = (area.height * 3 / 4).clamp(10, 26).min(area.height);
        let popup = Rect {
            x: area.x + (area.width.saturating_sub(w)) / 2,
            y: area.y + (area.height.saturating_sub(h)) / 2,
            width: w,
            height: h,
        };
        self.scroller.height = popup.height.saturating_sub(2) as usize;
        self.scroller.ensure_visible();
        let lines: Vec<Line> = self
            .rows
            .iter()
            .skip(self.scroller.scroll)
            .take(self.scroller.height)
            .map(|row| match row {
                Row::Heading(title) => Line::from(Span::styled(
                    (*title).to_string(),
                    Style::default().fg(Color::Yellow).bold(),
                )),
                Row::Bind(key, action) => Line::from(vec![
                    Span::styled(format!("  {key:<20}"), Style::default().fg(Color::Cyan)),
                    Span::styled((*action).to_string(), Style::default().fg(Color::Gray)),
                ]),
            })
            .collect();
        frame.render_widget(Clear, popup);
        let para = Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan))
                .title(" Keybindings — q/Esc/? to close "),
        );
        frame.render_widget(para, popup);
    }

    fn help_rows() -> Vec<Row> {
        vec![
            Row::Heading("Motion"),
            Row::Bind("h j k l / arrows", "left / down / up / right"),
            Row::Bind("{count}j", "repeat motion (e.g. 12j)"),
            Row::Bind("gg / G", "top / bottom"),
            Row::Bind("Ctrl-d / Ctrl-u", "half page down / up"),
            Row::Bind("Ctrl-f / Ctrl-b", "page down / up"),
            Row::Bind("H / M / L", "screen top / middle / bottom"),
            Row::Bind("zz zt zb", "center / top / bottom cursor in screen"),
            Row::Bind("w b e", "word next / prev / end (decompiler only)"),
            Row::Bind("f{char} F{char}", "find char forward / back (decompiler only)"),
            Row::Bind("Space", "toggle listing <-> decompiler"),
            Row::Bind("Tab", "cycle focus: sidebar -> listing -> decompiler"),
            Row::Heading("Navigation"),
            Row::Bind("Enter / gd", "follow call/jump/symbol under cursor"),
            Row::Bind("Ctrl-o / Ctrl-i", "jump back / forward (jumplist)"),
            Row::Bind("x / X", "xrefs to / from here (popup)"),
            Row::Bind("[{ / ]}", "prev / next { } block (decompiler only)"),
            Row::Bind("%", "jump to matching bracket (decompiler only)"),
            Row::Bind("* / #", "search forward / back for word under cursor"),
            Row::Bind("/pat  n  N", "search current view, next / previous"),
            Row::Bind("K", "hover popup: dec/hex/oct/bin/char/string + comment"),
            Row::Heading("Views / Panes"),
            Row::Bind("Ctrl-w h / l / w", "focus sidebar / main / cycle focus"),
            Row::Bind(":fn", "focus function list (/ filters)"),
            Row::Bind(":str :imp :exp :seg", "strings / imports / exports / segments"),
            Row::Bind(":hex", "hex view"),
            Row::Bind("q", "close popup/panel"),
            Row::Heading("Editing (persisted via rizin projects)"),
            Row::Bind("r", "rename function / variable / label under cursor"),
            Row::Bind(";", "add / edit comment at current address"),
            Row::Bind("i", "(hex view) insert mode; Esc commits the patch"),
            Row::Bind(":w [file.rzdb]", "save renames/comments to a project file"),
            Row::Bind(":oo+", "reopen file in write mode"),
            Row::Bind(":q / :q!", "quit (:q warns on unsaved changes)"),
        ]
    }
}

impl Default for HelpPopup {
    fn default() -> Self {
        Self::new()
    }
}
