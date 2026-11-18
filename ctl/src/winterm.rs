//! Windows: the console `uzu` prints to.

use windows_sys::Win32::System::Console::{CONSOLE_SCREEN_BUFFER_INFO, ENABLE_VIRTUAL_TERMINAL_PROCESSING, GetConsoleMode, GetConsoleScreenBufferInfo, GetStdHandle, STD_OUTPUT_HANDLE, SetConsoleMode, SetConsoleOutputCP};

/// Lets the console read ANSI escapes (colours, cursor moves) and UTF-8 text. Windows Terminal
/// already does; the old console window needs to be asked. False when it refuses.
pub fn enable_ansi() -> bool {
    // SAFETY: plain calls on this process's own output handle.
    unsafe {
        let out = GetStdHandle(STD_OUTPUT_HANDLE);
        let mut mode = 0u32;
        if GetConsoleMode(out, &mut mode) == 0 {
            return false;
        }
        SetConsoleOutputCP(65001);
        mode & ENABLE_VIRTUAL_TERMINAL_PROCESSING != 0 || SetConsoleMode(out, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING) != 0
    }
}

/// The width of the console's window, in characters.
pub fn columns() -> Option<usize> {
    // SAFETY: `info` is a plain struct the call fills in.
    unsafe {
        let mut info: CONSOLE_SCREEN_BUFFER_INFO = std::mem::zeroed();
        if GetConsoleScreenBufferInfo(GetStdHandle(STD_OUTPUT_HANDLE), &mut info) == 0 {
            return None;
        }
        let width = info.srWindow.Right as i32 - info.srWindow.Left as i32 + 1;
        (width > 0).then_some(width as usize)
    }
}
