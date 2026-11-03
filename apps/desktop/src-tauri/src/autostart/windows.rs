use super::{ENTRY_NAME, LOGIN_ARG};
use anyhow::{bail, Context, Result};
use std::{io, path::Path};
use winreg::{
    enums::{HKEY_CURRENT_USER, KEY_SET_VALUE, REG_BINARY},
    RegKey, RegValue,
};

const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const APPROVED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";

fn command_line(exe: &Path) -> Result<String> {
    let path = exe.to_str().context("程序路径不是有效 Unicode")?;
    let command = format!("\"{path}\" {LOGIN_ARG}");
    // The documented Run-key command limit is 260 characters.
    if command.encode_utf16().count() > 260 {
        bail!("安装路径过长，请将软件安装到更短的路径");
    }
    Ok(command)
}

fn optional<T>(result: io::Result<T>) -> io::Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

pub fn is_enabled(exe: &Path) -> Result<bool> {
    let user = RegKey::predef(HKEY_CURRENT_USER);
    let Some(key) = optional(user.open_subkey(RUN))? else {
        return Ok(false);
    };
    if optional(key.get_value::<String, _>(ENTRY_NAME))?.as_deref()
        != Some(command_line(exe)?.as_str())
    {
        return Ok(false);
    }
    if let Some(key) = optional(user.open_subkey(APPROVED))? {
        if let Some(value) = optional(key.get_raw_value(ENTRY_NAME))? {
            if value.vtype != REG_BINARY || value.bytes.len() != 12 {
                bail!("无法识别系统启动应用状态，请在系统设置中检查 EasyCopy");
            }
            // Windows uses 2 (enabled) / 3 (disabled), with 6 / 7 on newer systems.
            return match value.bytes[0] {
                2 | 6 => Ok(true),
                3 | 7 => Ok(false),
                _ => {
                    bail!("系统启动应用状态未知，请在系统设置中检查 EasyCopy");
                }
            };
        }
    }
    Ok(true)
}

pub fn set_enabled(exe: &Path, enabled: bool) -> Result<()> {
    let user = RegKey::predef(HKEY_CURRENT_USER);
    if enabled {
        let command = command_line(exe)?;
        let (run, _) = user.create_subkey(RUN)?;
        run.set_value(ENTRY_NAME, &command)?;
        // Only reset this application's per-user override after an explicit enable action.
        if let Some(approved) = optional(user.open_subkey_with_flags(APPROVED, KEY_SET_VALUE))? {
            approved.set_raw_value(
                ENTRY_NAME,
                &RegValue {
                    vtype: REG_BINARY,
                    bytes: vec![2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0].into(),
                },
            )?;
        }
    } else if let Some(run) = optional(user.open_subkey_with_flags(RUN, KEY_SET_VALUE))? {
        optional(run.delete_value(ENTRY_NAME))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quotes_installed_path_as_one_executable() {
        assert_eq!(
            command_line(Path::new(r"C:\Program Files\中文目录\EasyCopy.exe")).unwrap(),
            "\"C:\\Program Files\\中文目录\\EasyCopy.exe\" --autostart"
        );
    }
}
