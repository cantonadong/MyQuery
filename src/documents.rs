use crate::{AppWindow, I18n, SqlTab, tr};
use slint::{ComponentHandle, Model, ModelRc, SharedString, StandardListViewItem, TableColumn, VecModel};
use std::{cell::RefCell, path::PathBuf, rc::Rc};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
struct Document {
    id: i32,
    path: Option<PathBuf>,
    text: String,
    saved: String,
    name: Option<String>,
    #[serde(skip)]
    running: bool,
    #[serde(skip)]
    results: Option<ResultView>,
    #[serde(skip)]
    notice: i32,
    #[serde(skip)]
    history: Vec<(String, ResultView)>,
    #[serde(skip)]
    history_index: Option<usize>,
    #[serde(skip)]
    run_sql: Option<String>,
}
#[derive(Clone)]
struct ResultView {
    captured: std::time::Instant,
    error: SharedString,
    running: bool,
    has_run: bool,
    elapsed: f32,
    phase: i32,
    received: SharedString,
    status: bool,
    selected_row: i32,
    selected_rows: ModelRc<bool>,
    selected_cells: ModelRc<bool>,
    selected_count: i32,
    selection_anchor: i32,
    cell_anchor: i32,
    names: ModelRc<SharedString>,
    columns: ModelRc<TableColumn>,
    rows: ModelRc<ModelRc<StandardListViewItem>>,
    index: i32,
    page: i32,
    pages: i32,
    count: SharedString,
}
impl ResultView {
    fn capture(ui: &AppWindow) -> Self {
        Self { selected_row: ui.get_selected_row(), selected_rows: ui.get_selected_rows(), selected_cells: ui.get_selected_cells(), selected_count: ui.get_selected_count(), selection_anchor: ui.get_selection_anchor(), cell_anchor: ui.get_cell_anchor_column(), status: ui.get_result_status(), captured: std::time::Instant::now(), error: ui.get_query_error(), running: ui.get_query_running(), has_run: ui.get_query_has_run(), elapsed: ui.get_query_elapsed(), phase: ui.get_query_phase(), received: ui.get_query_rows(), names: ui.get_result_names(), columns: ui.get_result_columns(), rows: ui.get_result_rows(), index: ui.get_result_index(), page: ui.get_page(), pages: ui.get_pages(), count: ui.get_row_count() }
    }
    fn restore(&self, ui: &AppWindow) {
        ui.set_query_error(self.error.clone());
        ui.set_query_running(self.running);
        ui.set_query_has_run(self.has_run);
        ui.set_query_elapsed(self.elapsed + if self.running { self.captured.elapsed().as_secs_f32() } else { 0.0 });
        ui.set_query_phase(self.phase);
        ui.set_query_rows(self.received.clone());
        ui.set_selected_row(self.selected_row);
        ui.set_selected_rows(self.selected_rows.clone());
        ui.set_selected_cells(self.selected_cells.clone());
        ui.set_selected_count(self.selected_count);
        ui.set_selection_anchor(self.selection_anchor);
        ui.set_cell_anchor_column(self.cell_anchor);
        ui.set_result_names(self.names.clone());
        ui.set_result_columns(self.columns.clone());
        ui.set_result_rows(self.rows.clone());
        ui.set_result_status(self.status);
        crate::localize_result(ui);
        ui.set_result_index(self.index);
        ui.set_page(self.page);
        ui.set_pages(self.pages);
        ui.set_row_count(self.count.clone());
    }
}
impl Document {
    fn title(&self) -> String {
        if let Some(name) = &self.name { return name.clone(); }
        self.path
            .as_ref()
            .and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| format!("SQL {}", self.id))
    }
    fn show_results(&self, ui: &AppWindow) {
        crate::clear_results(ui);
        if let Some(results) = &self.results { results.restore(ui); }
        ui.set_history_count(self.history.len() as i32);
        ui.set_history_index(self.history_index.map_or(-1, |index| index as i32));
    }
    fn dirty(&self) -> bool {
        self.text != self.saved
    }
}
#[derive(Deserialize, Serialize)]
pub struct Documents {
    tabs: Vec<Document>,
    active: usize,
    next_id: i32,
    #[serde(skip)] pending: Option<i32>,
    #[serde(skip)] prompt_id: Option<i32>,
    #[serde(skip)] last_saved: String,
    #[serde(skip)] persistence_disabled: bool,
    #[serde(skip)] autosave: slint::Timer,
    #[serde(skip)] notification: Option<crate::ResultNotification>,
}
impl Documents {
    fn session_path() -> Result<PathBuf, String> {
        Ok(PathBuf::from(std::env::var_os("LOCALAPPDATA").ok_or("LOCALAPPDATA unavailable")?).join("MyQuery/tabs.json"))
    }
    fn persist(&mut self) -> Result<(), String> {
        use std::io::Write;
        if self.persistence_disabled { return Err("Tab recovery file could not be loaded; original file preserved".into()); }
        let data = serde_json::to_string(self).map_err(|error| error.to_string())?;
        if data == self.last_saved { return Ok(()); }
        let path = Self::session_path()?;
        let dir = path.parent().unwrap();
        std::fs::create_dir_all(dir).map_err(|error| error.to_string())?;
        let mut temp = tempfile::NamedTempFile::new_in(dir).map_err(|error| error.to_string())?;
        temp.write_all(data.as_bytes()).map_err(|error| error.to_string())?;
        temp.as_file().sync_all().map_err(|error| error.to_string())?;
        temp.persist(path).map_err(|error| error.to_string())?;
        self.last_saved = data;
        Ok(())
    }
    fn sync(&self, ui: &AppWindow) {
        let tabs = self
            .tabs
            .iter()
            .map(|doc| SqlTab {
                id: doc.id,
                title: doc.title().into(),
                dirty: doc.dirty(),
                running: doc.running,
                notice: doc.notice,
            })
            .collect::<Vec<_>>();
        let model = ui.get_sql_tabs();
        if model.row_count() == tabs.len() {
            for (index, tab) in tabs.into_iter().enumerate() {
                if model.row_data(index).as_ref() != Some(&tab) { model.set_row_data(index, tab); }
            }
        } else {
            ui.set_sql_tabs(Rc::new(VecModel::from(tabs)).into());
        }
        ui.set_queries_running(self.tabs.iter().any(|doc| doc.running));
        ui.set_active_sql(self.active as i32);
        ui.set_active_document(self.tabs[self.active].id);
        ui.set_document_title(self.tabs[self.active].title().into());
    }
    fn capture(&mut self, ui: &AppWindow) {
        if let Some(doc) = self.tabs.get_mut(self.active) {
            let dirty = doc.dirty();
            doc.text = ui.get_sql().to_string();
            doc.results = Some(ResultView::capture(ui));
            if doc.dirty() != dirty {
                self.sync(ui);
            }
        }
    }
    fn activate(&mut self, ui: &AppWindow, index: usize) {
        if index >= self.tabs.len() {
            return;
        }
        self.active = index;
        self.tabs[index].show_results(ui);
        ui.set_sql(self.tabs[index].text.as_str().into());
        self.sync(ui);
    }
    fn can_add(&self, ui: &AppWindow) -> bool {
        if self.tabs.len() >= 32 {
            ui.set_failed(true);
            ui.set_status(
                tr(
                    ui.global::<I18n>().get_zh(),
                    "最多打开 32 个 SQL 标签页，请先关闭部分标签。",
                    "At most 32 SQL tabs; close a tab first.",
                )
                .into(),
            );
            return false;
        }
        true
    }
    fn add(&mut self, ui: &AppWindow, path: Option<PathBuf>, text: String) {
        if let Some(path) = &path {
            if let Some(index) = self.tabs.iter().position(|doc| {
                doc.path.as_ref().is_some_and(|existing| {
                    existing
                        .to_string_lossy()
                        .eq_ignore_ascii_case(&path.to_string_lossy())
                })
            }) {
                self.activate(ui, index);
                return;
            }
        }
        if !self.can_add(ui) {
            return;
        }
        let id = self.next_id;
        self.next_id += 1;
        self.tabs.push(Document {
            id,
            path,
            saved: text.clone(),
            name: None,
            running: false,
            results: None,
            notice: 0, history: Vec::new(), history_index: None, run_sql: None,
            text,
        });
        self.activate(ui, self.tabs.len() - 1);
    }
    fn remove(&mut self, ui: &AppWindow, id: i32) {
        let Some(index) = self.tabs.iter().position(|doc| doc.id == id) else {
            return;
        };
        if index == self.active {
            crate::clear_results(ui);
        }
        self.tabs.remove(index);
        ui.invoke_document_closed(id);
        if self.tabs.is_empty() {
            self.add(ui, None, String::new());
            return;
        }
        let active = if index < self.active {
            self.active - 1
        } else {
            self.active.min(self.tabs.len() - 1)
        };
        self.activate(ui, active);
    }
    fn prompt(&mut self, ui: &AppWindow) {
        let Some(id) = self.pending else { return; };
        let Some(index) = self.tabs.iter().position(|doc| doc.id == id) else {
            return;
        };
        if !self.tabs[index].dirty() {
            self.remove(ui, id);
            self.pending = None;
            return;
        }
        self.activate(ui, index);
        self.prompt_id = Some(id);
        ui.set_config_open(false);
        ui.set_timeout_open(false);
        ui.set_group_open(false);
        ui.set_save_prompt_title(self.tabs[index].title().into());
        ui.set_save_prompt_open(true);
    }
    fn save(&mut self, ui: &AppWindow, id: i32) {
        if ui.get_document_saving() { return; }
        let Some(doc) = self.tabs.iter().find(|doc| doc.id == id) else {
            return;
        };
        ui.invoke_request_document_save(
            id,
            doc.path
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default()
                .into(),
            doc.text.as_str().into(),
            doc.title().into(),
        );
    }
    fn finish_close(&mut self, ui: &AppWindow, id: i32) {
        self.remove(ui, id);
        self.prompt_id = None;
        self.pending = None;
    }
}

pub fn bind(ui: &AppWindow) -> Rc<RefCell<Documents>> {
    let state = Rc::new(RefCell::new(Documents {
        tabs: vec![Document {
            id: 1,
            path: None,
            text: ui.get_sql().to_string(),
            saved: String::new(),
            name: None,
            running: false,
            results: None,
            notice: 0, history: Vec::new(), history_index: None, run_sql: None,
        }],
        active: 0,
        next_id: 2,
        pending: None,
        prompt_id: None,
        last_saved: String::new(), persistence_disabled: false, autosave: slint::Timer::default(), notification: None,
    }));
    let loaded = Documents::session_path().and_then(|path| {
        match std::fs::read(path) {
            Ok(data) => {
                let docs: Documents = serde_json::from_slice(&data).map_err(|error| error.to_string())?;
                let mut ids = std::collections::HashSet::new();
                if docs.tabs.is_empty() || docs.tabs.len() > 32 || docs.active >= docs.tabs.len()
                    || docs.tabs.iter().any(|doc| doc.id <= 0 || !ids.insert(doc.id) || doc.id >= docs.next_id) {
                    return Err("Invalid saved SQL tabs".into());
                }
                Ok(Some(docs))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    });
    match loaded {
        Ok(Some(docs)) => *state.borrow_mut() = docs,
        Err(error) => { state.borrow_mut().persistence_disabled = true; ui.set_failed(true); ui.set_status(error.into()); }
        _ => {}
    }
    let active = state.borrow().active;
    state.borrow_mut().activate(ui, active);
    let docs = Rc::downgrade(&state);
    let weak = ui.as_weak();
    state.borrow().autosave.start(slint::TimerMode::Repeated, std::time::Duration::from_secs(1), move || {
        if let (Some(docs), Some(ui)) = (docs.upgrade(), weak.upgrade()) {
            let mut docs = docs.borrow_mut();
            docs.capture(&ui);
            if let Err(error) = docs.persist() {
                ui.set_failed(true);
                ui.set_status(format!("{}: {error}", tr(ui.global::<I18n>().get_zh(), "标签恢复文件保存失败", "Cannot save tab recovery file")).into());
            }
        }
    });
    let docs = state.clone();
    let weak = ui.as_weak();
    ui.on_run_submitted(move |sql| {
        if let Some(ui) = weak.upgrade() {
            let mut docs = docs.borrow_mut();
            docs.capture(&ui);
            let active = docs.active;
            let doc = &mut docs.tabs[active];
            if let (Some(index), Some(view)) = (doc.history_index, doc.results.clone()) { doc.history[index].1 = view; }
            doc.run_sql = Some(sql.to_string());
            doc.notice = 0;
            docs.sync(&ui);
        }
    });
    let docs = state.clone();
    let weak = ui.as_weak();
    ui.on_history_selected(move |id, index| {
        if let Some(ui) = weak.upgrade() {
            let mut docs = docs.borrow_mut();
            docs.capture(&ui);
            let Some(tab) = docs.tabs.iter().position(|doc| doc.id == id) else { return; };
            let active = docs.active == tab;
            let doc = &mut docs.tabs[tab];
            if let (Some(current), Some(view)) = (doc.history_index, doc.results.clone()) { doc.history[current].1 = view; }
            let Some((sql, view)) = doc.history.get(index as usize).cloned() else { return; };
            doc.history_index = Some(index as usize);
            doc.text = sql.clone();
            doc.results = Some(view);
            if active {
                doc.show_results(&ui);
                ui.set_failed(false);
                ui.set_status("".into());
                ui.set_sql(sql.into());
            }
            docs.sync(&ui);
        }
    });
    let docs = state.clone();
    ui.on_run_not_executed(move |id| {
        if let Some(doc) = docs.borrow_mut().tabs.iter_mut().find(|doc| doc.id == id) {
            doc.run_sql = None;
            doc.history_index = None;
        }
    });
    let docs = state.clone();
    let weak = ui.as_weak();
    ui.on_acknowledge_query(move || {
        if let Some(ui) = weak.upgrade() {
            let mut docs = docs.borrow_mut();
            let active = docs.active;
            if docs.tabs[active].notice != 0 { docs.tabs[active].notice = 0; docs.sync(&ui); }
        }
    });
    let docs = state.clone();
    ui.on_dismiss_notification(move || {
        if let Some(notification) = docs.borrow_mut().notification.take() { let _ = notification.hide(); }
    });
    let docs = state.clone();
    let weak = ui.as_weak();
    ui.on_query_state_changed(move |id, running| {
        if let Some(ui) = weak.upgrade() {
            let mut docs = docs.borrow_mut();
            if let Some(doc) = docs.tabs.iter_mut().find(|doc| doc.id == id) {
                doc.running = running;
                if !running {
                    let view = ResultView::capture(&ui);
                    doc.notice = if view.phase == 3 { 1 } else { 2 };
                    if let Some(sql) = doc.run_sql.take() {
                        doc.history.push((sql, view));
                        if doc.history.len() > 20 { doc.history.remove(0); }
                        doc.history_index = Some(doc.history.len() - 1);
                    }
                    let status = if ui.get_query_error() == "CANCELLED" { tr(ui.global::<I18n>().get_zh(), "已取消操作", "Operation cancelled") }
                        else if doc.notice == 1 { tr(ui.global::<I18n>().get_zh(), "成功", "Success") }
                        else { tr(ui.global::<I18n>().get_zh(), "错误", "Error") };
                    let message = format!("{} · {status} · {:.2}s", doc.title(), ui.get_query_elapsed());
                    docs.notification = crate::dialogs::notify_result(&ui, &message);
                }
            }
            docs.sync(&ui);
        }
    });
    let docs = state.clone();
    let weak = ui.as_weak();
    ui.on_result_update_begin(move |id| {
        let Some(ui) = weak.upgrade() else { return false; };
        let mut docs = docs.borrow_mut();
        let Some(index) = docs.tabs.iter().position(|doc| doc.id == id) else { return false; };
        let active = docs.active;
        docs.tabs[active].results = Some(ResultView::capture(&ui));
        docs.tabs[index].show_results(&ui);
        true
    });
    let docs = state.clone();
    let weak = ui.as_weak();
    ui.on_result_update_end(move |id| {
        if let Some(ui) = weak.upgrade() {
            let mut docs = docs.borrow_mut();
            if let Some(doc) = docs.tabs.iter_mut().find(|doc| doc.id == id) { doc.results = Some(ResultView::capture(&ui)); }
            docs.tabs[docs.active].show_results(&ui);
        }
    });
    let docs = state.clone();
    let weak = ui.as_weak();
    ui.on_reset_result_views(move || {
        if let Some(ui) = weak.upgrade() {
            for doc in &mut docs.borrow_mut().tabs { doc.results = None; }
            crate::clear_results(&ui);
        }
    });
    let docs = state.clone();
    let weak = ui.as_weak();
    ui.on_rename_sql(move |index, name| {
        if let Some(ui) = weak.upgrade() {
            let name = name.trim();
            if name.is_empty() { return; }
            let mut docs = docs.borrow_mut();
            if let Some(doc) = docs.tabs.get_mut(index as usize) { doc.name = Some(name.to_string()); }
            docs.sync(&ui);
            ui.set_rename_sql_open(false);
        }
    });
    let docs = state.clone();
    let weak = ui.as_weak();
    ui.on_move_sql(move |from, to| {
        if let Some(ui) = weak.upgrade() {
            if ui.get_document_saving() || ui.get_save_prompt_open() { return; }
            let mut docs = docs.borrow_mut();
            let (from, to) = (from as usize, to as usize);
            if from >= docs.tabs.len() || to >= docs.tabs.len() || from == to { return; }
            docs.capture(&ui);
            let active = docs.tabs[docs.active].id;
            let tab = docs.tabs.remove(from);
            docs.tabs.insert(to, tab);
            docs.active = docs.tabs.iter().position(|doc| doc.id == active).unwrap();
            docs.sync(&ui);
        }
    });
    let docs = state.clone();
    let weak = ui.as_weak();
    ui.on_sql_edited(move |_| {
        if let Some(ui) = weak.upgrade() {
            docs.borrow_mut().capture(&ui);
        }
    });
    let docs = state.clone();
    let weak = ui.as_weak();
    ui.on_sql_selected(move |index| {
        if let Some(ui) = weak.upgrade() {
            if ui.get_document_saving() || ui.get_save_prompt_open() {
                return;
            }
            let mut docs = docs.borrow_mut();
            if index == docs.active as i32 { return; }
            docs.capture(&ui);
            docs.activate(&ui, index as usize);
        }
    });
    let docs = state.clone();
    let weak = ui.as_weak();
    ui.on_new_sql(move || {
        if let Some(ui) = weak.upgrade() {
            if ui.get_document_saving() || ui.get_save_prompt_open() {
                return;
            }
            let mut docs = docs.borrow_mut();
            docs.capture(&ui);
            docs.add(&ui, None, String::new());
        }
    });
    let docs = state.clone();
    let weak = ui.as_weak();
    ui.on_close_sql(move |index| {
        if let Some(ui) = weak.upgrade() {
            if ui.get_document_saving() || ui.get_save_prompt_open() {
                return;
            }
            let mut docs = docs.borrow_mut();
            docs.capture(&ui);
            if let Some(doc) = docs.tabs.get(index as usize) {
                let id = doc.id;
                docs.pending = Some(id);
                docs.prompt(&ui);
            }
        }
    });
    let docs = state.clone();
    let weak = ui.as_weak();
    ui.on_close_documents(move || {
        if let Some(ui) = weak.upgrade() {
            let mut docs = docs.borrow_mut();
            docs.capture(&ui);
            match docs.persist() {
                Ok(()) => { let _ = ui.hide(); }
                Err(error) => { ui.set_failed(true); ui.set_status(error.into()); }
            }
        }
    });
    let docs = state.clone();
    let weak = ui.as_weak();
    ui.on_file_loaded(move |path, text| {
        if let Some(ui) = weak.upgrade() {
            let mut docs = docs.borrow_mut();
            docs.capture(&ui);
            docs.add(&ui, Some(PathBuf::from(path.as_str())), text.to_string());
        }
    });
    let docs = state.clone();
    let weak = ui.as_weak();
    ui.on_document_saved(move |id, path, text| {
        if let Some(ui) = weak.upgrade() {
            ui.set_document_saving(false);
            let mut docs = docs.borrow_mut();
            if let Some(doc) = docs.tabs.iter_mut().find(|doc| doc.id == id) {
                doc.path = Some(PathBuf::from(path.as_str()));
                doc.saved = text.to_string();
            }
            docs.sync(&ui);
            if docs.prompt_id == Some(id) {
                docs.finish_close(&ui, id);
            }
        }
    });
    let docs = state.clone();
    let weak = ui.as_weak();
    ui.on_document_save_cancelled(move || {
        if let Some(ui) = weak.upgrade() {
            ui.set_document_saving(false);
            let mut docs = docs.borrow_mut();
            docs.pending = None;
            docs.prompt_id = None;
            ui.set_save_prompt_open(false);
        }
    });
    state
}

pub fn action(ui: &AppWindow, docs: &Rc<RefCell<Documents>>, action: &str) -> bool {
    if action == "import" {
        return !docs.borrow().can_add(ui);
    }
    if !matches!(
        action,
        "save-document" | "prompt-save" | "prompt-discard" | "prompt-cancel"
    ) {
        return false;
    }
    let mut docs = docs.borrow_mut();
    docs.capture(ui);
    match action {
        "save-document" => {
            ui.set_save_prompt_open(false);
            let id = docs.tabs[docs.active].id;
            docs.save(ui, id);
        }
        "prompt-save" => {
            ui.set_save_prompt_open(false);
            if let Some(id) = docs.prompt_id {
                docs.save(ui, id);
            }
        }
        "prompt-discard" => {
            ui.set_save_prompt_open(false);
            if let Some(id) = docs.prompt_id {
                docs.finish_close(ui, id);
            }
        }
        _ => {
            docs.pending = None;
            docs.prompt_id = None;
            ui.set_save_prompt_open(false);
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::Document;
    #[test]
    fn dirty_tracks_the_saved_snapshot_and_undo() {
        let mut doc = Document {
            id: 1,
            path: None,
            text: "SELECT 1;".into(),
            saved: "SELECT 1;".into(),
            name: None,
            running: false,
            results: None,
            notice: 0, history: Vec::new(), history_index: None, run_sql: None,
        };
        assert!(!doc.dirty());
        doc.text = "SELECT 2;".into();
        let snapshot = doc.text.clone();
        doc.text.push_str("\nSELECT 3;");
        doc.saved = snapshot;
        assert!(doc.dirty());
        doc.text = "SELECT 2;".into();
        assert!(!doc.dirty());
    }
}
