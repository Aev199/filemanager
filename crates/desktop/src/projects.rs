use std::{io, path::{Path, PathBuf}, time::Duration};
use gpui::{AppContext, Context, Focusable, Window};
use filemanager_core::panes::{GroupId, Workspace};
use crate::Explorer;
#[derive(Clone)]
pub(crate) struct FavoriteDrag { pub index: usize }
#[derive(Clone)]
pub(crate) struct PlaceDrag { pub index: usize }
/// Visible «Места» in the user's order, one path per line.
pub(crate) const PLACES: &str = "places";

/// The standard places in the saved order; places missing from the saved
/// list are hidden. Without a saved list every place shows.
pub(crate) fn arrange_places(all: &[filemanager_core::places::Place], layout: Option<&str>) -> Vec<filemanager_core::places::Place> {
    let Some(layout) = layout else { return all.to_vec() };
    layout.lines().filter_map(|line| all.iter().find(|place| place.path == Path::new(line)).cloned()).collect()
}
impl Explorer {
    pub(crate) fn prepare_window_close(&mut self,cx: &mut Context<Self>) -> bool {
        match self.persist_workspace() {
            Ok(()) => { self.close_document_preview(cx); true }
            Err(error) => { self.close_error=Some(error.to_string()); cx.notify(); false }
        }
    }
    fn session_writable(&self) -> io::Result<()> {
        if let Some(error)=&self.session_restore_error {
            return Err(io::Error::other(format!("Сеанс не восстановлен: {error}. Сохранение отключено, чтобы не потерять прежний сеанс. Перезапустите приложение после устранения ошибки.")));
        }
        Ok(())
    }
    pub(crate) fn persist_workspace(&self) -> io::Result<()> {
        self.save_session(&self.browser,&[])
    }
    /// Writes `next` with the projects it closed or replaced; the window
    /// changes only after the write succeeded.
    fn save_session(&self,next: &Workspace,outgoing: &[(String,Workspace)]) -> io::Result<()> {
        self.session_writable()?;
        let store=self.workspaces.as_ref().ok_or_else(||io::Error::other("База проектов недоступна"))?;
        store.save_session(next,outgoing)
    }
    fn commit_session(&mut self,next: Workspace,outgoing: Vec<(String,Workspace)>,cx: &mut Context<Self>) -> io::Result<()> {
        self.save_session(&next,&outgoing)?;
        self.browser=next;self.reset_project_view(cx);self.refresh_workspace_names();
        Ok(())
    }
    pub(crate) fn start_project_autosave(&mut self,cx: &mut Context<Self>) {
        cx.spawn(async move |weak,cx| {loop {
            cx.background_spawn(async {std::thread::sleep(Duration::from_secs(2));}).await;
            if weak.update(cx,|this,cx| {
                if this.session_restore_error.is_some() {return;}
                let state=format!("{:?}",this.browser);
                if state!=this.saved_layout { match this.persist_workspace() {
                    Ok(())=>this.saved_layout=state,
                    Err(error)=>{this.status=format!("Сеанс не сохранён: {error}");cx.notify();}
                }}
            }).is_err() {break;}
        }}).detach();
    }
    fn reset_project_view(&mut self,cx: &mut Context<Self>) {
        self.pane_measured_widths.clear();self.column_scroll_handles.clear();self.column_reveal_keys.clear();
        self.column_sizes.clear();self.pane_bounds.clear();self.tab_scroll.clear();self.tab_reveal.clear();
        self.scroll_handles.clear();self.folder_views.clear();self.column_resize=None;self.dock_hover=None;
        self.marquee=None;self.anchor=None;self.address_editing=None;self.select_first_when_loaded=false;
        self.selected=None;self.marked.clear();self.confirm_recycle=None;self.selected_history_event=None;
        self.context_menu=None;self.clear_filter=true;self.close_search();self.close_document_preview(cx);self.inspector_path=None;
    }
    /// Opens project `name` in the active pane; the other panes stay. A
    /// project already open in some pane is focused there instead.
    pub(crate) fn restore_workspace(&mut self,name: &str,cx: &mut Context<Self>) {
        if let Some(group)=self.browser.project_group(name) {self.focus_group(group,cx);return;}
        let result=self.workspaces.as_ref().ok_or_else(||io::Error::other("База проектов недоступна"))
            .and_then(|store|store.load_project(name)?.ok_or_else(||io::Error::other("Проект не найден")))
            .and_then(|content| {
                let mut next=self.browser.clone();
                let outgoing=next.open_project(next.active,name,&content)?;
                self.commit_session(next,outgoing.into_iter().collect(),cx)
            });
        match result {Ok(())=>self.status.clear(),Err(e)=>self.status=format!("Проект не открыт: {e}")}
        cx.notify();
    }
    /// Returns pane `group` to the tabs it showed before its project.
    pub(crate) fn close_project(&mut self,group: GroupId,cx: &mut Context<Self>) {
        let mut next=self.browser.clone();
        let Some(closed)=next.close_project(group) else {return};
        match self.commit_session(next,vec![closed],cx) {Ok(())=>self.status.clear(),Err(e)=>self.status=format!("Проект не закрыт: {e}")}
        cx.notify();
    }
    /// The active pane's tabs become a new project.
    pub(crate) fn save_named_workspace(&mut self,cx: &mut Context<Self>) {
        let name=self.workspace_input.read(cx).value().trim().to_string();
        let result=if !filemanager_core::projects::valid_name(&name) {Err(io::Error::other("Введите другое имя проекта"))}
            else if self.workspace_names.iter().any(|old|old==&name) {Err(io::Error::other("Проект с таким именем уже есть"))}
            else {
                let mut next=self.browser.clone();
                next.start_project(next.active,&name).and_then(|outgoing| {
                    self.save_session(&next,&outgoing.into_iter().collect::<Vec<_>>())?;
                    self.browser=next;self.refresh_workspace_names();Ok(())
                })
            };
        match result {Ok(())=>{self.saving_workspace=false;self.project_error=None;self.status.clear();},
            Err(e)=>self.project_error=Some(e.to_string())}
        cx.notify();
    }
    pub(crate) fn delete_workspace(&mut self,name: &str,cx: &mut Context<Self>) {
        if let Err(error)=self.session_writable() {self.status=error.to_string();cx.notify();return;}
        if let Some(group)=self.browser.project_group(name) {
            self.close_project(group,cx);
            if self.browser.project_group(name).is_some() {return;}
        }
        if let Some(store)=&self.workspaces {if let Err(e)=store.delete(name) {self.status=format!("Проект не удалён: {e}");}}
        self.refresh_workspace_names();cx.notify();
    }
    pub(crate) fn refresh_workspace_names(&mut self) {
        self.workspace_names=self.workspaces.as_ref().and_then(|store|store.list().ok()).unwrap_or_default()
            .into_iter().filter(|name|filemanager_core::projects::valid_name(name)).collect();
    }
    pub(crate) fn begin_save_workspace(&mut self,window: &mut Window,cx: &mut Context<Self>) {
        self.saving_workspace=true;self.project_error=None;
        let suggestion=self.browser.active().title.clone();
        self.workspace_input.update(cx,|input,cx|input.set_value(suggestion,window,cx));
        window.focus(&self.workspace_input.focus_handle(cx),cx);cx.notify();
    }
    fn set_favorites(&mut self,paths: Vec<PathBuf>,cx: &mut Context<Self>) {
        let result=self.workspaces.as_ref().ok_or_else(||io::Error::other("База избранного недоступна"))
            .and_then(|store|store.save_favorites(&paths));
        match result {Ok(())=>self.favorites=paths,Err(e)=>self.status=format!("Избранное не сохранено: {e}")}
        self.context_menu=None;cx.notify();
    }
    pub(crate) fn toggle_favorite(&mut self,path: &Path,cx: &mut Context<Self>) {
        let mut paths=self.favorites.clone();
        if let Some(index)=paths.iter().position(|p|p==path) {paths.remove(index);} else {paths.push(path.to_path_buf());}
        self.set_favorites(paths,cx);
    }
    pub(crate) fn move_favorite(&mut self,from: usize,to: usize,cx: &mut Context<Self>) {
        if from>=self.favorites.len() || to>=self.favorites.len() || from==to {return;}
        let mut paths=self.favorites.clone();let path=paths.remove(from);paths.insert(to,path);self.set_favorites(paths,cx);
    }
    fn save_places(&mut self,visible: Vec<PathBuf>,cx: &mut Context<Self>) {
        let layout=visible.iter().map(|p|p.to_string_lossy().into_owned()).collect::<Vec<_>>().join("\n");
        let result=self.workspaces.as_ref().ok_or_else(||io::Error::other("База настроек недоступна"))
            .and_then(|store|store.set_value(PLACES,&layout));
        match result {Ok(())=>self.places=arrange_places(&self.all_places,Some(&layout)),Err(e)=>self.status=format!("«Места» не сохранены: {e}")}
        self.context_menu=None;cx.notify();
    }
    fn visible_places(&self) -> Vec<PathBuf> { self.places.iter().map(|p|p.path.clone()).collect() }
    pub(crate) fn hide_place(&mut self,index: usize,cx: &mut Context<Self>) {
        let mut visible=self.visible_places();
        if index<visible.len() {visible.remove(index);}
        self.save_places(visible,cx);
    }
    pub(crate) fn move_place(&mut self,from: usize,to: usize,cx: &mut Context<Self>) {
        let mut visible=self.visible_places();
        if from>=visible.len() || to>=visible.len() || from==to {return;}
        let path=visible.remove(from);visible.insert(to,path);self.save_places(visible,cx);
    }
    /// Hidden places come back at the end, in their standard order.
    pub(crate) fn show_all_places(&mut self,cx: &mut Context<Self>) {
        let mut visible=self.visible_places();
        for place in &self.all_places { if !visible.contains(&place.path) {visible.push(place.path.clone());} }
        self.save_places(visible,cx);
    }
    pub(crate) fn hidden_places(&self) -> usize { self.all_places.len().saturating_sub(self.places.len()) }
}
