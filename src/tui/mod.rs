// The TUI's terminal loop. [i] leaves the TUI so `poenv init` can ask its questions on the plain
// terminal, then reopens it.
mod app;
#[cfg(test)]
mod tests;

use crate::commands::{Ctx, init};
use crate::errors::{Res, code};
use crate::project::find_project;
use crate::store::open_store;
use app::App;
use crossterm::event::{self, Event, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use std::io::stdout;

/// Opens the TUI (people only: values can be revealed in it).
pub fn run(ctx: &Ctx) -> Res<i32> {
    loop {
        let store = open_store()?;
        let project = find_project(&ctx.cwd)?;
        let root = project.as_ref().map(|p| p.root.clone());
        let mut app = App::new(store, project, &ctx.cwd, true);
        session(&mut app)?;
        if !app.want_init {
            return Ok(code::OK);
        }
        let cwd = root.map(Into::into).unwrap_or_else(|| ctx.cwd.clone());
        init::run(&Ctx { cwd, json: false, human: true }, &[])?;
    }
}

fn session(app: &mut App) -> Res<()> {
    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen)?;
    let result = (|| -> Res<()> {
        let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;
        while !app.quit {
            terminal.draw(|f| app.render(f))?;
            if let Event::Key(k) = event::read()? {
                if k.kind != KeyEventKind::Release {
                    app.handle(k);
                }
            }
        }
        Ok(())
    })();
    let _ = execute!(stdout(), LeaveAlternateScreen);
    let _ = disable_raw_mode();
    result
}
