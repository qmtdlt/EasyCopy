use easycopy_client_core::TransferEvent;
use std::collections::HashSet;
use tauri::Manager;

#[derive(Default)]
pub struct OverlayState {
    pub ready: bool,
    pub current: Option<TransferEvent>,
    retired: HashSet<String>,
}

impl OverlayState {
    pub fn accept(&mut self, event: TransferEvent) -> Option<bool> {
        if self.retired.contains(&event.task_id) {
            return None;
        }
        let same = self
            .current
            .as_ref()
            .is_some_and(|e| e.task_id == event.task_id);
        if same {
            if self
                .current
                .as_ref()
                .is_some_and(|e| matches!(e.phase.as_str(), "complete" | "failed"))
            {
                return None;
            }
        } else {
            if event.phase != "start" {
                return None;
            }
            if let Some(old) = self.current.take() {
                self.retired.insert(old.task_id);
            }
        }
        self.current = Some(event);
        Some(!same)
    }
    pub fn finish(&mut self, task_id: &str) -> bool {
        if self.current.as_ref().is_some_and(|e| {
            e.task_id == task_id && matches!(e.phase.as_str(), "complete" | "failed")
        }) {
            self.current = None;
            self.retired.insert(task_id.to_owned());
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn event(id: &str, phase: &str) -> TransferEvent {
        TransferEvent {
            task_id: id.into(),
            direction: "upload".into(),
            phase: phase.into(),
            received_bytes: 0,
            total_bytes: None,
            error: None,
        }
    }
    #[test]
    fn late_preparing_or_start_cannot_replace_new_task() {
        let mut state = OverlayState::default();
        assert_eq!(state.accept(event("a", "start")), Some(true));
        assert_eq!(state.accept(event("b", "start")), Some(true));
        assert_eq!(state.accept(event("a", "preparing")), None);
        assert_eq!(state.accept(event("a", "start")), None);
        assert!(!state.finish("a"));
        assert_eq!(state.current.as_ref().unwrap().task_id, "b");
    }
    #[test]
    fn terminal_cannot_regress_when_ws_progress_arrives_after_http_complete() {
        let mut state = OverlayState::default();
        state.accept(event("a", "start"));
        state.accept(event("a", "complete"));
        assert_eq!(state.accept(event("a", "transferring")), None);
        assert!(state.finish("a"));
        assert_eq!(state.accept(event("a", "start")), None);
    }
}

pub fn create(app: &tauri::App) -> tauri::Result<()> {
    let window = tauri::WebviewWindowBuilder::new(
        app,
        "transfer",
        tauri::WebviewUrl::App("index.html?overlay=1".into()),
    )
    .title("EasyCopy 传输")
    .inner_size(128.0, 150.0)
    .decorations(false)
    .transparent(true)
    .shadow(false)
    .visible(false)
    .focused(false)
    .focusable(false)
    .resizable(false)
    .skip_taskbar(true)
    .always_on_top(true)
    .visible_on_all_workspaces(true)
    .build()?;
    window.set_ignore_cursor_events(true)?;
    #[cfg(target_os = "macos")]
    unsafe {
        use objc2_app_kit::{NSWindow, NSWindowCollectionBehavior as Behavior};
        let native = &*(window.ns_window()? as *const NSWindow);
        native.setCollectionBehavior(
            Behavior::CanJoinAllSpaces
                | Behavior::FullScreenAuxiliary
                | Behavior::Stationary
                | Behavior::IgnoresCycle,
        );
        native.setHidesOnDeactivate(false);
        native.setLevel(25); // status-window level, above normal and full-screen windows
    }
    Ok(())
}

pub fn present(app: &tauri::AppHandle) {
    let Some(window) = app.get_webview_window("transfer") else {
        return;
    };
    // AppKit visibleFrame uses logical points and accounts for Dock/menu bar on each display.
    #[cfg(target_os = "macos")]
    if let Some(mtm) = objc2::MainThreadMarker::new() {
        use objc2_app_kit::{NSEvent, NSScreen, NSWindow};
        use objc2_foundation::NSPoint;
        let cursor = NSEvent::mouseLocation();
        let screens = NSScreen::screens(mtm);
        let screen = screens
            .iter()
            .find(|s| {
                let f = s.frame();
                cursor.x >= f.origin.x
                    && cursor.x < f.origin.x + f.size.width
                    && cursor.y >= f.origin.y
                    && cursor.y < f.origin.y + f.size.height
            })
            .or_else(|| screens.iter().next());
        if let Some(screen) = screen {
            if let Ok(ptr) = window.ns_window() {
                let frame = screen.visibleFrame();
                unsafe {
                    let native = &*(ptr as *const NSWindow);
                    native.setFrameOrigin(NSPoint::new(
                        frame.origin.x + frame.size.width - 148.0,
                        frame.origin.y + 20.0,
                    ));
                    native.orderFrontRegardless();
                }
                return;
            }
        }
    }
    // Windows monitor work area is physical pixels; scale the logical overlay and margins.
    if let Ok(cursor) = app.cursor_position() {
        if let Ok(Some(monitor)) = app.monitor_from_point(cursor.x, cursor.y) {
            let area = monitor.work_area();
            let scale = monitor.scale_factor();
            let _ = window.set_position(tauri::PhysicalPosition::new(
                area.position.x + area.size.width as i32 - (148.0 * scale).round() as i32,
                area.position.y + area.size.height as i32 - (170.0 * scale).round() as i32,
            ));
        }
    }
    let _ = window.show(); // focusable=false: never activates or steals input focus
}
