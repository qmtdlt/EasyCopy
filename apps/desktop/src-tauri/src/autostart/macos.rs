use super::{ENTRY_NAME, LOGIN_ARG};
use anyhow::{bail, Context, Result};
use plist::{Dictionary, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn registration_path() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").context("无法确定用户主目录")?;
    Ok(PathBuf::from(home)
        .join("Library/LaunchAgents")
        .join(format!("{ENTRY_NAME}.plist")))
}

fn domain() -> String {
    format!("gui/{}", unsafe { libc::getuid() })
}

fn launchctl(args: &[&str]) -> Result<String> {
    let output = Command::new("/bin/launchctl").args(args).output()?;
    if !output.status.success() {
        bail!(
            "launchctl：{}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8(output.stdout)?)
}

fn is_disabled(output: &str) -> Result<bool> {
    for line in output.lines() {
        if let Some((name, value)) = line.split_once("=>") {
            if name.trim().trim_matches('"') == ENTRY_NAME {
                return match value.trim().trim_end_matches(';') {
                    "true" | "disabled" => Ok(true),
                    "false" | "enabled" => Ok(false),
                    _ => anyhow::bail!("无法识别系统登录项状态，请检查系统设置"),
                };
            }
        }
    }
    Ok(false)
}

fn registration(exe: &Path) -> Result<Value> {
    let mut dict = Dictionary::new();
    dict.insert("Label".into(), ENTRY_NAME.into());
    dict.insert(
        "AssociatedBundleIdentifiers".into(),
        Value::Array(vec![ENTRY_NAME.into()]),
    );
    dict.insert(
        "ProgramArguments".into(),
        Value::Array(vec![
            exe.to_str().context("程序路径不是有效 UTF-8")?.into(),
            LOGIN_ARG.into(),
        ]),
    );
    dict.insert("RunAtLoad".into(), true.into());
    dict.insert("LimitLoadToSessionType".into(), "Aqua".into());
    Ok(Value::Dictionary(dict))
}

pub fn is_enabled(exe: &Path) -> Result<bool> {
    if !registration_matches(&registration_path()?, exe)? {
        return Ok(false);
    }
    Ok(!is_disabled(&launchctl(&["print-disabled", &domain()])?)?)
}

fn registration_matches(path: &Path, exe: &Path) -> Result<bool> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    let actual = Value::from_reader(file)?;
    // A moved application or edited/stale entry must not appear enabled.
    Ok(actual == registration(exe)?)
}

pub fn set_enabled(exe: &Path, enabled: bool) -> Result<()> {
    let path = registration_path()?;
    if !enabled {
        match fs::remove_file(path) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        }
    }
    let staged = stage_registration(&path, exe)?;
    launchctl(&["enable", &format!("{}/{ENTRY_NAME}", domain())])?;
    staged.persist(path)?;
    // RunAtLoad is picked up at the next login. Do not bootstrap a second client now.
    Ok(())
}

fn stage_registration(path: &Path, exe: &Path) -> Result<tempfile::NamedTempFile> {
    let parent = path.parent().context("无效登录项路径")?;
    fs::create_dir_all(parent)?;
    let mut staged = tempfile::NamedTempFile::new_in(parent)?;
    registration(exe)?.to_writer_xml(&mut staged)?;
    staged.as_file().sync_all()?;
    Ok(staged)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_file_lifecycle_without_changing_system_startup() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("LaunchAgents/test.plist");
        let exe = Path::new("/Applications/EasyCopy.app/Contents/MacOS/EasyCopy");
        assert!(!registration_matches(&path, exe).unwrap());
        stage_registration(&path, exe)
            .unwrap()
            .persist(&path)
            .unwrap();
        assert!(registration_matches(&path, exe).unwrap());
        assert!(!registration_matches(&path, Path::new("/different/EasyCopy")).unwrap());
        fs::write(&path, "invalid plist").unwrap();
        assert!(registration_matches(&path, exe).is_err());
        stage_registration(&path, exe)
            .unwrap()
            .persist(&path)
            .unwrap();
        assert!(registration_matches(&path, exe).unwrap());
        fs::remove_file(&path).unwrap();
        assert!(!registration_matches(&path, exe).unwrap());
    }

    #[test]
    fn paths_with_spaces_unicode_and_xml_characters_round_trip() {
        let exe = Path::new("/Applications/工具 & Tools/EasyCopy.app/Contents/MacOS/EasyCopy");
        let value = registration(exe).unwrap();
        let mut bytes = Vec::new();
        value.to_writer_xml(&mut bytes).unwrap();
        assert_eq!(
            Value::from_reader(std::io::Cursor::new(bytes)).unwrap(),
            value
        );
        assert_eq!(
            value.as_dictionary().unwrap()["ProgramArguments"]
                .as_array()
                .unwrap()[0]
                .as_string(),
            exe.to_str()
        );
    }

    #[test]
    fn reads_only_our_system_override() {
        assert!(is_disabled("disabled services = {\n\"com.easycopy.desktop\" => true\n}").unwrap());
        assert!(!is_disabled("\"com.easycopy.desktop\" => false\n\"other\" => true").unwrap());
        assert!(is_disabled("\"com.easycopy.desktop\" => disabled").unwrap());
        assert!(!is_disabled("\"com.easycopy.desktop\" => enabled").unwrap());
        assert!(is_disabled("\"com.easycopy.desktop\" => unknown").is_err());
    }
}
