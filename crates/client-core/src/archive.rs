use anyhow::{bail, Context, Result};
use easycopy_protocol::DISK_RESERVE_BYTES;
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File, OpenOptions},
    io::{BufWriter, Read, Write},
    path::{Component, Path, PathBuf},
};
use unicode_normalization::UnicodeNormalization;

// Buffer before GuardedWriter so each actual disk write still reserves space.
// TAR's small writes otherwise issue a filesystem space query for every 8 KiB.
const FILE_BUFFER_BYTES: usize = 256 * 1024;

pub fn ensure_space(path: &Path, bytes: u64) -> Result<()> {
    let free = fs2::available_space(path)?;
    if free < bytes.saturating_add(DISK_RESERVE_BYTES) {
        bail!("磁盘空间不足，必须保留至少 1 GiB 空间");
    }
    Ok(())
}

pub fn validate_component(name: &str) -> Result<()> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.ends_with([' ', '.'])
        || name
            .chars()
            .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c))
    {
        bail!("文件名不能在两端安全表示：{name:?}");
    }
    let stem = name.split('.').next().unwrap().to_ascii_uppercase();
    if ["CON", "PRN", "AUX", "NUL"].contains(&stem.as_str())
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && matches!(stem.as_bytes()[3], b'1'..=b'9'))
    {
        bail!("Windows 保留文件名：{name}");
    }
    if name.encode_utf16().count() > 255 || name.len() > 255 {
        bail!("文件名过长：{name}");
    }
    Ok(())
}

fn checked_relative(path: &Path) -> Result<String> {
    let mut names = Vec::new();
    for part in path.components() {
        match part {
            Component::Normal(name) => {
                let name = name.to_str().context("文件名不是有效 Unicode")?;
                validate_component(name)?;
                names.push(name);
            }
            _ => bail!("归档包含不安全路径：{}", path.display()),
        }
    }
    if names.is_empty() {
        bail!("归档包含空路径");
    }
    Ok(names.join("/"))
}

fn collision_key(name: &str) -> String {
    name.nfc().collect::<String>().to_lowercase()
}
fn check_prefix_aliases(name: &str, aliases: &mut HashMap<String, String>) -> Result<()> {
    let mut prefix = String::new();
    for component in name.split('/') {
        if !prefix.is_empty() {
            prefix.push('/');
        }
        prefix.push_str(component);
        let key = collision_key(&prefix);
        if let Some(previous) = aliases.get(&key) {
            if previous != &prefix {
                bail!("目录名称在跨平台还原时冲突：{previous} / {prefix}");
            }
        } else {
            aliases.insert(key, prefix.clone());
        }
    }
    Ok(())
}
static RESERVED_BYTES: std::sync::Mutex<u64> = std::sync::Mutex::new(0);
pub struct DiskReservation(u64);
impl Drop for DiskReservation {
    fn drop(&mut self) {
        let mut total = RESERVED_BYTES.lock().unwrap();
        *total = total.saturating_sub(self.0);
    }
}
pub fn reserve_write(path: &Path, bytes: u64) -> Result<DiskReservation> {
    let mut outstanding = RESERVED_BYTES.lock().unwrap();
    ensure_space(path, bytes.saturating_add(*outstanding))?;
    *outstanding = outstanding.saturating_add(bytes);
    Ok(DiskReservation(bytes))
}
pub fn write_checked(path: &Path, bytes: &[u8]) -> Result<()> {
    let directory = path.parent().context("写入目录无效")?;
    let mut output = GuardedWriter {
        file: File::create(path)?,
        directory: directory.into(),
    };
    output.write_all(bytes)?;
    output.file.sync_all()?;
    Ok(())
}

struct GuardedWriter {
    file: File,
    directory: PathBuf,
}
impl Write for GuardedWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let _reservation =
            reserve_write(&self.directory, buf.len() as u64).map_err(std::io::Error::other)?;
        self.file.write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

pub fn hash_file(path: &Path) -> Result<(u64, String)> {
    let mut input = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buf = [0u8; 128 * 1024];
    let mut total = 0;
    loop {
        let n = input.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
        total += n as u64;
    }
    Ok((total, hex::encode(hash.finalize())))
}

/// Snapshot ordinary files without following links. TAR preserves nested and empty directories.
pub fn pack(paths: &[PathBuf], destination: &Path) -> Result<(u64, String)> {
    pack_with_limit(paths, destination, None)
}

pub(crate) fn pack_with_limit(
    paths: &[PathBuf],
    destination: &Path,
    limit: Option<u64>,
) -> Result<(u64, String)> {
    if paths.is_empty() {
        bail!("没有要同步的文件");
    }
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    let mut aliases = HashMap::new();
    let mut estimate = 10240u64;
    let mut file_bytes = 0u64;
    for root in paths {
        let root_name = root.file_name().context("不能同步文件系统根目录")?;
        let parent = root.parent().context("没有父目录")?;
        validate_component(root_name.to_str().context("文件名不是 Unicode")?)?;
        for entry in walkdir::WalkDir::new(root)
            .follow_links(false)
            .sort_by_file_name()
        {
            let entry = entry?;
            let meta = fs::symlink_metadata(entry.path())?;
            if !meta.is_file() && !meta.is_dir() {
                bail!("不支持符号链接或特殊文件：{}", entry.path().display());
            }
            if meta.is_file() {
                file_bytes = file_bytes
                    .checked_add(meta.len())
                    .context("文件总大小超出支持范围")?;
                if limit.is_some_and(|limit| file_bytes > limit) {
                    bail!("文件已增大至超过上传确认阈值，请重新复制并确认上传");
                }
            }
            let relative = entry.path().strip_prefix(parent)?;
            let name = checked_relative(relative)?;
            check_prefix_aliases(&name, &mut aliases)?;
            if !seen.insert(collision_key(&name)) {
                bail!("文件名称在跨平台还原时冲突：{name}");
            }
            estimate = estimate.saturating_add(meta.len()).saturating_add(2048);
            entries.push((entry.path().to_path_buf(), name, meta));
        }
    }
    let directory = destination.parent().context("缓存目录无效")?;
    fs::create_dir_all(directory)?;
    ensure_space(directory, estimate)?;
    let writer = GuardedWriter {
        file: OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(destination)?,
        directory: directory.into(),
    };
    let mut archive = tar::Builder::new(BufWriter::with_capacity(FILE_BUFFER_BYTES, writer));
    archive.follow_symlinks(false);
    for (source, name, before) in entries {
        let now = fs::symlink_metadata(&source)?;
        if now.is_symlink() || now.len() != before.len() || now.modified()? != before.modified()? {
            bail!("准备期间源文件已变化：{}", source.display());
        }
        if now.is_dir() {
            archive.append_dir(name, source)?;
        } else {
            let mut options = OpenOptions::new();
            options.read(true);
            #[cfg(target_os = "macos")]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.custom_flags(0x100);
            }
            #[cfg(target_os = "linux")]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.custom_flags(0x20000);
            }
            #[cfg(target_os = "windows")]
            {
                use std::os::windows::fs::OpenOptionsExt;
                options.custom_flags(0x00200000);
            }
            let mut input = options.open(&source)?;
            let opened = input.metadata()?;
            #[cfg(target_os = "windows")]
            {
                use std::os::windows::fs::MetadataExt;
                if opened.file_attributes() & 0x400 != 0 {
                    bail!("不支持重解析点：{}", source.display());
                }
            }
            if !opened.is_file()
                || opened.len() != before.len()
                || opened.modified()? != before.modified()?
            {
                bail!("准备期间源文件已变化：{}", source.display());
            }
            archive.append_file(name, &mut input)?;
            let after = input.metadata()?;
            if after.len() != before.len() || after.modified()? != before.modified()? {
                bail!("读取期间源文件已变化：{}", source.display());
            }
        }
    }
    let mut writer = archive.into_inner()?;
    writer.flush()?;
    writer.get_ref().file.sync_all()?;
    hash_file(destination)
}

/// Never delegates extraction to TAR's generic unpack: only ordinary files/dirs,
/// create_new, portable relative paths, and no duplicate/case/Unicode aliases.
pub fn unpack(payload: &Path, destination: &Path) -> Result<Vec<PathBuf>> {
    if destination.exists() {
        bail!("还原目标已存在");
    }
    fs::create_dir_all(destination)?;
    let result = (|| {
        ensure_space(destination, fs::metadata(payload)?.len())?;
        let mut archive = tar::Archive::new(File::open(payload)?);
        let mut seen = HashSet::new();
        let mut aliases = HashMap::new();
        let mut roots = HashSet::new();
        for entry in archive.entries()? {
            let mut entry = entry?;
            let kind = entry.header().entry_type();
            if !(kind.is_file() || kind.is_dir()) {
                bail!("归档含链接或不支持的条目");
            }
            let path = entry.path()?.into_owned();
            let name = checked_relative(&path)?;
            check_prefix_aliases(&name, &mut aliases)?;
            if !seen.insert(collision_key(&name)) {
                bail!("归档含重复或跨平台冲突路径：{name}");
            }
            roots.insert(name.split('/').next().unwrap().to_string());
            let target = destination.join(&path);
            if kind.is_dir() {
                fs::create_dir_all(&target)?;
            } else {
                ensure_space(destination, entry.size())?;
                fs::create_dir_all(target.parent().unwrap())?;
                let output = GuardedWriter {
                    file: OpenOptions::new()
                        .create_new(true)
                        .write(true)
                        .open(&target)?,
                    directory: destination.into(),
                };
                let mut output = BufWriter::with_capacity(FILE_BUFFER_BYTES, output);
                std::io::copy(&mut entry, &mut output)?;
                output.flush()?;
                output.get_ref().file.sync_all()?;
            }
        }
        if roots.is_empty() {
            bail!("文件归档为空");
        }
        let mut roots: Vec<_> = roots.into_iter().map(|r| destination.join(r)).collect();
        roots.sort();
        Ok(roots)
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(destination);
    }
    result
}

pub fn copy_saved(source: &Path, target: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(source)?;
    if metadata.is_symlink() || (!metadata.is_file() && !metadata.is_dir()) {
        bail!("缓存内容不再是普通文件");
    }
    if target.exists() {
        bail!("目标已存在，不会覆盖：{}", target.display());
    }
    if metadata.is_dir() {
        fs::create_dir(target)?;
        for child in fs::read_dir(source)? {
            let child = child?;
            copy_saved(&child.path(), &target.join(child.file_name()))?;
        }
    } else {
        let parent = target.parent().context("保存路径无效")?;
        ensure_space(parent, metadata.len())?;
        let mut output = GuardedWriter {
            file: OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(target)?,
            directory: parent.into(),
        };
        std::io::copy(&mut File::open(source)?, &mut output)?;
        output.file.sync_all()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn buffered_archive_digest_and_snapshot_survive_source_changes() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source.bin");
        let bytes: Vec<_> = (0..FILE_BUFFER_BYTES * 2 + 123)
            .map(|i| (i % 251) as u8)
            .collect();
        fs::write(&source, &bytes).unwrap();
        let payload = root.path().join("payload.tar");
        let packed_digest = pack(std::slice::from_ref(&source), &payload).unwrap();
        assert_eq!(packed_digest, hash_file(&payload).unwrap());
        fs::write(&source, "changed after capture").unwrap();
        let files = unpack(&payload, &root.path().join("snapshot")).unwrap();
        assert_eq!(fs::read(&files[0]).unwrap(), bytes);
        assert_eq!(packed_digest, hash_file(&payload).unwrap());
    }

    #[test]
    fn nested_and_empty_directories_round_trip() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("source/嵌套/空目录")).unwrap();
        fs::write(root.path().join("source/嵌套/data.txt"), "hello").unwrap();
        let tar = root.path().join("payload.tar");
        pack(&[root.path().join("source")], &tar).unwrap();
        let files = unpack(&tar, &root.path().join("restored")).unwrap();
        assert_eq!(files.len(), 1);
        assert!(files[0].join("嵌套/空目录").is_dir());
        assert_eq!(
            fs::read_to_string(files[0].join("嵌套/data.txt")).unwrap(),
            "hello"
        );
    }
    #[test]
    fn rejects_nonportable_names() {
        for name in ["CON", "aux.txt", "COM9", "trailing.", "a:b", "a\\b", ".."] {
            assert!(validate_component(name).is_err(), "{name}");
        }
    }
    #[test]
    fn rejects_parent_aliases_even_without_directory_entries() {
        let root = tempfile::tempdir().unwrap();
        let archive = root.path().join("aliases.tar");
        let mut builder = tar::Builder::new(File::create(&archive).unwrap());
        for name in ["Root/a.txt", "root/b.txt"] {
            let mut header = tar::Header::new_gnu();
            header.set_size(1);
            header.set_mode(0o600);
            header.set_cksum();
            builder.append_data(&mut header, name, &b"x"[..]).unwrap();
        }
        builder.finish().unwrap();
        assert!(unpack(&archive, &root.path().join("out")).is_err());
        assert!(!root.path().join("out").exists());
    }
    #[test]
    fn refuses_impossible_disk_reservation() {
        let root = tempfile::tempdir().unwrap();
        assert!(ensure_space(root.path(), u64::MAX).is_err());
        assert!(reserve_write(root.path(), u64::MAX).is_err());
        assert!(reserve_write(root.path(), 1).is_ok());
    }
    #[cfg(unix)]
    #[test]
    fn refuses_source_symlinks_without_touching_target() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("target"), "secret").unwrap();
        std::os::unix::fs::symlink(root.path().join("target"), root.path().join("link")).unwrap();
        assert!(pack(&[root.path().join("link")], &root.path().join("payload")).is_err());
        assert_eq!(
            fs::read_to_string(root.path().join("target")).unwrap(),
            "secret"
        );
    }
    #[test]
    fn rejects_link_archive() {
        let root = tempfile::tempdir().unwrap();
        let mut builder = tar::Builder::new(File::create(root.path().join("bad.tar")).unwrap());
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_size(0);
        header.set_mode(0o777);
        builder
            .append_link(&mut header, "link", "../../outside")
            .unwrap();
        builder.finish().unwrap();
        assert!(unpack(&root.path().join("bad.tar"), &root.path().join("out")).is_err());
        assert!(!root.path().join("out").exists());
    }
}
