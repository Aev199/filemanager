use std::{io, path::{Path, PathBuf}, time::Duration};
use gpui::{AppContext, Context, Focusable, Window};
use filemanager_core::projects::ProjectSession;
use crate::Explorer;
#[derive(Clone)]
pub(crate) struct FavoriteDrag { pub index: usize }
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
        self.session_writable()?;
        let store=self.workspaces.as_ref().ok_or_else(||io::Error::other("База проектов недоступна"))?;
        self.project.checkpoint(store,&self.browser)
    }
    pub(crate) fn start_project_autosave(&mut self,cx: &mut Context<Self>) {
        cx.spawn(async move |weak,cx| {loop {
            cx.background_spawn(async {std::thread::sleep(Duration::from_secs(2));}).await;
            if weak.update(cx,|this,cx| {
                if this.session_restore_error.is_some() {return;}
                let state=format!("{:?}{:?}",this.project.name,this.browser);
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
    pub(crate) fn restore_workspace(&mut self,name: &str,cx: &mut Context<Self>) {
        let result=self.session_writable().and_then(|_|self.workspaces.as_ref().ok_or_else(||io::Error::other("База проектов недоступна")))
            .and_then(|store|self.project.open(store,name,&mut self.browser));
        match result {Ok(())=>{self.reset_project_view(cx);self.status.clear();},Err(e)=>self.status=format!("Проект не открыт: {e}")}
        cx.notify();
    }
    pub(crate) fn close_project(&mut self,cx: &mut Context<Self>) {
        let result=self.session_writable().and_then(|_|self.workspaces.as_ref().ok_or_else(||io::Error::other("База проектов недоступна")))
            .and_then(|store|self.project.close(store,&mut self.browser));
        match result {Ok(())=>{self.reset_project_view(cx);self.status.clear();},Err(e)=>self.status=format!("Проект не закрыт: {e}")}
        cx.notify();
    }
    pub(crate) fn save_named_workspace(&mut self,cx: &mut Context<Self>) {
        let name=self.workspace_input.read(cx).value().trim().to_string();
        let result=self.session_writable().and_then(|_|self.workspaces.as_ref().ok_or_else(||io::Error::other("База проектов недоступна")))
            .and_then(|store|self.project.create(store,&name,&self.browser));
        match result {Ok(())=>{self.saving_workspace=false;self.project_error=None;self.status.clear();self.refresh_workspace_names();},
            Err(e)=>self.project_error=Some(e.to_string())}
        cx.notify();
    }
    pub(crate) fn delete_workspace(&mut self,name: &str,cx: &mut Context<Self>) {
        if let Err(error)=self.session_writable() {self.status=error.to_string();cx.notify();return;}
        if self.project.name.as_deref()==Some(name) {self.close_project(cx);if self.project.name.is_some() {return;}}
        if let Some(store)=&self.workspaces {if let Err(e)=store.delete(name) {self.status=format!("Проект не удалён: {e}");}}
        self.refresh_workspace_names();cx.notify();
    }
    pub(crate) fn refresh_workspace_names(&mut self) {
        self.workspace_names=self.workspaces.as_ref().and_then(|store|store.list().ok()).unwrap_or_default()
            .into_iter().filter(|name|ProjectSession::valid_name(name)).collect();
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
}
