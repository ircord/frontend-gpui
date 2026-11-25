//! The rich-text representation alongside the native plain clipboard.

use std::ops::Range;

pub struct Html {
    pub source: String,
    /// UTF-8 byte range of the copied selection when the desktop supplies
    /// its enclosing HTML for context.
    pub selection: Option<Range<usize>>,
}

#[cfg(target_os = "macos")]
pub fn html() -> Option<Html> {
    use objc2_app_kit::{NSPasteboard, NSPasteboardTypeHTML};

    // The pasteboard retains this constant for the lifetime of the process.
    let kind = unsafe { NSPasteboardTypeHTML };
    let source = NSPasteboard::generalPasteboard()
        .stringForType(kind)?
        .to_string();
    Some(Html {
        source,
        selection: None,
    })
}

#[cfg(windows)]
pub fn html() -> Option<Html> {
    use windows::Win32::Foundation::HGLOBAL;
    use windows::Win32::System::DataExchange::{
        CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
        RegisterClipboardFormatW,
    };
    use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
    use windows::core::w;

    struct Open;
    impl Drop for Open {
        fn drop(&mut self) {
            // Each successful open on this thread is closed on every path.
            let _ = unsafe { CloseClipboard() };
        }
    }

    // CF_HTML is a registered clipboard format backed by an HGLOBAL that
    // Windows owns. Only copy the data while the clipboard and memory are locked.
    unsafe {
        let format = RegisterClipboardFormatW(w!("HTML Format"));
        if format == 0 || IsClipboardFormatAvailable(format).is_err() {
            return None;
        }
        OpenClipboard(None).ok()?;
        let _open = Open;
        let memory = HGLOBAL(GetClipboardData(format).ok()?.0);
        let size = GlobalSize(memory);
        if size == 0 || size > 2 * 1024 * 1024 {
            return None;
        }
        let data = GlobalLock(memory);
        if data.is_null() {
            return None;
        }
        let bytes = std::slice::from_raw_parts(data.cast::<u8>(), size);
        let end = bytes.iter().position(|byte| *byte == 0).unwrap_or(size);
        let result = std::str::from_utf8(&bytes[..end])
            .ok()
            .and_then(windows_html);
        let _ = GlobalUnlock(memory);
        result
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn html() -> Option<Html> {
    None
}

#[cfg(any(windows, test))]
fn windows_html(data: &str) -> Option<Html> {
    let header = data.split_once('<').map_or(data, |(header, _)| header);
    let offset = |name| {
        header.split(['\r', '\n']).find_map(|line| {
            let (label, offset) = line.split_once(':')?;
            (label == name)
                .then(|| offset.trim().parse::<usize>().ok())
                .flatten()
        })
    };
    let selection = offset("StartFragment")?..offset("EndFragment")?;
    let fragment = data.get(selection.clone())?;
    if let (Some(start), Some(end)) = (offset("StartHTML"), offset("EndHTML"))
        && start <= selection.start
        && selection.end <= end
        && let Some(context) = data.get(start..end)
    {
        return Some(Html {
            source: context.to_owned(),
            selection: Some(selection.start - start..selection.end - start),
        });
    }
    Some(Html {
        source: fragment.to_owned(),
        selection: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cf_html_offsets_are_bytes_and_preserve_the_context() {
        let prefix = "<ol start='7'><li>préface</li>";
        let selected = "<li>sélectionné</li>";
        let html = format!("{prefix}{selected}</ol>");
        let header = |start: usize| {
            let from = start + prefix.len();
            format!(
                "Version:1.0\rStartHTML:{start:010}\rEndHTML:{:010}\rStartFragment:{from:010}\rEndFragment:{:010}\r",
                start + html.len(),
                from + selected.len()
            )
        };
        let copied = format!("{}{html}", header(header(0).len()));
        let parsed = windows_html(&copied).unwrap();
        assert_eq!(parsed.source, html);
        assert_eq!(&parsed.source[parsed.selection.unwrap()], selected);
    }

    #[test]
    fn cf_html_rejects_an_invalid_fragment_range() {
        assert!(
            windows_html("Version:1.0\r\nStartFragment:96\r\nEndFragment:5\r\n<li>Invalid</li>")
                .is_none()
        );
    }
}
