//! Linux start at login: an XDG autostart entry (freedesktop "Desktop Application Autostart"),
//! `$XDG_CONFIG_HOME/autostart/io.github.hanami117.uzumaki.desktop` (else `~/.config/autostart`).
//! GNOME, KDE Plasma (through systemd's xdg-autostart-generator), Xfce, Cinnamon, MATE, LXQt and
//! most Wayland compositors' session tools start what is listed there.
//!
//! The entry runs this copy: the `.AppImage` file (`$APPIMAGE`) for an AppImage, else the
//! `uzumaki` on `PATH` that is this binary (the AUR's `/usr/bin/uzumaki`, install.sh's
//! `~/.local/bin/uzumaki`), else this binary's own path. `--hidden` starts it with no window.
//!
//! Only std: the module also compiles (and its tests run) on the Mac.

use std::path::{Path, PathBuf};

use super::Login;

/// The entry's file name: the app id, like the menu entry.
pub const FILE_NAME: &str = "io.github.hanami117.uzumaki.desktop";

/// A key in every entry Uzumaki writes, so `repair` only touches its own.
const MARK: &str = "X-Uzumaki-Autostart=true";

/// `$XDG_CONFIG_HOME/autostart/<FILE_NAME>` (an absolute `XDG_CONFIG_HOME` only, as the spec
/// says), else `~/.config/autostart/<FILE_NAME>`.
pub fn entry_path() -> Option<PathBuf> {
    entry_path_in(std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from), std::env::var_os("HOME").map(PathBuf::from))
}

fn entry_path_in(xdg_config: Option<PathBuf>, home: Option<PathBuf>) -> Option<PathBuf> {
    let base = xdg_config.filter(|p| p.is_absolute()).or_else(|| home.filter(|h| h.is_absolute()).map(|h| h.join(".config")))?;
    Some(base.join("autostart").join(FILE_NAME))
}

/// What the autostart entry runs for this copy (see the module doc).
pub fn program() -> Option<PathBuf> {
    let appimage = crate::bridge::running_from_appimage().then(|| std::env::var_os("APPIMAGE").map(PathBuf::from)).flatten();
    let exe = std::env::current_exe().ok()?;
    let exe = exe.canonicalize().unwrap_or(exe);
    let on_path = std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .filter(|d| d.is_absolute())
            .map(|d| d.join("uzumaki"))
            .find(|c| c.canonicalize().is_ok_and(|c| c == exe))
    });
    Some(pick_program(appimage, exe, on_path))
}

/// The AppImage file wins (this binary sits in a temporary mount that is gone after the run),
/// then the name on `PATH` that leads to this binary (stays right across package updates, and
/// reads better in the desktop's autostart list), then the binary itself.
fn pick_program(appimage: Option<PathBuf>, exe: PathBuf, on_path: Option<PathBuf>) -> PathBuf {
    appimage.filter(|p| p.is_absolute()).or(on_path).unwrap_or(exe)
}

/// The entry's text.
pub fn entry(program: &Path, hidden: bool) -> Result<String, String> {
    let program = program.to_str().ok_or("the program's path isn't valid UTF-8")?;
    if program.contains(['\n', '\r']) {
        return Err("the program's path has a line break".into());
    }
    let mut exec = exec_arg(program);
    if hidden {
        exec.push_str(" --hidden");
    }
    Ok(format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Uzumaki\n\
         Comment=Starts Uzumaki when you log in (Uzumaki: Settings, General)\n\
         Exec={exec}\n\
         Icon=io.github.hanami117.uzumaki\n\
         Terminal=false\n\
         StartupNotify=false\n\
         X-GNOME-Autostart-enabled=true\n\
         X-KDE-autostart-phase=2\n\
         {MARK}\n"
    ))
}

/// One `Exec` argument: quoted when it holds a reserved character (desktop entry spec,
/// "The Exec key"), with `"`, `` ` ``, `$` and `\` escaped inside the quotes; then `%` doubled
/// (field codes) and every `\` doubled again (the string escape a reader undoes first).
fn exec_arg(arg: &str) -> String {
    const RESERVED: &[char] = &[' ', '\t', '"', '\'', '\\', '>', '<', '~', '|', '&', ';', '$', '*', '?', '#', '(', ')', '`', '='];
    let quoted = if arg.is_empty() || arg.contains(RESERVED) {
        let mut q = String::from("\"");
        for c in arg.chars() {
            if matches!(c, '"' | '`' | '$' | '\\') {
                q.push('\\');
            }
            q.push(c);
        }
        q.push('"');
        q
    } else {
        arg.to_string()
    };
    quoted.replace('%', "%%").replace('\\', "\\\\")
}

/// What an autostart entry says, as far as the switches care.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Parsed {
    /// Not turned off (`Hidden=true` deletes an entry; `X-GNOME-Autostart-enabled=false` turns it off).
    pub enabled: bool,
    /// `--hidden` is among the `Exec` arguments.
    pub hidden: bool,
    /// `Exec`'s program.
    pub program: Option<PathBuf>,
    /// Written by Uzumaki.
    pub ours: bool,
}

/// Reads the keys of the `[Desktop Entry]` group.
pub fn parse(text: &str) -> Parsed {
    let mut p = Parsed { enabled: true, hidden: false, program: None, ours: false };
    let mut in_entry = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_entry || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        match (key.trim(), value.trim()) {
            ("Hidden", "true") => p.enabled = false,
            ("X-GNOME-Autostart-enabled", "false") => p.enabled = false,
            ("X-Uzumaki-Autostart", "true") => p.ours = true,
            ("Exec", v) => {
                let args = exec_args(v);
                p.hidden = args.iter().skip(1).any(|a| a == "--hidden");
                p.program = args.into_iter().next().map(PathBuf::from);
            }
            _ => {}
        }
    }
    p
}

/// Splits an `Exec` value into arguments (the reverse of `exec_arg`): the string escapes
/// first, then field codes (`%%` is a `%`, the others are dropped), then quoting.
fn exec_args(value: &str) -> Vec<String> {
    let mut unescaped = String::new();
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('s') => unescaped.push(' '),
                Some('n') => unescaped.push('\n'),
                Some('t') => unescaped.push('\t'),
                Some('r') => unescaped.push('\r'),
                Some(o) => unescaped.push(o),
                None => unescaped.push('\\'),
            }
        } else {
            unescaped.push(c);
        }
    }
    // Field codes, as GLib does it, on the whole line: `%%` is a `%`, the others go.
    let mut plain = String::new();
    let mut chars = unescaped.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            plain.push(c);
        } else if chars.next() == Some('%') {
            plain.push('%');
        }
    }
    let mut args = Vec::new();
    let mut cur: Option<String> = None;
    let mut chars = plain.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                let arg = cur.get_or_insert_with(String::new);
                while let Some(q) = chars.next() {
                    match q {
                        '"' => break,
                        '\\' => arg.extend(chars.next()),
                        _ => arg.push(q),
                    }
                }
            }
            ' ' | '\t' => args.extend(cur.take()),
            _ => cur.get_or_insert_with(String::new).push(c),
        }
    }
    args.extend(cur);
    args
}

/// The entry's state on disk.
pub fn read() -> Login {
    let Some(path) = entry_path() else { return Login::unavailable("No home folder to keep the autostart entry in.") };
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            let p = parse(&text);
            Login { on: p.enabled, hidden: p.enabled.then_some(p.hidden), ..Login::default() }
        }
        Err(_) => Login::default(),
    }
}

/// Writes the entry (`on`, running this copy, `--hidden` when `hidden`) or removes it.
pub fn set(on: bool, hidden: bool) -> Result<Login, String> {
    let path = entry_path().ok_or("no home folder")?;
    if on {
        let program = program().ok_or("can't tell where this copy of Uzumaki is")?;
        write(&path, &entry(&program, hidden)?)?;
    } else {
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("can't remove {}: {e}", path.display())),
        }
    }
    Ok(read())
}

/// Atomic write: a temporary file beside it, then a rename.
fn write(path: &Path, text: &str) -> Result<(), String> {
    let dir = path.parent().ok_or("no autostart folder")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("can't create {}: {e}", dir.display()))?;
    let tmp = path.with_extension("desktop.tmp");
    std::fs::write(&tmp, text).and_then(|()| std::fs::rename(&tmp, path)).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("can't write {}: {e}", path.display())
    })
}

/// At start: Uzumaki's own entry whose program is gone (an AppImage replaced by a newer file
/// with another name, a moved install) is pointed at this copy, keeping `--hidden`.
pub fn repair() {
    let Some(path) = entry_path() else { return };
    let Ok(text) = std::fs::read_to_string(&path) else { return };
    let p = parse(&text);
    let gone = p.program.as_deref().is_none_or(|prog| prog.is_absolute() && !prog.exists());
    if !(p.ours && p.enabled && gone) {
        return;
    }
    let Some(program) = program() else { return };
    match entry(&program, p.hidden).and_then(|t| write(&path, &t)) {
        Ok(()) => eprintln!("uzumaki: start at login now runs {}", program.display()),
        Err(e) => eprintln!("uzumaki: couldn't update the autostart entry: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_entry_runs_the_program_and_hides_on_request() {
        let text = entry(Path::new("/usr/bin/uzumaki"), true).unwrap();
        assert_eq!(
            text,
            "[Desktop Entry]\nType=Application\nName=Uzumaki\nComment=Starts Uzumaki when you log in (Uzumaki: Settings, General)\n\
             Exec=/usr/bin/uzumaki --hidden\nIcon=io.github.hanami117.uzumaki\nTerminal=false\nStartupNotify=false\n\
             X-GNOME-Autostart-enabled=true\nX-KDE-autostart-phase=2\nX-Uzumaki-Autostart=true\n"
        );
        let shown = entry(Path::new("/usr/bin/uzumaki"), false).unwrap();
        assert!(shown.contains("\nExec=/usr/bin/uzumaki\n"), "{shown}");
        for t in [&text, &shown] {
            let p = parse(t);
            assert!(p.enabled && p.ours);
            assert_eq!(p.program.as_deref(), Some(Path::new("/usr/bin/uzumaki")));
        }
        assert!(parse(&text).hidden);
        assert!(!parse(&shown).hidden);
    }

    #[test]
    fn awkward_paths_are_quoted_and_read_back() {
        for path in [
            "/home/u/Apps/Uzumaki 0.2.1-x86_64.AppImage",
            "/home/u/My \"Apps\"/Uzumaki.AppImage",
            "/home/u/$HOME/`x`/back\\slash/100%/Uzumaki.AppImage",
            "/home/u/été/a=b;c/Uzumaki.AppImage",
        ] {
            let text = entry(Path::new(path), true).unwrap();
            let p = parse(&text);
            assert_eq!(p.program.as_deref(), Some(Path::new(path)), "{text}");
            assert!(p.hidden, "{text}");
        }
        assert_eq!(exec_arg("/home/u/Apps/Uzumaki 0.2.1.AppImage"), "\"/home/u/Apps/Uzumaki 0.2.1.AppImage\"");
        // `$` escaped inside the quotes, then that backslash doubled for the string escape.
        assert_eq!(exec_arg("/a/$b"), "\"/a/\\\\$b\"");
        assert_eq!(exec_arg("/a/100%"), "/a/100%%");
        assert!(entry(Path::new("/a/b\nc"), false).is_err());
    }

    #[test]
    fn an_entry_turned_off_by_the_desktop_reads_as_off() {
        let on = entry(Path::new("/usr/bin/uzumaki"), false).unwrap();
        assert!(!parse(&format!("{on}Hidden=true\n")).enabled);
        assert!(!parse(&on.replace("X-GNOME-Autostart-enabled=true", "X-GNOME-Autostart-enabled=false")).enabled);
        // Keys of other groups and comments don't count; another program's entry isn't ours.
        let other = "# Hidden=true\n[Desktop Entry]\nExec=uzumaki --hidden %U\n[Desktop Action x]\nHidden=true\n";
        let p = parse(other);
        assert!(p.enabled && p.hidden && !p.ours);
        assert_eq!(p.program.as_deref(), Some(Path::new("uzumaki")));
    }

    #[test]
    fn the_appimage_file_wins_then_the_name_on_path_then_the_binary() {
        let exe = PathBuf::from("/usr/lib/uzumaki/uzumaki");
        let image = PathBuf::from("/home/u/Apps/Uzumaki-x86_64.AppImage");
        let on_path = PathBuf::from("/usr/bin/uzumaki");
        assert_eq!(pick_program(Some(image.clone()), exe.clone(), Some(on_path.clone())), image);
        assert_eq!(pick_program(None, exe.clone(), Some(on_path.clone())), on_path);
        assert_eq!(pick_program(None, exe.clone(), None), exe);
        assert_eq!(pick_program(Some(PathBuf::from("relative.AppImage")), exe.clone(), None), exe);
    }

    #[test]
    fn the_entry_lives_in_the_xdg_config_autostart_folder() {
        let home = Some(PathBuf::from("/home/u"));
        assert_eq!(entry_path_in(None, home.clone()), Some(PathBuf::from("/home/u/.config/autostart/io.github.hanami117.uzumaki.desktop")));
        assert_eq!(entry_path_in(Some("/x/cfg".into()), home.clone()), Some(PathBuf::from("/x/cfg/autostart/io.github.hanami117.uzumaki.desktop")));
        assert_eq!(entry_path_in(Some("rel".into()), home), Some(PathBuf::from("/home/u/.config/autostart/io.github.hanami117.uzumaki.desktop")));
        assert_eq!(entry_path_in(None, None), None);
    }

    #[test]
    fn set_writes_and_removes_the_entry_in_a_scratch_folder() {
        let dir = std::env::temp_dir().join(format!("uzumaki-autostart-test-{}", std::process::id()));
        let path = dir.join("autostart").join(FILE_NAME);
        write(&path, &entry(Path::new("/opt/Uzumaki App/uzumaki"), true).unwrap()).unwrap();
        let p = parse(&std::fs::read_to_string(&path).unwrap());
        assert_eq!(p.program.as_deref(), Some(Path::new("/opt/Uzumaki App/uzumaki")));
        assert!(p.hidden && p.ours && p.enabled);
        assert!(!path.with_extension("desktop.tmp").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
