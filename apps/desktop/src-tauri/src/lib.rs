mod overlay;

use easycopy_client_core::{
    ClientCore, ClientEvent, HistoryEntry, PublicSettings, SettingsInput, TransferEvent,
};
use easycopy_protocol::{ContentKind, ServerInfo};
use serde::Serialize;
use std::sync::{Arc, Mutex};
use tauri::{Emitter, Manager};
use tauri_plugin_dialog::DialogExt;
use uuid::Uuid;

#[derive(Clone, Serialize)]
struct ConnectionState {
    connected: bool,
    message: String,
}
struct DesktopState {
    core: Arc<ClientCore>,
    connection: Mutex<ConnectionState>,
    overlay: Mutex<overlay::OverlayState>,
}

#[tauri::command]
fn list_history(state: tauri::State<'_, DesktopState>) -> Vec<HistoryEntry> {
    state.core.history()
}
#[tauri::command]
fn get_settings(state: tauri::State<'_, DesktopState>) -> PublicSettings {
    state.core.settings()
}
#[tauri::command]
fn get_connection(state: tauri::State<'_, DesktopState>) -> ConnectionState {
    state.connection.lock().unwrap().clone()
}
#[tauri::command]
async fn test_connection(
    state: tauri::State<'_, DesktopState>,
    input: SettingsInput,
) -> Result<ServerInfo, String> {
    state
        .core
        .test_connection(input)
        .await
        .map_err(|e| format!("{e:#}"))
}
#[tauri::command]
async fn save_settings(
    state: tauri::State<'_, DesktopState>,
    input: SettingsInput,
) -> Result<(), String> {
    state
        .core
        .save_settings(input)
        .await
        .map_err(|e| format!("{e:#}"))
}
#[tauri::command]
async fn copy_item(state: tauri::State<'_, DesktopState>, id: String) -> Result<(), String> {
    state
        .core
        .copy_item(Uuid::parse_str(&id).map_err(|_| "无效记录身份")?)
        .await
        .map_err(|e| e.to_string())
}
#[tauri::command]
async fn save_item(
    app: tauri::AppHandle,
    state: tauri::State<'_, DesktopState>,
    id: String,
) -> Result<(), String> {
    let id = Uuid::parse_str(&id).map_err(|_| "无效记录身份")?;
    let entry = state
        .core
        .history()
        .into_iter()
        .find(|e| e.id == id)
        .ok_or("记录已不在历史中")?;
    let path = tauri::async_runtime::spawn_blocking(move || match entry.kind {
        ContentKind::Files => app
            .dialog()
            .file()
            .set_title("选择保存目录")
            .blocking_pick_folder(),
        ContentKind::Text => app
            .dialog()
            .file()
            .set_title("另存为文本")
            .add_filter("文本", &["txt"])
            .set_file_name("EasyCopy.txt")
            .blocking_save_file(),
        ContentKind::Image => app
            .dialog()
            .file()
            .set_title("另存为图片")
            .add_filter("PNG 图片", &["png"])
            .set_file_name("EasyCopy.png")
            .blocking_save_file(),
    })
    .await
    .map_err(|e| e.to_string())?;
    if let Some(path) = path {
        state
            .core
            .save_item(id, path.into_path().map_err(|e| e.to_string())?)
            .await
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}
#[tauri::command]
fn overlay_ready(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
) {
    if window.label() != "transfer" {
        return;
    }
    let mut overlay = state.overlay.lock().unwrap();
    overlay.ready = true;
    if let Some(event) = overlay.current.clone() {
        let mut start = event.clone();
        start.phase = "start".into();
        start.received_bytes = 0;
        overlay::present(&app);
        let _ = app.emit_to("transfer", "transfer", start);
        let _ = app.emit_to("transfer", "transfer", event);
    }
}
#[tauri::command]
fn overlay_hidden(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    task_id: String,
) {
    if window.label() != "transfer" {
        return;
    }
    let mut overlay = state.overlay.lock().unwrap();
    if overlay.finish(&task_id) {
        let _ = window.hide();
    }
}

fn transfer(app: &tauri::AppHandle, event: TransferEvent) {
    let _ = app.emit_to("main", "transfer", &event);
    let state = app.state::<DesktopState>();
    let mut overlay = state.overlay.lock().unwrap();
    let Some(is_new) = overlay.accept(event.clone()) else {
        return;
    };
    if overlay.ready {
        if is_new {
            overlay::present(app);
        }
        let _ = app.emit_to("transfer", "transfer", &event);
    }
}

fn show_main(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            list_history,
            get_settings,
            get_connection,
            test_connection,
            save_settings,
            copy_item,
            save_item,
            overlay_ready,
            overlay_hidden
        ])
        .setup(|app| {
            // Isolated development acceptance profiles never replace the user's
            // normal settings/history or try to read another profile's keychain item.
            let data_dir = if cfg!(debug_assertions) {
                std::env::var_os("EASYCOPY_PROFILE_DIR")
                    .map(std::path::PathBuf::from)
                    .unwrap_or(app.path().app_data_dir()?)
            } else {
                app.path().app_data_dir()?
            };
            let core = ClientCore::open(data_dir)?;
            let mut events = core.subscribe();
            app.manage(DesktopState {
                core: core.clone(),
                connection: Mutex::new(ConnectionState {
                    connected: false,
                    message: "尚未连接服务器".into(),
                }),
                overlay: Mutex::new(overlay::OverlayState::default()),
            });
            let mut main = tauri::WebviewWindowBuilder::new(
                app,
                "main",
                tauri::WebviewUrl::App("index.html".into()),
            )
            .title("EasyCopy")
            .inner_size(1000.0, 700.0)
            .min_inner_size(760.0, 540.0)
            .center();
            #[cfg(target_os = "macos")]
            {
                main = main
                    .title_bar_style(tauri::TitleBarStyle::Overlay)
                    .hidden_title(true);
            }
            main.build()?;
            overlay::create(app)?;
            let open =
                tauri::menu::MenuItem::with_id(app, "open", "打开 EasyCopy", true, None::<&str>)?;
            let quit = tauri::menu::MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let menu = tauri::menu::Menu::with_items(app, &[&open, &quit])?;
            tauri::tray::TrayIconBuilder::with_id("easycopy")
                .icon(tray_icon())
                .icon_as_template(true)
                .tooltip("EasyCopy")
                .menu(&menu)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "open" => show_main(app),
                    "quit" => {
                        app.state::<DesktopState>().core.shutdown();
                        app.exit(0);
                    }
                    _ => {}
                })
                .build(app)?;
            let handle = app.handle().clone();
            let starter = core.clone();
            tauri::async_runtime::spawn(async move {
                starter.start().await;
            });
            tauri::async_runtime::spawn(async move {
                loop {
                    match events.recv().await {
                        Ok(ClientEvent::HistoryChanged) => {
                            let _ = handle.emit_to("main", "history-changed", core.history());
                        }
                        Ok(ClientEvent::Connection { connected, message }) => {
                            let value = ConnectionState { connected, message };
                            *handle.state::<DesktopState>().connection.lock().unwrap() =
                                value.clone();
                            let _ = handle.emit_to("main", "connection-changed", value);
                        }
                        Ok(ClientEvent::Transfer(event)) => {
                            let h = handle.clone();
                            let _ = handle.run_on_main_thread(move || transfer(&h, event));
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                            let _ = handle.emit_to("main", "history-changed", core.history());
                        }
                        Err(_) => break,
                    }
                }
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == "main" {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("EasyCopy 启动失败")
        .run(|app, event| {
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = event {
                show_main(app);
            }
            if let tauri::RunEvent::Exit = event {
                app.state::<DesktopState>().core.shutdown();
            }
        });
}

fn tray_icon() -> tauri::image::Image<'static> {
    // Code-native monochrome clipboard outline, rendered as a template in the system tray.
    let mut rgba = vec![0u8; 24 * 24 * 4];
    for y in 0..24 {
        for x in 0..24 {
            let back = (9..=20).contains(&x)
                && (2..=17).contains(&y)
                && (x <= 10 || x >= 19 || y <= 3 || y >= 16);
            let front = (3..=16).contains(&x)
                && (7..=22).contains(&y)
                && (x <= 4 || x >= 15 || y <= 8 || y >= 21);
            if front || (back && (x > 16 || y < 7)) {
                rgba[(y * 24 + x) * 4 + 3] = 255;
            }
        }
    }
    tauri::image::Image::new_owned(rgba, 24, 24)
}
