//! Windows-only glue: where the app's text output goes, letting the running copy come to the
//! front, the app id (taskbar and toasts), the window's icon and title bar, Explorer's "show in
//! folder", ending Electron with the app, and a few registry calls (`reg`).

use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, IntoRawHandle};
use std::path::Path;

use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE, SetStdHandle};
use windows_sys::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;
use windows_sys::Win32::UI::WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow};

use uzumaki_ctl::paths::{self, Kind};

/// The log is started again when it is larger than this at start.
const LOG_MAX: u64 = 2 * 1024 * 1024;

pub fn wide(s: &str) -> Vec<u16> {
    std::ffi::OsStr::new(s).encode_wide().chain(Some(0)).collect()
}

fn has(std_handle: u32) -> bool {
    // SAFETY: no preconditions.
    let h: HANDLE = unsafe { GetStdHandle(std_handle) };
    !h.is_null() && h != INVALID_HANDLE_VALUE
}

fn point(std_handle: u32, file: std::fs::File) {
    // SAFETY: the handle stays open for the life of the process (it is never closed).
    unsafe { SetStdHandle(std_handle, file.into_raw_handle()) };
}

/// Uzumaki is a windowed program, so it starts with no console. This gives its messages a place:
/// - output that is already redirected (`uzumaki --list-scenes > file`, a pipe) is left alone;
/// - started from a terminal, it writes to that terminal;
/// - started from Explorer, the Start menu or at login, it appends to
///   `%LOCALAPPDATA%\Uzumaki\Logs\uzumaki.log`.
pub fn attach_output() {
    let (out, err) = (has(STD_OUTPUT_HANDLE), has(STD_ERROR_HANDLE));
    if out && err {
        return;
    }
    let console = || std::fs::OpenOptions::new().read(true).write(true).open("CONOUT$").ok();
    // SAFETY: no preconditions; fails when the parent has no console.
    let attached = unsafe { AttachConsole(ATTACH_PARENT_PROCESS) } != 0;
    for (present, which) in [(out, STD_OUTPUT_HANDLE), (err, STD_ERROR_HANDLE)] {
        if present {
            continue;
        }
        let target = if attached { console() } else { None }.or_else(log_file);
        if let Some(f) = target {
            point(which, f);
        }
    }
    if !attached {
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        eprintln!("--- uzumaki {} started, pid {}, unix time {secs}", uzumaki_ctl::version_line(), std::process::id());
    }
}

/// Where the log is.
pub fn log_path() -> std::path::PathBuf {
    paths::dir(Kind::Logs).join("uzumaki.log")
}

fn log_file() -> Option<std::fs::File> {
    let path = log_path();
    std::fs::create_dir_all(path.parent()?).ok()?;
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > LOG_MAX) {
        let _ = std::fs::rename(&path, path.with_extension("log.1"));
    }
    std::fs::OpenOptions::new().create(true).append(true).open(&path).ok()
}

/// The second copy calls this before it asks the running one to show its window: Windows only
/// lets a background program come to the front when the foreground one allows it.
pub fn allow_focus() {
    // SAFETY: no preconditions.
    unsafe { AllowSetForegroundWindow(ASFW_ANY) };
}

/// Gives this process Uzumaki's app id (the installer's Start menu shortcut carries the same
/// one), so the taskbar groups its windows under that shortcut and toasts are Uzumaki's. Call
/// it before any window opens.
pub fn set_app_id() {
    let id = wide(crate::APP_ID);
    // SAFETY: a NUL-terminated string that lives across the call.
    unsafe { SetCurrentProcessExplicitAppUserModelID(id.as_ptr()) };
}

/// The 256 px app icon, written to the app's folder so Windows can draw it on toasts.
const ICON_PNG: &[u8] = include_bytes!("../../assets/icons/hicolor/256x256/apps/io.github.hanami117.uzumaki.png");

/// Where the app id's name and icon are registered for toasts, under `HKEY_CURRENT_USER`.
pub fn app_id_key() -> String {
    format!(r"Software\Classes\AppUserModelId\{}", crate::APP_ID)
}

/// Registers the app id's name and icon for the current user: what a toast and the media
/// overlay show as the sender, also for a copy that was only unpacked (no installer, no
/// shortcut). The installer's uninstaller removes the key. False when the registry refused.
pub fn register_app_id() -> bool {
    let key = app_id_key();
    let mut ok = reg::set_string(&key, "DisplayName", "Uzumaki").is_ok();
    let icon = paths::dir(Kind::State).join("uzumaki-icon.png");
    let fresh = std::fs::metadata(&icon).is_ok_and(|m| m.len() == ICON_PNG.len() as u64);
    if fresh || (icon.parent().is_some_and(|d| std::fs::create_dir_all(d).is_ok()) && std::fs::write(&icon, ICON_PNG).is_ok()) {
        ok &= reg::set_string(&key, "IconUri", &icon.to_string_lossy()).is_ok();
    }
    ok
}

/// The window shows: give it the program's own icon (the one `build.rs` put into the exe, id 1),
/// for the title bar, the taskbar and Alt+Tab (Rune sets none, and Windows would draw its blank
/// default), and a dark title bar (the app is dark whatever Windows' app mode is). On the UI
/// thread, once per window.
pub fn on_window(handle: &rune::WindowHandle) {
    use rune::raw_window_handle::RawWindowHandle;
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, ICON_BIG, ICON_SMALL, IMAGE_ICON, LR_SHARED, LoadImageW, SM_CXICON, SM_CXSMICON, SM_CYICON, SM_CYSMICON, SendMessageW, WM_SETICON};

    let _ = handle.with_raw_handle(|h| {
        let RawWindowHandle::Win32(h) = h.as_raw() else { return };
        let hwnd = h.hwnd.get() as windows_sys::Win32::Foundation::HWND;
        // SAFETY: the window is alive for this callback; resource id 1 is passed the way
        // MAKEINTRESOURCE does (a small number in place of a name); LR_SHARED icons belong
        // to the system and are never freed by us. A missing icon gives null, and the
        // window keeps the default.
        unsafe {
            let module = GetModuleHandleW(std::ptr::null());
            for (which, cx, cy) in [(ICON_BIG, SM_CXICON, SM_CYICON), (ICON_SMALL, SM_CXSMICON, SM_CYSMICON)] {
                let icon = LoadImageW(module, std::ptr::without_provenance::<u16>(1), IMAGE_ICON, GetSystemMetrics(cx), GetSystemMetrics(cy), LR_SHARED);
                if !icon.is_null() {
                    SendMessageW(hwnd, WM_SETICON, which as usize, icon as isize);
                }
            }
        }
    });
    sync_caption(handle);
}

/// The title bar in the app's palette: dark with the dark theme, light with the light one,
/// whatever Windows' own app mode is. On the UI thread; again when the theme flips.
pub fn sync_caption(handle: &rune::WindowHandle) {
    use rune::raw_window_handle::RawWindowHandle;
    use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;

    /// `DWMWA_USE_IMMERSIVE_DARK_MODE` (Windows 10 2004 and later; ignored before).
    const DARK_MODE: u32 = 20;

    let dark: i32 = (!crate::theme::is_light()) as i32;
    let _ = handle.with_raw_handle(|h| {
        let RawWindowHandle::Win32(h) = h.as_raw() else { return };
        let hwnd = h.hwnd.get() as windows_sys::Win32::Foundation::HWND;
        // SAFETY: the window is alive for this callback; the attribute is a BOOL read during the call.
        unsafe { DwmSetWindowAttribute(hwnd, DARK_MODE, (&raw const dark).cast(), std::mem::size_of::<i32>() as u32) };
    });
}

/// Explorer with `path` selected in its folder (an Explorer window already showing the folder is
/// reused). Blocks briefly: call it on a worker thread.
pub fn show_in_folder(path: &Path) {
    use windows_sys::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
    use windows_sys::Win32::UI::Shell::{ILCreateFromPathW, ILFree, SHOpenFolderAndSelectItems};

    let w: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: COM is started for this thread and stopped again when it was started here; the
    // item list the shell allocates is freed with ILFree.
    let shown = unsafe {
        let init = CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32);
        let item = ILCreateFromPathW(w.as_ptr());
        let ok = !item.is_null() && SHOpenFolderAndSelectItems(item, 0, std::ptr::null(), 0) >= 0;
        if !item.is_null() {
            ILFree(item);
        }
        if init >= 0 {
            CoUninitialize();
        }
        ok
    };
    // The file is gone, or the shell refused: open the folder.
    if !shown {
        if let Some(dir) = path.parent() {
            let _ = std::process::Command::new("explorer.exe").arg(dir).spawn();
        }
    }
}

/// Puts `child` (Electron) in a job that Windows ends when this process does, however it ends:
/// a crash or Task Manager's End task must not leave a hidden player holding the sign-in profile.
/// Its own helper processes join the job as they start.
pub fn end_with_us(child: &std::process::Child) {
    use std::sync::OnceLock;
    use windows_sys::Win32::System::JobObjects::{AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation, SetInformationJobObject};

    /// The job's handle, as a number (a raw handle isn't `Sync`). Never closed: closing it is
    /// what ends the processes, and that happens when this process ends.
    static JOB: OnceLock<usize> = OnceLock::new();
    let job = *JOB.get_or_init(|| {
        // SAFETY: plain calls; `info` is a zeroed plain struct with one flag set.
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return 0;
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            SetInformationJobObject(job, JobObjectExtendedLimitInformation, (&raw const info).cast(), std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32);
            job as usize
        }
    });
    if job != 0 {
        // SAFETY: both handles are open; a refusal (the child is in a job that forbids it) only
        // means the old behaviour: Electron ends when its input closes.
        unsafe { AssignProcessToJobObject(job as HANDLE, child.as_raw_handle() as HANDLE) };
    }
}

/// The side of a notification-area icon in pixels at the system's scale: 16 at 100 %, 20 at
/// 125 %, 24 at 150 %, 32 at 200 %.
pub fn tray_icon_px() -> u32 {
    use windows_sys::Win32::UI::HiDpi::{GetDpiForSystem, GetSystemMetricsForDpi};
    use windows_sys::Win32::UI::WindowsAndMessaging::SM_CXSMICON;
    // SAFETY: no preconditions.
    let px = unsafe { GetSystemMetricsForDpi(SM_CXSMICON, GetDpiForSystem()) };
    (px.clamp(16, 64)) as u32
}

/// The few registry calls the app needs, all under `HKEY_CURRENT_USER`.
pub mod reg {
    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_BINARY, RRF_RT_REG_SZ, RegCloseKey, RegCreateKeyExW, RegDeleteKeyValueW, RegGetValueW, RegSetValueExW};

    use super::wide;

    fn io(code: u32) -> std::io::Error {
        std::io::Error::from_raw_os_error(code as i32)
    }

    /// Writes a text value; the key is made when it is missing.
    pub fn set_string(key: &str, name: &str, value: &str) -> std::io::Result<()> {
        let mut h: HKEY = std::ptr::null_mut();
        // SAFETY: the key name is NUL-terminated; the handle is closed below.
        let r = unsafe { RegCreateKeyExW(HKEY_CURRENT_USER, wide(key).as_ptr(), 0, std::ptr::null(), REG_OPTION_NON_VOLATILE, KEY_SET_VALUE, std::ptr::null(), &mut h, std::ptr::null_mut()) };
        if r != ERROR_SUCCESS {
            return Err(io(r));
        }
        let data = wide(value);
        // SAFETY: `h` is open for writing; `data` is NUL-terminated UTF-16 and its size is in bytes.
        let r = unsafe { RegSetValueExW(h, wide(name).as_ptr(), 0, REG_SZ, data.as_ptr().cast(), (data.len() * 2) as u32) };
        // SAFETY: `h` was opened above.
        unsafe { RegCloseKey(h) };
        if r == ERROR_SUCCESS { Ok(()) } else { Err(io(r)) }
    }

    /// A value's bytes, when it has the type `flags` asks for. `None`: no such key or value.
    fn get(key: &str, name: &str, flags: u32) -> std::io::Result<Option<Vec<u8>>> {
        let (key, name) = (wide(key), wide(name));
        let mut buf = vec![0u8; 1024];
        loop {
            let mut size = buf.len() as u32;
            // SAFETY: the strings are NUL-terminated; `buf` is `size` bytes long.
            let r = unsafe { RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr(), flags, std::ptr::null_mut(), buf.as_mut_ptr().cast(), &mut size) };
            match r {
                ERROR_SUCCESS => {
                    buf.truncate(size as usize);
                    return Ok(Some(buf));
                }
                ERROR_MORE_DATA if buf.len() < 64 * 1024 => buf.resize(size as usize + 2, 0),
                ERROR_FILE_NOT_FOUND => return Ok(None),
                e => return Err(io(e)),
            }
        }
    }

    /// A text value.
    pub fn get_string(key: &str, name: &str) -> std::io::Result<Option<String>> {
        Ok(get(key, name, RRF_RT_REG_SZ)?.map(|b| {
            let w: Vec<u16> = b.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).take_while(|c| *c != 0).collect();
            String::from_utf16_lossy(&w)
        }))
    }

    /// A binary value.
    pub fn get_binary(key: &str, name: &str) -> std::io::Result<Option<Vec<u8>>> {
        get(key, name, RRF_RT_REG_BINARY)
    }

    /// Removes a value; one that isn't there is fine.
    pub fn delete_value(key: &str, name: &str) -> std::io::Result<()> {
        // SAFETY: both strings are NUL-terminated.
        match unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, wide(key).as_ptr(), wide(name).as_ptr()) } {
            ERROR_SUCCESS | ERROR_FILE_NOT_FOUND => Ok(()),
            e => Err(io(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_machine_answers_sanely() {
        assert!((16..=64).contains(&tray_icon_px()));
        assert!(log_path().ends_with("uzumaki.log"));
    }

    #[test]
    fn registry_values_round_trip_on_a_scratch_key() {
        use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RegDeleteTreeW};
        let key = format!(r"Software\UzumakiTest-app-{}", std::process::id());
        assert_eq!(reg::get_string(&key, "a").unwrap(), None);
        reg::delete_value(&key, "a").unwrap();
        reg::set_string(&key, "a", r#""C:\Program Files\Uzumaki\uzumaki.exe" --hidden"#).unwrap();
        assert_eq!(reg::get_string(&key, "a").unwrap().as_deref(), Some(r#""C:\Program Files\Uzumaki\uzumaki.exe" --hidden"#));
        assert_eq!(reg::get_binary(&key, "missing").unwrap(), None);
        reg::delete_value(&key, "a").unwrap();
        assert_eq!(reg::get_string(&key, "a").unwrap(), None);
        // SAFETY: deletes the scratch key made above; the name is NUL-terminated.
        unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, wide(&key).as_ptr()) };
    }
}
