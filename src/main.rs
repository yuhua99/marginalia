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

use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use clap::{Parser, Subcommand};

use repo::Repo;
use session::{Input, Session};
use store::Store;

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
        /// Shell command run on quitting the TUI, with $MRG_SESSION set
        #[arg(long)]
        on_quit: Option<String>,
    },
    /// Add comments from a JSON file (`-` for stdin) to a session
    Comment {
        #[arg(long)]
        session: String,
        file: PathBuf,
    },
    /// Open a session in the TUI
    Resume {
        #[arg(long)]
        session: String,
        /// Replaces the saved on-quit hook
        #[arg(long)]
        on_quit: Option<String>,
    },
    /// Print a session as JSON
    Show {
        #[arg(long)]
        session: String,
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
            tui(store, repo, None)
        }
        Some(Cmd::New { range, on_quit }) => {
            let (base, end) = repo.resolve(&range)?;
            let session = Session::new(store.new_id(), range, base, end, on_quit);
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
            session.apply(input, &mut repo)?;
            store.save(&session)
        }
        Some(Cmd::Resume { session, on_quit }) => {
            need_terminal()?;
            let mut session = store.load(&session)?;
            repo.verify(&session.base, &session.end)?;
            if on_quit.is_some() {
                session.on_quit = on_quit;
                store.save(&session)?;
            }
            tui(store, repo, Some(session))
        }
        Some(Cmd::Show { session }) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&store.load(&session)?.view(&mut repo)?)?
            );
            Ok(())
        }
        Some(Cmd::List) => {
            let list: Vec<_> = store
                .list()?
                .into_iter()
                .map(|(s, updated)| {
                    serde_json::json!({
                        "id": s.id,
                        "range": s.range,
                        "updated": updated,
                        "open": s.open_count(),
                        "waiting_on_user": s.waiting_on_user_count(),
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
/// `Q` in the review does the same and goes back to the picker.
fn tui(mut store: Store, mut repo: Repo, mut session: Option<Session>) -> Result<()> {
    let settings = settings::Settings::load()?;
    let (mut theme, mut wrap) = (theme::find(&settings.theme), settings.wrap);
    loop {
        let ctx = ratatui::run(|terminal| -> Result<Option<app::Ctx>> {
            let session = match session {
                Some(session) => session,
                None => {
                    match ui::picker::run(terminal, &store, &mut repo, &mut theme, &mut wrap)? {
                        Some(session) => session,
                        None => return Ok(None),
                    }
                }
            };
            let mut ctx = app::Ctx::new(session, repo, store, theme, wrap)?;
            app::run(terminal, &mut ctx)?;
            Ok(Some(ctx))
        })?;
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
        if let Some(cmd) = &s.on_quit {
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
        (store, repo, session, theme, wrap) = (ctx.store, ctx.repo, None, ctx.theme, ctx.wrap);
    }
}
