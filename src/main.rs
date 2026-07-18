//! vizin — vim-style TUI disassembler/decompiler on rizin + Ghidra (rz-ghidra).

mod app;
mod backend;
mod cmdline;
mod command;
mod decompiler;
mod pipe;
mod ts;
mod ui;
mod views;
mod vim;

use anyhow::Result;
use app::App;
use backend::Backend;
use clap::Parser;
use ratatui::crossterm::event::{self, Event};
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;
use ratatui::DefaultTerminal;
use std::time::Duration;

#[derive(Parser)]
#[command(name = "vizin", version, about = "Decompiler-powered binary viewer with vim keys")]
struct Args {
    /// Binary to analyze
    file: String,
    /// Open in write mode (enables patching)
    #[arg(short = 'w', long)]
    write: bool,
    /// Load/save annotations from a rizin project file (.rzdb)
    #[arg(short = 'p', long)]
    project: Option<String>,
    /// Skip auto-analysis (aaa)
    #[arg(long)]
    no_analysis: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();
    if !std::path::Path::new(&args.file).exists() {
        anyhow::bail!("no such file: {}", args.file);
    }

    let mut terminal = ratatui::init();
    let result = run(&mut terminal, &args);
    ratatui::restore();
    result
}

fn splash(terminal: &mut DefaultTerminal, msg: &str) {
    let msg = msg.to_string();
    let _ = terminal.draw(|f| {
        let area = f.area();
        let line = Line::from(Span::styled(msg, Style::default().fg(Color::Cyan).bold()))
            .centered();
        f.render_widget(
            Paragraph::new(line),
            Rect {
                x: 0,
                y: area.height / 2,
                width: area.width,
                height: 1,
            },
        );
    });
}

fn run(terminal: &mut DefaultTerminal, args: &Args) -> Result<()> {
    splash(terminal, &format!("vizin ▸ opening {} …", args.file));
    let mut backend = Backend::open(&args.file, args.write, args.project.as_deref())?;
    if !args.no_analysis {
        splash(terminal, "vizin ▸ analyzing (aaa) — this can take a moment …");
        backend.analyze()?;
    }

    let mut app = App::new(backend, args.project.clone());

    let mut redraw = true;
    loop {
        if redraw {
            app.spinner = app.spinner.wrapping_add(1);
            app.prepare(terminal.size()?.width);
            terminal.draw(|f| app.draw(f))?;
            redraw = false;
        }
        // While a decompile is pending, wake often to animate the spinner and
        // pick up the result; otherwise idle until the next key.
        let timeout = if app.decomp_waiting() {
            Duration::from_millis(120)
        } else {
            Duration::from_millis(500)
        };
        if event::poll(timeout)? {
            match event::read()? {
                Event::Key(key) => {
                    app.on_key(key);
                    redraw = true;
                }
                Event::Resize(_, _) => redraw = true,
                _ => {}
            }
        }
        if app.poll_decomp() || app.decomp_waiting() {
            redraw = true;
        }
        if app.quit {
            return Ok(());
        }
    }
}
