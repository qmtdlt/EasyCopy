//! Per-user login startup. No preference file is authoritative and reads never register the app.
use serde::Serialize;
use std::path::Path;

#[cfg(target_os = "macos")]
#[path = "autostart/macos.rs"]
mod platform;
#[cfg(target_os = "windows")]
#[path = "autostart/windows.rs"]
mod platform;

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod platform {
    pub fn is_enabled(_: &std::path::Path) -> anyhow::Result<bool> {
        anyhow::bail!("当前仅支持 macOS 和 Windows 的开机自启动")
    }
    pub fn set_enabled(_: &std::path::Path, _: bool) -> anyhow::Result<()> {
        anyhow::bail!("当前仅支持 macOS 和 Windows 的开机自启动")
    }
}

pub const LOGIN_ARG: &str = "--autostart";
pub const ENTRY_NAME: &str = "com.easycopy.desktop";

pub fn is_login_launch(args: impl IntoIterator<Item = impl AsRef<str>>) -> bool {
    args.into_iter()
        .skip(1)
        .any(|arg| arg.as_ref() == LOGIN_ARG)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    enabled: bool,
    can_enable: bool,
    reason: Option<String>,
}

fn enable_restriction(exe: &Path, development: bool) -> Option<String> {
    if development {
        return Some("开发调试版本不支持开启自启动，请安装正式版本后开启。".into());
    }
    if exe.components().any(|part| {
        matches!(
            part.as_os_str().to_str(),
            Some("target" | ".local" | "AppTranslocation")
        )
    }) {
        return Some("请先将正式版本安装到固定位置，再开启自启动。".into());
    }
    #[cfg(target_os = "macos")]
    if exe.starts_with("/Volumes")
        || !exe
            .ancestors()
            .any(|p| p.extension().is_some_and(|e| e == "app"))
    {
        return Some("请先将 EasyCopy.app 安装到应用程序文件夹，再开启自启动。".into());
    }
    None
}

#[tauri::command]
pub async fn get_autostart() -> Result<Status, String> {
    tauri::async_runtime::spawn_blocking(read_status)
        .await
        .map_err(|e| e.to_string())?
}

fn read_status() -> Result<Status, String> {
    let exe = std::env::current_exe().map_err(|e| format!("无法读取程序位置：{e}"))?;
    let reason = enable_restriction(&exe, cfg!(debug_assertions) || tauri::is_dev());
    Ok(Status {
        enabled: platform::is_enabled(&exe)
            .map_err(|e| format!("无法读取系统自启动状态：{e:#}"))?,
        can_enable: reason.is_none(),
        reason,
    })
}

#[tauri::command]
pub async fn set_autostart(enabled: bool) -> Result<Status, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        if enabled {
            if let Some(reason) =
                enable_restriction(&exe, cfg!(debug_assertions) || tauri::is_dev())
            {
                return Err(reason);
            }
        }
        platform::set_enabled(&exe, enabled)
            .map_err(|e| format!("{}自启动失败：{e:#}", if enabled { "开启" } else { "关闭" }))?;
        let status = read_status()?;
        if status.enabled != enabled {
            return Err("系统未接受自启动变更，请检查系统的登录项或启动应用设置后重试。".into());
        }
        Ok(status)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_explicit_login_argument_hides_window() {
        assert!(is_login_launch(["EasyCopy", "--autostart"]));
        assert!(!is_login_launch(["EasyCopy", "--autostart=false"]));
        assert!(!is_login_launch(["/apps/--autostart/EasyCopy"]));
        assert!(!is_login_launch(["EasyCopy"]));
    }

    #[test]
    fn development_and_transient_paths_cannot_register() {
        let installed = Path::new("/Applications/EasyCopy.app/Contents/MacOS/EasyCopy");
        assert!(enable_restriction(installed, false).is_none());
        assert!(enable_restriction(installed, true).is_some());
        assert!(enable_restriction(Path::new("/repo/target/release/EasyCopy"), false).is_some());
        assert!(enable_restriction(
            Path::new("/tmp/AppTranslocation/EasyCopy.app/Contents/MacOS/EasyCopy"),
            false
        )
        .is_some());
    }
}
