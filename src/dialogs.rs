use crate::{AppWindow, DialogWindow, I18n, Theme};
use slint::{ComponentHandle, Model, VecModel};
use std::{cell::{Cell, RefCell}, rc::Rc, time::Duration};
use windows_sys::Win32::{Foundation::HWND, System::Threading::GetCurrentThreadId, UI::{Input::KeyboardAndMouse::EnableWindow, WindowsAndMessaging::{EnumThreadWindows, GetWindowTextLengthW, GetWindowTextW, SetWindowLongPtrW, SetForegroundWindow, GWLP_HWNDPARENT}}};

pub(crate) fn native_window(title: &str) -> HWND {
    struct Search { title: Vec<u16>, found: HWND }
    unsafe extern "system" fn visit(window: HWND, data: isize) -> i32 {
        let search = unsafe { &mut *(data as *mut Search) };
        if unsafe { GetWindowTextLengthW(window) } as usize != search.title.len() { return 1; }
        let mut text = vec![0u16; search.title.len() + 1];
        let length = unsafe { GetWindowTextW(window, text.as_mut_ptr(), text.len() as i32) } as usize;
        if text[..length] == search.title { search.found = window; return 0; }
        1
    }
    let mut search = Search { title: title.encode_utf16().collect(), found: std::ptr::null_mut() };
    unsafe { EnumThreadWindows(GetCurrentThreadId(), Some(visit), &mut search as *mut Search as isize); }
    search.found
}

pub(crate) fn notify_result(ui: &AppWindow, message: &str) -> Option<crate::ResultNotification> {
    use windows_sys::Win32::Graphics::Gdi::{MonitorFromWindow, GetMonitorInfoW, MONITORINFO, MONITOR_DEFAULTTONEAREST};
    unsafe {
        use windows_sys::Win32::{System::Threading::GetCurrentProcessId, UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId}};
        let mut process = 0;
        GetWindowThreadProcessId(GetForegroundWindow(), &mut process);
        if process == GetCurrentProcessId() { return None; }
    }
    let bubble = crate::ResultNotification::new().ok()?;
    bubble.global::<Theme>().set_dark(ui.global::<Theme>().get_dark());
    bubble.set_message(message.into());
    let owner = native_window(&ui.get_window_title());
    let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    let monitor = unsafe { MonitorFromWindow(owner, MONITOR_DEFAULTTONEAREST) };
    if unsafe { GetMonitorInfoW(monitor, &mut info) } == 0 { return None; }
    bubble.show().ok()?;
    unsafe {
        use windows_sys::Win32::UI::WindowsAndMessaging::{GetWindowLongPtrW, GWL_EXSTYLE, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW};
        let window = native_window("MyQuery SQL");
        if !window.is_null() { SetWindowLongPtrW(window, GWL_EXSTYLE, GetWindowLongPtrW(window, GWL_EXSTYLE) | WS_EX_NOACTIVATE as isize | WS_EX_TOOLWINDOW as isize); }
    }
    let size = bubble.window().size();
    let scale = ui.window().scale_factor();
    let margin = (16.0 * scale).ceil() as i32;
    let x = info.rcWork.right - size.width as i32 - margin;
    let mut y = info.rcWork.bottom - size.height as i32 - margin;
    unsafe {
        use windows_sys::Win32::{Foundation::{POINT, RECT}, Graphics::Gdi::ClientToScreen, UI::WindowsAndMessaging::{GetClientRect, IsIconic}};
        let mut client: RECT = std::mem::zeroed();
        if IsIconic(owner) == 0 && GetClientRect(owner, &mut client) != 0 {
            let mut bottom = POINT { x: 0, y: client.bottom };
            if ClientToScreen(owner, &mut bottom) != 0 {
                let status_top = bottom.y - (40.0 * scale).ceil() as i32;
                if x < bottom.x + client.right && x + size.width as i32 > bottom.x && y < bottom.y && y + size.height as i32 > status_top {
                    y = status_top - size.height as i32 - margin;
                }
            }
        }
    }
    bubble.window().set_position(slint::PhysicalPosition::new(x, y.max(info.rcWork.top + margin)));
    let weak = bubble.as_weak();
    slint::Timer::single_shot(Duration::from_secs(5), move || { if let Some(bubble) = weak.upgrade() { let _ = bubble.hide(); } });
    Some(bubble)
}

fn selected(ui: &AppWindow) -> i32 {
    if ui.get_save_prompt_open() { 5 }
    else if ui.get_rename_sql_open() { 4 }
    else if ui.get_config_open() { 1 }
    else if ui.get_scopes_open() { 2 }
    else if ui.get_timeout_open() { 3 }
    else if ui.get_group_open() { 6 }
    else { 0 }
}

fn close(ui: &AppWindow, mode: i32) {
    match mode {
        1 => ui.set_config_open(false), 2 => ui.set_scopes_open(false),
        3 => ui.set_timeout_open(false), 4 => ui.set_rename_sql_open(false),
        5 => ui.invoke_action("prompt-cancel".into()), 6 => ui.set_group_open(false), _ => {}
    }
}

fn edits_to_main(dialog: &DialogWindow, ui: &AppWindow) {
    ui.set_profile_index(dialog.get_profile_index());
    ui.set_profile_name(dialog.get_profile_name());
    ui.set_engine(dialog.get_engine());
    ui.set_host(dialog.get_host());
    ui.set_port(dialog.get_port());
    ui.set_database(dialog.get_database());
    ui.set_username(dialog.get_username());
    ui.set_password(dialog.get_password());
    ui.set_tls(dialog.get_tls());
    ui.set_trust_server_certificate(dialog.get_trust_server_certificate());
    ui.set_rename_sql_text(dialog.get_rename_sql_text());
    ui.set_group_search(dialog.get_group_search());
    ui.set_group_index(dialog.get_group_index());
    ui.set_group_selected_index(dialog.get_group_selected_index());
    ui.set_timeout_input(dialog.get_timeout_input());
    ui.set_scope_profile_index(dialog.get_scope_profile_index());
    ui.set_scope_database_index(dialog.get_scope_database_index());
    ui.set_scope_schema_index(dialog.get_scope_schema_index());
    ui.set_config_open(dialog.get_config_open());
    ui.set_scopes_open(dialog.get_scopes_open());
    ui.set_timeout_open(dialog.get_timeout_open());
    ui.set_rename_sql_open(dialog.get_rename_sql_open());
    ui.set_save_prompt_open(dialog.get_save_prompt_open());
    ui.set_group_open(dialog.get_group_open());
}

fn refresh(ui: &AppWindow, dialog: &DialogWindow) {
    dialog.set_profile_index(ui.get_profile_index());
    dialog.set_profile_name(ui.get_profile_name());
    dialog.set_engine(ui.get_engine());
    dialog.set_host(ui.get_host());
    dialog.set_port(ui.get_port());
    dialog.set_database(ui.get_database());
    dialog.set_username(ui.get_username());
    dialog.set_password(ui.get_password());
    dialog.set_tls(ui.get_tls());
    dialog.set_trust_server_certificate(ui.get_trust_server_certificate());
    dialog.set_rename_sql_text(ui.get_rename_sql_text());
    dialog.set_group_search(ui.get_group_search());
    dialog.set_group_index(ui.get_group_index());
    dialog.set_group_selected(ui.get_group_selected());
    dialog.set_group_selected_index(ui.get_group_selected_index());
    dialog.set_timeout_input(ui.get_timeout_input());
    dialog.set_scope_profile_index(ui.get_scope_profile_index());
    dialog.set_scope_database_index(ui.get_scope_database_index());
    dialog.set_scope_schema_index(ui.get_scope_schema_index());
    dialog.set_config_open(ui.get_config_open());
    dialog.set_scopes_open(ui.get_scopes_open());
    dialog.set_timeout_open(ui.get_timeout_open());
    dialog.set_rename_sql_open(ui.get_rename_sql_open());
    dialog.set_save_prompt_open(ui.get_save_prompt_open());
    dialog.set_group_open(ui.get_group_open());
    dialog.set_busy(ui.get_busy() || ui.get_switching() || (ui.get_queries_running() && (ui.get_config_open() || ui.get_scopes_open())));
    dialog.set_document_saving(ui.get_document_saving());
    dialog.set_profiles(ui.get_profiles());
    dialog.set_browser_scopes(ui.get_browser_scopes());
    dialog.set_scope_profiles(ui.get_scope_profiles());
    dialog.set_scope_databases(ui.get_scope_databases());
    dialog.set_scope_schemas(ui.get_scope_schemas());
    dialog.set_scope_database_names(ui.get_scope_database_names());
    dialog.set_scope_schema_names(ui.get_scope_schema_names());
    dialog.set_group_fields(ui.get_group_fields());
    dialog.set_rename_sql_index(ui.get_rename_sql_index());
    dialog.set_save_prompt_title(ui.get_save_prompt_title());
    dialog.set_object_title(ui.get_object_title());
    dialog.set_timeout(ui.get_timeout());
    dialog.set_connected(ui.get_connected());
    dialog.set_failed(ui.get_failed());
    dialog.set_status(ui.get_status());
    dialog.set_toast_text(ui.get_toast_text());
    dialog.set_toast_age(ui.get_toast_age());
    dialog.global::<I18n>().set_zh(ui.global::<I18n>().get_zh());
    dialog.global::<Theme>().set_dark(ui.global::<Theme>().get_dark());
}

pub fn bind(ui: &AppWindow) -> slint::Timer {
    let weak = ui.as_weak();
    let active: Rc<RefCell<Option<DialogWindow>>> = Rc::new(RefCell::new(None));
    let syncing = Rc::new(Cell::new(false));
    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::Repeated, Duration::from_millis(25), move || {
        let Some(ui) = weak.upgrade() else { return; };
        let mode = if ui.window().is_visible() { selected(&ui) } else { 0 };
        let changed = active.borrow().as_ref().is_some_and(|dialog| dialog.get_dialog_mode() != mode);
        if changed {
            if let Some(dialog) = active.borrow_mut().take() { let _ = dialog.hide(); }
            let owner = native_window(&ui.get_window_title());
            if !owner.is_null() { unsafe { EnableWindow(owner, 1); SetForegroundWindow(owner); } }
        }
        if mode == 0 { return; }
        if active.borrow().is_none() {
            let dialog = match DialogWindow::new() {
                Ok(dialog) => dialog,
                Err(error) => { close(&ui, mode); ui.set_failed(true); ui.set_status(error.to_string().into()); return; }
            };
            dialog.set_dialog_mode(mode);
            syncing.set(true);
            refresh(&ui, &dialog);
            syncing.set(false);
            let parent = ui.as_weak();
            let child = dialog.as_weak();
            let updating = syncing.clone();
            dialog.on_dialog_edited(move || {
                if updating.get() { return; }
                if let (Some(ui), Some(dialog)) = (parent.upgrade(), child.upgrade()) { edits_to_main(&dialog, &ui); }
            });
            dialog.on_filter_options(|model, query| {
                let query = query.trim().to_lowercase();
                Rc::new(VecModel::from(model.iter().enumerate().filter_map(|(index, value)| value.to_lowercase().contains(&query).then_some(index as i32)).collect::<Vec<_>>())).into()
            });
            let parent = ui.as_weak();
            let child = dialog.as_weak();
            dialog.on_action(move |action| {
                if let (Some(ui), Some(dialog)) = (parent.upgrade(), child.upgrade()) {
                    edits_to_main(&dialog, &ui);
                    ui.invoke_action(action);
                }
            });
            let parent = ui.as_weak();
            dialog.on_profile_selected(move |index| { if let Some(ui) = parent.upgrade() { ui.invoke_profile_selected(index); } });
            let parent = ui.as_weak();
            dialog.on_scope_choice(move |level, index, chosen| { if let Some(ui) = parent.upgrade() { ui.invoke_scope_choice(level, index, chosen); } });
            let parent = ui.as_weak();
            dialog.on_remove_scope(move |index| { if let Some(ui) = parent.upgrade() { ui.invoke_remove_scope(index); } });
            let parent = ui.as_weak();
            let child = dialog.as_weak();
            dialog.on_move_scope(move |from, to| {
                if let (Some(ui), Some(dialog)) = (parent.upgrade(), child.upgrade()) {
                    edits_to_main(&dialog, &ui);
                    ui.invoke_move_scope(from, to);
                }
            });
            let parent = ui.as_weak();
            dialog.on_filter_group(move || { if let Some(ui) = parent.upgrade() { ui.invoke_filter_group(); } });
            let parent = ui.as_weak();
            dialog.on_rename_sql(move |index, title| { if let Some(ui) = parent.upgrade() { ui.invoke_rename_sql(index, title); } });
            let parent = ui.as_weak();
            dialog.window().on_close_requested(move || {
                if let Some(ui) = parent.upgrade() {
                    if ui.get_document_saving() { return slint::CloseRequestResponse::KeepWindowShown; }
                    close(&ui, mode);
                }
                slint::CloseRequestResponse::KeepWindowShown
            });
            if let Err(error) = dialog.show() { close(&ui, mode); ui.set_failed(true); ui.set_status(error.to_string().into()); return; }
            let parent = ui.as_weak();
            let child = dialog.as_weak();
            if let Err(error) = slint::spawn_local(async move {
                use slint::winit_030::{WinitWindowAccessor, winit::raw_window_handle::{HasWindowHandle, RawWindowHandle}};
                let (Some(ui), Some(dialog)) = (parent.upgrade(), child.upgrade()) else { return; };
                let Ok(window) = dialog.window().winit_window().await else { return; };
                if !dialog.window().is_visible() || selected(&ui) != mode { return; }
                let origin = ui.window().position();
                let parent_size = ui.window().size();
                let size = dialog.window().size();
                dialog.window().set_position(slint::PhysicalPosition::new(origin.x + (parent_size.width as i32 - size.width as i32) / 2, origin.y + (parent_size.height as i32 - size.height as i32) / 2));
                let owner = native_window(&ui.get_window_title());
                if let Ok(RawWindowHandle::Win32(handle)) = window.window_handle().map(|handle| handle.as_raw()) {
                    let child = handle.hwnd.get() as HWND;
                    if !owner.is_null() {
                        unsafe {
                            use windows_sys::Win32::UI::{Input::KeyboardAndMouse::{SetActiveWindow, SetFocus}, WindowsAndMessaging::BringWindowToTop};
                            SetWindowLongPtrW(child, GWLP_HWNDPARENT, owner as isize);
                            EnableWindow(owner, 0);
                            BringWindowToTop(child);
                            SetForegroundWindow(child);
                            SetActiveWindow(child);
                            SetFocus(child);
                        }
                    }
                }
                window.focus_window();
                if mode != 4 { dialog.invoke_focus_dialog(); }
            }) {
                close(&ui, mode);
                let _ = dialog.hide();
                ui.set_failed(true);
                ui.set_status(error.to_string().into());
                return;
            }
            *active.borrow_mut() = Some(dialog);
        }
        if let Some(dialog) = active.borrow().as_ref() {
            syncing.set(true);
            refresh(&ui, dialog);
            syncing.set(false);
        }
    });
    timer
}
