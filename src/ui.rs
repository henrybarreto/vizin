//! Layout: function sidebar | main pane(s), status bar, command line, popups.

use crate::app::{App, Focus, MainView, ValuePopup};
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

impl App {
    pub fn draw(&mut self, frame: &mut Frame) {
        let area = frame.area();
        let rows = Layout::vertical([
            Constraint::Min(3),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(area);
        let &[row_main, row_status, row_cmd] = rows.as_ref() else {
            return;
        };

        let cols =
            Layout::horizontal([Constraint::Length(32), Constraint::Min(20)]).split(row_main);
        let &[col_sidebar, col_main] = cols.as_ref() else {
            return;
        };
        let sidebar_focus = self.focus == Focus::Sidebar && self.popup.is_none();
        self.funcs.render(frame, col_sidebar, sidebar_focus);

        let main_focus = self.focus == Focus::Main && self.popup.is_none();
        let fn_name = self.current_fn_name();

        // Pass the search pattern to views for highlighting.
        let hl = if self.search_pattern.is_empty() {
            None
        } else {
            Some(self.search_pattern.clone())
        };
        self.listing.search_highlight = hl.clone();
        self.decomp.search_highlight = hl;

        let mut anchor_rect = col_main;
        match self.main_view {
            MainView::Panel => {
                if let Some(p) = self.panel.as_mut() {
                    p.render(frame, col_main, main_focus);
                }
            }
            MainView::Hex => {
                if let Some(h) = self.hex.as_mut() {
                    h.render(frame, col_main, main_focus);
                }
            }
            MainView::Listing | MainView::Decomp => {
                if self.dual_pane {
                    let panes = Layout::horizontal([
                        Constraint::Percentage(50),
                        Constraint::Percentage(50),
                    ])
                    .split(col_main);
                    if let &[pane_left, pane_right] = panes.as_ref() {
                        self.listing.render(
                            frame,
                            pane_left,
                            main_focus && self.main_view == MainView::Listing,
                            &fn_name,
                        );
                        self.decomp.render(
                            frame,
                            pane_right,
                            main_focus && self.main_view == MainView::Decomp,
                            &fn_name,
                        );
                        anchor_rect = if self.main_view == MainView::Listing {
                            pane_left
                        } else {
                            pane_right
                        };
                    }
                } else if self.main_view == MainView::Listing {
                    self.listing.render(frame, col_main, main_focus, &fn_name);
                } else {
                    self.decomp.render(frame, col_main, main_focus, &fn_name);
                }
            }
        }

        if let Some(popup) = self.popup.as_mut() {
            popup.render(frame, row_main);
        }

        if let Some(help) = self.help_popup.as_mut() {
            help.render(frame, row_main);
        }

        if let Some(cp) = self.completion_popup.as_mut() {
            cp.render(frame, area, row_cmd.y);
        }

        if let Some(vp) = &self.value_popup {
            Self::draw_value_popup(frame, anchor_rect, vp);
        }

        self.draw_status(frame, row_status, &fn_name);
        self.draw_cmdline(frame, row_cmd);
    }

    /// Small LSP-hover-style floating box anchored at the cursor's row in the
    /// current pane, showing dec/hex/oct/bin for whatever `K` was pressed on.
    fn draw_value_popup(frame: &mut Frame, pane: Rect, vp: &ValuePopup) {
        let max_len = vp.lines.iter().map(|l| l.chars().count()).max().unwrap_or(0);
        let width = u16::try_from(max_len.saturating_add(2))
            .unwrap_or(u16::MAX)
            .clamp(10, pane.width.saturating_sub(2));
        let height = u16::try_from(vp.lines.len()).unwrap_or(u16::MAX).saturating_add(2);
        // cursor row is 1 below the pane's top border; prefer popping below it,
        // fall back to above if there's no room at the bottom of the pane.
        let cursor_row = pane.y + 1 + u16::try_from(vp.row).unwrap_or(0);
        let y = if cursor_row + 1 + height <= pane.y + pane.height {
            cursor_row + 1
        } else {
            cursor_row.saturating_sub(height)
        };
        let x = (pane.x + 2).min(pane.x + pane.width.saturating_sub(width));
        let popup = Rect {
            x,
            y: y.clamp(pane.y, pane.y + pane.height.saturating_sub(height.min(pane.height))),
            width: width.min(pane.width),
            height: height.min(pane.height),
        };
        let lines: Vec<Line> = vp
            .lines
            .iter()
            .enumerate()
            .map(|(i, l)| {
                let style = if i == 0 {
                    Style::default().fg(Color::White).bold()
                } else {
                    Style::default().fg(Color::Gray)
                };
                Line::from(Span::styled(l.clone(), style))
            })
            .collect();
        ratatui::widgets::Clear.render(popup, frame.buffer_mut());
        let para = ratatui::widgets::Paragraph::new(lines).block(
            ratatui::widgets::Block::default()
                .borders(ratatui::widgets::Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan)),
        );
        frame.render_widget(para, popup);
    }

    fn draw_status(&self, frame: &mut Frame, area: Rect, fn_name: &str) {
        let mode = if self.hex.as_ref().is_some_and(|h| h.editing) {
            ("INSERT", Color::LightRed)
        } else if self.input.is_some() {
            ("CMD", Color::Yellow)
        } else {
            ("NORMAL", Color::Green)
        };
        let rw = if self.backend.writable { "RW" } else { "RO" };
        let dirty = if self.dirty { " [+]" } else { "" };
        let line = Line::from(vec![
            Span::styled(
                format!(" {} ", mode.0),
                Style::default().bg(mode.1).fg(Color::Black).bold(),
            ),
            Span::styled(
                format!(" {} ", self.backend.file),
                Style::default().fg(Color::White).bold(),
            ),
            Span::styled(
                format!("[{} {}{}] ", self.arch, rw, dirty),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(
                format!(" {fn_name} "),
                Style::default().fg(Color::LightBlue),
            ),
            Span::styled(
                format!(" {:#x} ", self.seek),
                Style::default().fg(Color::Yellow),
            ),
        ]);
        let para = Paragraph::new(line).style(Style::default().bg(Color::Rgb(25, 25, 35)));
        frame.render_widget(para, area);
    }

    fn draw_cmdline(&self, frame: &mut Frame, area: Rect) {
        // clippy's map_or_else rewrite here nests the message/default branch
        // inside the "none" closure, which reads worse than this 3-way if-chain.
        #[allow(clippy::option_if_let_else)]
        let line = if let Some(input) = &self.input {
            Line::from(vec![
                Span::styled(input.prompt.clone(), Style::default().fg(Color::Yellow)),
                Span::raw(input.buffer.clone()),
                Span::styled("▏", Style::default().fg(Color::White)),
            ])
        } else if !self.message.is_empty() {
            let color = if self.message_is_error {
                Color::LightRed
            } else {
                Color::Gray
            };
            Line::from(Span::styled(self.message.clone(), Style::default().fg(color)))
        } else {
            Line::default()
        };
        frame.render_widget(Paragraph::new(line), area);
    }
}
