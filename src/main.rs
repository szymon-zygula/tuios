mod app;
mod config;
mod input;
mod picker;
mod ui;
mod usos;

use std::time::Duration;

use anyhow::{Result, bail};
use ratatui::DefaultTerminal;
use ratatui::crossterm::event;
use tokio::sync::mpsc;

use app::{Action, App, Msg};
use usos::Usos;

const USAGE: &str = "\
usage: tuios [--mock]

Grade USOS course tests (Sprawdziany) from the terminal. The first start walks you
through getting a USOS API key and signing in.

  --mock   try the interface on random made-up data, kept only in memory; nothing
           connects to USOS
";

#[tokio::main]
async fn main() -> Result<()> {
    let mut mock = false;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--mock" => mock = true,
            "-h" | "--help" => {
                print!("{USAGE}");
                return Ok(());
            }
            other => bail!("unknown argument {other}\n\n{USAGE}"),
        }
    }
    let settings = if mock { None } else { config::load_settings()? };
    let usos = if mock {
        Some(Usos::mock())
    } else {
        settings
            .clone()
            .map(|s| Usos::real(s, config::load_token()))
    };

    let mut terminal = ratatui::init();
    let result = run(&mut terminal, usos, settings).await;
    ratatui::restore();
    result
}

async fn run(
    terminal: &mut DefaultTerminal,
    usos: Option<Usos>,
    settings: Option<config::Settings>,
) -> Result<()> {
    let (tx, mut rx) = mpsc::unbounded_channel();
    spawn_input(tx.clone());
    let mut app = App::new(usos, settings, tx);
    let mut tick = tokio::time::interval(Duration::from_millis(80));
    loop {
        terminal.draw(|f| ui::draw(f, &mut app))?;
        let msg = tokio::select! {
            m = rx.recv() => match m { Some(m) => m, None => break },
            _ = tick.tick() => Msg::Tick,
        };
        if let Action::Quit = app.handle(msg) {
            break;
        }
    }
    Ok(())
}

/// Reads terminal events on a plain thread so slow USOS calls never delay typing.
fn spawn_input(tx: mpsc::UnboundedSender<Msg>) {
    std::thread::spawn(move || {
        while let Ok(ev) = event::read() {
            if tx.send(Msg::Input(ev)).is_err() {
                break;
            }
        }
    });
}
