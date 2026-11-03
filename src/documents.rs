use crate::{AppWindow, I18n, SqlTab, tr};
use slint::{ComponentHandle, ModelRc, SharedString, StandardListViewItem, TableColumn, VecModel};
use std::{cell::RefCell, path::PathBuf, rc::Rc};

struct Document {
    id: i32,
    path: Option<PathBuf>,
    text: String,
    saved: String,
    name: Option<String>,
    results: Option<ResultView>,
}
struct ResultView {
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
        Self { names: ui.get_result_names(), columns: ui.get_result_columns(), rows: ui.get_result_rows(), index: ui.get_result_index(), page: ui.get_page(), pages: ui.get_pages(), count: ui.get_row_count() }
    }
    fn restore(&self, ui: &AppWindow) {
        ui.set_result_names(self.names.clone());
        ui.set_result_columns(self.columns.clone());
        ui.set_result_rows(self.rows.clone());
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
    }
    fn dirty(&self) -> bool {
        self.text != self.saved
    }
}
#[derive(Clone, Copy)]
enum Close {
    Tab(i32),
    Window,
}
pub struct Documents {
    tabs: Vec<Document>,
    active: usize,
    next_id: i32,
    pending: Option<Close>,
    prompt_id: Option<i32>,
}
impl Documents {
    fn sync(&self, ui: &AppWindow) {
        let tabs = self
            .tabs
            .iter()
            .map(|doc| SqlTab {
                id: doc.id,
                title: doc.title().into(),
                dirty: doc.dirty(),
            })
            .collect::<Vec<_>>();
        ui.set_sql_tabs(Rc::new(VecModel::from(tabs)).into());
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
            results: None,
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
        let id = match self.pending {
            Some(Close::Tab(id)) => Some(id),
            Some(Close::Window) => self.tabs.iter().find(|doc| doc.dirty()).map(|doc| doc.id),
            None => return,
        };
        let Some(id) = id else {
            self.pending = None;
            let _ = ui.hide();
            return;
        };
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
        match self.pending {
            Some(Close::Window) => self.prompt(ui),
            _ => {
                self.pending = None;
            }
        }
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
            results: None,
        }],
        active: 0,
        next_id: 2,
        pending: None,
        prompt_id: None,
    }));
    state.borrow().sync(ui);
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
                docs.pending = Some(Close::Tab(id));
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
            docs.pending = Some(Close::Window);
            docs.prompt(&ui);
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
            results: None,
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
