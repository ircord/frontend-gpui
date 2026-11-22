//! A second copy of the app on its way out: a plain message box from the
//! system, and on Windows the right for the running copy to come to the
//! front (only the process the person just started may hand that on).

/// Let any process take the foreground once: called by a second copy before
/// it asks the running one to show its window. Windows only; elsewhere the
/// window manager decides.
pub fn allow_foreground() {
    #[cfg(windows)]
    {
        use windows::Win32::UI::WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow};
        // SAFETY: a plain call with a constant.
        if let Err(e) = unsafe { AllowSetForegroundWindow(ASFW_ANY) } {
            log::warn!("could not hand on the foreground: {e}");
        }
    }
}

/// Show `text` in a small message box titled `title` and wait until it is
/// closed. Windows: `MessageBoxW`. macOS: an alert through `osascript`.
/// Linux: `zenity` or `kdialog`, whichever is installed, else a
/// notification. Returns whether anything was shown.
pub fn message(title: &str, text: &str) -> bool {
    #[cfg(windows)]
    {
        use windows::Win32::UI::WindowsAndMessaging::{
            MB_ICONINFORMATION, MB_OK, MB_SETFOREGROUND, MB_TOPMOST, MessageBoxW,
        };
        use windows::core::HSTRING;
        let (text, title) = (HSTRING::from(text), HSTRING::from(title));
        // SAFETY: both strings outlive the call; no owner window.
        let shown = unsafe {
            MessageBoxW(
                None,
                &text,
                &title,
                MB_OK | MB_ICONINFORMATION | MB_SETFOREGROUND | MB_TOPMOST,
            )
        };
        shown.0 != 0
    }
    #[cfg(target_os = "macos")]
    {
        // The strings travel as arguments, so nothing needs quoting.
        run(
            "osascript",
            &[
                "-e",
                "on run argv",
                "-e",
                "display alert (item 1 of argv) message (item 2 of argv)",
                "-e",
                "end run",
                title,
                text,
            ],
        )
    }
    #[cfg(target_os = "linux")]
    {
        run(
            "zenity",
            &["--info", "--no-markup", "--title", title, "--text", text],
        ) || run("kdialog", &["--title", title, "--msgbox", text])
            || notify_rust::Notification::new()
                .summary(title)
                .body(text)
                .show()
                .is_ok()
    }
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    {
        let _ = (title, text);
        false
    }
}

/// Run a dialog program to the end; `false` when it is not installed or
/// could not show anything.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn run(program: &str, args: &[&str]) -> bool {
    match std::process::Command::new(program).args(args).status() {
        // zenity and kdialog exit 1 when closed without OK, which still showed.
        Ok(status) => status.code().is_some_and(|c| c == 0 || c == 1),
        Err(_) => false,
    }
}
