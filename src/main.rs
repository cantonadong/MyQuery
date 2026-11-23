#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod cache;
mod completion;
mod documents;
mod dialogs;
mod syntax;

use myquery_core::{config, db, results};
use config::{Profile, Settings};
use results::{Cells, PAGE_SIZE, ResultSet};
use slint::{ComponentHandle, Model, ModelRc, SharedString, StandardListViewItem, TableColumn, VecModel};
use std::{
    future::Future,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

slint::include_modules!();

fn tr(zh: bool, cn: &str, en: &str) -> String {
    if zh { cn.into() } else { en.into() }
}
fn sql_file_name(name: &str) -> String {
    let mut name: String = name.chars().map(|c| if c.is_control() || r#"<>:"/\|?*"#.contains(c) { '_' } else { c }).collect();
    if !name.to_ascii_lowercase().ends_with(".sql") { name.push_str(".sql"); }
    name
}
fn strings(values: Vec<String>) -> ModelRc<SharedString> {
    std::rc::Rc::new(VecModel::from(
        values.into_iter().map(Into::into).collect::<Vec<_>>(),
    ))
    .into()
}
fn items(values: Vec<String>) -> ModelRc<StandardListViewItem> {
    std::rc::Rc::new(VecModel::from(
        values
            .into_iter()
            .map(|text| StandardListViewItem::from(SharedString::from(text)))
            .collect::<Vec<_>>(),
    ))
    .into()
}
fn update(ui: &slint::Weak<AppWindow>, f: impl FnOnce(AppWindow) + Send + 'static) {
    let _ = ui.upgrade_in_event_loop(f);
}
fn update_result(ui: &slint::Weak<AppWindow>, id: i32, f: impl FnOnce(&AppWindow) + Send + 'static) {
    update(ui, move |ui| {
        if ui.invoke_result_update_begin(id) {
            f(&ui);
            ui.invoke_result_update_end(id);
        }
    });
}
fn clear_results(ui: &AppWindow) {
    ui.set_query_error("".into());
    ui.set_query_running(false);
    ui.set_query_has_run(false);
    ui.set_query_elapsed(0.0);
    ui.set_result_status(false);
    ui.set_result_names(ModelRc::default());
    ui.set_result_columns(ModelRc::default());
    ui.set_result_rows(ModelRc::default());
    ui.set_result_index(-1);
    ui.set_selected_row(-1);
    ui.set_selected_rows(ModelRc::default());
    ui.set_selected_cells(ModelRc::default());
    ui.set_cell_anchor_column(-1);
    ui.set_selected_count(0);
    ui.set_selection_anchor(-1);
    ui.set_row_count("0".into());
    ui.set_page(0);
    ui.set_pages(0);
}
fn localize_result(ui: &AppWindow) {
    if !ui.get_result_status() { return; }
    let zh = ui.global::<I18n>().get_zh();
    let columns = ui.get_result_columns();
    for (index, title) in [tr(zh, "状态", "Status"), tr(zh, "影响行数", "Affected rows")].into_iter().enumerate() {
        if let Some(mut column) = columns.row_data(index) { column.title = title.into(); columns.set_row_data(index, column); }
    }
    for row in ui.get_result_rows().iter() {
        if let Some(mut cell) = row.row_data(0) { cell.text = tr(zh, "执行成功", "Success").into(); row.set_row_data(0, cell); }
    }
}
fn loading_fields(ui: &AppWindow, name: &str) {
    ui.set_field_panel_hidden(false);
    ui.set_fields(ModelRc::default());
    ui.set_field_details(ModelRc::default());
    ui.set_fields_loading_name(name.into());
    ui.set_fields_loading(true);
}
fn profile_from_ui(ui: &AppWindow) -> Result<Profile, String> {
    let zh = ui.global::<I18n>().get_zh();
    let port = ui
        .get_port()
        .trim()
        .parse::<u16>()
        .ok()
        .filter(|p| *p > 0)
        .ok_or_else(|| tr(zh, "端口必须为 1–65535", "Port must be 1–65535"))?;
    let profile = Profile {
        shared_id: String::new(),
        extra: Default::default(),
        name: ui.get_profile_name().trim().into(),
        redshift: ui.get_engine() == 1,
        host: ui.get_host().trim().into(),
        port,
        database: ui.get_database().trim().into(),
        user: ui.get_username().trim().into(),
        tls: ui.get_tls(),
        trust_server_certificate: ui.get_trust_server_certificate(),
    };
    if profile.host.is_empty() || profile.database.is_empty() || profile.user.is_empty() {
        return Err(tr(
            zh,
            "请填写主机、数据库、用户名",
            "Host, database and username are required",
        ));
    }
    Ok(profile)
}
fn show_profile(ui: &AppWindow, profile: Profile, password: String) {
    ui.set_profile_name(profile.name.into());
    ui.set_engine(i32::from(profile.redshift));
    ui.set_host(profile.host.into());
    ui.set_port(profile.port.to_string().into());
    ui.set_database(profile.database.into());
    ui.set_username(profile.user.into());
    ui.set_password(password.into());
    ui.set_tls(profile.tls);
    ui.set_trust_server_certificate(profile.trust_server_certificate);
}

fn filter_objects(ui: &AppWindow) {
    use slint::Model;
    if ui.get_switching() { return; }
    let search = ui.get_object_search().to_lowercase();
    let prefix = ui.get_active_prefix().to_lowercase();
    let show_hidden = ui.get_show_hidden();
    let hidden: std::collections::HashSet<_> = ui.get_hidden_objects().iter().map(|object| (object.name, object.kind)).collect();
    let mut counts = [0; 3];
    let mut names = Vec::new();
    let objects = if show_hidden { ui.get_hidden_objects() } else { ui.get_objects() };
    let columns: Vec<(String, Vec<String>)> = ui.get_column_index().iter().filter_map(|entry| serde_json::from_str(entry.as_str()).ok()).collect();
    for object in objects.iter() {
        let kind = object.kind as usize;
        let name = object.name.to_lowercase();
        // In "All" views names are qualified; prefixes apply to the object name.
        let base = name.rsplit('.').next().unwrap_or(name.as_str());
        if kind < 3 && (name.contains(&search) || columns.iter().any(|(table, fields)| table == object.name.as_str() && fields.iter().any(|field| field.to_lowercase().contains(&search)))) && (show_hidden || base.starts_with(&prefix))
            && (show_hidden || !hidden.contains(&(object.name.clone(), object.kind))) {
            counts[kind] += 1;
            if object.kind == ui.get_object_kind() {
                names.push(object.name.to_string());
            }
        }
    }
    let changed = ui
        .get_tables()
        .iter()
        .map(|item| item.text.to_string())
        .collect::<Vec<_>>()
        != names;
    if changed {
        ui.set_tables(items(names));
        ui.set_table_index(-1);
        ui.set_fields(ModelRc::default());
        ui.set_field_details(ModelRc::default());
        ui.set_field_names(ModelRc::default());
        ui.set_object_name("".into());
        ui.set_object_title("".into());
    }
    ui.set_table_count(counts[0]);
    ui.set_view_count(counts[1]);
    ui.set_mv_count(counts[2]);
    let changed = ui.get_changed_tables();
    ui.set_table_changes(std::rc::Rc::new(VecModel::from(ui.get_tables().iter().map(|item| changed.iter().any(|name| name == item.text)).collect::<Vec<_>>())).into());
}

fn filter_group(ui: &AppWindow) {
    use slint::Model;
    let search = ui.get_group_search().to_lowercase();
    let selected: Vec<_> = ui.get_group_selected().iter().map(|item| item.text).collect();
    ui.set_group_fields(items(ui.get_field_names().iter().filter(|name| name.to_lowercase().contains(&search) && !selected.contains(name)).map(|name| name.to_string()).collect()));
    ui.set_group_index(-1);
}

/// Relabels the leading "All" entry of the connection, database and schema selectors.
fn localize_all(ui: &AppWindow) {
    let label: SharedString = tr(ui.global::<I18n>().get_zh(), "全部", "All").into();
    for model in [ui.get_databases(), ui.get_schemas()] {
        if model.row_count() > 0 { model.set_row_data(0, label.clone()); }
    }
}
fn with_all(zh: bool, values: Vec<String>) -> ModelRc<SharedString> {
    strings(std::iter::once(tr(zh, "全部", "All")).chain(values).collect())
}

fn prefix_options(ui: &AppWindow) {
    use slint::Model;
    let mut options = vec![tr(ui.global::<I18n>().get_zh(), "全部", "All")];
    options.extend(ui.get_table_prefixes().iter().map(|prefix| prefix.to_string()));
    options.push(tr(ui.global::<I18n>().get_zh(), "隐藏项", "Hidden items"));
    ui.set_prefix_options(strings(options));
}

enum Command {
    Definition(i32, String, String, i32),
    RestoreScope(String, String, String),
    Connect(Profile, String),
    SwitchDatabase(String),
    /// Browse every database of the selected connection (1) or every schema (2).
    BrowseAll(i32),
    Disconnect,
    SaveProfile(Profile, String, Option<usize>),
    DeleteProfile(usize),
    LoadProfile(usize),
    UseProfile(usize),
    OpenScopes,
    SaveScopes(Vec<config::BrowserSelection>),
    ScopeChoices(i32, Vec<ScopeChoice>, Vec<ScopeChoice>, Vec<ScopeChoice>),
    SavePrefixes(Vec<String>, Option<String>),
    SetObjectHidden(String, i32, bool),
    Language(bool),
    Theme(bool),
    SaveTimeout(u64),
    Tables(String),
    Fields(String, String, String),
    Refresh,
    RefreshSchema,
    Reconnect,
    Cancel(i32),
    CloseDocument(i32),
    CloseResult(usize, usize, usize),
    Run(String, u64),
    History(usize),
    Page(usize, usize),
    Sort(usize, usize, bool),
    Copy(usize, Option<usize>, Option<Vec<usize>>, Option<Vec<(usize, usize)>>, bool),
    CopyCell(usize, usize, usize, usize),
    Export(usize),
    ImportSql,
    SaveSql(String, String),
}
struct Request {
    document_id: i32,
    open_documents: Vec<i32>,
    command: Command,
    zh: bool,
    started: Instant,
}

struct Worker {
    ui: slint::Weak<AppWindow>,
    settings: Settings,
    session: Option<db::Session>,
    session_profile: Option<Profile>,
    active: Option<(Profile, String)>,
    sets: Vec<ResultSet>,
    tab_sets: std::collections::HashMap<i32, Vec<ResultSet>>,
    result_tab: i32,
    cancelled: Arc<AtomicBool>,
    zh: bool,
    settings_error: Option<String>,
    current_schema: String,
    metadata_cache: cache::MetadataCache,
    query_only: bool,
    /// Browser sessions retained for switching connections.
    idle_sessions: Vec<(Profile, db::Session)>,
    browse: Browse,
    /// "All" view entries: display name → (profile with database, schema, object).
    all_index: std::collections::HashMap<String, (Profile, String, String)>,
}

#[derive(Clone, Copy, Default, PartialEq)]
struct Browse {
    databases: bool,
    schemas: bool,
}
impl Browse {
    fn any(self) -> bool { self.databases || self.schemas }
}

/// Sessions a SQL tab keeps between runs.
#[derive(Default)]
struct TabSessions {
    session: Option<db::Session>,
    profile: Option<Profile>,
}

/// UI bookkeeping that runs after a worker command finishes.
struct Finish {
    selected_fields: Option<(String, String)>,
    fields_request: bool,
    browser_refresh: bool,
    notify_success: bool,
    prefetch: bool,
    switch: bool,
    /// A switch still holds the session, so the connection indicator must not change.
    keep_connection: bool,
}
impl Finish {
    fn new(command: &Command) -> Self {
        Self {
            selected_fields: match command {
                Command::Fields(schema, table, action) if action.is_empty() => Some((schema.clone(), table.clone())),
                _ => None,
            },
            fields_request: matches!(command, Command::Fields(_, _, action) if action != "completion"),
            browser_refresh: matches!(command, Command::Refresh | Command::RefreshSchema),
            notify_success: matches!(command, Command::Fields(_, _, action) if action == "copy-structure") || !matches!(command,
                Command::Run(..) | Command::History(..) | Command::Page(..) | Command::Tables(..) |
                Command::Fields(..) | Command::ScopeChoices(..) | Command::OpenScopes |
                Command::LoadProfile(..) | Command::UseProfile(..)),
            prefetch: matches!(command, Command::Connect(..) | Command::UseProfile(..) | Command::Reconnect),
            switch: is_switch(command),
            keep_connection: false,
        }
    }
}

fn spawn_query(
    worker: &Worker,
    request: Request,
    queries: &mut tokio::task::JoinSet<(i32, Worker, Result<String, String>, Instant)>,
    sessions: &mut std::collections::HashMap<i32, TabSessions>,
    cancellations: &mut std::collections::HashMap<i32, Arc<AtomicBool>>,
) {
    if cancellations.contains_key(&request.document_id) { return; }
    let cancelled = Arc::new(AtomicBool::new(false));
    cancellations.insert(request.document_id, cancelled.clone());
    let saved = sessions.remove(&request.document_id).unwrap_or_default();
    let mut query = Worker {
        ui: worker.ui.clone(), settings: worker.settings.clone(), session: saved.session, session_profile: saved.profile,
        active: worker.active.clone(), sets: Vec::new(), tab_sets: std::collections::HashMap::new(),
        result_tab: request.document_id, cancelled, zh: request.zh, settings_error: worker.settings_error.clone(),
        current_schema: worker.current_schema.clone(), metadata_cache: cache::MetadataCache::default(), query_only: true,
        idle_sessions: Vec::new(), browse: worker.browse, all_index: std::collections::HashMap::new(),
    };
    queries.spawn(async move {
        let result = query.handle(request.command).await;
        (request.document_id, query, result, request.started)
    });
}

fn finish_request(worker: &mut Worker, prefetches: &mut tokio::task::JoinSet<Result<(), String>>, finish: Finish, result: Result<String, String>) {
    if finish.prefetch && result.is_ok() {
        if let Some((profile, _)) = &worker.active {
            let name = profile.name.clone();
            let mut background = Worker {
                ui: worker.ui.clone(), settings: worker.settings.clone(), session: None, session_profile: None,
                active: None, sets: Vec::new(), tab_sets: std::collections::HashMap::new(),
                result_tab: -1, cancelled: Arc::new(AtomicBool::new(false)), zh: worker.zh,
                settings_error: None, current_schema: String::new(),
                metadata_cache: cache::MetadataCache::load(), query_only: true,
                idle_sessions: Vec::new(), browse: Browse::default(), all_index: std::collections::HashMap::new(),
            };
            prefetches.spawn_blocking(move || {
                let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|error| error.to_string())?;
                runtime.block_on(background.fetch_scopes(false, Some(&name)))
            });
        }
    }
    let connected = worker.session.as_ref().is_some_and(|session| !session.client.is_closed());
    let zh = worker.zh;
    let Finish { selected_fields, fields_request, browser_refresh, notify_success, switch, keep_connection, .. } = finish;
    update(&worker.ui, move |ui| {
        if switch { ui.set_switching(false); filter_objects(&ui); }
        else if selected_fields.is_none() { ui.set_busy(false); }
        if browser_refresh { ui.set_browser_refresh(0); }
        if fields_request && selected_fields.as_ref().is_none_or(|(schema, table)| ui.get_schema() == schema.as_str() && ui.get_fields_loading_name() == table.as_str()) { ui.set_fields_loading(false); }
        if !keep_connection {
            ui.set_connected(connected);
            ui.set_reconnecting(false);
        }
        match result {
            Ok(message) => {
                ui.set_failed(false);
                ui.set_status("".into());
                if notify_success && !message.is_empty() { ui.invoke_notify(message.into()); }
            }
            Err(error) => {
                ui.set_failed(true);
                ui.set_status(format!("{}\n{}", tr(zh, "操作失败", "Operation failed"), explain(zh, error)).into());
            }
        }
    });
}

/// Connection and browser changes run in the background so the SQL area and results stay usable.
fn is_switch(command: &Command) -> bool {
    matches!(command, Command::RestoreScope(..) | Command::Connect(..) | Command::UseProfile(..) | Command::SwitchDatabase(..) | Command::BrowseAll(..)
        | Command::Tables(..) | Command::Refresh | Command::RefreshSchema | Command::Reconnect)
}
/// Commands that only touch SQL tabs or held results; allowed while a switch is running.
fn is_result_command(command: &Command) -> bool {
    matches!(command, Command::Run(..) | Command::History(..) | Command::Cancel(..) | Command::Page(..) | Command::Sort(..) | Command::Copy(..)
        | Command::CopyCell(..) | Command::Export(..) | Command::CloseResult(..) | Command::ImportSql | Command::SaveSql(..))
}

async fn guarded<T, E: From<&'static str>>(
    future: impl Future<Output = Result<T, E>>,
    cancelled: Arc<AtomicBool>,
    seconds: u64,
) -> Result<T, E> {
    tokio::select! {
        value = future => value,
        _ = tokio::time::sleep(Duration::from_secs(seconds)) => Err("TIMEOUT".into()),
        _ = async { loop { if cancelled.load(Ordering::Relaxed) { break; } tokio::time::sleep(Duration::from_millis(50)).await; } } => Err("CANCELLED".into()),
    }
}

impl Worker {
    fn message(&self, cn: &str, en: &str) -> String {
        tr(self.zh, cn, en)
    }
    fn sync_profiles(&self) {
        let profiles = self
            .settings
            .profiles
            .iter()
            .map(|p| p.name.clone())
            .collect();
        let visible = self.settings.visible_profiles();
        let physical_index = self.physical_index(&visible);
        update(&self.ui, move |ui| {
            ui.set_profiles(strings(profiles));
            ui.set_visible_profiles(strings(visible));
            ui.set_physical_index(physical_index);
        });
    }
    fn physical_index(&self, visible: &[String]) -> i32 {
        self.active.as_ref().and_then(|(active, _)| visible.iter().position(|name| name == &active.name)).map_or(-1, |index| index as i32)
    }
    fn default_database(&self, profile: &Profile) -> String {
        if let Some((active, _)) = self.active.as_ref().filter(|(active, _)| active.name == profile.name) { return active.database.clone(); }
        let scopes: Vec<_> = self.settings.browser_scopes.iter().filter(|scope| scope.profile == profile.name).collect();
        match scopes.first() {
            Some(first) if !scopes.iter().any(|scope| scope.database == profile.database) => first.database.clone(),
            _ => profile.database.clone(),
        }
    }
    /// Lists cached objects for the current "All" view without opening new connections.
    fn show_all(&mut self) -> Result<String, String> {
        let browse = self.browse;
        let active = self.active.as_ref().map(|(profile, _)| profile.clone());
        let profiles: Vec<Profile> = active.iter().cloned().collect();
        let scopes = cache::cached_scopes();
        let mut objects = Vec::new();
        let mut columns = Vec::new();
        self.all_index.clear();
        for profile in &profiles {
            let default_database = self.default_database(profile);
            let allowed: Vec<_> = self.settings.browser_scopes.iter().filter(|scope| scope.profile == profile.name).collect();
            for ((redshift, host, port, user, database, schema), scope) in &scopes {
                if (*redshift, host.as_str(), *port, user.as_str()) != (profile.redshift, profile.host.as_str(), profile.port, profile.user.as_str()) { continue; }
                if !browse.databases && *database != default_database { continue; }
                if !allowed.is_empty() && !allowed.iter().any(|allowed| &allowed.database == database && &allowed.schema == schema) { continue; }
                let mut fields = std::collections::HashMap::<&str, Vec<String>>::new();
                for row in &scope.fields {
                    if let (Some(table), Some(column)) = (row[0].as_deref(), row[1].as_deref()) { fields.entry(table).or_default().push(column.into()); }
                }
                for object in &scope.objects {
                    let kind = match object.kind.as_str() { "view" => 1, "materialized_view" => 2, _ => 0 };
                    if self.settings.hidden_objects.iter().any(|hidden| hidden.scope.profile == profile.name && &hidden.scope.database == database && &hidden.scope.schema == schema && hidden.name == object.object_name && hidden.kind == kind) { continue; }
                    let mut segments = Vec::new();
                    if *database != default_database { segments.push(database.as_str()); }
                    segments.extend([schema.as_str(), object.object_name.as_str()]);
                    let display = segments.join(".");
                    if self.all_index.contains_key(&display) { continue; }
                    columns.push(serde_json::to_string(&(&display, fields.get(object.object_name.as_str()).cloned().unwrap_or_default())).unwrap());
                    let mut located = profile.clone();
                    located.database = database.clone();
                    self.all_index.insert(display.clone(), (located, schema.clone(), object.object_name.clone()));
                    objects.push(DbObject { name: display.into(), kind });
                }
            }
        }
        objects.sort_by(|a, b| a.name.cmp(&b.name));
        let empty = objects.is_empty();
        self.current_schema.clear();
        let zh = self.zh;
        update(&self.ui, move |ui| {
            if browse.databases {
                ui.set_database_index(0);
                ui.set_schemas(with_all(zh, Vec::new()));
            }
            ui.set_schema_index(0);
            ui.set_schema("".into());
            ui.set_hidden_objects(ModelRc::default());
            ui.set_show_hidden(false);
            ui.set_changed_tables(ModelRc::default());
            ui.set_column_index(strings(columns));
            ui.set_table_index(-1);
            ui.set_fields(ModelRc::default());
            ui.set_objects(std::rc::Rc::new(VecModel::from(objects)).into());
            filter_objects(&ui);
        });
        self.sync_profiles();
        Ok(if empty {
            self.message("没有已缓存的表结构；请先逐个打开要浏览的模式，或在“浏览范围”中添加。", "No cached metadata yet; open each schema once or add it under Browsing scope.")
        } else { String::new() })
    }
    /// Reloads the browser for the current selection after a reconnect or refresh.
    async fn reload(&mut self) -> Result<(), String> {
        if self.browse.any() { self.show_all().map(|_| ()) }
        else if self.current_schema.is_empty() { self.refresh().await }
        else { self.tables(&self.current_schema.clone()).await }
    }
    fn sync_hidden(&self) {
        let objects = self.settings.hidden_objects.iter().filter(|object| {
            self.active.as_ref().is_some_and(|(profile, _)| object.scope.profile == profile.name && object.scope.database == profile.database && object.scope.schema == self.current_schema)
        }).map(|object| DbObject { name: object.name.as_str().into(), kind: object.kind }).collect::<Vec<_>>();
        update(&self.ui, move |ui| {
            ui.set_hidden_objects(std::rc::Rc::new(VecModel::from(objects)).into());
            filter_objects(&ui);
        });
    }
    fn sync_prefixes(&self) {
        let filter = self.active.as_ref().and_then(|(profile, _)| self.settings.prefix_filters.get(&profile.name)).cloned().unwrap_or_default();
        update(&self.ui, move |ui| {
            let selected = filter.selected.as_ref().and_then(|prefix| filter.prefixes.iter().position(|saved| saved == prefix));
            ui.set_show_hidden(false);
            ui.set_active_prefix(selected.map(|index| filter.prefixes[index].as_str()).unwrap_or_default().into());
            ui.set_table_prefixes(strings(filter.prefixes));
            prefix_options(&ui);
            ui.set_prefix_index(selected.map_or(0, |index| index as i32 + 1));
            ui.set_object_search("".into());
            filter_objects(&ui);
        });
    }
    fn disconnect(&mut self) {
        if self.query_only {
            self.session = None;
            self.session_profile = None;
            return;
        }
        self.session = None;
        self.session_profile = None;
        self.active = None;
        self.current_schema.clear();
        self.browse = Browse::default();
        self.all_index.clear();
        update(&self.ui, |ui| {
            ui.set_connected(false);
            ui.set_connection_label("".into());
            ui.set_databases(ModelRc::default());
            ui.set_database_index(-1);
            ui.set_schemas(ModelRc::default());
            ui.set_schema("".into());
            ui.set_schema_index(-1);
            ui.set_tables(ModelRc::default());
            ui.set_objects(ModelRc::default());
            ui.set_column_index(ModelRc::default());
            ui.set_hidden_objects(ModelRc::default());
            ui.set_show_hidden(false);
            ui.set_table_count(0);
            ui.set_view_count(0);
            ui.set_mv_count(0);
            ui.set_fields(ModelRc::default());
            ui.set_fields_loading(false);
            ui.set_table_index(-1);
        });
    }
    async fn invalidate(&mut self) {
        if let Some(session) = &self.session {
            let token = session.client.cancel_token();
            if let Some((profile, _)) = self.active.as_ref().filter(|(profile, _)| profile.tls) {
                if let Ok(tls) = db::tls_connector(profile) {
                    let _ = tokio::time::timeout(
                        Duration::from_secs(3),
                        token.cancel_query(postgres_native_tls::MakeTlsConnector::new(tls)),
                    )
                    .await;
                }
            } else {
                let _ = tokio::time::timeout(
                    Duration::from_secs(3),
                    token.cancel_query(tokio_postgres::NoTls),
                )
                .await;
            }
        }
        self.disconnect();
    }
    async fn metadata(&mut self, sql: &str) -> Result<Vec<Cells>, String> {
        if let Some((profile, _)) = &self.active {
            if let Some(rows) = self.metadata_cache.get(profile, sql) {
                return Ok(rows);
            }
        }
        self.ensure_session().await?;
        let session = self
            .session
            .as_ref()
            .ok_or_else(|| self.message("请先连接数据库", "Connect first"))?;
        let result = guarded(
            db::metadata(&session.client, sql),
            self.cancelled.clone(),
            30,
        )
        .await;
        if result.as_ref().is_err_and(|error| !error.recoverable) {
            self.invalidate().await;
        }
        let rows = result.map_err(|error| error.message)?;
        if let Some((profile, _)) = &self.active {
            self.metadata_cache.insert(profile, sql, &rows);
        }
        Ok(rows)
    }
    async fn ensure_session(&mut self) -> Result<(), String> {
        let (profile, password) = self
            .active
            .as_ref()
            .ok_or_else(|| self.message("请先选择连接", "Select a connection first"))?
            .clone();
        if self.session_profile.as_ref() == Some(&profile)
            && self
                .session
                .as_ref()
                .is_some_and(|session| !session.client.is_closed())
        {
            return Ok(());
        }
        if !self.query_only { update(&self.ui, |ui| ui.set_reconnecting(true)); }
        let result = guarded(db::connect(&profile, &password), self.cancelled.clone(), 20).await;
        if !self.query_only {
            let connected = result.is_ok();
            update(&self.ui, move |ui| { ui.set_connected(connected); ui.set_reconnecting(false); });
        }
        match result {
            Ok(session) => {
                self.session = Some(session);
                self.session_profile = Some(profile);
                Ok(())
            }
            Err(error) => {
                self.disconnect();
                Err(error)
            }
        }
    }
    /// Prepares the selected connection's session for a run and applies the selected schema.
    async fn prepare_session(&mut self) -> Result<Arc<tokio_postgres::Client>, String> {
        self.ensure_session().await?;
        let client = self.session.as_ref().ok_or_else(|| self.message("请先连接数据库", "Connect first"))?.client.clone();
        // With "All" schemas selected, names are schema-qualified and the server default path applies.
        let set_path = if self.current_schema.is_empty() { "RESET search_path".to_string() } else { format!("SET search_path TO {}", db::identifier(&self.current_schema)) };
        if let Err(error) = guarded(async { client.batch_execute(&set_path).await.map_err(db::query_error) }, self.cancelled.clone(), 15).await {
            if error.state.as_ref() != Some(&tokio_postgres::error::SqlState::IN_FAILED_SQL_TRANSACTION) {
                if !error.recoverable { self.invalidate().await; }
                return Err(error.message);
            }
        }
        Ok(client)
    }
    async fn scope_metadata(&mut self, profile: &Profile, sql: &str) -> Result<Vec<Cells>, String> {
        if let Some(rows) = self.metadata_cache.get(profile, sql) { return Ok(rows); }
        let password = config::password(&profile.name)?;
        let session = guarded(db::connect(profile, &password), self.cancelled.clone(), 20).await?;
        let rows = guarded(db::metadata(&session.client, sql), self.cancelled.clone(), 30).await.map_err(|error| error.message)?;
        self.metadata_cache.insert(profile, sql, &rows);
        Ok(rows)
    }
    async fn connect(&mut self, profile: Profile, password: String) -> Result<String, String> {
        // Results already shown in SQL tabs are kept; they do not depend on the browser connection.
        if let (Some(saved), Some(session)) = (self.session_profile.take(), self.session.take()) {
            if !session.client.is_closed() { self.idle_sessions.push((saved, session)); }
        }
        self.current_schema.clear();
        self.browse = Browse::default();
        self.all_index.clear();
        let scopes: Vec<_> = self.settings.browser_scopes.iter().filter(|scope| scope.profile == profile.name).collect();
        let mut profile = profile;
        if let Some(first) = scopes.first() {
            if !scopes.iter().any(|scope| scope.database == profile.database) {
                profile.database = first.database.clone();
            }
        }
        self.idle_sessions.retain(|(_, session)| !session.client.is_closed());
        if let Some(index) = self.idle_sessions.iter().position(|(saved, _)| saved == &profile) {
            let (saved, session) = self.idle_sessions.remove(index);
            self.session_profile = Some(saved);
            self.session = Some(session);
        }
        self.active = Some((profile.clone(), password));
        self.sync_prefixes();
        let label = format!("{} @ {} / {}", profile.user, profile.host, profile.database);
        let physical_index = self.physical_index(&self.settings.visible_profiles());
        let connected = self.session.is_some();
        update(&self.ui, move |ui| {
            ui.set_connected(connected);
            ui.set_connection_label(label.into());
            ui.set_database(profile.database.clone().into());
            ui.set_physical_index(physical_index);
        });
        self.refresh().await?;
        Ok(self.message("已选择连接", "Connection selected"))
    }
    async fn refresh(&mut self) -> Result<(), String> {
        let profile = self.active.as_ref().ok_or("No connection")?.0.clone();
        let scopes: Vec<_> = self.settings.browser_scopes.iter().filter(|scope| scope.profile == profile.name).cloned().collect();
        let mut databases: Vec<String> = if scopes.is_empty() {
            self.metadata("SELECT datname FROM pg_catalog.pg_database WHERE datallowconn = true ORDER BY datname").await?.into_iter().filter_map(|r| r.into_iter().next().flatten()).collect()
        } else {
            scopes.iter().map(|scope| scope.database.clone()).collect()
        };
        if scopes.is_empty() { databases.sort(); }
        let mut seen = std::collections::HashSet::new();
        databases.retain(|name| seen.insert(name.clone()));
        let redshift = self
            .active
            .as_ref()
            .is_some_and(|(profile, _)| profile.redshift);
        let schema_sql = if redshift {
            "SELECT nspname AS schema_name FROM pg_catalog.pg_namespace ORDER BY nspname"
        } else {
            "SELECT schema_name FROM information_schema.schemata ORDER BY schema_name"
        };
        let mut schemas: Vec<String> = if scopes.is_empty() {
            self.metadata(schema_sql).await?.into_iter().filter_map(|r| r.into_iter().next().flatten()).collect()
        } else {
            scopes.iter().filter(|scope| scope.database == profile.database).map(|scope| scope.schema.clone()).collect()
        };
        if scopes.is_empty() { schemas.sort(); }
        seen.clear();
        schemas.retain(|name| seen.insert(name.clone()));
        let restored = self
            .settings
            .last_selection
            .as_ref()
            .filter(|last| {
                self.active
                    .as_ref()
                    .is_some_and(|(p, _)| p.name == last.profile && p.database == last.database)
            })
            .map(|last| last.schema.as_str());
        let schema = schemas
            .iter()
            .find(|s| Some(s.as_str()) == restored)
            .or_else(|| schemas.iter().find(|s| *s == "public"))
            .or_else(|| schemas.first())
            .cloned()
            .unwrap_or_default();
        // Index 0 of both selectors is "All".
        let database_index = self
            .active
            .as_ref()
            .and_then(|(profile, _)| databases.iter().position(|name| name == &profile.database))
            .map_or(-1, |index| index as i32 + 1);
        let schema_index = schemas
            .iter()
            .position(|name| name == &schema)
            .map_or(-1, |index| index as i32 + 1);
        let selected_schema = schema.clone();
        let zh = self.zh;
        update(&self.ui, move |ui| {
            ui.set_databases(with_all(zh, databases));
            ui.set_database_index(database_index);
            ui.set_schemas(with_all(zh, schemas));
            ui.set_schema_index(schema_index);
            ui.set_schema(schema.clone().into());
        });
        self.tables(&selected_schema).await
    }
    async fn tables(&mut self, schema: &str) -> Result<(), String> {
        if self.active.as_ref().is_some_and(|(profile, _)| self.settings.browser_scopes.iter().any(|scope| scope.profile == profile.name) && !self.settings.browser_scopes.iter().any(|scope| scope.profile == profile.name && scope.database == profile.database && scope.schema == schema)) {
            return Err(self.message("此模式不在浏览范围内", "Schema is outside the browsing scope"));
        }
        let context_changed = self.current_schema != schema || self.browse.any();
        if self.browse.any() {
            self.browse = Browse::default();
            self.all_index.clear();
            self.sync_profiles();
        }
        self.current_schema = schema.to_owned();
        self.sync_hidden();
        let objects = if schema.is_empty() { Vec::new() } else { self.load_objects(schema).await? };
        let columns = self.active.as_ref().map(|(profile, _)| {
            let scope = cache::load_scope(profile, schema);
            let mut fields = std::collections::HashMap::<&str, Vec<&str>>::new();
            if let Some(scope) = &scope {
                for row in &scope.fields {
                    if let (Some(table), Some(column)) = (row[0].as_deref(), row[1].as_deref()) {
                        fields.entry(table).or_default().push(column);
                    }
                }
            }
            objects.iter().map(|object| {
                if scope.is_some() {
                    serde_json::to_string(&(object.name.as_str(), fields.get(object.name.as_str()).cloned().unwrap_or_default())).unwrap()
                } else {
                    serde_json::to_string(&(object.name.as_str(), self.metadata_cache.columns(profile, schema, &object.name))).unwrap()
                }
            }).collect()
        }).unwrap_or_default();
        let changed = self.active.as_ref().map(|(profile, _)| self.metadata_cache.changed_tables(profile, schema, &objects)).unwrap_or_default();
        let selected = schema.to_owned();
        update(&self.ui, move |ui| {
            ui.set_schema(selected.into());
            ui.set_column_index(strings(columns));
            ui.set_changed_tables(strings(changed));
            if context_changed || !ui.get_objects().iter().eq(objects.iter().cloned()) {
                ui.set_table_index(-1);
                ui.set_fields(ModelRc::default());
                ui.set_objects(std::rc::Rc::new(VecModel::from(objects)).into());
            }
            filter_objects(&ui);
        });
        self.remember_selection()?;
        Ok(())
    }
    async fn load_objects(&mut self, schema: &str) -> Result<Vec<DbObject>, String> {
        let result = self.load_scope(schema, false).await;
        update(&self.ui, |ui| {
            if ui.get_browser_refresh() == 3 { ui.set_browser_refresh(0); }
        });
        result
    }
    async fn load_scope(&mut self, schema: &str, force: bool) -> Result<Vec<DbObject>, String> {
        let profile = self.active.as_ref().ok_or("No connection")?.0.clone();
        if !force {
            if let Some(scope) = cache::load_scope(&profile, schema) {
                return Ok(scope.objects.into_iter().map(|object| DbObject { name: object.object_name.into(), kind: match object.kind.as_str() { "view" => 1, "materialized_view" => 2, _ => 0 } }).collect());
            }
        }
        if !force && !self.query_only && self.current_schema == schema {
            update(&self.ui, |ui| ui.set_browser_refresh(3));
        }
        let _lock = cache::lock_scope(&profile, schema)?;
        if !force {
            if let Some(scope) = cache::load_scope(&profile, schema) {
                return Ok(scope.objects.into_iter().map(|object| DbObject { name: object.object_name.into(), kind: match object.kind.as_str() { "view" => 1, "materialized_view" => 2, _ => 0 } }).collect());
            }
        }
        self.metadata_cache.invalidate_scope(&profile, schema);
        let objects = self.fetch_objects(schema).await?;
        let fields = self.metadata(&db::columns_sql(schema, None, profile.redshift)).await?;
        cache::save_scope(&profile, schema, &objects, fields)?;
        Ok(objects)
    }
    async fn fetch_scopes(&mut self, force: bool, profile_name: Option<&str>) -> Result<(), String> {
        let original = self.active.clone();
        let scopes = self.settings.browser_scopes.clone();
        let result = async {
            for scope in scopes {
                if profile_name.is_some_and(|name| scope.profile != name) { continue; }
                let mut profile = self.settings.profiles.iter().find(|profile| profile.name == scope.profile).ok_or("No profile")?.clone();
                profile.database = scope.database;
                let password = config::password(&profile.name)?;
                self.active = Some((profile, password));
                self.load_scope(&scope.schema, force).await?;
            }
            Ok(())
        }.await;
        self.active = original;
        result
    }
    async fn fetch_objects(&mut self, schema: &str) -> Result<Vec<DbObject>, String> {
        let redshift = self
            .active
            .as_ref()
            .is_some_and(|(profile, _)| profile.redshift);
        let quoted = db::literal(schema, redshift);
        let mut objects: Vec<DbObject> = Vec::new();
        if redshift {
            let rows = self.metadata(&format!("SELECT table_name, table_type FROM svv_all_tables WHERE database_name = current_database() AND schema_name = {quoted} ORDER BY table_name")).await?;
            let mv_rows = self.metadata(&format!("SELECT TRIM(name) FROM svv_mv_info WHERE database_name = current_database() AND schema_name = {quoted} ORDER BY name")).await?;
            let materialized: std::collections::HashSet<String> = mv_rows
                .into_iter()
                .filter_map(|r| r.into_iter().next().flatten())
                .collect();
            for row in rows {
                if let Some(name) = &row[0] {
                    let kind = if materialized.contains(name) {
                        2
                    } else if row[1]
                        .as_deref()
                        .unwrap_or("")
                        .to_ascii_lowercase()
                        .contains("view")
                    {
                        1
                    } else {
                        0
                    };
                    objects.push(DbObject {
                        name: name.as_str().into(),
                        kind,
                    });
                }
            }
            let known: std::collections::HashSet<String> = objects
                .iter()
                .map(|object| object.name.to_string())
                .collect();
            for name in materialized {
                if !known.contains(&name) {
                    objects.push(DbObject {
                        name: name.into(),
                        kind: 2,
                    });
                }
            }
        } else {
            let rows = self.metadata(&format!("SELECT c.relname, c.relkind FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace WHERE n.nspname = {quoted} AND c.relkind IN ('r','p','f','v','m') ORDER BY c.relname")).await?;
            for row in rows {
                if let Some(name) = &row[0] {
                    objects.push(DbObject {
                        name: name.as_str().into(),
                        kind: match row[1].as_deref() {
                            Some("v") => 1,
                            Some("m") => 2,
                            _ => 0,
                        },
                    });
                }
            }
        }
        objects.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(objects)
    }
    fn remember_selection(&mut self) -> Result<(), String> {
        if self.settings_error.is_some() {
            return Ok(());
        }
        if let Some((profile, _)) = &self.active {
            if self
                .settings
                .profiles
                .iter()
                .any(|saved| saved.name == profile.name)
            {
                if self.settings.last_selection.as_ref().is_some_and(|last| {
                    last.profile == profile.name
                        && last.database == profile.database
                        && last.schema == self.current_schema
                }) {
                    return Ok(());
                }
                self.settings.last_selection = Some(config::BrowserSelection {
                    profile: profile.name.clone(),
                    database: profile.database.clone(),
                    schema: self.current_schema.clone(),
                });
                config::save(&self.settings)?;
            }
        }
        Ok(())
    }
    fn page(&mut self, index: usize, page: usize) -> Result<(), String> {
        let set = self.sets.get_mut(index).ok_or("No result set")?;
        let pages = set.count.div_ceil(PAGE_SIZE);
        let page = page.min(pages.saturating_sub(1));
        let mut rows = set.page(page, true, None)?;
        let count = set.count.to_string();
        let status = set.status;
        let headers = if set.status {
            for row in &mut rows { row[0] = Some(tr(self.zh, "执行成功", "Success")); }
            vec![tr(self.zh, "状态", "Status"), tr(self.zh, "影响行数", "Affected rows")]
        } else { set.headers.clone() };
        update_result(&self.ui, self.result_tab, move |ui| {
            ui.set_result_status(status);
            let columns: Vec<TableColumn> = headers
                .into_iter()
                .map(|title| {
                    let mut column = TableColumn::default();
                    column.title = title.into();
                    column.width = 180.0;
                    column.min_width = 60.0;
                    column
                })
                .collect();
            let rows: Vec<ModelRc<StandardListViewItem>> = rows
                .into_iter()
                .map(|row| {
                    items(
                        row.into_iter()
                            .map(|v| match v {
                                Some(text) => text,
                                None => "NULL".into(),
                            })
                            .collect(),
                    )
                })
                .collect();
            ui.set_result_columns(std::rc::Rc::new(VecModel::from(columns)).into());
            ui.set_result_rows(std::rc::Rc::new(VecModel::from(rows)).into());
            ui.set_row_count(count.into());
            ui.set_page(page as i32);
            ui.set_pages(pages as i32);
            ui.set_result_index(index as i32);
            ui.set_selected_row(-1);
            ui.set_selected_rows(ModelRc::default());
            ui.set_selected_cells(ModelRc::default());
            ui.set_cell_anchor_column(-1);
            ui.set_selected_count(0);
            ui.set_selection_anchor(-1);
        });
        Ok(())
    }
    async fn handle(&mut self, mut command: Command) -> Result<String, String> {
        if matches!(
            &command,
            Command::SaveProfile(..) | Command::DeleteProfile(..) | Command::Language(..) | Command::Theme(..) | Command::SaveTimeout(..) | Command::SaveScopes(..) | Command::SavePrefixes(..) | Command::SetObjectHidden(..)
        ) {
            if let Some(error) = &self.settings_error {
                return Err(format!("{}\n{error}", self.message("原配置无法读取，已阻止覆盖。请修复配置文件后重启。", "Existing configuration could not be read; overwriting is blocked. Repair the file and restart.")));
            }
        }
        if self.settings_error.is_none() && !(is_result_command(&command) && !matches!(command, Command::Run(..))) {
            let latest = config::load()?;
            let selected = match &mut command {
                Command::LoadProfile(index) | Command::UseProfile(index) | Command::DeleteProfile(index) => Some(index),
                Command::SaveProfile(_, _, Some(index)) => Some(index),
                _ => None,
            };
            if let Some(index) = selected {
                let old = self.settings.profiles.get(*index).ok_or("No profile")?;
                *index = latest.profiles.iter().position(|profile| (!old.shared_id.is_empty() && profile.shared_id == old.shared_id) || profile.name == old.name).ok_or_else(|| self.message("连接已被删除，请重新打开连接设置", "Connection was removed; reopen connection settings"))?;
            }
            self.settings = latest;
            if !is_switch(&command) { self.sync_profiles(); }
        }
        match command {
            Command::Cancel(_) | Command::CloseDocument(_) => Ok(String::new()),
            Command::Reconnect => {
                if self.active.is_some() {
                    self.session = None;
                    self.session_profile = None;
                    self.ensure_session().await?;
                    self.reload().await?;
                    Ok(self.message("连接成功", "Connected"))
                } else {
                    let index = self.settings.last_selection.as_ref().and_then(|last| self.settings.profiles.iter().position(|profile| profile.name == last.profile)).ok_or_else(|| self.message("请先选择连接", "Select a connection first"))?;
                    let profile = self.settings.profiles[index].clone();
                    let password = config::password(&profile.name)?;
                    self.connect(profile, password).await?;
                    self.ensure_session().await?;
                    Ok(self.message("连接成功", "Connected"))
                }
            }
            Command::CloseResult(index, active, owner) => {
                if self.sets.get(index).is_some_and(|set| !set.exported && set.count > 0) {
                    let zh = self.zh;
                    let confirmed = tokio::task::spawn_blocking(move || {
                        use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_YESNO, MB_ICONQUESTION, MB_TOPMOST, IDYES};
                        let title: Vec<u16> = tr(zh, "关闭结果", "Close result").encode_utf16().chain(Some(0)).collect();
                        let message: Vec<u16> = tr(zh, "此结果尚未导出，确定关闭？", "This result has not been exported. Close it?").encode_utf16().chain(Some(0)).collect();
                        unsafe { MessageBoxW(owner as _, message.as_ptr(), title.as_ptr(), MB_YESNO | MB_ICONQUESTION | MB_TOPMOST) == IDYES }
                    }).await.map_err(|error| error.to_string())?;
                    if !confirmed { return Ok(String::new()); }
                }
                if index < self.sets.len() { self.sets.remove(index); }
                let names = self.sets.iter().enumerate().map(|(i, set)| if !set.name.is_empty() { set.name.clone() } else if set.repeats > 1 { format!("{} × {}", i + 1, set.repeats) } else { (i + 1).to_string() }).collect();
                update_result(&self.ui, self.result_tab, move |ui| {
                    let elapsed = ui.get_query_elapsed();
                    let phase = ui.get_query_phase();
                    let has_run = ui.get_query_has_run();
                    clear_results(ui);
                    ui.set_query_elapsed(elapsed);
                    ui.set_query_phase(phase);
                    ui.set_query_has_run(has_run);
                    ui.set_result_names(strings(names));
                });
                if !self.sets.is_empty() { self.page((if index < active { active - 1 } else { active }).min(self.sets.len() - 1), 0)?; }
                Ok(String::new())
            }
            Command::ScopeChoices(level, profiles, previous_databases, previous_schemas) => {
                let mut databases = Vec::new();
                if level == 0 {
                    for choice in profiles.iter().filter(|choice| choice.selected) {
                        let profile = self.settings.profiles.iter().find(|profile| profile.name == choice.profile.as_str()).ok_or("No profile")?.clone();
                        let rows = self.scope_metadata(&profile, "SELECT datname FROM pg_catalog.pg_database WHERE datallowconn = true ORDER BY datname").await?;
                        for database in rows.into_iter().filter_map(|row| row.into_iter().next().flatten()) {
                            let selected = previous_databases.iter().any(|old| old.profile == choice.profile && old.database == database.as_str() && old.selected);
                            databases.push(ScopeChoice { profile: choice.profile.clone(), database: database.into(), schema: "".into(), selected });
                        }
                    }
                } else {
                    databases = previous_databases;
                }
                let mut schemas = Vec::new();
                for choice in databases.iter().filter(|choice| choice.selected) {
                    let mut profile = self.settings.profiles.iter().find(|profile| profile.name == choice.profile.as_str()).ok_or("No profile")?.clone();
                    profile.database = choice.database.to_string();
                    let sql = if profile.redshift { "SELECT nspname AS schema_name FROM pg_catalog.pg_namespace ORDER BY nspname" } else { "SELECT schema_name FROM information_schema.schemata ORDER BY schema_name" };
                    let rows = self.scope_metadata(&profile, sql).await?;
                    for schema in rows.into_iter().filter_map(|row| row.into_iter().next().flatten()) {
                        let selected = previous_schemas.iter().any(|old| old.profile == choice.profile && old.database == choice.database && old.schema == schema.as_str() && old.selected);
                        schemas.push(ScopeChoice { profile: choice.profile.clone(), database: choice.database.clone(), schema: schema.into(), selected });
                    }
                }
                update(&self.ui, move |ui| {
                    ui.set_scope_database_names(strings(databases.iter().map(|choice| choice.database.to_string()).collect()));
                    ui.set_scope_schema_names(strings(schemas.iter().map(|choice| choice.schema.to_string()).collect()));
                    ui.set_scope_databases(std::rc::Rc::new(VecModel::from(databases)).into());
                    ui.set_scope_schemas(std::rc::Rc::new(VecModel::from(schemas)).into());
                });
                Ok(self.message("已加载级联选项", "Cascading choices loaded"))
            }
            Command::SetObjectHidden(name, kind, hidden) => {
                if self.browse.any() { return Err(self.message("请先选择具体的连接、库名和模式", "Select a specific connection, database and schema first")); }
                let profile = &self.active.as_ref().ok_or_else(|| self.message("请先连接数据库", "Connect first"))?.0;
                let object = config::HiddenObject { scope: config::BrowserSelection { profile: profile.name.clone(), database: profile.database.clone(), schema: self.current_schema.clone() }, name, kind };
                let old = self.settings.hidden_objects.clone();
                self.settings.hidden_objects.retain(|saved| saved != &object);
                if hidden { self.settings.hidden_objects.push(object); }
                if let Err(error) = config::save(&self.settings) { self.settings.hidden_objects = old; return Err(error); }
                self.sync_hidden();
                Ok(if hidden { self.message("已隐藏", "Hidden") } else { self.message("已恢复显示", "Shown") })
            }
            Command::SavePrefixes(prefixes, selected) => {
                let profile = self.active.as_ref().ok_or("No connection")?.0.name.clone();
                let old = self.settings.clone();
                self.settings.prefix_filters.insert(profile, config::PrefixFilter { prefixes, selected });
                if let Err(error) = config::save(&self.settings) { self.settings = old; self.sync_prefixes(); return Err(error); }
                Ok(String::new())
            }
            Command::OpenScopes => {
                let scopes: Vec<BrowserScope> = self.settings.browser_scopes.iter().map(|scope| BrowserScope { profile: scope.profile.as_str().into(), database: scope.database.as_str().into(), schema: scope.schema.as_str().into() }).collect();
                let profiles: Vec<ScopeChoice> = self.settings.profiles.iter().map(|profile| ScopeChoice { profile: profile.name.as_str().into(), ..ScopeChoice::default() }).collect();
                update(&self.ui, move |ui| {
                    ui.set_browser_scopes(std::rc::Rc::new(VecModel::from(scopes)).into());
                    ui.set_scope_profiles(std::rc::Rc::new(VecModel::from(profiles)).into());
                    ui.set_scope_profile_index(-1);
                    ui.set_scope_database_index(-1);
                    ui.set_scope_schema_index(-1);
                    ui.set_scope_database_names(ModelRc::default());
                    ui.set_scope_schema_names(ModelRc::default());
                    ui.set_scope_databases(ModelRc::default());
                    ui.set_scope_schemas(ModelRc::default());
                    ui.set_config_open(false);
                    ui.set_timeout_open(false);
                    ui.set_group_open(false);
                    ui.set_scopes_open(true);
                });
                Ok(self.message("已打开浏览范围设置", "Browsing scope settings opened"))
            }
            Command::SaveScopes(scopes) => {
                if scopes.iter().any(|scope| scope.database.trim().is_empty() || scope.schema.trim().is_empty() || !self.settings.profiles.iter().any(|profile| profile.name == scope.profile)) {
                    return Err(self.message("请选择已保存连接并填写库名、模式", "Select a saved connection and enter a database and schema"));
                }
                let old = self.settings.browser_scopes.clone();
                self.settings.browser_scopes = scopes;
                if let Err(error) = config::save(&self.settings) {
                    self.settings.browser_scopes = old;
                    return Err(error);
                }
                self.fetch_scopes(false, None).await?;
                self.sync_profiles();
                update(&self.ui, |ui| ui.set_scopes_open(false));
                if let Some((mut profile, password)) = self.active.clone() {
                    let allowed: Vec<_> = self.settings.browser_scopes.iter().filter(|scope| scope.profile == profile.name).collect();
                    if !allowed.is_empty() && !allowed.iter().any(|scope| scope.database == profile.database) {
                        profile.database = allowed[0].database.clone();
                    }
                    self.connect(profile, password).await?;
                }
                Ok(self.message("浏览范围已保存", "Browsing scope saved"))
            }
            Command::Definition(id, schema, table, kind) => {
                let result: Result<String, String> = async {
                    let (profile, schema, table) = if schema.is_empty() {
                        self.all_index.get(&table).cloned().ok_or_else(|| self.message("未找到对象所属的连接，请刷新后重试", "Object connection was not found; refresh and retry"))?
                    } else {
                        (self.active.as_ref().ok_or_else(|| self.message("请先选择连接", "Select a connection first"))?.0.clone(), schema, table)
                    };
                    let password = match &self.active {
                        Some((active, password)) if active.name == profile.name => password.clone(),
                        _ => config::password(&profile.name)?,
                    };
                    let session = guarded(db::connect(&profile, &password), self.cancelled.clone(), 20).await?;
                    guarded(db::definition(&session.client, &schema, &table, kind, profile.redshift), self.cancelled.clone(), 60).await.map_err(|error| error.message)
                }.await;
                let text = result.unwrap_or_else(|error| format!("{}\n\n{}", self.message("加载定义失败", "Failed to load definition"), explain(self.zh, error)));
                update(&self.ui, move |ui| ui.invoke_definition_loaded(id, text.into()));
                Ok(String::new())
            }
            Command::RestoreScope(name, database, schema) => {
                if name.is_empty() { return Ok(String::new()); }
                let index = self.settings.profiles.iter().position(|profile| profile.name == name).ok_or("No profile")?;
                let previous = self.settings.last_selection.clone();
                self.settings.last_selection = Some(config::BrowserSelection { profile: name.clone(), database: database.clone(), schema: schema.clone() });
                if self.active.as_ref().is_none_or(|(profile, _)| profile.name != name || profile.database != database) {
                    let mut profile = self.settings.profiles[index].clone();
                    profile.database = database;
                    let password = config::password(&name)?;
                    let shown = profile.clone();
                    let shown_password = password.clone();
                    update(&self.ui, move |ui| { ui.set_profile_index(index as i32); show_profile(&ui, shown, shown_password); });
                    self.connect(profile, password).await?;
                }
                if self.current_schema != schema || self.browse.any() { self.tables(&schema).await?; }
                let selected = self.current_schema.clone();
                update(&self.ui, move |ui| {
                    let index = ui.get_schemas().iter().position(|name| name == selected.as_str()).map_or(-1, |index| index as i32);
                    ui.set_schema_index(index);
                });
                self.settings.last_selection = previous;
                self.remember_selection()?;
                Ok(String::new())
            }
            Command::Connect(profile, password) => {
                self.session = None;
                self.session_profile = None;
                self.idle_sessions.retain(|(saved, _)| saved != &profile);
                self.connect(profile, password).await?;
                self.ensure_session().await?;
                Ok(self.message("连接成功", "Connected"))
            }
            Command::UseProfile(index) => {
                let mut profile = self
                    .settings
                    .profiles
                    .get(index)
                    .ok_or("No profile")?
                    .clone();
                let password = config::password(&profile.name)?;
                if let Some(last) = &self.settings.last_selection {
                    if last.profile == profile.name {
                        profile.database = last.database.clone();
                    }
                }
                let shown = profile.clone();
                let shown_password = password.clone();
                update(&self.ui, move |ui| {
                    ui.set_profile_index(index as i32);
                    show_profile(&ui, shown, shown_password);
                });
                self.connect(profile, password).await
            }
            Command::SwitchDatabase(database) => {
                let (mut profile, password) = self.active.clone().ok_or("No connection")?;
                if self.settings.browser_scopes.iter().any(|scope| scope.profile == profile.name) && !self.settings.browser_scopes.iter().any(|scope| scope.profile == profile.name && scope.database == database) {
                    return Err(self.message("此库不在浏览范围内", "Database is outside the browsing scope"));
                }
                profile.database = database;
                self.connect(profile, password).await?;
                Ok(self.message("已切换库名", "Database selected"))
            }
            Command::BrowseAll(level) => {
                if self.active.is_none() {
                    return Err(self.message("请先连接数据库", "Connect first"));
                }
                self.browse = Browse { databases: level == 1, schemas: true };
                self.show_all()
            }
            Command::Disconnect => {
                self.idle_sessions.clear();
                self.disconnect();
                Ok(self.message("已断开连接", "Disconnected"))
            }
            Command::RefreshSchema => {
                self.active.as_ref().ok_or_else(|| self.message("请先连接数据库", "Connect first"))?;
                let schema = self.current_schema.clone();
                if schema.is_empty() { return Err(self.message("请先选择模式", "Select a schema first")); }
                self.load_scope(&schema, true).await?;
                self.tables(&schema).await?;
                Ok(self.message("当前模式的表、字段和类型已刷新", "Tables, columns and types refreshed for the current schema"))
            }
            Command::Refresh => {
                let profile = self.active.as_ref().ok_or("No connection")?.0.clone();
                let schema = self.current_schema.clone();
                self.fetch_scopes(true, None).await?;
                if !self.settings.browser_scopes.iter().any(|scope| scope.profile == profile.name && scope.database == profile.database && scope.schema == schema) && !schema.is_empty() {
                    self.load_scope(&schema, true).await?;
                }
                if self.browse.any() { self.show_all()?; } else { self.refresh().await?; }
                Ok(self.message("已刷新", "Refreshed"))
            }
            Command::Tables(schema) => {
                self.tables(&schema).await?;
                Ok(self.message("已加载表与视图", "Tables and views loaded"))
            }
            Command::Fields(schema, table, action) => {
                let redshift = self
                    .active
                    .as_ref()
                    .is_some_and(|(profile, _)| profile.redshift);
                let sql = db::columns_sql(&schema, Some(&table), redshift);
                // "All" entries are qualified display names resolved through the cached snapshot.
                let located = if schema.is_empty() { self.all_index.get(&table).cloned() } else { None };
                if schema.is_empty() && located.is_none() {
                    return Err(self.message("未找到该对象的缓存字段，请刷新后重试", "Cached columns for this object were not found; refresh and retry"));
                }
                let cached = match &located {
                    Some((profile, real_schema, real_table)) => Some(self.metadata_cache.schema_columns(profile, real_schema, real_table)
                        .ok_or_else(|| self.message("未找到该对象的缓存字段，请刷新后重试", "Cached columns for this object were not found; refresh and retry"))?),
                    None => self.active.as_ref().and_then(|(profile, _)| self.metadata_cache.schema_columns(profile, &schema, &table)),
                };
                let from_schema = cached.is_some() && located.is_none();
                let cached = cached.or_else(|| self.active.as_ref().and_then(|(profile, _)| self.metadata_cache.get(profile, &sql)));
                let mut rows = match cached {
                    Some(rows) => rows,
                    None => {
                        if action != "completion" {
                            let schema = schema.clone();
                            let table = table.clone();
                            let selection = action.is_empty();
                            update(&self.ui, move |ui| {
                                if ui.get_schema() == schema.as_str()
                                    && (!selection || ui.get_tables().row_data(ui.get_table_index() as usize).is_some_and(|item| item.text == table.as_str())) {
                                    loading_fields(&ui, &table);
                                }
                            });
                        }
                        self.metadata(&sql).await?
                    }
                };
                let added = if located.is_some() { Vec::new() } else { self.active.as_ref().map(|(profile, _)| if from_schema { self.metadata_cache.changed_schema_columns(profile, &schema, &table, &rows) } else { self.metadata_cache.changed_columns(profile, &sql, &rows) }).unwrap_or_default() };
                let objects = vec![DbObject { name: table.as_str().into(), kind: 0 }];
                let table_changed = located.is_none() && self.active.as_ref().is_some_and(|(profile, _)| !self.metadata_cache.changed_tables(profile, &schema, &objects).is_empty());
                for row in &mut rows {
                    let bytes = row.get(4).and_then(|value| value.as_deref()).and_then(|value| value.parse::<i32>().ok()).filter(|value| *value > 0);
                    if let Some(datatype) = &mut row[1] {
                        let lower = datatype.to_ascii_lowercase();
                        if lower == "timestamp" || lower == "timestamptz" || lower.starts_with("timestamp(") || lower.starts_with("timestamp ") {
                            *datatype = "timestamp".into();
                        } else if let Some(suffix) = datatype.strip_prefix("character varying").filter(|suffix| suffix.is_empty() || suffix.starts_with('(')) {
                            *datatype = format!("varchar{suffix}");
                        }
                        if datatype != "timestamp" && !datatype.contains('(') {
                            let name = if datatype == "integer" { "int" } else { datatype.as_str() };
                            *datatype = match bytes {
                                Some(bytes) => format!("{name}({bytes})"),
                                None => format!("{name}(variable)"),
                            };
                        }
                    }
                    row.truncate(4);
                }
                let copy_structure = action == "copy-structure";
                if copy_structure {
                    let mut structure = ResultSet::new(vec![self.message("字段", "Column"), self.message("类型", "Type")])?;
                    for row in &rows { structure.push(vec![row[0].clone(), row[1].clone()])?; }
                    let cancelled = self.cancelled.clone();
                    let text = format!("{table}\n{}", structure.clipboard(None, None, None, true, || cancelled.load(Ordering::Relaxed))?);
                    arboard::Clipboard::new().and_then(|mut clipboard| clipboard.set_text(text)).map_err(|error| error.to_string())?;
                }
                let details: Vec<FieldRow> = rows.iter().enumerate().map(|(index, row)| FieldRow {
                    name: row[0].as_deref().unwrap_or_default().into(),
                    datatype: row[1].as_deref().unwrap_or_default().into(),
                    added: added.get(index).copied().unwrap_or(false),
                }).collect();
                let field_names = rows
                    .iter()
                    .filter_map(|row| row[0].clone())
                    .collect::<Vec<_>>();
                let fields = field_names.clone();
                let title = if schema.is_empty() { table.clone() } else { format!("{schema}.{table}") };
                update(&self.ui, move |ui| {
                    use slint::Model;
                    if ui.get_schema() != schema.as_str() { return; }
                    let mut index: Vec<String> = ui.get_column_index().iter().map(|entry| entry.to_string()).filter(|entry| serde_json::from_str::<(String, Vec<String>)>(entry).is_ok_and(|(name, _)| name != table)).collect();
                    index.push(serde_json::to_string(&(&table, &field_names)).unwrap());
                    ui.set_column_index(strings(index));
                    ui.set_completion_revision(ui.get_completion_revision() + 1);
                    if action == "completion" { filter_objects(&ui); return; }
                    if action.is_empty()
                        && (ui.get_schema() != schema.as_str()
                            || ui
                                .get_tables()
                                .row_data(ui.get_table_index() as usize)
                                .is_none_or(|item| item.text != table.as_str()))
                    {
                        return;
                    }
                    ui.set_fields_loading(false);
                    ui.set_field_panel_hidden(false);
                    ui.set_fields(items(fields));
                    ui.set_field_details(std::rc::Rc::new(VecModel::from(details)).into());
                    let mut changed: Vec<_> = ui.get_changed_tables().iter().map(|name| name.to_string()).collect();
                    changed.retain(|name| name != &table);
                    if table_changed { changed.push(table.clone()); }
                    ui.set_changed_tables(strings(changed));
                    filter_objects(&ui);
                    ui.set_object_title(title.into());
                    ui.set_object_name(table.into());
                    ui.set_field_names(strings(field_names));
                    if action == "quick-group" {
                        ui.set_group_search("".into());
                        ui.set_group_selected(ModelRc::default());
                        ui.set_group_selected_index(-1);
                        filter_group(&ui);
                        ui.set_group_open(true);
                    }
                });
                Ok(if copy_structure { self.message("已复制表结构", "Table structure copied") } else { self.message("已加载字段", "Columns loaded") })
            }
            Command::Run(sql, timeout) => {
                if sql.trim().is_empty() {
                    return Err(self.message("SQL 不能为空", "SQL is empty"));
                }
                if sql.len() > 2 * 1024 * 1024 {
                    return Err(self.message("SQL 超过 2 MiB 上限", "SQL exceeds 2 MiB"));
                }
                let client = self.prepare_session().await?;
                let labels = syntax::dq_labels(&sql, &mut 0);
                self.sets.clear();
                update_result(&self.ui, self.result_tab, |ui| ui.set_query_phase(1));
                let progress_ui = self.ui.clone();
                let document_id = self.result_tab;
                let result = guarded(
                    db::execute_limited(&client, &sql, move |rows| {
                        update_result(&progress_ui, document_id, move |ui| {
                            ui.set_query_rows(rows.to_string().into());
                            ui.set_query_phase(2);
                        });
                    }, usize::MAX, results::MAX_BYTES),
                    self.cancelled.clone(), timeout,
                ).await;
                let mut sets = match result {
                    Ok((sets, _)) => sets,
                    Err(error) => {
                        if !error.recoverable { self.invalidate().await; }
                        return Err(error.message);
                    }
                };
                for set in &mut sets {
                    set.name = set.statements.iter().filter_map(|&statement| labels.get(statement).filter(|label| !label.is_empty())).cloned().collect::<Vec<_>>().join(" / ");
                }
                self.sets = sets;
                let names = self.sets.iter().enumerate()
                    .map(|(i, set)| if !set.name.is_empty() { set.name.clone() } else if set.repeats > 1 { format!("{} × {}", i + 1, set.repeats) } else { (i + 1).to_string() })
                    .collect();
                update_result(&self.ui, self.result_tab, move |ui| ui.set_result_names(strings(names)));
                if !self.sets.is_empty() {
                    self.page(0, 0)?;
                }
                Ok(String::new())
            }
            Command::History(_) => Ok(String::new()),
            Command::Sort(index, column, descending) => {
                let cancelled = self.cancelled.clone();
                self.sets.get_mut(index).ok_or("No result set")?.sort(column, descending, || cancelled.load(Ordering::Relaxed))?;
                self.page(index, 0)?;
                Ok(self.message("已排序全部结果", "All result rows sorted"))
            }
            Command::Page(index, page) => {
                self.page(index, page)?;
                Ok(self.message("已加载页面", "Page loaded"))
            }
            Command::CopyCell(index, page, row, column) => {
                let rows = self.sets.get_mut(index).ok_or("No result set")?.page(page, false, Some(row))?;
                let cell = rows.first().and_then(|row| row.get(column)).ok_or("No selected cell")?;
                arboard::Clipboard::new()
                    .and_then(|mut clipboard| clipboard.set_text(cell.as_deref().unwrap_or("NULL").to_string()))
                    .map_err(|error| error.to_string())?;
                Ok(self.message("已复制", "Copied"))
            }
            Command::Copy(index, page, rows, cells, headers) => {
                let set = self.sets.get_mut(index).ok_or("No result set")?;
                if rows.as_ref().is_some_and(|rows| rows.is_empty()) {
                    return Err("No selected row".into());
                }
                let cancelled = self.cancelled.clone();
                let text = set.clipboard(page, rows.as_deref(), cells.as_deref(), headers, || cancelled.load(Ordering::Relaxed))?;
                arboard::Clipboard::new()
                    .map_err(|e| e.to_string())?
                    .set_text(text)
                    .map_err(|e| e.to_string())?;
                let message = if headers { self.message("已复制原始数据（含标题行）", "Copied original data with headers") } else { self.message("已复制原始数据", "Copied original data") };
                Ok(message)
            }
            Command::Export(index) => {
                let title = self.message("导出完整结果", "Export complete result");
                if let Some(path) = rfd::FileDialog::new()
                    .set_title(title)
                    .add_filter("CSV", &["csv"])
                    .set_file_name("result.csv")
                    .save_file()
                {
                    let cancelled = self.cancelled.clone();
                    self.sets
                        .get_mut(index)
                        .ok_or("No result set")?
                        .export(&path, || cancelled.load(Ordering::Relaxed))?;
                    self.sets[index].exported = true;
                    Ok(format!(
                        "{}: {}",
                        self.message("导出成功", "Exported"),
                        path.display()
                    ))
                } else {
                    Ok(self.message("已取消文件选择", "File selection cancelled"))
                }
            }
            Command::ImportSql => {
                if let Some(path) = rfd::FileDialog::new()
                    .set_title(self.message("导入 SQL（UTF-8）", "Import SQL (UTF-8)"))
                    .add_filter("SQL", &["sql"])
                    .pick_file()
                {
                    let mut data = Vec::new();
                    std::fs::File::open(&path)
                        .map_err(|e| e.to_string())?
                        .take(2 * 1024 * 1024 + 1)
                        .read_to_end(&mut data)
                        .map_err(|e| e.to_string())?;
                    if data.len() > 2 * 1024 * 1024 {
                        return Err(
                            self.message("SQL 文件超过 2 MiB 上限", "SQL file exceeds 2 MiB")
                        );
                    }
                    let sql = String::from_utf8(data).map_err(|_| {
                        self.message("SQL 文件必须为 UTF-8 编码", "SQL file must be UTF-8")
                    })?;
                    let sql = sql.trim_start_matches('\u{feff}').to_owned();
                    let path_text = path.to_string_lossy().into_owned();
                    update(&self.ui, move |ui| {
                        ui.invoke_file_loaded(path_text.into(), sql.into())
                    });
                    Ok(format!(
                        "{}: {}",
                        self.message("已导入", "Imported"),
                        path.display()
                    ))
                } else {
                    Ok(self.message("已取消文件选择", "File selection cancelled"))
                }
            }
            Command::SaveSql(sql, name) => {
                if let Some(path) = rfd::FileDialog::new()
                    .set_title(self.message("导出 SQL", "Export SQL"))
                    .add_filter("SQL", &["sql"])
                    .set_file_name(sql_file_name(&name))
                    .save_file()
                {
                    save_sql(&path, &sql)?;
                    Ok(format!(
                        "{}: {}",
                        self.message("已保存", "Saved"),
                        path.display()
                    ))
                } else {
                    Ok(self.message("已取消文件选择", "File selection cancelled"))
                }
            }
            Command::SaveProfile(mut profile, password, selected) => {
                if profile.name.is_empty() {
                    return Err(self.message("连接名称不能为空", "Connection name is required"));
                }
                if let Some(existing) = selected.and_then(|index| self.settings.profiles.get(index)) {
                    let latest = config::load()?;
                    if let Some(saved) = latest.profiles.iter().find(|saved| saved.name == existing.name) {
                        profile.shared_id = saved.shared_id.clone();
                        profile.extra = saved.extra.clone();
                    }
                }
                let old = self.settings.clone();
                let selected = selected.filter(|index| *index < self.settings.profiles.len());
                if self
                    .settings
                    .profiles
                    .iter()
                    .enumerate()
                    .any(|(index, saved)| saved.name == profile.name && Some(index) != selected)
                {
                    return Err(self.message(
                        "连接名称已存在，请使用不同名称",
                        "Connection name already exists",
                    ));
                }
                if selected.is_none() && old.profiles.len() >= 128 {
                    return Err(
                        self.message("最多保存 128 个连接", "At most 128 saved connections")
                    );
                }
                let old_password = config::password(&profile.name)?;
                config::save_password(&profile.name, &password)?;
                let name = profile.name.clone();
                let index = selected.unwrap_or(self.settings.profiles.len());
                let original_name =
                    selected.map(|index| self.settings.profiles[index].name.clone());
                if index == self.settings.profiles.len() {
                    self.settings.profiles.push(profile);
                } else {
                    self.settings.profiles[index] = profile;
                }
                if let Some(original) = &original_name {
                    if original != &name {
                        if let Some(filter) = self.settings.prefix_filters.remove(original) {
                            self.settings.prefix_filters.insert(name.clone(), filter);
                        }
                    }
                    for object in &mut self.settings.hidden_objects {
                        if object.scope.profile == *original { object.scope.profile = name.clone(); }
                    }
                    for scope in &mut self.settings.browser_scopes {
                        if scope.profile == *original { scope.profile = name.clone(); }
                    }
                    if let Some(last) = &mut self.settings.last_selection {
                        if last.profile == *original {
                            last.profile = name.clone();
                        }
                    }
                }
                if let Err(e) = config::save(&self.settings) {
                    self.settings = old;
                    return match config::save_password(&name, &old_password) {
                        Ok(()) => Err(e),
                        Err(restore) => Err(format!("{e}\n{restore}")),
                    };
                }
                if let Some(original) = &original_name {
                    if let Some((active, _)) = &mut self.active {
                        if active.name == *original {
                            active.name = name.clone();
                            if let Some(session) = &mut self.session_profile {
                                session.name = name.clone();
                            }
                        }
                    }
                }
                self.sync_profiles();
                let active_index = self.physical_index(&self.settings.visible_profiles());
                update(&self.ui, move |ui| {
                    ui.set_profile_index(index as i32);
                    ui.set_physical_index(active_index);
                });
                if let Some(original) = original_name {
                    if original != name {
                        config::delete_password(&original)?;
                    }
                }
                self.remember_selection()?;
                Ok(self.message(
                    "连接配置已保存，修改在重新连接后生效",
                    "Connection saved; changes apply after reconnecting",
                ))
            }
            Command::LoadProfile(index) => {
                let profile = self
                    .settings
                    .profiles
                    .get(index)
                    .ok_or("No profile")?
                    .clone();
                let password = config::password(&profile.name)?;
                update(&self.ui, move |ui| show_profile(&ui, profile, password));
                Ok(self.message(
                    "配置已加载，点击连接以使用",
                    "Profile loaded; click Connect to use it",
                ))
            }
            Command::DeleteProfile(index) => {
                let old = self.settings.clone();
                let profile = self
                    .settings
                    .profiles
                    .get(index)
                    .ok_or("No profile")?
                    .clone();
                self.settings.profiles.remove(index);
                self.settings.prefix_filters.remove(&profile.name);
                self.settings.hidden_objects.retain(|object| object.scope.profile != profile.name);
                self.settings.browser_scopes.retain(|scope| scope.profile != profile.name);
                if self
                    .settings
                    .last_selection
                    .as_ref()
                    .is_some_and(|last| last.profile == profile.name)
                {
                    self.settings.last_selection = None;
                }
                if let Err(e) = config::save(&self.settings) {
                    self.settings = old;
                    return Err(e);
                }
                if let Err(error) = config::delete_password(&profile.name) {
                    self.settings = old;
                    config::save(&self.settings)
                        .map_err(|restore| format!("{error}\n{restore}"))?;
                    return Err(error);
                }
                if self
                    .active
                    .as_ref()
                    .is_some_and(|(active, _)| active.name == profile.name)
                {
                    self.disconnect();
                }
                self.sync_profiles();
                let active_index = self.physical_index(&self.settings.visible_profiles());
                update(&self.ui, |ui| {
                    ui.set_profile_index(-1);
                    ui.set_profile_name("".into());
                    ui.set_password("".into());
                });
                update(&self.ui, move |ui| ui.set_physical_index(active_index));
                Ok(self.message("已删除保存的连接", "Saved connection deleted"))
            }
            Command::Theme(dark) => {
                let old = self.settings.dark_theme;
                self.settings.dark_theme = dark;
                if let Err(error) = config::save(&self.settings) {
                    self.settings.dark_theme = old;
                    return Err(error);
                }
                update(&self.ui, move |ui| { ui.global::<Theme>().set_dark(dark); ui.invoke_sync_theme_menu(); });
                Ok(self.message("主题已保存", "Theme saved"))
            }
            Command::SaveTimeout(timeout) => {
                let old = self.settings.query_timeout;
                self.settings.query_timeout = Some(timeout);
                if let Err(error) = config::save(&self.settings) { self.settings.query_timeout = old; return Err(error); }
                update(&self.ui, move |ui| { ui.set_timeout((timeout / 60) as i32); ui.set_timeout_open(false); });
                Ok(self.message("已保存", "Saved"))
            }
            Command::Language(zh) => {
                self.settings.language = Some(if zh { "zh" } else { "en" }.into());
                config::save(&self.settings)?;
                Ok(self.message("语言设置已保存", "Language saved"))
            }
        }
    }
}

fn save_sql(path: &Path, sql: &str) -> Result<(), String> {
    let mut file = tempfile::NamedTempFile::new_in(path.parent().ok_or("Invalid path")?)
        .map_err(|e| e.to_string())?;
    file.write_all(sql.as_bytes()).map_err(|e| e.to_string())?;
    file.as_file().sync_all().map_err(|e| e.to_string())?;
    file.persist(path).map_err(|e| e.to_string())?;
    Ok(())
}

fn explain(zh: bool, error: String) -> String {
    match error.as_str() {
        "PASSWORDS_BUSY" => tr(
            zh,
            "另一窗口正在更新密码文件，请稍后重试。",
            "Another window is updating the password file; please retry.",
        ),
        "TIMEOUT" => tr(
            zh,
            "操作超时，已断开会话。已提交的修改不会撤销。",
            "Operation timed out; session disconnected. Committed changes are not undone.",
        ),
        "CANCELLED" => tr(
            zh,
            "已取消操作",
            "Operation cancelled",
        ),
        "LIMIT_RESULT" => tr(
            zh,
            "结果超过 512 MiB，已停止查询并断开连接。请缩小查询范围。",
            "Results exceed 512 MiB; query stopped and session disconnected. Narrow the query.",
        ),
        "LIMIT_ROW" => tr(
            zh,
            "单行超过 4 MiB，查询已停止。请缩小返回字段。",
            "A row exceeds 4 MiB; query stopped. Select smaller fields.",
        ),
        "LIMIT_COLUMNS" => tr(
            zh,
            "最多支持 32 个结果集，每个最多 256 列。查询已停止。",
            "At most 32 result sets with 256 columns each; query stopped.",
        ),
        "LIMIT_METADATA" => tr(
            zh,
            "库表元数据超过上限（20,000 条或 8 MiB）。",
            "Metadata exceeds the limit (20,000 entries or 8 MiB).",
        ),
        "LIMIT_COPY" => tr(
            zh,
            "复制数据超过 8 MiB，请改用 CSV 导出。",
            "Clipboard data exceeds 8 MiB; use CSV export.",
        ),
        _ => error,
    }
}

fn submit(
    ui: &AppWindow,
    tx: &tokio::sync::mpsc::Sender<Request>,
    cancelled: &AtomicBool,
    command: Command,
) {
    let query = matches!(&command, Command::Run(..));
    let cancel_request = matches!(&command, Command::Cancel(..));
    let reconnect = matches!(&command, Command::Reconnect);
    let field_selection = matches!(&command, Command::Fields(_, _, action) if action.is_empty());
    let switch = is_switch(&command);
    // Running queries own their sessions, so switching the browser connection does not affect them.
    if (ui.get_busy() && !cancel_request && !matches!(&command, Command::RestoreScope(..))) || (query && ui.get_query_running())
        || (ui.get_switching() && !is_result_command(&command) && !matches!(&command, Command::RestoreScope(..))) {
        return;
    }
    let zh = ui.global::<I18n>().get_zh();
    cancelled.store(false, Ordering::Relaxed);
    let browser_refresh = match &command { Command::Refresh => 1, Command::RefreshSchema => 2, _ => 0 };
    let run_sql = if let Command::Run(sql, _) = &command { Some(sql.clone()) } else { None };
    match tx.try_send(Request { command, zh, started: Instant::now(), document_id: ui.get_active_document(), open_documents: ui.get_sql_tabs().iter().map(|tab| tab.id).collect() }) {
        Ok(()) => {
            if reconnect { ui.set_reconnecting(true); }
            if query { ui.invoke_run_submitted(run_sql.unwrap().into()); clear_results(ui); ui.invoke_query_started(); ui.invoke_query_state_changed(ui.get_active_document(), true); }
            if switch { ui.set_switching(true); }
            else if !query && !cancel_request && !field_selection { ui.set_busy(true); }
            if browser_refresh != 0 { ui.set_browser_refresh(browser_refresh); }
            ui.set_failed(false);
            ui.set_status("".into());
        }
        Err(e) => {
            if field_selection { ui.set_fields_loading(false); }
            ui.set_failed(true);
            ui.set_status(e.to_string().into());
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Slint's script fallback for Han picks a font missing many simplified glyphs (导/认/执); make YaHei the generic fallback.
    let yahei = std::path::Path::new(r"C:\Windows\Fonts\msyh.ttc");
    if std::env::var_os("SLINT_FONT_PATH").is_none() && yahei.is_file() {
        // SAFETY: runs first in main, before any other thread exists.
        unsafe { std::env::set_var("SLINT_FONT_PATH", yahei) };
    }
    slint::BackendSelector::new().with_winit_window_attributes_hook(|attributes| attributes.with_active(false)).select()?;
    let ui = AppWindow::new()?;
    ui.on_format_count(|count| {
        let mut formatted = String::new();
        for (index, digit) in count.chars().enumerate() {
            if index > 0 && (count.len() - index) % 3 == 0 { formatted.push(','); }
            formatted.push(digit);
        }
        formatted.into()
    });
    ui.on_selection_size(|selected_rows, selected_cells, column_count| {
        let columns = column_count.max(0) as usize;
        if columns == 0 { return SelectionSize::default(); }
        let mut rows = std::collections::HashSet::new();
        let mut fields = std::collections::HashSet::new();
        for (row, selected) in selected_rows.iter().enumerate() {
            if selected { rows.insert(row); fields.extend(0..columns); }
        }
        for (cell, selected) in selected_cells.iter().enumerate() {
            if selected { rows.insert(cell / columns); fields.insert(cell % columns); }
        }
        SelectionSize { rows: rows.len() as i32, columns: fields.len() as i32 }
    });
    let document_state = documents::bind(&ui);
    let (settings, error) = match config::load() {
        Ok(s) => (s, None),
        Err(e) => (Settings::default(), Some(e)),
    };
    let saved_maximized = settings.maximized;
    ui.set_timeout(settings.query_timeout.unwrap_or(3600).div_ceil(60).clamp(1, 3600) as i32);
    let zh = settings
        .language
        .as_deref()
        .map(|l| l == "zh")
        .unwrap_or_else(|| {
            sys_locale::get_locale()
                .unwrap_or_default()
                .starts_with("zh")
        });
    ui.global::<I18n>().set_zh(zh);
    ui.invoke_sync_language_menu();
    ui.global::<Theme>().set_dark(settings.dark_theme);
    ui.invoke_sync_theme_menu();
    ui.set_profiles(strings(
        settings.profiles.iter().map(|p| p.name.clone()).collect(),
    ));
    ui.set_visible_profiles(strings(settings.visible_profiles()));
    prefix_options(&ui);
    let restore_index = settings.last_selection.as_ref().and_then(|last| {
        settings
            .profiles
            .iter()
            .position(|p| p.name == last.profile)
    }).or_else(|| (!settings.profiles.is_empty()).then_some(0));
    if let Some(error) = &error {
        ui.set_failed(true);
        ui.set_status(error.as_str().into());
    } else {
        ui.set_status(
            tr(
                zh,
                "请在左侧「连接配置」填写数据库连接。",
                "Open Connections on the left to configure a database.",
            )
            .into(),
        );
    }
    let cancelled = Arc::new(AtomicBool::new(false));
    let closing = Arc::new(AtomicBool::new(false));
    let weak = ui.as_weak();
    let close_cancel = cancelled.clone();
    let close_pending = closing.clone();
    ui.window().on_close_requested(move || {
        if let Some(ui) = weak.upgrade() {
            if ui.get_document_saving() {
                ui.set_failed(true);
                ui.set_status(tr(ui.global::<I18n>().get_zh(), "请先完成或取消保存", "Finish or cancel the save first").into());
                return slint::CloseRequestResponse::KeepWindowShown;
            }
            if ui.get_busy() || ui.get_switching() || ui.get_sql_tabs().iter().any(|tab| tab.running) {
                close_pending.store(true, Ordering::Relaxed);
                close_cancel.store(true, Ordering::Relaxed);
                ui.set_status(
                    tr(
                        ui.global::<I18n>().get_zh(),
                        "正在取消并退出；若有文件对话框，请先关闭。",
                        "Cancelling and closing; close any open file dialog.",
                    )
                    .into(),
                );
                return slint::CloseRequestResponse::KeepWindowShown;
            }
        }
        if let Some(ui) = weak.upgrade() {
            ui.invoke_close_documents();
        }
        slint::CloseRequestResponse::KeepWindowShown
    });
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Request>(64);
    let weak = ui.as_weak();
    let sender = tx.clone();
    let cancel = cancelled.clone();
    ui.on_restore_query_scope(move |profile, database, schema| {
        if let Some(ui) = weak.upgrade() {
            submit(&ui, &sender, &cancel, Command::RestoreScope(profile.to_string(), database.to_string(), schema.to_string()));
        }
    });
    let weak = ui.as_weak();
    let cancel = cancelled.clone();
    let worker = std::thread::Builder::new()
        .name("sql-worker".into())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build();
            let runtime = match runtime {
                Ok(r) => r,
                Err(e) => {
                    update(&weak, move |ui| {
                        ui.set_failed(true);
                        ui.set_status(e.to_string().into());
                        ui.set_busy(false);
                        ui.set_browser_refresh(0);
                    });
                    return;
                }
            };
            runtime.block_on(async move {
                let mut worker = Worker {
                    ui: weak,
                    settings,
                    session: None,
                    session_profile: None,
                    active: None,
                    sets: Vec::new(),
                    tab_sets: std::collections::HashMap::new(),
                    result_tab: -1,
                    cancelled: cancel,
                    zh,
                    settings_error: error,
                    current_schema: String::new(),
                    metadata_cache: cache::MetadataCache::load(),
                    query_only: false,
                    idle_sessions: Vec::new(),
                    browse: Browse::default(),
                    all_index: std::collections::HashMap::new(),
                };
                let mut queries = tokio::task::JoinSet::<(i32, Worker, Result<String, String>, Instant)>::new();
                let mut sessions = std::collections::HashMap::<i32, TabSessions>::new();
                let mut histories = std::collections::HashMap::<i32, (std::collections::VecDeque<Vec<ResultSet>>, usize)>::new();
                // Connection/browser changes run here; runs submitted meanwhile wait in `pending`.
                let mut switches = tokio::task::JoinSet::<(Worker, Result<String, String>, Finish)>::new();
                let mut pending: Vec<Request> = Vec::new();
                let mut pending_scope: Option<Request> = None;
                let mut cancellations = std::collections::HashMap::<i32, Arc<AtomicBool>>::new();
                let mut open_documents = std::collections::HashSet::new();
                let mut heartbeat = tokio::time::interval(Duration::from_millis(250));
                let mut reconnects = tokio::task::JoinSet::new();
                let mut prefetches = tokio::task::JoinSet::new();
                let mut last_reconnect = Instant::now();
                loop {
                    let request = tokio::select! {
                        _ = async {}, if pending_scope.is_some() && switches.is_empty() => pending_scope.take().unwrap(),
                        request = rx.recv() => { let Some(request) = request else { break; }; request },
                        completed = queries.join_next(), if !queries.is_empty() => {
                            if let Some(Ok((id, mut query, result, started))) = completed {
                                cancellations.remove(&id);
                                if !open_documents.contains(&id) { continue; }
                                sessions.insert(id, TabSessions { session: query.session.take(), profile: query.session_profile.take() });
                                let previous = if worker.result_tab == id { std::mem::take(&mut worker.sets) } else { worker.tab_sets.remove(&id).unwrap_or_default() };
                                let (history, selected) = histories.entry(id).or_default();
                                if let Some(slot) = history.get_mut(*selected) { *slot = previous; }
                                history.push_back(Vec::new());
                                if history.len() > 20 { history.pop_front(); }
                                *selected = history.len() - 1;
                                if worker.result_tab == id { worker.sets = std::mem::take(&mut query.sets); }
                                else { worker.tab_sets.insert(id, std::mem::take(&mut query.sets)); }
                                let elapsed = started.elapsed().as_secs_f32();
                                let scope = if result.is_ok() { query.active.as_ref().map(|(profile, _)| (profile.name.clone(), profile.database.clone(), query.current_schema.clone())) } else { None };
                                update_result(&worker.ui, id, move |ui| {
                                    if let Some((profile, database, schema)) = scope { ui.invoke_query_scope_succeeded(id, profile.into(), database.into(), schema.into()); }
                                    ui.set_query_running(false);
                                    ui.set_query_elapsed(elapsed);
                                    ui.set_query_phase(if result.is_ok() { 3 } else { 4 });
                                    ui.set_query_error(result.err().unwrap_or_default().into());
                                    ui.invoke_query_state_changed(id, false);
                                });
                            }
                            continue;
                        },
                        switched = switches.join_next(), if !switches.is_empty() => {
                            match switched {
                                Some(Ok((mut browser, result, finish))) => {
                                    worker.session = browser.session.take();
                                    worker.session_profile = browser.session_profile.take();
                                    worker.idle_sessions = std::mem::take(&mut browser.idle_sessions);
                                    worker.active = browser.active.take();
                                    worker.current_schema = std::mem::take(&mut browser.current_schema);
                                    worker.metadata_cache = std::mem::take(&mut browser.metadata_cache);
                                    worker.settings = std::mem::take(&mut browser.settings);
                                    worker.browse = browser.browse;
                                    worker.all_index = std::mem::take(&mut browser.all_index);
                                    worker.sync_profiles();
                                    if result.is_ok() { last_reconnect = Instant::now() - Duration::from_secs(5); }
                                    finish_request(&mut worker, &mut prefetches, finish, result);
                                }
                                _ => update(&worker.ui, |ui| { ui.set_switching(false); ui.set_browser_refresh(0); }),
                            }
                            for request in if pending_scope.is_none() { std::mem::take(&mut pending) } else { Vec::new() } {
                                if open_documents.contains(&request.document_id) {
                                    spawn_query(&worker, request, &mut queries, &mut sessions, &mut cancellations);
                                }
                            }
                            continue;
                        },
                        prefetched = prefetches.join_next(), if !prefetches.is_empty() => {
                            if let Some(Ok(Err(error))) = prefetched {
                                let zh = worker.zh;
                                update(&worker.ui, move |ui| ui.invoke_notify(format!("{}: {}", tr(zh, "元数据同步失败", "Metadata sync failed"), explain(zh, error)).into()));
                            }
                            continue;
                        },
                        reconnected = reconnects.join_next(), if !reconnects.is_empty() => {
                            if let Some(Ok((profile, Ok(session)))) = reconnected {
                                if switches.is_empty() && worker.active.as_ref().is_some_and(|(active, _)| active == &profile)
                                    && worker.session.as_ref().is_none_or(|session| session.client.is_closed()) {
                                    worker.session = Some(session);
                                    worker.session_profile = Some(profile);
                                    update(&worker.ui, |ui| {
                                        ui.set_connected(true);
                                        ui.set_reconnecting(false);
                                        ui.set_busy(true);
                                    });
                                    let result = worker.reload().await;
                                    let connected = worker.session.as_ref().is_some_and(|session| !session.client.is_closed());
                                    let zh = worker.zh;
                                    update(&worker.ui, move |ui| {
                                        ui.set_busy(false);
                                        ui.set_connected(connected);
                                        if let Err(error) = result {
                                            ui.set_failed(true);
                                            ui.set_status(explain(zh, error).into());
                                        }
                                    });
                                }
                            }
                            update(&worker.ui, |ui| ui.set_reconnecting(false));
                            continue;
                        },
                        _ = heartbeat.tick() => {
                            if closing.load(Ordering::Relaxed) {
                                for cancel in cancellations.values() { cancel.store(true, Ordering::Relaxed); }
                                pending.clear();
                                if queries.is_empty() && switches.is_empty() {
                                    closing.store(false, Ordering::Relaxed);
                                    update(&worker.ui, |ui| ui.invoke_close_documents());
                                }
                            }
                            // The session is on loan to the running switch.
                            if !switches.is_empty() { continue; }
                            let connected = worker.session.as_ref().is_some_and(|session| !session.client.is_closed());
                            if !connected && !closing.load(Ordering::Relaxed) && reconnects.is_empty() && last_reconnect.elapsed() >= Duration::from_secs(5) {
                                if let Some((profile, password)) = worker.active.clone() {
                                    last_reconnect = Instant::now();
                                    update(&worker.ui, |ui| ui.set_reconnecting(true));
                                    reconnects.spawn(async move {
                                        let result = tokio::time::timeout(Duration::from_secs(20), db::connect(&profile, &password)).await
                                            .unwrap_or_else(|_| Err("TIMEOUT".into()));
                                        (profile, result)
                                    });
                                }
                            }
                            update(&worker.ui, move |ui| ui.set_connected(connected));
                            continue;
                        }
                    };
                    open_documents = request.open_documents.iter().copied().collect();
                    if let Command::CloseDocument(id) = &request.command {
                        open_documents.remove(id);
                        sessions.remove(id);
                        histories.remove(id);
                        worker.tab_sets.remove(id);
                        if worker.result_tab == *id { worker.sets.clear(); }
                        if let Some(cancel) = cancellations.get(id) { cancel.store(true, Ordering::Relaxed); }
                        pending.retain(|queued| queued.document_id != *id);
                        continue;
                    }
                    if let Command::Cancel(id) = &request.command {
                        if let Some(cancel) = cancellations.get(id) { cancel.store(true, Ordering::Relaxed); }
                        if let Some(index) = pending.iter().position(|queued| queued.document_id == *id) {
                            let queued = pending.remove(index);
                            let (id, elapsed) = (*id, queued.started.elapsed().as_secs_f32());
                            update_result(&worker.ui, id, move |ui| {
                                ui.invoke_run_not_executed(id);
                                ui.set_query_running(false);
                                ui.set_query_elapsed(elapsed);
                                ui.set_query_phase(4);
                                ui.set_query_error("CANCELLED".into());
                                ui.invoke_query_state_changed(id, false);
                            });
                        }
                        continue;
                    }
                    sessions.retain(|id, _| request.open_documents.contains(id));
                    for (id, cancel) in &cancellations {
                        if !request.open_documents.contains(id) { cancel.store(true, Ordering::Relaxed); }
                    }
                    pending.retain(|queued| request.open_documents.contains(&queued.document_id));
                    if matches!(&request.command, Command::Run(..)) {
                        // A run submitted while the connection is switching uses the new selection.
                        if !switches.is_empty() {
                            if !pending.iter().any(|queued| queued.document_id == request.document_id) { pending.push(request); }
                        } else {
                            spawn_query(&worker, request, &mut queries, &mut sessions, &mut cancellations);
                        }
                        continue;
                    }
                    if is_switch(&request.command) {
                        if !switches.is_empty() {
                            if matches!(&request.command, Command::RestoreScope(..)) { pending_scope = Some(request); }
                            continue;
                        }
                        worker.zh = request.zh;
                        let finish = Finish::new(&request.command);
                        let mut browser = Worker {
                            ui: worker.ui.clone(), settings: worker.settings.clone(), session: worker.session.take(),
                            session_profile: worker.session_profile.take(), active: worker.active.clone(), sets: Vec::new(),
                            tab_sets: std::collections::HashMap::new(), result_tab: worker.result_tab, cancelled: worker.cancelled.clone(),
                            zh: worker.zh, settings_error: worker.settings_error.clone(), current_schema: worker.current_schema.clone(),
                            metadata_cache: std::mem::take(&mut worker.metadata_cache), query_only: false, idle_sessions: std::mem::take(&mut worker.idle_sessions),
                            browse: worker.browse, all_index: std::mem::take(&mut worker.all_index),
                        };
                        let command = request.command;
                        switches.spawn(async move {
                            let result = browser.handle(command).await;
                            (browser, result, finish)
                        });
                        continue;
                    }
                    if worker.result_tab != request.document_id {
                        worker.tab_sets.insert(worker.result_tab, std::mem::take(&mut worker.sets));
                        worker.sets = worker.tab_sets.remove(&request.document_id).unwrap_or_default();
                        worker.result_tab = request.document_id;
                    }
                    worker.tab_sets.retain(|id, _| request.open_documents.contains(id));
                    histories.retain(|id, _| request.open_documents.contains(id));
                    if let Command::History(index) = &request.command {
                        if let Some((history, selected)) = histories.get_mut(&request.document_id) {
                            if *index < history.len() {
                                history[*selected] = std::mem::take(&mut worker.sets);
                                worker.sets = std::mem::take(&mut history[*index]);
                                *selected = *index;
                                let index = *index as i32;
                                let id = request.document_id;
                                update(&worker.ui, move |ui| ui.invoke_history_selected(id, index));
                            }
                        }
                    }
                    worker.zh = request.zh;
                    let finish = Finish::new(&request.command);
                    let result = worker.handle(request.command).await;
                    // During a switch the session is on loan; keep the connection indicator as it is.
                    let finish = Finish { keep_connection: !switches.is_empty(), ..finish };
                    finish_request(&mut worker, &mut prefetches, finish, result);
                }
            });
        })?;
    let weak = ui.as_weak();
    let sender = tx.clone();
    let cancel = cancelled.clone();
    ui.on_action(move |action| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if action == "copy-error" {
            match arboard::Clipboard::new().and_then(|mut clipboard| clipboard.set_text(explain(ui.global::<I18n>().get_zh(), ui.get_displayed_error().to_string()))) {
                Ok(()) => ui.invoke_notify(tr(ui.global::<I18n>().get_zh(), "已复制", "Copied").into()),
                Err(error) => { ui.set_failed(true); ui.set_status(error.to_string().into()); }
            }
            return;
        }
        if documents::action(&ui, &document_state, &action) { return; }
        if ui.get_save_prompt_open() { return; }
        if ui.get_document_read_only() && matches!(action.as_str(), "run" | "generate-group" | "quick-limit" | "history-prev" | "history-next") { return; }
        if action == "view-definition" {
            if ui.get_busy() || ui.get_switching() || ui.get_document_saving() { return; }
            if let Some(table) = ui.get_tables().row_data(ui.get_table_index() as usize) {
                let schema = ui.get_schema().to_string();
                let title = if schema.is_empty() { table.text.to_string() } else { format!("{schema}.{}", table.text) };
                if let Some(id) = documents::open_definition(&ui, &document_state, title) {
                    submit(&ui, &sender, &cancel, Command::Definition(id, schema, table.text.to_string(), ui.get_object_kind()));
                }
            }
            return;
        }
        if action == "add-scope" {
            use slint::Model;
            let additions: Vec<BrowserScope> = ui.get_scope_schemas().row_data(ui.get_scope_schema_index() as usize).into_iter()
                .map(|choice| BrowserScope { profile: choice.profile, database: choice.database, schema: choice.schema }).collect();
            if additions.is_empty() || additions.iter().any(|scope| scope.profile.is_empty() || scope.database.is_empty() || scope.schema.is_empty()) {
                ui.set_failed(true);
                ui.set_status(tr(ui.global::<I18n>().get_zh(), "请先选择连接、库名和模式", "Select connections, databases and schemas first").into());
                return;
            }
            let mut scopes: Vec<_> = ui.get_browser_scopes().iter().collect();
            let before = scopes.len();
            for scope in additions {
                if !scopes.iter().any(|saved| saved.profile == scope.profile && saved.database == scope.database && saved.schema == scope.schema) {
                    scopes.push(scope);
                }
            }
            let count = scopes.len() - before;
            ui.set_browser_scopes(std::rc::Rc::new(VecModel::from(scopes)).into());
            ui.invoke_notify(format!("{}: {count}", tr(ui.global::<I18n>().get_zh(), "已添加组合", "Combinations added")).into());
            return;
        }
        if matches!(action.as_str(), "add-prefix" | "remove-prefix" | "clear-prefix") {
            use slint::Model;
            ui.set_show_hidden(false);
            let mut prefixes: Vec<String> = ui.get_table_prefixes().iter().map(|prefix| prefix.to_string()).collect();
            if action == "add-prefix" {
                let prefix = ui.get_object_search().trim().to_string();
                if prefix.is_empty() { return; }
                let index = prefixes.iter().position(|saved| saved == &prefix).unwrap_or_else(|| { prefixes.push(prefix.clone()); prefixes.len() - 1 });
                ui.set_active_prefix(prefix.into());
                ui.set_prefix_index(index as i32 + 1);
                ui.set_object_search("".into());
            } else if action == "remove-prefix" {
                let index = (ui.get_prefix_index() - 1) as usize;
                if index < prefixes.len() { prefixes.remove(index); }
                ui.set_active_prefix("".into());
                ui.set_prefix_index(0);
            } else if action == "clear-prefix" {
                ui.set_active_prefix("".into());
                ui.set_prefix_index(0);
            }
            ui.set_table_prefixes(strings(prefixes.clone()));
            prefix_options(&ui);
            let selected = (!ui.get_active_prefix().is_empty()).then(|| ui.get_active_prefix().to_string());
            filter_objects(&ui);
            submit(&ui, &sender, &cancel, Command::SavePrefixes(prefixes, selected));
            return;
        }
        if action == "cancel" {
            if ui.get_query_running() { submit(&ui, &sender, &cancel, Command::Cancel(ui.get_active_document())); }
            else { cancel.store(true, Ordering::Relaxed); }
            return;
        }
        if action == "new-profile" {
            ui.set_profile_index(-1);
            show_profile(&ui, Profile { port: 5432, tls: true, ..Profile::default() }, String::new());
            return;
        }
        if matches!(action.as_str(), "add-group" | "remove-group" | "group-up" | "group-down") {
            use slint::Model;
            let mut selected: Vec<_> = ui.get_group_selected().iter().collect();
            let mut index = ui.get_group_selected_index();
            if action == "add-group" {
                let Some(field) = ui.get_group_fields().row_data(ui.get_group_index() as usize) else { return; };
                if selected.iter().any(|item| item.text == field.text) { return; }
                selected.push(field);
                index = selected.len() as i32 - 1;
            } else {
                if index < 0 || index as usize >= selected.len() { return; }
                if action == "remove-group" {
                    selected.remove(index as usize);
                    index = index.min(selected.len() as i32 - 1);
                } else {
                    let next = index + if action == "group-up" { -1 } else { 1 };
                    if next < 0 || next as usize >= selected.len() { return; }
                    selected.swap(index as usize, next as usize);
                    index = next;
                }
            }
            ui.set_group_selected(std::rc::Rc::new(VecModel::from(selected)).into());
            ui.set_group_selected_index(index);
            filter_group(&ui);
            return;
        }
        if action == "generate-group" {
            use slint::Model;
            let fields: Vec<_> = ui.get_group_selected().iter().map(|item| item.text.to_string()).collect();
            if fields.is_empty() { return; }
            let field = fields.join(", ");
            ui.set_sql(format!("SELECT {field}, COUNT(*) AS row_count\nFROM {}\nGROUP BY {field}\nORDER BY COUNT(*) DESC;", ui.get_object_name()).into());
            ui.set_group_open(false);
            return;
        }
        if matches!(action.as_str(), "copy-structure" | "quick-group" | "quick-limit" | "copy-table") {
            use slint::Model;
            if let Some(table) = ui.get_tables().row_data(ui.get_table_index() as usize) {
                if action == "copy-table" {
                    let result = arboard::Clipboard::new().and_then(|mut clipboard| clipboard.set_text(table.text.to_string()));
                    match result {
                        Ok(()) => {
                            let message = tr(ui.global::<I18n>().get_zh(), "已复制表名", "Object name copied");
                            ui.invoke_notify(format!("{message}: {}", table.text).into());
                        }
                        Err(error) => { ui.set_failed(true); ui.set_status(error.to_string().into()); }
                    }
                } else if action == "quick-limit" {
                    ui.set_sql(format!("SELECT *\nFROM {}\nLIMIT 10;", table.text).into());
                } else {
                    submit(&ui, &sender, &cancel, Command::Fields(ui.get_schema().to_string(), table.text.to_string(), action.to_string()));
                }
            }
            return;
        }
        let result = match action.as_str() {
            "save-timeout" => ui.get_timeout_input().trim().parse::<u64>().ok().filter(|timeout| (1..=3600).contains(timeout)).map(|minutes| Command::SaveTimeout(minutes * 60))
                .ok_or_else(|| tr(ui.global::<I18n>().get_zh(), "请输入 1–3600 之间的整数分钟数", "Enter a whole number of minutes from 1 to 3600")),
            "scope-settings" => Ok(Command::OpenScopes),
            "save-scopes" => {
                use slint::Model;
                Ok(Command::SaveScopes(ui.get_browser_scopes().iter().map(|scope| config::BrowserSelection {
                    profile: scope.profile.to_string(), database: scope.database.to_string(), schema: scope.schema.to_string(),
                }).collect()))
            }
            "connect" => {
                profile_from_ui(&ui).map(|p| Command::Connect(p, ui.get_password().to_string()))
            }
            "reconnect" => Ok(Command::Reconnect),
            "save-profile" => {
                profile_from_ui(&ui).map(|p| Command::SaveProfile(p, ui.get_password().to_string(), (ui.get_profile_index() >= 0).then_some(ui.get_profile_index() as usize)))
            }
            "delete-profile" => Ok(Command::DeleteProfile(ui.get_profile_index() as usize)),
            "disconnect" => Ok(Command::Disconnect),
            "refresh" => Ok(Command::Refresh),
            "refresh-schema" => Ok(Command::RefreshSchema),
            "hide-object" | "restore-object" => ui.get_tables().row_data(ui.get_table_index() as usize)
                .map(|object| Command::SetObjectHidden(object.text.to_string(), ui.get_object_kind(), action == "hide-object"))
                .ok_or_else(|| tr(ui.global::<I18n>().get_zh(), "请先选择对象", "Select an object first")),
            "run" => Ok(Command::Run(
                ui.get_sql().to_string(),
                ui.get_timeout().clamp(1, 3600) as u64 * 60,
            )),
            "history-prev" => Ok(Command::History(ui.get_history_index().saturating_sub(1).max(0) as usize)),
            "history-next" => Ok(Command::History((ui.get_history_index() + 1).max(0) as usize)),
            "import" => Ok(Command::ImportSql),
            "save-sql" => Ok(Command::SaveSql(ui.get_sql().to_string(), ui.get_document_title().to_string())),
            "export" => Ok(Command::Export(ui.get_result_index() as usize)),
            "copy-page" => Ok(Command::Copy(
                ui.get_result_index() as usize,
                Some(ui.get_page() as usize),
                None,
                None,
                ui.get_copy_headers(),
            )),
            "copy-all" => Ok(Command::Copy(
                ui.get_result_index() as usize,
                None,
                None,
                None,
                ui.get_copy_headers(),
            )),
            "copy-row" => {
                use slint::Model;
                let columns = ui.get_result_columns().row_count();
                let cells: Vec<_> = ui.get_selected_cells().iter().enumerate().filter_map(|(index, selected)| selected.then_some((index / columns.max(1), index % columns.max(1)))).collect();
                let mut rows: Vec<_> = if cells.is_empty() {
                    ui.get_selected_rows().iter().enumerate().filter_map(|(index, selected)| selected.then_some(index)).collect()
                } else { cells.iter().map(|(row, _)| *row).collect() };
                rows.dedup();
                Ok(Command::Copy(ui.get_result_index() as usize, Some(ui.get_page() as usize), Some(rows), (!cells.is_empty()).then_some(cells), ui.get_copy_headers()))
            }
            _ => return,
        };
        match result {
            Ok(command) => submit(&ui, &sender, &cancel, command),
            Err(error) => {
                ui.set_failed(true);
                ui.set_status(error.into());
            }
        }
    });
    let weak = ui.as_weak();
    ui.on_request_document_save(move |id, path, sql, name| {
        let Some(ui) = weak.upgrade() else { return; };
        if ui.get_document_saving() { return; }
        ui.set_document_saving(true);
        let zh = ui.global::<I18n>().get_zh();
        let weak = ui.as_weak();
        std::thread::spawn(move || {
            let result: Result<Option<PathBuf>, String> = (|| {
                let mut dialog = rfd::FileDialog::new().set_title(tr(zh, "保存 SQL", "Save SQL")).add_filter("SQL", &["sql"]).set_file_name(sql_file_name(name.as_str()));
                if let Some(parent) = Path::new(path.as_str()).parent().filter(|p| !p.as_os_str().is_empty()) { dialog = dialog.set_directory(parent); }
                let Some(path) = dialog.save_file() else { return Ok(None); };
                save_sql(&path, sql.as_str())?;
                Ok(Some(path))
            })();
            update(&weak, move |ui| match result {
                Ok(Some(path)) => { ui.invoke_document_saved(id, path.to_string_lossy().as_ref().into(), sql); ui.invoke_notify(tr(zh, "已保存", "Saved").into()); }
                Ok(None) => ui.invoke_document_save_cancelled(),
                Err(error) => { ui.invoke_document_save_cancelled(); ui.set_failed(true); ui.set_status(error.into()); }
            });
        });
    });
    let weak = ui.as_weak();
    let sender = tx.clone();
    let cancel = cancelled.clone();
    ui.on_profile_selected(move |index| {
        if let Some(ui) = weak.upgrade() {
            submit(&ui, &sender, &cancel, Command::LoadProfile(index as usize));
        }
    });
    let weak = ui.as_weak();
    ui.on_remove_scope(move |index| {
        use slint::Model;
        if let Some(ui) = weak.upgrade() {
            let scopes: Vec<_> = ui.get_browser_scopes().iter().enumerate().filter_map(|(row, scope)| (row != index as usize).then_some(scope)).collect();
            ui.set_browser_scopes(std::rc::Rc::new(VecModel::from(scopes)).into());
        }
    });
    let weak = ui.as_weak();
    ui.on_move_scope(move |from, to| {
        if let Some(ui) = weak.upgrade() {
            let mut scopes: Vec<_> = ui.get_browser_scopes().iter().collect();
            let (from, to) = (from as usize, to as usize);
            if from >= scopes.len() || to >= scopes.len() || from == to { return; }
            let scope = scopes.remove(from);
            scopes.insert(to, scope);
            ui.set_browser_scopes(std::rc::Rc::new(VecModel::from(scopes)).into());
        }
    });
    let weak = ui.as_weak();
    let sender = tx.clone();
    let cancel = cancelled.clone();
    ui.on_scope_choice(move |level, index, selected| {
        use slint::Model;
        if let Some(ui) = weak.upgrade() {
            if ui.get_busy() { return; }
            let mut profiles: Vec<_> = ui.get_scope_profiles().iter().collect();
            let mut databases: Vec<_> = ui.get_scope_databases().iter().collect();
            let mut schemas: Vec<_> = ui.get_scope_schemas().iter().collect();
            let choices = match level { 0 => &mut profiles, 1 => &mut databases, _ => &mut schemas };
            if index < 0 || index as usize >= choices.len() { return; }
            for choice in choices.iter_mut() { choice.selected = false; }
            choices[index as usize].selected = selected;
            if level == 2 {
                ui.set_scope_schemas(std::rc::Rc::new(VecModel::from(schemas)).into());
                return;
            }
            schemas.clear();
            ui.set_scope_schema_index(-1);
            ui.set_scope_schema_names(ModelRc::default());
            if level == 0 {
                databases.clear();
                ui.set_scope_database_index(-1);
                ui.set_scope_database_names(ModelRc::default());
            }
            ui.set_scope_profiles(std::rc::Rc::new(VecModel::from(profiles.clone())).into());
            ui.set_scope_databases(if level == 0 { ModelRc::default() } else { std::rc::Rc::new(VecModel::from(databases.clone())).into() });
            ui.set_scope_schemas(ModelRc::default());
            submit(&ui, &sender, &cancel, Command::ScopeChoices(level, profiles, databases, schemas));
        }
    });
    let weak = ui.as_weak();
    let sender = tx.clone();
    let cancel = cancelled.clone();
    ui.on_prefix_selected(move |index| {
        use slint::Model;
        if let Some(ui) = weak.upgrade() {
            if index as usize == ui.get_table_prefixes().row_count() + 1 {
                ui.set_show_hidden(true);
                ui.set_fields(ModelRc::default());
                ui.set_field_names(ModelRc::default());
                ui.set_table_index(-1);
                filter_objects(&ui);
                return;
            }
            ui.set_show_hidden(false);
            if index == 0 { ui.invoke_action("clear-prefix".into()); return; }
            if let Some(prefix) = ui.get_table_prefixes().row_data((index - 1) as usize) {
                ui.set_active_prefix(prefix);
                filter_objects(&ui);
                let prefixes = ui.get_table_prefixes().iter().map(|prefix| prefix.to_string()).collect();
                submit(&ui, &sender, &cancel, Command::SavePrefixes(prefixes, Some(ui.get_active_prefix().to_string())));
            }
        }
    });
    let weak = ui.as_weak();
    ui.on_filter_group(move || {
        if let Some(ui) = weak.upgrade() { filter_group(&ui); }
    });
    let weak = ui.as_weak();
    let sender = tx.clone();
    let cancel = cancelled.clone();
    ui.on_physical_selected(move |index| {
        use slint::Model;
        if let Some(ui) = weak.upgrade() {
            if let Some(name) = ui.get_visible_profiles().row_data(index as usize) {
                if let Some(index) = ui.get_profiles().iter().position(|profile| profile == name) {
                    submit(&ui, &sender, &cancel, Command::UseProfile(index));
                }
            }
        }
    });
    let weak = ui.as_weak();
    ui.on_filter_objects(move || {
        if let Some(ui) = weak.upgrade() {
            filter_objects(&ui);
        }
    });
    let weak = ui.as_weak();
    let completion_sender = tx.clone();
    let completion_cancel = cancelled.clone();
    ui.on_complete_sql(move |sql, offset| {
        let Some(ui) = weak.upgrade() else { return ModelRc::default(); };
        let index: Vec<(String, Vec<String>)> = ui.get_column_index().iter().filter_map(|entry| serde_json::from_str(entry.as_str()).ok()).collect();
        let tables: Vec<_> = ui.get_tables().iter().map(|object| (object.text.to_string(), index.iter().find(|(name, _)| name == object.text.as_str()).map(|(_, columns)| columns.clone()).unwrap_or_default())).collect();
        let mut missing = None;
        let suggestions = completion::suggest(&sql, offset.max(0) as usize, &tables, &ui.get_schema(), &mut missing);
        if let Some(table) = missing.filter(|_| !ui.get_busy() && !ui.get_switching() && ui.get_connected()) {
            submit(&ui, &completion_sender, &completion_cancel, Command::Fields(ui.get_schema().to_string(), table, "completion".into()));
        }
        strings(suggestions)
    });
    ui.on_insert_completion(|sql, offset, value| completion::insert(&sql, offset.max(0) as usize, &value));
    ui.on_indent_newline(|sql, anchor, cursor| {
        let start = anchor.min(cursor).max(0) as usize;
        let end = anchor.max(cursor).max(0) as usize;
        let line_start = sql[..start].rfind('\n').map_or(0, |index| index + 1);
        let indent = &sql[line_start..start];
        let indent = &indent[..indent.find(|c| c != ' ' && c != '\t').unwrap_or(indent.len())];
        SqlCompletion {
            text: format!("{}\n{indent}{}", &sql[..start], &sql[end..]).into(),
            offset: (start + 1 + indent.len()) as i32,
        }
    });
    ui.on_pair_quote(|sql, anchor, cursor, quote| {
        let start = anchor.min(cursor).max(0) as usize;
        let end = anchor.max(cursor).max(0) as usize;
        SqlCompletion {
            text: format!("{}{quote}{}{quote}{}", &sql[..start], &sql[start..end], &sql[end..]).into(),
            offset: (start + 1) as i32,
        }
    });
    let weak = ui.as_weak();
    ui.on_copy_text(move |text, show_name| {
        let Some(ui) = weak.upgrade() else { return; };
        match arboard::Clipboard::new().and_then(|mut clipboard| clipboard.set_text(text.to_string())) {
            Ok(()) => {
                let message = tr(ui.global::<I18n>().get_zh(), "已复制", "Copied");
                ui.invoke_notify(if show_name { format!("{message}: {text}").into() } else { message.into() });
            }
            Err(error) => { ui.set_failed(true); ui.set_status(error.to_string().into()); }
        }
    });
    let weak = ui.as_weak();
    ui.on_browser_select(move |fields, index, shift, control| {
        let Some(ui) = weak.upgrade() else { return; };
        let model = if fields { ui.get_fields() } else { ui.get_tables() };
        let previous = if fields { ui.get_field_selection() } else { ui.get_object_selection() };
        let anchor = if fields { ui.get_field_anchor() } else { ui.get_object_anchor() };
        let count = model.row_count();
        if index < -1 || index >= count as i32 { return; }
        let mut selected: Vec<bool> = (0..count).map(|row| previous.row_data(row).unwrap_or(false)).collect();
        if index == -1 {
            selected.fill(true);
        } else if shift && anchor >= 0 && anchor < count as i32 {
            if !control { selected.fill(false); }
            for row in anchor.min(index)..=anchor.max(index) { selected[row as usize] = true; }
        } else {
            if !control { selected.fill(false); }
            selected[index as usize] = !selected[index as usize];
        }
        let selected_count = selected.iter().filter(|selected| **selected).count() as i32;
        let selected = std::rc::Rc::new(VecModel::from(selected)).into();
        if fields {
            ui.set_field_selection(selected);
            ui.set_field_selection_count(selected_count);
            if !shift || anchor < 0 { ui.set_field_anchor(index.max(0)); }
        } else {
            ui.set_object_selection(selected);
            ui.set_object_selection_count(selected_count);
            if !shift || anchor < 0 { ui.set_object_anchor(index.max(0)); }
        }
    });
    let weak = ui.as_weak();
    ui.on_browser_copy(move |fields| {
        let Some(ui) = weak.upgrade() else { return; };
        let model = if fields { ui.get_fields() } else { ui.get_tables() };
        let selection = if fields { ui.get_field_selection() } else { ui.get_object_selection() };
        let names: Vec<_> = model.iter().enumerate().filter(|(row, _)| selection.row_data(*row).unwrap_or(false)).map(|(_, item)| item.text.to_string()).collect();
        if !names.is_empty() { ui.invoke_copy_text(names.join("\r\n").into(), false); }
    });
    ui.on_localize_error(|error, zh| explain(zh, error.to_string()).into());
    let weak = ui.as_weak();
    let sender = tx.clone();
    ui.on_document_closed(move |id| {
        if let Some(ui) = weak.upgrade() {
            let _ = sender.try_send(Request { command: Command::CloseDocument(id), zh: ui.global::<I18n>().get_zh(), started: Instant::now(), document_id: id, open_documents: ui.get_sql_tabs().iter().map(|tab| tab.id).filter(|open| *open != id).collect() });
        }
    });
    let weak = ui.as_weak();
    ui.on_find_sql({
        let weak = ui.as_weak();
        move |needle, current, backward| {
            let empty = SearchMatch { start: -1, end: -1, count: 0, ordinal: 0 };
            let Some(ui) = weak.upgrade() else { return empty; };
            if needle.is_empty() { return empty; }
            let sql = ui.get_sql().to_ascii_lowercase();
            let needle = needle.to_ascii_lowercase();
            let matches: Vec<usize> = sql.match_indices(&needle).map(|(start, _)| start).collect();
            if matches.is_empty() { return empty; }
            let index = if backward {
                matches.iter().rposition(|&start| (start as i32) < current).unwrap_or(matches.len() - 1)
            } else {
                matches.iter().position(|&start| (start as i32) > current).unwrap_or(0)
            };
            SearchMatch { start: matches[index] as i32, end: (matches[index] + needle.len()) as i32, count: matches.len() as i32, ordinal: index as i32 + 1 }
        }
    });
    let measure_ui = ui.as_weak();
    ui.on_sql_line_measured(move |sql, index, y| {
        let Some(ui) = measure_ui.upgrade() else { return; };
        if ui.get_sql() != sql || ui.get_highlighted_sql() != sql { return; }
        let Some(line) = ui.get_sql_lines().row_data(index as usize) else { return; };
        if let Some(mut run) = line.row_data(0) {
            if run.source != sql { return; }
            if run.line_y != y { run.line_y = y; line.set_row_data(0, run); }
        }
    });
    ui.on_highlight(move |sql| {
        if let Some(ui) = weak.upgrade() {
            if ui.get_highlighted_sql() == sql { return; }
            let lines: Vec<ModelRc<SqlRun>> = syntax::highlight(&sql).into_iter().map(|line| std::rc::Rc::new(VecModel::from(line)).into()).collect();
            ui.set_sql_lines(std::rc::Rc::new(VecModel::from(lines)).into());
            ui.set_highlighted_sql(sql);
        }
    });
    ui.invoke_highlight(ui.get_sql());
    let weak = ui.as_weak();
    let sender = tx.clone();
    let cancel = cancelled.clone();
    ui.on_database_selected(move |index, name| {
        if let Some(ui) = weak.upgrade() {
            let command = if index == 0 { Command::BrowseAll(1) } else { Command::SwitchDatabase(name.to_string()) };
            submit(&ui, &sender, &cancel, command);
        }
    });
    let weak = ui.as_weak();
    let sender = tx.clone();
    let cancel = cancelled.clone();
    ui.on_schema_selected(move |index, name| {
        if let Some(ui) = weak.upgrade() {
            let command = if index == 0 { Command::BrowseAll(2) } else { Command::Tables(name.to_string()) };
            submit(&ui, &sender, &cancel, command);
        }
    });
    let weak = ui.as_weak();
    let sender = tx.clone();
    let cancel = cancelled.clone();
    ui.on_table_selected(move |index| {
        use slint::Model;
        if let Some(ui) = weak.upgrade() {
            if ui.get_save_prompt_open() {
                return;
            }
            if let Some(table) = ui.get_tables().row_data(index as usize) {
                submit(
                    &ui,
                    &sender,
                    &cancel,
                    Command::Fields(
                        ui.get_schema().to_string(),
                        table.text.to_string(),
                        String::new(),
                    ),
                );
            }
        }
    });
    let weak = ui.as_weak();
    let sender = tx.clone();
    let cancel = cancelled.clone();
    ui.on_result_selected(move |index| {
        if let Some(ui) = weak.upgrade() {
            submit(&ui, &sender, &cancel, Command::Page(index as usize, 0));
        }
    });
    let weak = ui.as_weak();
    ui.on_result_row_selected(move |index, shift, control| {
        use slint::Model;
        if let Some(ui) = weak.upgrade() {
            let count = ui.get_result_rows().row_count();
            let index = index as usize;
            if index >= count { return; }
            ui.set_selected_cells(ModelRc::default());
            ui.set_cell_anchor_column(-1);
            let mut selected: Vec<_> = ui.get_selected_rows().iter().collect();
            selected.resize(count, false);
            if shift {
                let anchor = if ui.get_selection_anchor() >= 0 { ui.get_selection_anchor() as usize } else { index };
                if !control { selected.fill(false); }
                selected[anchor.min(index)..=anchor.max(index).min(count - 1)].fill(true);
            } else {
                if !control { selected.fill(false); }
                selected[index] = !selected[index];
                ui.set_selection_anchor(index as i32);
            }
            ui.set_selected_count(selected.iter().filter(|selected| **selected).count() as i32);
            ui.set_selected_rows(std::rc::Rc::new(VecModel::from(selected)).into());
        }
    });
    ui.on_result_column_selected({
        let weak = ui.as_weak();
        move |column, shift, control| {
            let Some(ui) = weak.upgrade() else { return; };
            let rows = ui.get_result_rows().row_count();
            let columns = ui.get_result_columns().row_count();
            let column = column as usize;
            if column >= columns || rows == 0 { return; }
            let mut selected: Vec<_> = ui.get_selected_cells().iter().collect();
            selected.resize(rows * columns, false);
            let anchor = if shift { ui.get_cell_anchor_column().max(0) as usize } else { column };
            let select = shift || !control || !(0..rows).all(|row| selected[row * columns + column]);
            if !control { selected.fill(false); }
            for row in 0..rows { selected[row * columns + anchor.min(column)..=row * columns + anchor.max(column)].fill(select); }
            ui.set_selected_rows(ModelRc::default());
            ui.set_selection_anchor(0);
            if !shift { ui.set_cell_anchor_column(column as i32); }
            ui.set_selected_count(selected.iter().filter(|selected| **selected).count() as i32);
            ui.set_selected_cells(std::rc::Rc::new(VecModel::from(selected)).into());
        }
    });
    ui.on_result_column_at({
        let weak = ui.as_weak();
        move |x| {
            use slint::Model;
            let Some(ui) = weak.upgrade() else { return 0; };
            let mut right = 42.0;
            let columns = ui.get_result_columns();
            for (index, column) in columns.iter().enumerate() {
                right += if column.width > 0.0 { column.width } else { 160.0 };
                if x < right { return index as i32; }
            }
            columns.row_count().saturating_sub(1) as i32
        }
    });
    let weak = ui.as_weak();
    let sender = tx.clone();
    let cancel = cancelled.clone();
    ui.on_result_cell_double_clicked(move |row, column| {
        if let Some(ui) = weak.upgrade() {
            if row < 0 || column < 0 || ui.get_busy() { return; }
            submit(&ui, &sender, &cancel, Command::CopyCell(ui.get_result_index() as usize, ui.get_page() as usize, row as usize, column as usize));
        }
    });
    let weak = ui.as_weak();
    ui.on_result_cell_selected(move |row, column, shift, control| {
        use slint::Model;
        if let Some(ui) = weak.upgrade() {
            let rows = ui.get_result_rows().row_count();
            let columns = ui.get_result_columns().row_count();
            let (row, column) = (row as usize, column as usize);
            if row >= rows || column >= columns { return; }
            let mut selected: Vec<_> = ui.get_selected_cells().iter().collect();
            selected.resize(rows * columns, false);
            ui.set_selected_rows(ModelRc::default());
            if shift && ui.get_cell_anchor_column() >= 0 && ui.get_selection_anchor() >= 0 {
                let anchor_row = ui.get_selection_anchor() as usize;
                let anchor_column = ui.get_cell_anchor_column() as usize;
                if !control { selected.fill(false); }
                for r in anchor_row.min(row)..=anchor_row.max(row) {
                    selected[r * columns + anchor_column.min(column)..=r * columns + anchor_column.max(column)].fill(true);
                }
            } else {
                if !control { selected.fill(false); }
                selected[row * columns + column] = !selected[row * columns + column];
                ui.set_selection_anchor(row as i32);
                ui.set_cell_anchor_column(column as i32);
            }
            ui.set_selected_count(selected.iter().filter(|selected| **selected).count() as i32);
            ui.set_selected_cells(std::rc::Rc::new(VecModel::from(selected)).into());
        }
    });
    let weak = ui.as_weak();
    ui.on_result_select_all(move || {
        use slint::Model;
        if let Some(ui) = weak.upgrade() {
            let count = ui.get_result_rows().row_count();
            ui.set_selected_cells(ModelRc::default());
            ui.set_cell_anchor_column(-1);
            ui.set_selected_rows(std::rc::Rc::new(VecModel::from(vec![true; count])).into());
            ui.set_selected_count(count as i32);
            ui.set_selection_anchor(0);
        }
    });
    let weak = ui.as_weak();
    let sender = tx.clone();
    let cancel = cancelled.clone();
    ui.on_close_result(move |index| {
        if let Some(ui) = weak.upgrade() {
            submit(&ui, &sender, &cancel, Command::CloseResult(index as usize, ui.get_result_index().max(0) as usize, dialogs::native_window(&ui.get_window_title()) as usize));
        }
    });
    let weak = ui.as_weak();
    let sender = tx.clone();
    let cancel = cancelled.clone();
    ui.on_change_page(move |page| {
        if let Some(ui) = weak.upgrade() {
            submit(
                &ui,
                &sender,
                &cancel,
                Command::Page(ui.get_result_index() as usize, page as usize),
            );
        }
    });
    let weak = ui.as_weak();
    let sender = tx.clone();
    let cancel = cancelled.clone();
    ui.on_sort_result(move |column, descending| {
        if let Some(ui) = weak.upgrade() {
            if ui.get_busy() || ui.get_result_index() < 0 { return; }
            submit(&ui, &sender, &cancel, Command::Sort(ui.get_result_index() as usize, column as usize, descending));
        }
    });
    let weak = ui.as_weak();
    ui.on_fit_result_column(move |index| {
        use slint::Model;
        let Some(ui) = weak.upgrade() else { return; };
        if ui.get_busy() || index < 0 { return; }
        let index = index as usize;
        let columns = ui.get_result_columns();
        let Some(mut column) = columns.row_data(index) else { return; };
        let mut width = (ui.invoke_measure_result_text(column.title.clone()) + 32.0).max(60.0);
        for row in ui.get_result_rows().iter() {
            if let Some(cell) = row.row_data(index) { width = width.max(ui.invoke_measure_result_text(cell.text) + 12.0); }
        }
        column.width = width;
        columns.set_row_data(index, column);
    });
    let weak = ui.as_weak();
    let sender = tx.clone();
    let cancel = cancelled.clone();
    ui.on_language_changed(move || {
        if let Some(ui) = weak.upgrade() {
            localize_result(&ui);
            prefix_options(&ui);
            localize_all(&ui);
            submit(
                &ui,
                &sender,
                &cancel,
                Command::Language(ui.global::<I18n>().get_zh()),
            );
        }
    });
    let weak = ui.as_weak();
    let sender = tx.clone();
    let cancel = cancelled.clone();
    ui.on_theme_selected(move |dark| {
        if let Some(ui) = weak.upgrade() {
            submit(&ui, &sender, &cancel, Command::Theme(dark));
        }
    });
    if let Some(index) = restore_index {
        submit(&ui, &tx, &cancelled, Command::UseProfile(index));
    }
    let _dialogs = dialogs::bind(&ui);
    {
        use slint::winit_030::{WinitWindowAccessor, EventResult, winit::event::{WindowEvent, ElementState, TouchPhase, Touch}};
        let weak = ui.as_weak();
        ui.window().on_winit_window_event(move |_, event| {
            if matches!(event, WindowEvent::ScaleFactorChanged { .. }) {
                let weak = weak.clone();
                slint::Timer::single_shot(Duration::ZERO, move || {
                    if let Some(ui) = weak.upgrade() { ui.set_highlighted_sql("".into()); ui.invoke_highlight(ui.get_sql()); }
                });
            }
            if matches!(event, WindowEvent::Focused(true)) {
                if let Some(ui) = weak.upgrade() { ui.invoke_dismiss_notification(); }
            }
            if matches!(event, WindowEvent::MouseInput { state: ElementState::Pressed, .. } | WindowEvent::Touch(Touch { phase: TouchPhase::Started, .. })) {
                if let Some(ui) = weak.upgrade() { ui.invoke_acknowledge_query(); }
            }
            EventResult::Propagate
        });
    }
    ui.show()?;
    unsafe { windows_sys::Win32::UI::WindowsAndMessaging::SetForegroundWindow(dialogs::native_window(&ui.get_window_title())); }
    if saved_maximized {
        let weak = ui.as_weak();
        slint::Timer::single_shot(Duration::from_millis(100), move || {
            if let Some(ui) = weak.upgrade() { ui.window().set_maximized(true); }
        });
    }
    let result = slint::run_event_loop();
    let maximized = ui.window().is_maximized();
    cancelled.store(true, Ordering::Relaxed);
    drop(ui);
    drop(tx);
    let _ = worker.join();
    if result.is_ok() && maximized != saved_maximized {
        let mut settings = config::load()?;
        settings.maximized = maximized;
        config::save(&settings)?;
    }
    result.map_err(Into::into)
}
