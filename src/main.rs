mod app;
mod doc;
mod keymap;
mod repo;
mod session;
mod store;
mod ui;

use std::io::Write;
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
    /// Create a session for RANGE (A..<commit>|HEAD|staged|unstaged) and open the TUI
    Start {
        range: String,
        /// JSON file with comments to add
        #[arg(long)]
        comments: Option<PathBuf>,
        /// Shell command run on quit, with $MRG_SESSION set
        #[arg(long)]
        on_quit: Option<String>,
    },
    /// Reopen an existing session
    Resume {
        #[arg(long)]
        session: String,
        #[arg(long)]
        comments: Option<PathBuf>,
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
        None => bail!("`mrg` without arguments is not implemented yet; use `mrg start RANGE`"),
        Some(Cmd::Start {
            range,
            comments,
            on_quit,
        }) => {
            let (base, end) = repo.resolve(&range)?;
            let session = Session::new(store.new_id(), range, base, end, on_quit);
            open(store, repo, session, comments)
        }
        Some(Cmd::Resume {
            session,
            comments,
            on_quit,
        }) => {
            let mut session = store.load(&session)?;
            repo.verify(&session.base, &session.end)?;
            if on_quit.is_some() {
                session.on_quit = on_quit;
            }
            open(store, repo, session, comments)
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

fn open(
    store: Store,
    mut repo: Repo,
    mut session: Session,
    comments: Option<PathBuf>,
) -> Result<()> {
    if let Some(path) = comments {
        let text =
            std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        let input: Input =
            serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
        session.apply(input, &mut repo)?;
    }
    store.save(&session)?;
    let mut ctx = app::Ctx::new(session, repo, store)?;
    app::run(&mut ctx)?;
    let session = &ctx.session;
    if ctx.copy
        && let Some(md) = session.markdown(&ctx.doc.thread_order())
    {
        // OSC 52: the terminal sets its clipboard, which outlives this process.
        print!("\x1b]52;c;{}\x07", STANDARD.encode(md));
        std::io::stdout().flush()?;
        eprintln!("sent threads waiting on the agent to the terminal clipboard (OSC 52)");
    }
    if let Some(cmd) = &session.on_quit {
        let status = Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .env("MRG_SESSION", &session.id)
            .status()
            .context("run on-quit hook")?;
        if !status.success() {
            bail!("on-quit hook failed: {status}");
        }
    }
    Ok(())
}
