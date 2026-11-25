use anyhow::Result;

use crate::doc::{Doc, Scope};
use crate::repo::{Commit, Repo};
use crate::session::{self, Anchor, Session, Source, Thread};
use crate::store::Store;

pub struct Review {
    session: Session,
    pub repo: Repo,
    pub store: Store,
    pub doc: Doc,
    views: Vec<View>,
    shown: Shown,
    span: Span,
}

pub enum View {
    Commit(Commit),
    Uncommitted(&'static str),
}

#[derive(Clone, Copy, PartialEq)]
pub enum Shown {
    All,
    Run(usize, usize),
}

impl Shown {
    pub fn of((first, last): (usize, usize), n: usize) -> Shown {
        if first == 0 && last + 1 >= n {
            Shown::All
        } else {
            Shown::Run(first, last)
        }
    }

    pub fn bounds(self, n: usize) -> (usize, usize) {
        match self {
            Shown::All => (0, n.saturating_sub(1)),
            Shown::Run(first, last) => (first, last),
        }
    }
}

impl Review {
    pub fn new(session: Session, mut repo: Repo, store: Store) -> Result<Review> {
        repo.refresh();
        let mut views: Vec<View> = repo
            .commits(&session.base, &session.end)?
            .into_iter()
            .map(View::Commit)
            .collect();
        let staged = match session.end.as_str() {
            "staged" => true,
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

    pub fn session(&self) -> &Session {
        &self.session
    }

    pub fn views(&self) -> &[View] {
        &self.views
    }

    pub fn shown(&self) -> Shown {
        self.shown
    }

    pub fn doc_with_threads(&mut self) -> (&mut Doc, &[Thread]) {
        (&mut self.doc, &self.session.threads)
    }

    pub fn show(&mut self, shown: Shown) -> Result<()> {
        (self.doc, self.span) = load(&mut self.repo, &self.session, &self.views, shown)?;
        self.shown = shown;
        Ok(())
    }

    pub fn comment(&mut self, anchor: Anchor, body: String) -> Result<Result<()>> {
        let (first, last) = self.shown.bounds(self.views.len());
        let views = self
            .views
            .get(first..=last)
            .unwrap_or_default()
            .iter()
            .map(|v| Ok((parent(&self.repo, v)?, rev(v), commit(v))))
            .collect::<Result<Vec<_>>>()?;
        let anchor = session::attribute(&mut self.repo, anchor, &views)?;
        if let Err(e) = self.session.comment(anchor, "user", body, &mut self.repo) {
            return Ok(Err(e));
        }
        self.threads_changed().map(Ok)
    }

    pub fn reply(&mut self, idx: usize, body: String) -> Result<Result<()>> {
        let id = self.session.threads[idx].id.clone();
        if let Err(e) = self.session.reply(&id, "user", body) {
            return Ok(Err(e));
        }
        self.threads_changed().map(Ok)
    }

    pub fn edit(&mut self, idx: usize, body: String) -> Result<()> {
        self.session.edit(idx, body);
        self.threads_changed()
    }

    pub fn delete(&mut self, idx: usize) -> Result<()> {
        self.session.delete(idx);
        self.threads_changed()
    }

    fn threads_changed(&mut self) -> Result<()> {
        self.store.save(&self.session)?;
        set_threads(&mut self.repo, &self.session, &self.span, &mut self.doc)
    }
}

struct Span {
    left: String,
    right: String,
    scope: Scope,
    commits: Vec<String>,
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
        .filter_map(commit)
        .collect();
    let commit = (commits.len() == 1).then(|| commits[0].clone());
    Ok(Span {
        left,
        right,
        scope,
        commits,
        commit,
    })
}

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

fn set_threads(repo: &mut Repo, session: &Session, span: &Span, doc: &mut Doc) -> Result<()> {
    let places = session.locate(repo, &span.left, &span.right, &span.commits)?;
    doc.set_threads(&session.threads, places);
    Ok(())
}

fn parent(repo: &Repo, view: &View) -> Result<String> {
    match view {
        View::Commit(c) => repo.left(&c.sha),
        View::Uncommitted("staged") => repo.commit("HEAD"),
        View::Uncommitted(_) => Ok("staged".to_string()),
    }
}

fn rev(view: &View) -> String {
    match view {
        View::Commit(c) => c.sha.clone(),
        View::Uncommitted(name) => name.to_string(),
    }
}

fn commit(view: &View) -> Option<String> {
    match view {
        View::Commit(c) => Some(c.sha.clone()),
        View::Uncommitted(_) => None,
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
