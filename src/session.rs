use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

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
    pub messages: Vec<Message>,
}

#[derive(Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Open,
    Resolved,
}

#[derive(Serialize, Deserialize)]
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
    File { file: String },
    Commit { commit: String },
    Session,
}

#[derive(Serialize, Deserialize)]
pub struct Message {
    pub from: String,
    pub body: String,
}

/// The `--comments` file format.
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
    pub fn new(id: String, range: String, base: String, end: String, on_quit: Option<String>) -> Session {
        Session { id, range, base, end, on_quit, threads: Vec::new() }
    }

    pub fn apply(&mut self, input: Input) -> Result<()> {
        for c in input.comments {
            match (c.reply_to, c.anchor) {
                (Some(id), None) => self.reply(&id, &input.from, c.body)?,
                (None, Some(anchor)) => self.comment(anchor, &input.from, c.body)?,
                _ => bail!("each comment needs exactly one of reply_to or anchor"),
            }
        }
        Ok(())
    }

    pub fn comment(&mut self, mut anchor: Anchor, from: &str, body: String) -> Result<()> {
        if let Anchor::Lines { rev, start, end, .. } = &mut anchor {
            if *start == 0 || start > end {
                bail!("invalid line range {start}-{end}");
            }
            rev.get_or_insert_with(|| self.end.clone());
        }
        self.threads.push(Thread {
            id: format!("t{}", self.threads.len() + 1),
            status: Status::Open,
            anchor,
            messages: vec![Message { from: from.to_string(), body }],
        });
        Ok(())
    }

    /// Replying reopens a resolved thread.
    pub fn reply(&mut self, id: &str, from: &str, body: String) -> Result<()> {
        let Some(t) = self.threads.iter_mut().find(|t| t.id == id) else {
            bail!("no thread {id}");
        };
        t.status = Status::Open;
        t.messages.push(Message { from: from.to_string(), body });
        Ok(())
    }

    pub fn open_count(&self) -> usize {
        self.threads.iter().filter(|t| t.status == Status::Open).count()
    }

    pub fn waiting_on_user_count(&self) -> usize {
        self.threads
            .iter()
            .filter(|t| t.status == Status::Open && t.waiting_on() == "user")
            .count()
    }

    /// The session as JSON, with derived fields added to each thread.
    /// `waiting_on` is null for resolved threads.
    pub fn view(&self) -> Value {
        let mut v = serde_json::to_value(self).expect("session serializes");
        for (t, tv) in self.threads.iter().zip(v["threads"].as_array_mut().expect("threads array")) {
            tv["waiting_on"] = (t.status == Status::Open).then(|| t.waiting_on()).into();
        }
        v
    }
}

impl Thread {
    /// The last speaker being `user` means the agent should answer, otherwise the user.
    pub fn waiting_on(&self) -> &'static str {
        match self.messages.last() {
            Some(m) if m.from == "user" => "agent",
            _ => "user",
        }
    }
}
