use anyhow::Result;

use crate::doc::{Doc, Scope};
use crate::repo::{Commit, Repo};
use crate::session::{Anchor, Session};
use crate::store::Store;

/// A session open for review: its diff, and the threads placed in it.
pub struct Review {
    pub session: Session,
    pub repo: Repo,
    pub store: Store,
    pub doc: Doc,
    /// What the diff can show: each commit, then uncommitted changes.
    pub views: Vec<View>,
    pub shown: Shown,
    /// What `doc` covers, as of when it was loaded: HEAD may move since.
    span: Span,
}

pub enum View {
    /// `C^ C`.
    Commit(Commit),
    /// `staged` (`HEAD staged`) or `unstaged` (`staged unstaged`).
    Uncommitted(&'static str),
}

/// The views the diff shows: the whole range (`base end`), or a run of them as one diff.
#[derive(Clone, Copy, PartialEq)]
pub enum Shown {
    All,
    /// First and last of the `views` (oldest to newest), not all of them.
    Run(usize, usize),
}

impl Shown {
    /// The run `first..=last` of `n` views; every one of them is the whole range.
    pub fn of((first, last): (usize, usize), n: usize) -> Shown {
        if first == 0 && last + 1 >= n {
            Shown::All
        } else {
            Shown::Run(first, last)
        }
    }

    /// First and last of the `n` views shown.
    pub fn bounds(self, n: usize) -> (usize, usize) {
        match self {
            Shown::All => (0, n.saturating_sub(1)),
            Shown::Run(first, last) => (first, last),
        }
    }
}

impl Review {
    pub fn new(mut session: Session, mut repo: Repo, store: Store) -> Result<Review> {
        repo.use_objects(store.objects(&session.id))?;
        session.place(&mut repo)?;
        let mut views: Vec<View> = repo
            .commits(&session.base, &session.end)?
            .into_iter()
            .map(View::Commit)
            .collect();
        let staged = match session.end.as_str() {
            "staged" => true,
            // Left out from the index, and when nothing is staged.
            "unstaged" => session.base != "staged" && repo.uncommitted()?.contains(&"staged"),
            _ => false,
        };
        if staged {
            views.push(View::Uncommitted("staged"));
        }
        if session.end == "unstaged" {
            views.push(View::Uncommitted("unstaged"));
        }
        let (doc, span) = load(&mut repo, &session, &views, Shown::All)?;
        Ok(Review {
            session,
            repo,
            store,
            doc,
            views,
            shown: Shown::All,
            span,
        })
    }

    /// Switches the diff to `shown`.
    pub fn show(&mut self, shown: Shown) -> Result<()> {
        (self.doc, self.span) = load(&mut self.repo, &self.session, &self.views, shown)?;
        self.shown = shown;
        Ok(())
    }

    /// The user's comment on `anchor`. The inner error is the session turning it down,
    /// the outer one saving failing; so for `reply`.
    pub fn comment(&mut self, anchor: Anchor, body: String) -> Result<Result<()>> {
        if let Err(e) = self.session.comment(anchor, "user", body, &mut self.repo) {
            return Ok(Err(e));
        }
        self.threads_changed().map(Ok)
    }

    /// The user's reply to thread `idx`.
    pub fn reply(&mut self, idx: usize, body: String) -> Result<Result<()>> {
        let id = self.session.threads[idx].id.clone();
        if let Err(e) = self.session.reply(&id, "user", body) {
            return Ok(Err(e));
        }
        self.threads_changed().map(Ok)
    }

    /// Rewrites the last message of thread `idx`.
    pub fn edit(&mut self, idx: usize, body: String) -> Result<()> {
        self.session.edit(idx, body);
        self.threads_changed()
    }

    /// Removes the last message of thread `idx`, or the thread when it is the only one.
    pub fn delete(&mut self, idx: usize) -> Result<()> {
        self.session.delete(idx);
        self.threads_changed()
    }

    /// Saves the session and refreshes the thread rows after threads changed.
    fn threads_changed(&mut self) -> Result<()> {
        self.store.save(&self.session)?;
        self.session.place(&mut self.repo)?;
        set_threads(&mut self.repo, &self.session, &self.span, &mut self.doc)
    }
}

/// What the diff of `shown` covers: its sides, and the commit of its views when it is
/// the only one, which the diff shows with its threads.
struct Span {
    left: String,
    right: String,
    scope: Scope,
    commit: Option<String>,
}

fn span(repo: &Repo, session: &Session, views: &[View], shown: Shown) -> Result<Span> {
    let (left, right, scope) = match shown {
        Shown::All => (session.base.clone(), session.end.clone(), Scope::Range),
        Shown::Run(first, last) => (parent(repo, &views[first])?, rev(&views[last]), Scope::Run),
    };
    let (first, last) = shown.bounds(views.len());
    let commits: Vec<String> = views
        .get(first..=last)
        .unwrap_or_default()
        .iter()
        .filter_map(|v| match v {
            View::Commit(c) => Some(c.sha.clone()),
            View::Uncommitted(_) => None,
        })
        .collect();
    let commit = (commits.len() == 1).then(|| commits[0].clone());
    Ok(Span {
        left,
        right,
        scope,
        commit,
    })
}

/// The diff of `shown`, from the left side of its first view to the right side of the
/// last, and what it covers.
fn load(repo: &mut Repo, session: &Session, views: &[View], shown: Shown) -> Result<(Doc, Span)> {
    let span = span(repo, session, views, shown)?;
    let files = repo.diff(&span.left, &span.right, 3)?;
    let commit = match &span.commit {
        Some(c) => Some((c.clone(), repo.message(c)?)),
        None => None,
    };
    let mut doc = Doc::new(
        span.left.clone(),
        span.right.clone(),
        files,
        span.scope,
        commit,
    );
    set_threads(repo, session, &span, &mut doc)?;
    Ok((doc, span))
}

/// Puts the session's threads where they are in `doc`, the diff of `span`.
fn set_threads(repo: &mut Repo, session: &Session, span: &Span, doc: &mut Doc) -> Result<()> {
    let places = session.locate(repo, &span.left, &span.right, span.commit.as_slice())?;
    doc.set_threads(&session.threads, places);
    Ok(())
}

/// The left side of `view`.
fn parent(repo: &Repo, view: &View) -> Result<String> {
    match view {
        View::Commit(c) => match repo.parent(&c.sha)? {
            Some(parent) => Ok(parent),
            None => repo.empty_tree(),
        },
        View::Uncommitted("staged") => repo.commit("HEAD"),
        View::Uncommitted(_) => Ok("staged".to_string()),
    }
}

/// The right side of `view`.
fn rev(view: &View) -> String {
    match view {
        View::Commit(c) => c.sha.clone(),
        View::Uncommitted(name) => name.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_view_is_all() {
        assert!(Shown::of((0, 3), 4) == Shown::All);
        assert!(Shown::of((0, 0), 1) == Shown::All);
        assert!(Shown::of((1, 3), 4) == Shown::Run(1, 3));
        assert!(Shown::of((0, 2), 4) == Shown::Run(0, 2));
    }
}
