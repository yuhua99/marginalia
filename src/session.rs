use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::repo::Repo;

#[derive(Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    /// The range as given by the user, for display.
    pub range: String,
    pub base: String,
    pub end: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_quit: Option<String>,
    pub threads: Vec<Thread>,
}

#[derive(Serialize, Deserialize)]
pub struct Thread {
    pub id: String,
    pub status: Status,
    pub anchor: Anchor,
    /// The anchored lines when the thread was created, for `lines` anchors.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    pub messages: Vec<Message>,
}

#[derive(Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Open,
    Resolved,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Anchor {
    Lines {
        /// Defaults to the range end when omitted in input.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rev: Option<String>,
        file: String,
        start: u32,
        end: u32,
    },
    File {
        file: String,
    },
    Commit {
        commit: String,
    },
    Session,
}

#[derive(Serialize, Deserialize)]
pub struct Message {
    pub from: String,
    pub body: String,
}

/// The `mrg comment` input format.
#[derive(Deserialize)]
pub struct Input {
    pub from: String,
    pub comments: Vec<NewComment>,
}

#[derive(Deserialize)]
pub struct NewComment {
    #[serde(default)]
    pub reply_to: Option<String>,
    #[serde(default)]
    pub anchor: Option<Anchor>,
    pub body: String,
}

impl Session {
    pub fn new(
        id: String,
        range: String,
        base: String,
        end: String,
        on_quit: Option<String>,
    ) -> Session {
        Session {
            id,
            range,
            base,
            end,
            on_quit,
            threads: Vec::new(),
        }
    }

    pub fn apply(&mut self, input: Input, repo: &mut Repo) -> Result<()> {
        for c in input.comments {
            match (c.reply_to, c.anchor) {
                (Some(id), None) => self.reply(&id, &input.from, c.body)?,
                (None, Some(anchor)) => self.comment(anchor, &input.from, c.body, repo)?,
                _ => bail!("each comment needs exactly one of reply_to or anchor"),
            }
        }
        Ok(())
    }

    pub fn comment(
        &mut self,
        mut anchor: Anchor,
        from: &str,
        body: String,
        repo: &mut Repo,
    ) -> Result<()> {
        let mut text = None;
        if let Anchor::Lines {
            rev,
            file,
            start,
            end,
        } = &mut anchor
        {
            if *start == 0 || start > end {
                bail!("invalid line range {start}-{end}");
            }
            let rev = rev.get_or_insert_with(|| self.end.clone());
            // Commits become shas; only the range end and the worktree states stay symbolic.
            if *rev != self.end && !matches!(rev.as_str(), "staged" | "unstaged") {
                *rev = repo.commit(rev)?;
            }
            text = Some(
                snippet(repo, rev, file, *start, *end)?
                    .with_context(|| format!("{file}:{start}-{end} not found at {rev}"))?,
            );
        }
        if let Anchor::Commit { commit } = &mut anchor {
            *commit = repo.commit(commit)?;
        }
        self.threads.push(Thread {
            id: format!("t{}", self.threads.len() + 1),
            status: Status::Open,
            anchor,
            text,
            messages: vec![Message {
                from: from.to_string(),
                body,
            }],
        });
        Ok(())
    }

    /// Replying reopens a resolved thread.
    pub fn reply(&mut self, id: &str, from: &str, body: String) -> Result<()> {
        let Some(t) = self.threads.iter_mut().find(|t| t.id == id) else {
            bail!("no thread {id}");
        };
        t.status = Status::Open;
        t.messages.push(Message {
            from: from.to_string(),
            body,
        });
        Ok(())
    }

    pub fn open_count(&self) -> usize {
        self.threads
            .iter()
            .filter(|t| t.status == Status::Open)
            .count()
    }

    pub fn waiting_on_user_count(&self) -> usize {
        self.threads.iter().filter(|t| t.waiting_on_user()).count()
    }

    /// The session as JSON, with derived fields added to each thread.
    /// `waiting_on` is null for resolved threads; `outdated` means the anchored
    /// lines no longer match the stored `text`.
    pub fn view(&self, repo: &mut Repo) -> Result<Value> {
        let mut v = serde_json::to_value(self)?;
        let outdated = self.outdated(repo)?;
        for ((t, tv), outdated) in self
            .threads
            .iter()
            .zip(v["threads"].as_array_mut().expect("threads array"))
            .zip(outdated)
        {
            tv["waiting_on"] = (t.status == Status::Open).then(|| t.waiting_on()).into();
            tv["outdated"] = outdated.into();
        }
        Ok(v)
    }

    /// Per thread: the anchored lines no longer match the stored `text`.
    pub fn outdated(&self, repo: &mut Repo) -> Result<Vec<bool>> {
        self.threads
            .iter()
            .map(|t| match &t.anchor {
                Anchor::Lines {
                    rev: Some(rev),
                    file,
                    start,
                    end,
                } if t.text.is_some() => Ok(snippet(repo, rev, file, *start, *end)? != t.text),
                _ => Ok(false),
            })
            .collect()
    }

    /// Where a thread is anchored, e.g. `src/a.rs:3-5`, plus `@rev` when not the range end.
    pub fn location(&self, anchor: &Anchor) -> String {
        match anchor {
            Anchor::Lines {
                rev,
                file,
                start,
                end,
            } => {
                let mut s = if start == end {
                    format!("{file}:{start}")
                } else {
                    format!("{file}:{start}-{end}")
                };
                if let Some(rev) = rev
                    && *rev != self.end
                {
                    s.push_str(&format!(" @{}", short(rev)));
                }
                s
            }
            Anchor::File { file } => file.clone(),
            Anchor::Commit { commit } => format!("commit {}", short(commit)),
            Anchor::Session => "session".into(),
        }
    }

    /// Open threads waiting on the agent, in `order`, as markdown; None if there are none.
    pub fn markdown(&self, order: &[usize]) -> Option<String> {
        let mut out = format!("# mrg review {} · {}\n", self.id, self.range);
        let mut any = false;
        for t in order.iter().map(|&i| &self.threads[i]) {
            if t.status != Status::Open || t.waiting_on() != "agent" {
                continue;
            }
            any = true;
            out.push_str(&format!("\n## {} · {}\n", t.id, self.location(&t.anchor)));
            for m in &t.messages {
                out.push_str(&format!(
                    "- **{}**: {}\n",
                    m.from,
                    m.body.replace('\n', "\n  ")
                ));
            }
        }
        any.then_some(out)
    }
}

impl Thread {
    /// The last speaker being `user` means the agent should answer, otherwise the user.
    pub fn waiting_on_user(&self) -> bool {
        self.status == Status::Open && self.waiting_on() == "user"
    }

    pub fn waiting_on(&self) -> &'static str {
        match self.messages.last() {
            Some(m) if m.from == "user" => "agent",
            _ => "user",
        }
    }
}

/// A commit sha shortened for display; other revs as is.
fn short(rev: &str) -> &str {
    if rev.len() >= 40 && rev.bytes().all(|b| b.is_ascii_hexdigit()) {
        &rev[..7]
    } else {
        rev
    }
}

/// Lines `start..=end` of `file` at `rev`; None if the file or lines are absent.
fn snippet(repo: &mut Repo, rev: &str, file: &str, start: u32, end: u32) -> Result<Option<String>> {
    let Some(content) = repo.read(rev, file)? else {
        return Ok(None);
    };
    let lines: Vec<&str> = content.lines().collect();
    Ok(lines
        .get(start as usize - 1..end as usize)
        .map(|l| l.join("\n")))
}
