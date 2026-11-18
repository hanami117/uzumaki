//! Windows: start at login through the current user's Run key. See `login.rs`.
//!
//! `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, value `Uzumaki` = `"<uzumaki.exe>"` or
//! `"<uzumaki.exe>" --hidden`. Windows keeps its own switch for every entry in
//! `…\Explorer\StartupApproved\Run` (Settings → Apps → Startup, Task Manager → Startup apps);
//! both are read, and turning the switch on here clears a "disabled" left there. No admin
//! rights are needed, and the installer's uninstaller removes the value.

use super::Login;
use crate::win::reg;

const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const APPROVED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
/// The value's name (what Windows' Startup list shows until it reads the program's own name).
const NAME: &str = "Uzumaki";

/// The Run value: the program in quotes (a path may have spaces), `--hidden` for a background
/// start. A Windows path can't contain a quote, so there is nothing to escape.
pub fn run_command(exe: &str, hidden: bool) -> String {
    format!("\"{}\"{}", exe.trim_matches('"'), if hidden { " --hidden" } else { "" })
}

/// Whether a Run value starts the app in the background.
fn is_hidden(command: &str) -> bool {
    command.rsplit('"').next().is_some_and(|args| args.split_whitespace().any(|a| a == "--hidden"))
}

/// Windows' own startup switch (Settings → Apps → Startup, Task Manager) for an entry, from its
/// `StartupApproved\Run` value: no value, or a first byte that is even (02, 06), is on; an odd
/// one (03, 07) is off.
pub fn startup_approved(value: Option<&[u8]>) -> bool {
    value.and_then(|v| v.first()).is_none_or(|b| b % 2 == 0)
}

pub fn read() -> Login {
    read_at(RUN, APPROVED, NAME)
}

pub fn set(on: bool, hidden: bool) -> Result<Login, String> {
    let exe = crate::win::exe_path().map_err(|e| format!("can't find Uzumaki's own path: {e}"))?;
    set_at(RUN, APPROVED, NAME, on.then(|| run_command(&exe.to_string_lossy(), hidden)))?;
    Ok(read())
}

/// An entry left pointing at a copy that is gone (the folder was moved, a zip unpacked somewhere
/// else) is pointed at this one, keeping its `--hidden`.
pub fn repair() {
    let Ok(Some(command)) = reg::get_string(RUN, NAME) else { return };
    let program = command.trim().trim_start_matches('"').split('"').next().unwrap_or_default().to_string();
    if program.is_empty() || std::path::Path::new(&program).exists() {
        return;
    }
    if let Ok(exe) = crate::win::exe_path() {
        let _ = reg::set_string(RUN, NAME, &run_command(&exe.to_string_lossy(), is_hidden(&command)));
    }
}

fn read_at(run: &str, approved: &str, name: &str) -> Login {
    let command = reg::get_string(run, name).ok().flatten().filter(|v| !v.trim().is_empty());
    let allowed = startup_approved(reg::get_binary(approved, name).ok().flatten().as_deref());
    let on = command.is_some() && allowed;
    Login { on, hidden: command.filter(|_| on).map(|c| is_hidden(&c)), ..Login::default() }
}

/// `Some(command)`: writes the entry and clears Windows' "disabled" for it. `None`: removes both.
fn set_at(run: &str, approved: &str, name: &str, command: Option<String>) -> Result<(), String> {
    let failed = |e: std::io::Error| format!("the registry refused: {e}");
    match command {
        Some(c) => reg::set_string(run, name, &c).map_err(failed)?,
        None => reg::delete_value(run, name).map_err(failed)?,
    }
    // Best effort: without this value the entry counts as on.
    let _ = reg::delete_value(approved, name);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_run_value_quotes_the_program_and_says_hidden() {
        assert_eq!(run_command(r"C:\Users\A B\Uzumaki\uzumaki.exe", false), r#""C:\Users\A B\Uzumaki\uzumaki.exe""#);
        assert_eq!(run_command(r"C:\Users\A B\Uzumaki\uzumaki.exe", true), r#""C:\Users\A B\Uzumaki\uzumaki.exe" --hidden"#);
        assert!(is_hidden(r#""C:\x\uzumaki.exe" --hidden"#));
        assert!(!is_hidden(r#""C:\x --hidden\uzumaki.exe""#), "a folder's name is not an argument");
        assert!(!is_hidden(r#""C:\x\uzumaki.exe""#));
        assert!(startup_approved(None) && startup_approved(Some(&[2, 0, 0])) && startup_approved(Some(&[6])));
        assert!(!startup_approved(Some(&[3, 0, 0])) && !startup_approved(Some(&[7])));
    }

    /// The real calls on a scratch key of this test (never the user's Run key).
    #[test]
    fn the_entry_is_written_read_and_removed() {
        use windows_sys::Win32::System::Registry::{HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_BINARY, RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegSetValueExW};
        let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
        let root = format!(r"Software\UzumakiTest-login-{}", std::process::id());
        let (run, approved) = (format!(r"{root}\Run"), format!(r"{root}\StartupApproved\Run"));
        assert!(!read_at(&run, &approved, NAME).on);
        let cmd = run_command(r"C:\Users\A B\Uzumaki\uzumaki.exe", true);
        set_at(&run, &approved, NAME, Some(cmd.clone())).unwrap();
        assert_eq!(read_at(&run, &approved, NAME), Login { on: true, hidden: Some(true), ..Login::default() });
        assert_eq!(reg::get_string(&run, NAME).unwrap(), Some(cmd.clone()));
        // The user switches it off in Task Manager: off here too, the entry itself stays.
        let mut h: HKEY = std::ptr::null_mut();
        // SAFETY: plain registry calls on the scratch key; strings are NUL-terminated.
        unsafe {
            assert_eq!(RegCreateKeyExW(HKEY_CURRENT_USER, wide(&approved).as_ptr(), 0, std::ptr::null(), 0, KEY_SET_VALUE, std::ptr::null(), &mut h, std::ptr::null_mut()), 0);
            let off = [3u8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
            assert_eq!(RegSetValueExW(h, wide(NAME).as_ptr(), 0, REG_BINARY, off.as_ptr(), off.len() as u32), 0);
            RegCloseKey(h);
        }
        assert!(!read_at(&run, &approved, NAME).on);
        assert_eq!(reg::get_string(&run, NAME).unwrap(), Some(cmd));
        // Turned on again in Uzumaki: on in Windows too, without `--hidden` this time.
        set_at(&run, &approved, NAME, Some(run_command(r"C:\Users\A B\Uzumaki\uzumaki.exe", false))).unwrap();
        assert_eq!(read_at(&run, &approved, NAME), Login { on: true, hidden: Some(false), ..Login::default() });
        // Off: gone, and off twice is fine.
        set_at(&run, &approved, NAME, None).unwrap();
        set_at(&run, &approved, NAME, None).unwrap();
        assert!(!read_at(&run, &approved, NAME).on);
        assert_eq!(reg::get_string(&run, NAME).unwrap(), None);
        // SAFETY: deletes the scratch key made above.
        unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, wide(&root).as_ptr()) };
    }
}
