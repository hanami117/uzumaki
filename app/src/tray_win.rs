//! The notification-area icon on Windows: `Shell_NotifyIconW` on a message-only window that owns
//! its thread and message loop (as Rune's tray did). A left click shows the window; a right click
//! opens a native popup menu (`TrackPopupMenu`) with `tray_menu.rs`'s items, the macOS menu bar
//! menu's layout: Shuffle and AutoPlay checked (`MF_CHECKED`), Repeat a submenu of radio items
//! (`MFT_RADIOCHECK`), the Volume submenu, greyed items where the window has no control. The
//! menu is built fresh each time it opens: the tray first asks the app for its current state
//! (`sync_tray_modes`, waited for at most `FRESH_WAIT`), so the marks and the volume are current.
//! The tooltip is the song; the icon comes back when Explorer restarts (`TaskbarCreated`) and
//! follows the App icon setting, drawn at the notification area's size for the system's scale.
//! Windows sends a tray icon no mouse wheel, so there is no scroll-for-volume here.

use std::cell::Cell;
use std::ptr;
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use windows_sys::Win32::Foundation::{GetLastError, ERROR_CLASS_ALREADY_EXISTS, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Shell::{Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW, NOTIFY_ICON_MESSAGE};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIcon, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyIcon, DestroyMenu, DestroyWindow, DispatchMessageW, GetCursorPos,
    GetMessageW, GetWindowLongPtrW, PostMessageW, PostQuitMessage, RegisterClassW, RegisterWindowMessageW, SetForegroundWindow, SetMenuItemInfoW,
    SetWindowLongPtrW, TrackPopupMenu, TranslateMessage, CREATESTRUCTW, GWLP_USERDATA, HICON, HMENU, HWND_MESSAGE, MENUITEMINFOW, MFS_CHECKED, MFS_DISABLED,
    MFT_RADIOCHECK, MF_CHECKED, MF_GRAYED, MF_POPUP, MF_SEPARATOR, MF_STRING, MIIM_FTYPE, MIIM_STATE, MSG, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON,
    WM_APP, WM_CLOSE, WM_DESTROY, WM_LBUTTONUP, WM_NCCREATE, WM_NCDESTROY, WM_NULL, WM_RBUTTONUP, WNDCLASSW,
};

use super::icon::icon_pixels;
use super::menu::{self, entries, numbered, Numbered, Snapshot};
use crate::models::Song;
use crate::settings::IconVariant;
use crate::state::output_volume::OutputVolume;
use crate::state::tray_modes::Modes;
use crate::state::{Handle, State};

const CALLBACK: u32 = WM_APP + 1;
/// The tooltip or the icon changed.
const UPDATE: u32 = WM_APP + 2;
const ICON_ID: u32 = 1;
/// How long a right click waits for the app's current state before showing the menu anyway.
const FRESH_WAIT: Duration = Duration::from_millis(250);

/// What the UI thread hands over; read on the tray's thread.
struct Shared {
    snap: Snapshot,
    variant: IconVariant,
    /// The icon must be drawn again (another App icon).
    icon_stale: bool,
}

struct Context {
    shared: Arc<Mutex<Shared>>,
    handle: Handle,
    icon: Cell<HICON>,
    /// The size `icon` was drawn at.
    icon_px: Cell<u32>,
    taskbar_created: u32,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// Menu text treats `&` as the access-key mark: a song's own ampersands are doubled.
fn menu_text(s: &str) -> Vec<u16> {
    wide(&s.replace('&', "&&"))
}

/// A 32-bit BGRA icon of the app icon at `px` a side; the AND mask is the inverted alpha, which
/// Windows ignores beside alpha.
fn create_icon(variant: IconVariant, px: u32) -> HICON {
    let mut bgra = icon_pixels(variant, px);
    let mut mask = Vec::with_capacity(bgra.len() / 4);
    for p in bgra.as_chunks_mut::<4>().0 {
        p.swap(0, 2);
        mask.push(p[3].wrapping_sub(u8::MAX));
    }
    // SAFETY: both buffers hold px × px pixels and outlive the call, which copies them.
    unsafe { CreateIcon(ptr::null_mut(), px as i32, px as i32, 1, 32, mask.as_ptr(), bgra.as_ptr()) }
}

fn data(hwnd: HWND, flags: u32) -> NOTIFYICONDATAW {
    // SAFETY: NOTIFYICONDATAW is plain data; zero is its documented empty value.
    let mut data: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    data.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    data.hWnd = hwnd;
    data.uID = ICON_ID;
    data.uFlags = flags;
    data
}

fn set_tip(data: &mut NOTIFYICONDATAW, tooltip: &str) {
    let tip: Vec<u16> = tooltip.encode_utf16().take(data.szTip.len() - 1).collect();
    data.szTip[..tip.len()].copy_from_slice(&tip);
    data.szTip[tip.len()] = 0;
}

/// Draws the icon again when the App icon or the notification area's size changed.
fn refresh_icon(context: &Context) {
    let (variant, stale) = {
        let mut s = context.shared.lock().unwrap();
        (s.variant, std::mem::take(&mut s.icon_stale))
    };
    let px = crate::win::tray_icon_px();
    if !stale && px == context.icon_px.get() && !context.icon.get().is_null() {
        return;
    }
    let old = context.icon.replace(create_icon(variant, px));
    context.icon_px.set(px);
    if !old.is_null() {
        // SAFETY: the old icon is ours; the shell copies the image on NIM_ADD/NIM_MODIFY, and
        // the new one is handed over before the old one is used again.
        unsafe { DestroyIcon(old) };
    }
}

fn notify(hwnd: HWND, context: &Context, message: NOTIFY_ICON_MESSAGE) -> bool {
    refresh_icon(context);
    let mut data = data(hwnd, NIF_MESSAGE | NIF_ICON | NIF_TIP);
    data.uCallbackMessage = CALLBACK;
    data.hIcon = context.icon.get();
    set_tip(&mut data, context.shared.lock().unwrap().snap.tooltip());
    // SAFETY: `data` is fully initialised for this window's icon.
    unsafe { Shell_NotifyIconW(message, &data) != 0 }
}

/// Asks the app for its current modes and volume, and waits (briefly) until they are in.
fn fresh(handle: &Handle) {
    let (tx, rx) = mpsc::sync_channel(1);
    handle.update_if(move |s: &mut State, _| {
        s.sync_tray_modes();
        let _ = tx.send(());
        false // nothing to redraw in the window
    });
    let _ = rx.recv_timeout(FRESH_WAIT);
}

/// Appends `items` to `menu` (submenus too; `DestroyMenu` on the top one frees them all).
///
/// # Safety
/// `menu` is a menu made on this thread.
unsafe fn append(menu: HMENU, items: &[Numbered]) {
    for item in items {
        // SAFETY: the menus are this thread's; the strings outlive each call, which copies them.
        unsafe {
            match item {
                Numbered::Item { id, label, checked, radio, enabled } => {
                    let mut flags = MF_STRING;
                    if !enabled {
                        flags |= MF_GRAYED;
                    }
                    if *checked && !radio {
                        flags |= MF_CHECKED;
                    }
                    AppendMenuW(menu, flags, *id, menu_text(label).as_ptr());
                    if *radio {
                        // A dot, not a tick, for the marked one of a group.
                        let mut info: MENUITEMINFOW = std::mem::zeroed();
                        info.cbSize = size_of::<MENUITEMINFOW>() as u32;
                        info.fMask = MIIM_FTYPE | MIIM_STATE;
                        info.fType = MFT_RADIOCHECK;
                        info.fState = if *checked { MFS_CHECKED } else { 0 } | if *enabled { 0 } else { MFS_DISABLED };
                        SetMenuItemInfoW(menu, *id as u32, 0, &info);
                    }
                }
                Numbered::Popup { label, enabled, items } => {
                    let sub = CreatePopupMenu();
                    if sub.is_null() {
                        continue;
                    }
                    append(sub, items);
                    let flags = MF_POPUP | if *enabled { 0 } else { MF_GRAYED };
                    AppendMenuW(menu, flags, sub as usize, menu_text(label).as_ptr());
                }
                Numbered::Separator => {
                    AppendMenuW(menu, MF_SEPARATOR, 0, ptr::null());
                }
            }
        }
    }
}

/// Shows the menu at the cursor and carries out the picked item.
fn pick(hwnd: HWND, context: &Context) {
    fresh(&context.handle);
    let snap = context.shared.lock().unwrap().snap.clone();
    let (items, cmds) = numbered(&entries(&snap));
    // SAFETY: the menu is made, shown and destroyed on this window's thread.
    let picked = unsafe {
        let popup = CreatePopupMenu();
        if popup.is_null() {
            return;
        }
        append(popup, &items);
        let mut at = POINT { x: 0, y: 0 };
        GetCursorPos(&mut at);
        // Without the foreground, the menu doesn't close when the user clicks elsewhere.
        SetForegroundWindow(hwnd);
        let picked = TrackPopupMenu(popup, TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON, at.x, at.y, 0, hwnd, ptr::null());
        PostMessageW(hwnd, WM_NULL, 0, 0);
        DestroyMenu(popup);
        picked
    };
    if let Some(&cmd) = (picked as usize).checked_sub(1).and_then(|i| cmds.get(i)) {
        menu::run(&context.handle, cmd);
    }
}

unsafe extern "system" fn procedure(hwnd: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    // SAFETY: Windows owns the message parameters. The context outlives the window and is only
    // used on its thread; it is cleared before destruction completes.
    unsafe {
        if message == WM_NCCREATE {
            let create = &*(l as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
        }
        let Some(context) = (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Context).as_ref() else {
            return DefWindowProcW(hwnd, message, w, l);
        };
        match message {
            CALLBACK => {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match l as u32 {
                    WM_LBUTTONUP => super::show_window(&context.handle),
                    WM_RBUTTONUP => pick(hwnd, context),
                    _ => {}
                }));
                0
            }
            UPDATE => {
                notify(hwnd, context, NIM_MODIFY);
                0
            }
            WM_DESTROY => {
                Shell_NotifyIconW(NIM_DELETE, &data(hwnd, 0));
                PostQuitMessage(0);
                0
            }
            WM_NCDESTROY => {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                DefWindowProcW(hwnd, message, w, l)
            }
            // Explorer restarted: the icon again (drawn anew if the notification area's size changed).
            m if m == context.taskbar_created => {
                notify(hwnd, context, NIM_ADD);
                0
            }
            _ => DefWindowProcW(hwnd, message, w, l),
        }
    }
}

/// Creates the window and icon on this thread; the context lives as long as the window.
fn create(context: &mut Context) -> Result<HWND, String> {
    let class = wide("UzumakiTray");
    // SAFETY: the class name is NUL-terminated; the context is stable until the loop ends.
    unsafe {
        let instance = GetModuleHandleW(ptr::null());
        let wc = WNDCLASSW { lpfnWndProc: Some(procedure), hInstance: instance, lpszClassName: class.as_ptr(), ..std::mem::zeroed() };
        if RegisterClassW(&wc) == 0 && GetLastError() != ERROR_CLASS_ALREADY_EXISTS {
            return Err(format!("RegisterClassW failed: Win32 error {}", GetLastError()));
        }
        let hwnd = CreateWindowExW(0, class.as_ptr(), ptr::null(), 0, 0, 0, 0, 0, HWND_MESSAGE, ptr::null_mut(), instance, (context as *mut Context).cast());
        if hwnd.is_null() {
            return Err(format!("CreateWindowExW failed: Win32 error {}", GetLastError()));
        }
        if !notify(hwnd, context, NIM_ADD) {
            let e = GetLastError();
            DestroyWindow(hwnd);
            return Err(format!("Shell_NotifyIconW failed: Win32 error {e}"));
        }
        Ok(hwnd)
    }
}

pub struct TrayCtl {
    /// The tray's window (an HWND, which isn't `Send`).
    window: isize,
    shared: Arc<Mutex<Shared>>,
    worker: Option<JoinHandle<()>>,
    tooltip: String,
}

impl TrayCtl {
    /// None when the notification-area icon can't be added (or in headless runs).
    pub fn start(handle: Handle, app_icon: IconVariant) -> Option<Self> {
        if rune::is_headless() {
            return None;
        }
        let shared = Arc::new(Mutex::new(Shared { snap: Snapshot::default(), variant: app_icon, icon_stale: true }));
        let theirs = shared.clone();
        let (ready, result) = mpsc::sync_channel(1);
        let worker = std::thread::Builder::new().name("uzumaki-tray".into()).spawn(move || {
            // SAFETY: the name is NUL-terminated.
            let taskbar_created = unsafe { RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()) };
            let mut context = Box::new(Context { shared: theirs, handle, icon: Cell::new(ptr::null_mut()), icon_px: Cell::new(0), taskbar_created });
            match create(&mut context) {
                Ok(hwnd) => {
                    let _ = ready.send(Ok(hwnd as isize));
                }
                Err(e) => {
                    let _ = ready.send(Err(e));
                    // SAFETY: the icon (if any) is ours and nothing else holds it.
                    unsafe {
                        if !context.icon.get().is_null() {
                            DestroyIcon(context.icon.get());
                        }
                    }
                    return;
                }
            }
            // SAFETY: a plain message loop for this thread's window; it ends with WM_QUIT.
            unsafe {
                let mut msg: MSG = std::mem::zeroed();
                while GetMessageW(&mut msg, ptr::null_mut(), 0, 0) > 0 {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                if !context.icon.get().is_null() {
                    DestroyIcon(context.icon.get());
                }
            }
            drop(context);
        });
        let worker = match worker {
            Ok(w) => w,
            Err(e) => {
                eprintln!("uzumaki: no tray icon: {e}");
                return None;
            }
        };
        match result.recv() {
            Ok(Ok(window)) => Some(Self { window, shared, worker: Some(worker), tooltip: "Uzumaki".into() }),
            Ok(Err(e)) => {
                let _ = worker.join();
                eprintln!("uzumaki: no tray icon: {e}");
                None
            }
            Err(_) => {
                let _ = worker.join();
                eprintln!("uzumaki: no tray icon: the tray thread ended");
                None
            }
        }
    }

    fn post_update(&self) {
        // SAFETY: posting to a window owned by the tray thread; a gone window drops the message.
        unsafe { PostMessageW(self.window as HWND, UPDATE, 0, 0) };
    }

    /// Draws the icon of the chosen app icon.
    pub fn set_app_icon(&mut self, icon: IconVariant) {
        let changed = {
            let mut s = self.shared.lock().unwrap();
            let changed = s.variant != icon;
            if changed {
                s.variant = icon;
                s.icon_stale = true;
            }
            changed
        };
        if changed {
            self.post_update();
        }
    }

    /// Relabels Play/Pause (in the next menu).
    pub fn set_playing(&mut self, playing: bool) {
        self.shared.lock().unwrap().snap.playing = playing;
    }

    /// The tooltip and the menu's first line: "Title — Artist"; the modes are greyed with no song.
    pub fn set_song(&mut self, song: Option<&Song>) {
        let line = super::song_line(song);
        let tip = {
            let mut s = self.shared.lock().unwrap();
            s.snap.loaded = song.is_some();
            s.snap.song = line;
            s.snap.tooltip().to_string()
        };
        if tip != self.tooltip {
            self.tooltip = tip;
            self.post_update();
        }
    }

    /// Checkmarks of Shuffle, Repeat and AutoPlay (in the next menu).
    pub fn set_modes(&mut self, modes: &Modes) {
        self.shared.lock().unwrap().snap.modes = Some(*modes);
    }

    /// The Volume submenu (in the next menu).
    pub fn set_output(&mut self, output: &OutputVolume) {
        self.shared.lock().unwrap().snap.output = Some(output.clone());
    }
}

impl Drop for TrayCtl {
    fn drop(&mut self) {
        // SAFETY: WM_CLOSE destroys the window on its own thread, which then quits its loop.
        unsafe { PostMessageW(self.window as HWND, WM_CLOSE, 0, 0) };
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::output_volume::Output;
    use crate::state::Repeat;
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetMenuItemCount, GetMenuItemInfoW, MFT_SEPARATOR, MIIM_ID, MIIM_STRING, MIIM_SUBMENU};

    /// One item read back from a real Win32 menu.
    #[derive(Debug)]
    struct Read {
        id: u32,
        text: String,
        radio: bool,
        separator: bool,
        checked: bool,
        greyed: bool,
        sub: Vec<Read>,
    }

    fn read(menu: HMENU) -> Vec<Read> {
        // SAFETY: `menu` is a live menu of this thread; the buffer outlives each call.
        unsafe {
            (0..GetMenuItemCount(menu) as u32)
                .map(|i| {
                    let mut buf = [0u16; 256];
                    let mut info: MENUITEMINFOW = std::mem::zeroed();
                    info.cbSize = size_of::<MENUITEMINFOW>() as u32;
                    info.fMask = MIIM_FTYPE | MIIM_STATE | MIIM_ID | MIIM_STRING | MIIM_SUBMENU;
                    info.dwTypeData = buf.as_mut_ptr();
                    info.cch = buf.len() as u32;
                    assert!(GetMenuItemInfoW(menu, i, 1, &mut info) != 0);
                    let text = String::from_utf16_lossy(&buf[..info.cch as usize]);
                    let sub = if info.hSubMenu.is_null() { Vec::new() } else { read(info.hSubMenu) };
                    Read {
                        id: info.wID,
                        text,
                        radio: info.fType & MFT_RADIOCHECK != 0,
                        separator: info.fType & MFT_SEPARATOR != 0,
                        checked: info.fState & MFS_CHECKED != 0,
                        greyed: info.fState & MFS_DISABLED != 0,
                        sub,
                    }
                })
                .collect()
        }
    }

    fn find<'a>(items: &'a [Read], text: &str) -> &'a Read {
        items.iter().find(|r| r.text.starts_with(text)).unwrap_or_else(|| panic!("no {text} in {items:?}"))
    }

    #[test]
    fn the_popup_menu_has_checkmarks_radio_items_and_greyed_items() {
        let snap = Snapshot {
            playing: false,
            song: "Simon & Garfunkel — The Boxer".into(),
            loaded: true,
            modes: Some(Modes { shuffle: true, repeat: Repeat::One, autoplay: false, autoplay_offered: false }),
            output: Some(OutputVolume { output: Output::ThisComputer, label: "This Computer".into(), volume: Some(0.8) }),
        };
        let (items, cmds) = numbered(&entries(&snap));
        // SAFETY: made and destroyed on this thread.
        let got = unsafe {
            let popup = CreatePopupMenu();
            append(popup, &items);
            let got = read(popup);
            DestroyMenu(popup);
            got
        };
        assert_eq!(got[0].text, "Simon && Garfunkel — The Boxer", "& is not an access key");
        assert!(got[0].greyed && got[1].separator);
        let play = find(&got, "Play");
        assert_eq!(cmds[play.id as usize - 1], menu::Cmd::PlayPause);
        let shuffle = find(&got, "Shuffle");
        assert!(shuffle.checked && !shuffle.radio && !shuffle.greyed);
        let autoplay = find(&got, "AutoPlay");
        assert!(!autoplay.checked && autoplay.greyed, "no ∞ button here: greyed");
        let repeat = find(&got, "Repeat");
        let marks: Vec<(&str, bool, bool)> = repeat.sub.iter().map(|r| (r.text.as_str(), r.radio, r.checked)).collect();
        assert_eq!(marks, [("Off", true, false), ("All", true, false), ("One", true, true)]);
        assert_eq!(cmds[repeat.sub[2].id as usize - 1], menu::Cmd::Repeat(Repeat::One));
        let volume = find(&got, "Volume: 80%");
        assert_eq!(volume.sub[0].text, "This Computer");
        assert!(volume.sub[0].greyed);
        let steps: Vec<(&str, bool)> = volume.sub.iter().filter(|r| r.radio).map(|r| (r.text.as_str(), r.checked)).collect();
        assert_eq!(steps, [("0%", false), ("25%", false), ("50%", false), ("75%", true), ("100%", false)]);
        assert_eq!(cmds[find(&volume.sub, "Volume Up").id as usize - 1], menu::Cmd::VolumeSteps(1));
        assert_eq!(cmds[find(&got, "Quit").id as usize - 1], menu::Cmd::Quit);
    }

    #[test]
    fn the_icon_is_made_at_the_notification_areas_size() {
        for px in [16, 24, 32] {
            let icon = create_icon(IconVariant::Coral, px);
            assert!(!icon.is_null(), "{px}");
            // SAFETY: ours.
            unsafe { DestroyIcon(icon) };
        }
    }
}
