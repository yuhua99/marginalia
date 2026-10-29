mod app;
mod doc;
mod highlight;
mod keymap;
mod repo;
mod session;
mod settings;
mod store;
mod theme;
mod ui;

use std::io::{IsTerminal, Read, Write};
use std::path::PathBuf;
use std::process::Command;

use ratatui::crossterm::event::{
    DisableBracketedPaste, EnableBracketedPaste, KeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::style::Print;

use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use clap::{Parser, Subcommand};

use repo::Repo;
use session::{Input, Session};
use store::Store;

/// Mouse reporting of clicks and the wheel in SGR, and its end.
const MOUSE_ON: &str = "\x1b[?1000h\x1b[?1006h";
const MOUSE_OFF: &str = "\x1b[?1000l\x1b[?1006l";

#[derive(Parser)]
#[command(
    name = "mrg",
    about = "Review code changes and discuss them with agents"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create a session for RANGE ([A]..<commit>|HEAD|staged|unstaged; no A starts before the root) and print its id
    New {
        range: String,
        #[arg(long)]
        name: Option<String>,
    },
    /// Add comments from a JSON file (`-` for stdin) to a session; prints {id, location} per comment
    Comment {
        #[arg(long)]
        session: String,
        file: PathBuf,
    },
    /// Open a session in the TUI
    Resume {
        #[arg(long)]
        session: String,
        /// Shell command run on quitting this time, with $MRG_SESSION set; not saved
        #[arg(long)]
        on_quit: Option<String>,
    },
    /// Delete notes: threads only the agent wrote in
    Delete {
        #[arg(long)]
        session: String,
        #[arg(required = true)]
        threads: Vec<String>,
    },
    /// Print a session as JSON
    Show {
        #[arg(long)]
        session: String,
        /// Only threads waiting on the agent, or those whose anchored lines changed since
        #[arg(long, value_parser = ["waiting", "outdated"])]
        only: Option<String>,
    },
    /// List sessions of the current repo as JSON
    List,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut repo = Repo::open()?;
    let store = Store::open(&repo.key())?;
    match cli.cmd {
        None => {
            need_terminal()?;
            tui(store, repo, None, None)
        }
        Some(Cmd::New { range, name }) => {
            let (base, end) = repo.resolve(&range)?;
            let mut session = Session::new(store.new_id(), range, base, end);
            if let Some(name) = name {
                session.set_name(&name)?;
            }
            store.save(&session)?;
            println!("{}", session.id);
            Ok(())
        }
        Some(Cmd::Comment { session, file }) => {
            let mut session = store.load(&session)?;
            let mut text = String::new();
            if file.as_os_str() == "-" {
                std::io::stdin()
                    .read_to_string(&mut text)
                    .context("read stdin")?;
            } else {
                text = std::fs::read_to_string(&file)
                    .with_context(|| format!("read {}", file.display()))?;
            }
            let input: Input = serde_json::from_str(&text).context("parse comments")?;
            let touched = session.apply(input, &mut repo)?;
            store.save(&session)?;
            for i in touched {
                let t = &session.threads[i];
                let location = session.location(&t.anchor);
                println!(
                    "{}",
                    serde_json::json!({ "id": t.id, "location": location })
                );
            }
            Ok(())
        }
        Some(Cmd::Resume { session, on_quit }) => {
            need_terminal()?;
            let session = store.load(&session)?;
            repo.verify(&session.base, &session.end)?;
            tui(store, repo, Some(session), on_quit)
        }
        Some(Cmd::Delete { session, threads }) => {
            let mut session = store.load(&session)?;
            session.delete_notes(&threads)?;
            store.save(&session)
        }
        Some(Cmd::Show { session, only }) => {
            let mut session = store.load(&session)?;
            session.check_outdated(&mut repo)?;
            match only.as_deref() {
                Some("waiting") => session.threads.retain(|t| t.waiting_on_agent()),
                Some("outdated") => session.threads.retain(|t| t.outdated),
                _ => {}
            }
            println!("{}", serde_json::to_string_pretty(&session.view()?)?);
            Ok(())
        }
        Some(Cmd::List) => {
            let list: Vec<_> = store
                .list()?
                .into_iter()
                .map(|(s, updated)| {
                    serde_json::json!({
                        "id": s.id,
                        "name": s.name,
                        "range": s.range,
                        "updated": updated,
                        "waiting_on_agent": s.waiting_on_agent_count(),
                    })
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&list)?);
            Ok(())
        }
    }
}

fn need_terminal() -> Result<()> {
    if !std::io::stdout().is_terminal() {
        bail!("mrg needs a terminal");
    }
    Ok(())
}

/// Opens `session` in the TUI, or the picker first when None; then copies or runs the hook.
/// `Q` in the review does the same and goes back to the picker. `on_quit` is for the
/// first session only.
fn tui(
    mut store: Store,
    mut repo: Repo,
    mut session: Option<Session>,
    mut on_quit: Option<String>,
) -> Result<()> {
    let settings = settings::Settings::load()?;
    keymap::init(&settings.keys)?;
    let mut look = app::Look {
        theme: theme::find(&settings.theme),
        wrap: settings.wrap,
    };
    // A panic leaves through ratatui's hook, which knows nothing of the mouse or paste.
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(std::io::stdout(), DisableBracketedPaste, Print(MOUSE_OFF));
        hook(info);
    }));
    loop {
        let ctx = ratatui::run(|terminal| -> Result<Option<app::Ctx>> {
            // Pasted newlines must not send; Shift-Enter must differ from Enter. The key
            // flags end with the alternate screen; bracketed paste is turned off below.
            // The mouse reports clicks and the wheel (1000) in SGR (1006), not motion;
            // Shift-drag still selects text in most terminals.
            execute!(
                std::io::stdout(),
                EnableBracketedPaste,
                Print(MOUSE_ON),
                PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
            )?;
            let (session, store, repo, look) = match session {
                Some(session) => (session, store, repo, look),
                None => {
                    let mut pick = ui::picker::Pick::new(store, repo, look)?;
                    ui::picker::run(terminal, &mut pick)?;
                    let Some(session) = pick.open else {
                        return Ok(None);
                    };
                    (session, pick.store, pick.repo, pick.look)
                }
            };
            let mut ctx = app::Ctx::new(session, repo, store, look)?;
            ctx.on_quit = on_quit.take();
            app::run(terminal, &mut ctx)?;
            Ok(Some(ctx))
        });
        execute!(std::io::stdout(), DisableBracketedPaste, Print(MOUSE_OFF))?;
        let ctx = ctx?;
        let Some(ctx) = ctx else {
            return Ok(());
        };
        let s = &ctx.session;
        if ctx.copy
            && let Some(md) = s.markdown(&ctx.doc.thread_order())
        {
            // OSC 52: the terminal sets its clipboard, which outlives this process.
            print!("\x1b]52;c;{}\x07", STANDARD.encode(md));
            std::io::stdout().flush()?;
            eprintln!("sent threads waiting on the agent to the terminal clipboard (OSC 52)");
        }
        if let Some(cmd) = &ctx.on_quit {
            let status = Command::new("sh")
                .arg("-c")
                .arg(cmd)
                .env("MRG_SESSION", &s.id)
                .status()
                .context("run on-quit hook")?;
            if !status.success() {
                bail!("on-quit hook failed: {status}");
            }
        }
        if !ctx.back {
            return Ok(());
        }
        (store, repo, session, look) = (ctx.store, ctx.repo, None, ctx.look);
    }
}
