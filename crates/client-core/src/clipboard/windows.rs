//! Win32 clipboard formats and WM_CLIPBOARDUPDATE message-only listener.
//! Raw ABI declarations keep ownership of HGLOBAL explicit: ownership transfers
//! only after successful SetClipboardData, and source files are never moved.
use super::{Clipboard, ClipboardContent};
use anyhow::{bail, Context, Result};
use std::{
    ffi::c_void,
    io::Cursor,
    path::PathBuf,
    ptr,
    sync::{
        atomic::{AtomicBool, AtomicPtr, Ordering},
        Once,
    },
    thread,
    time::Duration,
};
type Handle = *mut c_void;
const CF_UNICODETEXT: u32 = 13;
const CF_HDROP: u32 = 15;
const CF_DIB: u32 = 8;
const CF_DIBV5: u32 = 17;
const WM_CLIPBOARDUPDATE: u32 = 0x031d;
static CHANGED: AtomicBool = AtomicBool::new(true);
static LISTENER_READY: AtomicBool = AtomicBool::new(false);
static START: Once = Once::new();
static OWNER: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static LISTENER_FAILED: AtomicBool = AtomicBool::new(false);
#[repr(C)]
struct Point {
    x: i32,
    y: i32,
}
#[repr(C)]
struct Msg {
    hwnd: Handle,
    message: u32,
    wparam: usize,
    lparam: isize,
    time: u32,
    pt: Point,
    private: u32,
}
#[repr(C)]
struct WndClass {
    style: u32,
    procedure: Option<unsafe extern "system" fn(Handle, u32, usize, isize) -> isize>,
    cls_extra: i32,
    wnd_extra: i32,
    instance: Handle,
    icon: Handle,
    cursor: Handle,
    background: Handle,
    menu_name: *const u16,
    class_name: *const u16,
}
#[link(name = "user32")]
unsafe extern "system" {
    fn GetClipboardSequenceNumber() -> u32;
    fn OpenClipboard(owner: Handle) -> i32;
    fn CloseClipboard() -> i32;
    fn EmptyClipboard() -> i32;
    fn GetClipboardData(format: u32) -> Handle;
    fn SetClipboardData(format: u32, data: Handle) -> Handle;
    fn IsClipboardFormatAvailable(format: u32) -> i32;
    fn RegisterClipboardFormatW(name: *const u16) -> u32;
    fn RegisterClassW(class: *const WndClass) -> u16;
    fn CreateWindowExW(
        ex: u32,
        class: *const u16,
        name: *const u16,
        style: u32,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        parent: Handle,
        menu: Handle,
        instance: Handle,
        param: Handle,
    ) -> Handle;
    fn DefWindowProcW(hwnd: Handle, message: u32, wparam: usize, lparam: isize) -> isize;
    fn AddClipboardFormatListener(hwnd: Handle) -> i32;
    fn GetMessageW(message: *mut Msg, hwnd: Handle, min: u32, max: u32) -> i32;
    fn TranslateMessage(message: *const Msg) -> i32;
    fn DispatchMessageW(message: *const Msg) -> isize;
}
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GlobalAlloc(flags: u32, bytes: usize) -> Handle;
    fn GlobalLock(memory: Handle) -> Handle;
    fn GlobalUnlock(memory: Handle) -> i32;
    fn GlobalSize(memory: Handle) -> usize;
    fn GlobalFree(memory: Handle) -> Handle;
    fn GetModuleHandleW(name: *const u16) -> Handle;
}
#[link(name = "shell32")]
unsafe extern "system" {
    fn DragQueryFileW(drop: Handle, index: u32, name: *mut u16, count: u32) -> u32;
}
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain([0]).collect()
}
unsafe extern "system" fn procedure(
    hwnd: Handle,
    message: u32,
    wparam: usize,
    lparam: isize,
) -> isize {
    if message == WM_CLIPBOARDUPDATE {
        CHANGED.store(true, Ordering::SeqCst);
        return 0;
    }
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}
fn start_listener() {
    START.call_once(|| {
        thread::spawn(|| unsafe {
            let name = wide("EasyCopyClipboardListener");
            let instance = GetModuleHandleW(ptr::null());
            let class = WndClass {
                style: 0,
                procedure: Some(procedure),
                cls_extra: 0,
                wnd_extra: 0,
                instance,
                icon: ptr::null_mut(),
                cursor: ptr::null_mut(),
                background: ptr::null_mut(),
                menu_name: ptr::null(),
                class_name: name.as_ptr(),
            };
            if RegisterClassW(&class) == 0 {
                LISTENER_FAILED.store(true, Ordering::SeqCst);
                return;
            }
            let hwnd = CreateWindowExW(
                0,
                name.as_ptr(),
                name.as_ptr(),
                0,
                0,
                0,
                0,
                0,
                -3isize as Handle,
                ptr::null_mut(),
                instance,
                ptr::null_mut(),
            );
            if hwnd.is_null() || AddClipboardFormatListener(hwnd) == 0 {
                LISTENER_FAILED.store(true, Ordering::SeqCst);
                return;
            }
            OWNER.store(hwnd, Ordering::SeqCst);
            LISTENER_READY.store(true, Ordering::SeqCst);
            let mut message: Msg = std::mem::zeroed();
            while GetMessageW(&mut message, ptr::null_mut(), 0, 0) > 0 {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
            LISTENER_READY.store(false, Ordering::SeqCst);
        });
    });
}
struct Open;
impl Open {
    fn acquire() -> Result<Self> {
        start_listener();
        for _ in 0..100 {
            if LISTENER_FAILED.load(Ordering::SeqCst) {
                bail!("无法创建系统剪贴板监听窗口");
            }
            let owner = OWNER.load(Ordering::SeqCst);
            if !owner.is_null() && unsafe { OpenClipboard(owner) } != 0 {
                return Ok(Self);
            }
            thread::sleep(Duration::from_millis(10));
        }
        bail!("剪贴板正被其他应用占用");
    }
}
impl Drop for Open {
    fn drop(&mut self) {
        unsafe {
            CloseClipboard();
        }
    }
}
unsafe fn read_bytes(format: u32) -> Result<Vec<u8>> {
    let memory = unsafe { GetClipboardData(format) };
    if memory.is_null() {
        bail!("剪贴板数据不可用");
    }
    let size = unsafe { GlobalSize(memory) };
    let data = unsafe { GlobalLock(memory) };
    if data.is_null() && size != 0 {
        bail!("无法读取系统剪贴板内存");
    }
    let bytes = if size == 0 {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(data as *const u8, size).to_vec() }
    };
    unsafe {
        GlobalUnlock(memory);
    }
    Ok(bytes)
}
unsafe fn put_bytes(format: u32, bytes: &[u8]) -> Result<()> {
    let memory = unsafe { GlobalAlloc(0x0042, bytes.len()) }; // MOVEABLE | ZEROINIT
    if memory.is_null() {
        bail!("剪贴板内存不足");
    }
    let data = unsafe { GlobalLock(memory) };
    if data.is_null() {
        unsafe {
            GlobalFree(memory);
        }
        bail!("无法锁定剪贴板内存");
    }
    unsafe {
        ptr::copy_nonoverlapping(bytes.as_ptr(), data as *mut u8, bytes.len());
        GlobalUnlock(memory);
    }
    if unsafe { SetClipboardData(format, memory) }.is_null() {
        unsafe {
            GlobalFree(memory);
        }
        bail!("系统拒绝剪贴板格式 {format}");
    }
    Ok(())
}
fn png_format() -> u32 {
    unsafe { RegisterClipboardFormatW(wide("PNG").as_ptr()) }
}
#[derive(Default)]
pub struct NativeClipboard;
impl Clipboard for NativeClipboard {
    fn poll_changed(&self) -> bool {
        start_listener();
        !LISTENER_READY.load(Ordering::SeqCst) || CHANGED.swap(false, Ordering::SeqCst)
    }
    fn sequence(&self) -> Result<u64> {
        start_listener();
        Ok(unsafe { GetClipboardSequenceNumber() } as u64)
    }
    fn read(&self) -> Result<Option<ClipboardContent>> {
        let _open = Open::acquire()?;
        unsafe {
            if IsClipboardFormatAvailable(CF_HDROP) != 0 {
                let drop = GetClipboardData(CF_HDROP);
                if drop.is_null() {
                    bail!("无法读取文件引用");
                }
                let count = DragQueryFileW(drop, u32::MAX, ptr::null_mut(), 0);
                let mut paths = Vec::new();
                for index in 0..count {
                    let len = DragQueryFileW(drop, index, ptr::null_mut(), 0);
                    let mut buffer = vec![0u16; len as usize + 1];
                    if DragQueryFileW(drop, index, buffer.as_mut_ptr(), len + 1) != len {
                        bail!("文件引用读取不完整");
                    }
                    paths.push(PathBuf::from(
                        String::from_utf16(&buffer[..len as usize])
                            .context("文件路径不是有效 Unicode")?,
                    ));
                }
                if !paths.is_empty() {
                    return Ok(Some(ClipboardContent::Files(paths)));
                }
            }
            if IsClipboardFormatAvailable(png_format()) != 0 {
                return Ok(Some(ClipboardContent::Image(read_bytes(png_format())?)));
            }
            for format in [CF_DIBV5, CF_DIB] {
                if IsClipboardFormatAvailable(format) != 0 {
                    return Ok(Some(ClipboardContent::Image(dib_to_png(&read_bytes(
                        format,
                    )?)?)));
                }
            }
            if IsClipboardFormatAvailable(CF_UNICODETEXT) != 0 {
                let bytes = read_bytes(CF_UNICODETEXT)?;
                let text: Vec<u16> = bytes
                    .chunks_exact(2)
                    .map(|v| u16::from_le_bytes([v[0], v[1]]))
                    .take_while(|&v| v != 0)
                    .collect();
                return Ok(Some(ClipboardContent::Text(String::from_utf16(&text)?)));
            }
        }
        Ok(None)
    }
    fn write_if_sequence(&self, content: &ClipboardContent, expected: u64) -> Result<Option<u64>> {
        let _open = Open::acquire()?;
        if self.sequence()? != expected {
            return Ok(None);
        }
        // Prepare every representation before changing the global clipboard.
        let mut formats = Vec::new();
        match content {
            ClipboardContent::Text(text) => formats.push((
                CF_UNICODETEXT,
                wide(text)
                    .iter()
                    .flat_map(|v| v.to_le_bytes())
                    .collect::<Vec<_>>(),
            )),
            ClipboardContent::Image(png) => {
                formats.push((png_format(), png.clone()));
                formats.push((CF_DIBV5, png_to_dib(png)?));
            }
            ClipboardContent::Files(paths) => {
                let mut buffer = vec![0u8; 20];
                buffer[0..4].copy_from_slice(&20u32.to_le_bytes());
                buffer[16..20].copy_from_slice(&1u32.to_le_bytes());
                for path in paths {
                    if !path.exists() {
                        bail!("缓存文件不存在：{}", path.display());
                    }
                    let path = path.to_str().context("文件名不是有效 Unicode")?;
                    for v in wide(path) {
                        buffer.extend_from_slice(&v.to_le_bytes());
                    }
                }
                buffer.extend_from_slice(&[0, 0]);
                formats.push((CF_HDROP, buffer));
            }
        }
        unsafe {
            if EmptyClipboard() == 0 {
                bail!("无法更新系统剪贴板");
            }
            for (format, bytes) in formats {
                put_bytes(format, &bytes)?;
            }
        }
        Ok(Some(self.sequence()?))
    }
}
fn read_u32(bytes: &[u8], at: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(
        bytes
            .get(at..at + 4)
            .context("DIB 数据不完整")?
            .try_into()?,
    ))
}
fn dib_to_png(bytes: &[u8]) -> Result<Vec<u8>> {
    let header = read_u32(bytes, 0)? as usize;
    if header < 40 || header > bytes.len() {
        bail!("DIB 头无效");
    }
    let width = read_u32(bytes, 4)? as i32;
    let height = read_u32(bytes, 8)? as i32;
    let bit_count = u16::from_le_bytes(bytes.get(14..16).context("DIB 数据不完整")?.try_into()?);
    let compression = read_u32(bytes, 16)?;
    if width <= 0
        || height == 0
        || height == i32::MIN
        || ![24, 32].contains(&bit_count)
        || ![0, 3].contains(&compression)
    {
        bail!("不支持此剪贴板 DIB 格式");
    }
    let w = width as usize;
    let h = height.unsigned_abs() as usize;
    let row = w
        .checked_mul(bit_count as usize)
        .and_then(|n| n.checked_add(31))
        .context("图片尺寸溢出")?
        / 32
        * 4;
    let offset = header
        + if header == 40 && compression == 3 {
            12
        } else {
            0
        };
    let expected = offset
        .checked_add(row.checked_mul(h).context("图片尺寸溢出")?)
        .context("图片尺寸溢出")?;
    if expected > bytes.len() {
        bail!("图片数据不完整");
    }
    if compression == 3 {
        let base = if header >= 52 { 40 } else { header };
        if read_u32(bytes, base)? != 0x00ff0000
            || read_u32(bytes, base + 4)? != 0x0000ff00
            || read_u32(bytes, base + 8)? != 0x000000ff
        {
            bail!("不支持此位域颜色格式");
        }
    }
    let alpha = header >= 56 && read_u32(bytes, 52)? == 0xff000000;
    let mut image = image::RgbaImage::new(w as u32, h as u32);
    for y in 0..h {
        let source_y = if height > 0 { h - 1 - y } else { y };
        for x in 0..w {
            let p = offset + source_y * row + x * (bit_count as usize / 8);
            image.put_pixel(
                x as u32,
                y as u32,
                image::Rgba([
                    bytes[p + 2],
                    bytes[p + 1],
                    bytes[p],
                    if alpha && bit_count == 32 {
                        bytes[p + 3]
                    } else {
                        255
                    },
                ]),
            );
        }
    }
    let mut png = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image).write_to(&mut png, image::ImageFormat::Png)?;
    Ok(png.into_inner())
}
fn png_to_dib(png: &[u8]) -> Result<Vec<u8>> {
    let image = image::load_from_memory_with_format(png, image::ImageFormat::Png)?.to_rgba8();
    let mut bytes = vec![0u8; 124];
    for (at, value) in [
        (0, 124),
        (4, image.width()),
        (8, (-(image.height() as i32)) as u32),
        (16, 3),
        (20, image.width() * image.height() * 4),
        (40, 0x00ff0000),
        (44, 0x0000ff00),
        (48, 0x000000ff),
        (52, 0xff000000),
        (56, 0x73524742),
    ] {
        bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }
    bytes[12..14].copy_from_slice(&1u16.to_le_bytes());
    bytes[14..16].copy_from_slice(&32u16.to_le_bytes());
    for pixel in image.pixels() {
        bytes.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
    }
    Ok(bytes)
}
