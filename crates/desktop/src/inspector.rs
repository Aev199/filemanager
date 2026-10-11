use crate::*;

impl Explorer {
    pub(crate) fn load_selected_thumbnail(&mut self, cx: &mut Context<Self>) {
        if self.inspector_thumbnail_busy || self.inspector_thumbnail.is_some() {
            return;
        }
        let Some(path) = self.selected.clone().filter(|p| wants_shell_thumbnail(p)) else {
            return;
        };
        let request = self.inspector_request;
        self.inspector_thumbnail_busy = true;
        let selected = path.clone();
        let task = cx.background_spawn(async move {
            std::env::current_exe().and_then(|exe| {
                filemanager_core::preview_host::isolated_thumbnail(&exe, &path, 320)
            })
        });
        cx.spawn(async move |weak, cx| {
            let result = task.await;
            let _ = weak.update(cx, |this, cx| {
                this.inspector_thumbnail_busy = false;
                if this.inspector_request != request || this.selected.as_ref() != Some(&selected) {
                    this.load_selected_thumbnail(cx);
                    return;
                }
                this.inspector_thumbnail = result.ok();
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn open_document_preview(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self.preview_launch_busy {
            return;
        }
        self.document_preview.take();
        self.preview_launch_busy = true;
        self.preview_launch_generation = self.preview_launch_generation.wrapping_add(1);
        let generation = self.preview_launch_generation;
        self.status = "Открывается предпросмотр документа…".into();
        let task = cx.background_spawn(async move {
            std::env::current_exe().and_then(|exe| {
                filemanager_core::preview_host::PreviewProcess::document(&exe, &path)
            })
        });
        cx.spawn(async move |weak, cx| {
            let result = task.await;
            let _ = weak.update(cx, |this, cx| {
                this.preview_launch_busy = false;
                if this.preview_launch_generation != generation {
                    return;
                }
                match result {
                    Ok(child) => {
                        this.document_preview = Some(child);
                        this.status =
                            "Предпросмотр открыт в отдельном окне. Его можно закрыть здесь.".into();
                    }
                    Err(error) => this.status = format!("Предпросмотр недоступен: {error}"),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    pub(crate) fn close_document_preview(&mut self, cx: &mut Context<Self>) {
        self.preview_launch_generation = self.preview_launch_generation.wrapping_add(1);
        self.document_preview.take();
        self.status = "Предпросмотр закрыт".into();
        cx.notify();
    }
}
