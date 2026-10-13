//! Project transitions commit before replacing the visible workspace.
use std::io;
use crate::{panes::Workspace, workspace::WorkspaceStore};

#[derive(Clone, Debug, Default)]
pub struct ProjectSession {
    pub name: Option<String>,
    previous: Option<Workspace>,
}
impl ProjectSession {
    pub fn valid_name(name: &str) -> bool {
        !name.trim().is_empty() && name.chars().count() <= 100
            && name != "Default" && name != "__project_return__"
    }
    pub fn restore(store: &WorkspaceStore) -> io::Result<Option<(Workspace, Self)>> {
        let name = store.session_project()?;
        let Some(mut current) = store.load_inner("Default", true)? else { return Ok(None); };
        let previous = store.load_inner("__project_return__", true)?;
        if let Some(project) = name.as_deref() {
            if let Some(saved) = store.load_project(project)? { current = saved; }
        }
        let name = name.filter(|_|previous.is_some());
        Ok(Some((current, Self {name, previous})))
    }
    pub fn checkpoint(&self, store: &WorkspaceStore, current: &Workspace) -> io::Result<()> {
        store.checkpoint(current, self.name.as_deref(), self.previous.as_ref(), None)
    }
    pub fn open(&mut self, store: &WorkspaceStore, name: &str, current: &mut Workspace) -> io::Result<()> {
        if self.name.as_deref() == Some(name) { return Ok(()); }
        if !Self::valid_name(name) { return Err(io::Error::other("Недопустимое имя проекта")); }
        let next = store.load_project(name)?.ok_or_else(||io::Error::other("Проект не найден"))?;
        let previous = self.previous.as_ref().unwrap_or(current);
        store.checkpoint(&next, Some(name), Some(previous), self.name.as_deref().map(|old|(old,&*current)))?;
        if self.previous.is_none() { self.previous = Some(current.clone()); }
        self.name = Some(name.to_owned()); *current = next; Ok(())
    }
    pub fn create(&mut self, store: &WorkspaceStore, name: &str, current: &Workspace) -> io::Result<()> {
        if !Self::valid_name(name) { return Err(io::Error::other("Введите другое имя проекта")); }
        if store.list()?.iter().any(|old|old==name) { return Err(io::Error::other("Проект с таким именем уже есть")); }
        let previous = self.previous.as_ref().unwrap_or(current);
        store.checkpoint(current,Some(name),Some(previous),self.name.as_deref().map(|old|(old,current)))?;
        if self.previous.is_none() { self.previous = Some(current.clone()); }
        self.name = Some(name.to_owned()); Ok(())
    }
    pub fn close(&mut self, store: &WorkspaceStore, current: &mut Workspace) -> io::Result<()> {
        let (Some(name), Some(previous)) = (&self.name, &self.previous) else { return Ok(()); };
        store.checkpoint(previous,None,None,Some((name,current)))?;
        *current = self.previous.take().unwrap(); self.name = None; Ok(())
    }
}
