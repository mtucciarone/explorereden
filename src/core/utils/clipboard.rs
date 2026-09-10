use std::ffi::OsStr;
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND, POINT};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::{CF_HDROP, CF_UNICODETEXT};
use windows::Win32::UI::Shell::DragQueryFileW;
use windows::core::PCWSTR;

const CLIPBOARD_OPEN_ATTEMPTS: usize = 5;
const CLIPBOARD_OPEN_RETRY_DELAY: Duration = Duration::from_millis(10);

pub enum ClipboardFileRead {
    Files(Vec<PathBuf>),
    Empty,
    Unavailable,
}

/// Opens the clipboard for reading.
///
/// A NULL owner is fine here because we aren't calling EmptyClipboard or
/// SetClipboardData. The clipboard is only being inspected.
fn open_clipboard_for_read() -> bool {
    for attempt in 0..CLIPBOARD_OPEN_ATTEMPTS {
        if unsafe { OpenClipboard(None).is_ok() } {
            return true;
        }

        if attempt + 1 < CLIPBOARD_OPEN_ATTEMPTS {
            thread::sleep(CLIPBOARD_OPEN_RETRY_DELAY);
        }
    }

    false
}

/// Opens the clipboard for writing and associates it with our application window.
///
/// This must use a real HWND. Opening with NULL and then calling EmptyClipboard()
/// makes the clipboard owner NULL, which causes SetClipboardData() to fail.
fn open_clipboard_for_write(hwnd: Option<HWND>) -> bool {
    let Some(hwnd) = hwnd else {
        return false;
    };

    for attempt in 0..CLIPBOARD_OPEN_ATTEMPTS {
        if unsafe { OpenClipboard(Some(hwnd)).is_ok() } {
            return true;
        }

        if attempt + 1 < CLIPBOARD_OPEN_ATTEMPTS {
            thread::sleep(CLIPBOARD_OPEN_RETRY_DELAY);
        }
    }

    false
}

/// Clears the entire clipboard.
///
/// This requires a real HWND so that EmptyClipboard assigns clipboard ownership
/// to our application.
pub fn clear_clipboard_files(hwnd: Option<HWND>) {
    let Some(hwnd) = hwnd else {
        return;
    };

    unsafe {
        if open_clipboard_for_write(Some(hwnd)) {
            let _ = EmptyClipboard();
            let _ = CloseClipboard();
        }
    }
}

pub fn set_clipboard_files(hwnd: Option<HWND>, paths: &[PathBuf], cut: bool) -> bool {
    let Some(hwnd) = hwnd else {
        return false;
    };

    if paths.is_empty() {
        return false;
    }

    let mut wide: Vec<u16> = Vec::new();

    for path in paths {
        wide.extend(path.as_os_str().encode_wide());
        wide.push(0);
    }

    // DROPFILES requires a double-null-terminated file list.
    wide.push(0);

    #[repr(C)]
    struct DropFiles {
        p_files: u32,
        pt: POINT,
        f_nc: i32,
        f_wide: i32,
    }

    let size = size_of::<DropFiles>() + wide.len() * size_of::<u16>();

    let hglobal = match unsafe { GlobalAlloc(GMEM_MOVEABLE, size) } {
        Ok(h) => h,
        Err(_) => return false,
    };

    // Fill the DROPFILES structure.
    unsafe {
        let ptr = GlobalLock(hglobal);

        if ptr.is_null() {
            let _ = GlobalFree(Some(hglobal));
            return false;
        }

        let ptr = ptr as *mut u8;

        let drop_files = DropFiles {
            p_files: size_of::<DropFiles>() as u32,
            pt: POINT { x: 0, y: 0 },
            f_nc: 0,
            f_wide: 1,
        };

        std::ptr::copy_nonoverlapping(
            &drop_files as *const _ as *const u8,
            ptr,
            size_of::<DropFiles>(),
        );

        std::ptr::copy_nonoverlapping(
            wide.as_ptr() as *const u8,
            ptr.add(size_of::<DropFiles>()),
            wide.len() * size_of::<u16>(),
        );

        let _ = GlobalUnlock(hglobal);
    }

    let format_name: Vec<u16> = OsStr::new("Preferred DropEffect")
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    let format = unsafe { RegisterClipboardFormatW(PCWSTR(format_name.as_ptr())) };

    if format == 0 {
        unsafe {
            let _ = GlobalFree(Some(hglobal));
        }
        return false;
    }

    if !open_clipboard_for_write(Some(hwnd)) {
        unsafe {
            let _ = GlobalFree(Some(hglobal));
        }
        return false;
    }

    if unsafe { EmptyClipboard() }.is_err() {
        unsafe {
            let _ = GlobalFree(Some(hglobal));
            let _ = CloseClipboard();
        }
        return false;
    }

    // SetClipboardData takes ownership of hglobal on success.
    if unsafe { SetClipboardData(CF_HDROP.0 as u32, Some(HANDLE(hglobal.0))) }.is_err() {
        unsafe {
            let _ = GlobalFree(Some(hglobal));
            let _ = CloseClipboard();
        }
        return false;
    }

    // DROPEFFECT_MOVE = 2
    // DROPEFFECT_COPY = 1
    let effect: u32 = if cut { 2 } else { 1 };

    let hglobal_effect = match unsafe { GlobalAlloc(GMEM_MOVEABLE, size_of::<u32>()) } {
        Ok(h) => h,
        Err(_) => {
            // CF_HDROP was successfully installed. The primary operation succeeded.
            unsafe {
                let _ = CloseClipboard();
            }
            return true;
        }
    };

    let effect_written = unsafe {
        let ptr = GlobalLock(hglobal_effect);

        if ptr.is_null() {
            false
        } else {
            *(ptr as *mut u32) = effect;
            let _ = GlobalUnlock(hglobal_effect);
            true
        }
    };

    if !effect_written {
        unsafe {
            let _ = GlobalFree(Some(hglobal_effect));
            let _ = CloseClipboard();
        }

        // The file clipboard is still valid, so report success.
        return true;
    }

    // SetClipboardData takes ownership of hglobal_effect on success.
    if unsafe { SetClipboardData(format, Some(HANDLE(hglobal_effect.0))) }.is_err() {
        unsafe {
            let _ = GlobalFree(Some(hglobal_effect));
            let _ = CloseClipboard();
        }

        // CF_HDROP is already valid. Only the copy/cut indicator failed.
        return true;
    }

    unsafe {
        let _ = CloseClipboard();
    }

    true
}

pub fn read_clipboard_files() -> ClipboardFileRead {
    if !open_clipboard_for_read() {
        return ClipboardFileRead::Unavailable;
    }

    if unsafe { IsClipboardFormatAvailable(CF_HDROP.0 as u32) }.is_err() {
        unsafe {
            let _ = CloseClipboard();
        }
        return ClipboardFileRead::Empty;
    }

    let hdrop = match unsafe { GetClipboardData(CF_HDROP.0 as u32) } {
        Ok(h) if !h.0.is_null() => h,
        _ => {
            unsafe {
                let _ = CloseClipboard();
            }
            return ClipboardFileRead::Unavailable;
        }
    };

    let hdrop = windows::Win32::UI::Shell::HDROP(hdrop.0);

    // UINT_MAX requests the number of files in the HDROP.
    let count = unsafe { DragQueryFileW(hdrop, 0xFFFFFFFF, None) };

    let mut paths = Vec::with_capacity(count as usize);

    for i in 0..count {
        let len = unsafe { DragQueryFileW(hdrop, i, None) };

        if len == 0 {
            continue;
        }

        let mut buf = vec![0u16; (len + 1) as usize];

        let copied = unsafe { DragQueryFileW(hdrop, i, Some(&mut buf)) };

        if copied > 0 {
            let path = String::from_utf16_lossy(&buf[..copied as usize]);
            paths.push(PathBuf::from(path));
        }
    }

    unsafe {
        let _ = CloseClipboard();
    }

    if paths.is_empty() {
        ClipboardFileRead::Empty
    } else {
        ClipboardFileRead::Files(paths)
    }
}

pub fn is_clipboard_cut() -> bool {
    if !open_clipboard_for_read() {
        return false;
    }

    let format_name: Vec<u16> = OsStr::new("Preferred DropEffect")
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    let format = unsafe { RegisterClipboardFormatW(PCWSTR(format_name.as_ptr())) };

    if format == 0 {
        unsafe {
            let _ = CloseClipboard();
        }
        return false;
    }

    let mut is_cut = false;

    if let Ok(hglobal) = unsafe { GetClipboardData(format) } {
        if !hglobal.0.is_null() {
            let hglobal = HGLOBAL(hglobal.0 as *mut _);

            let ptr = unsafe { GlobalLock(hglobal) };

            if !ptr.is_null() {
                let value = unsafe { *(ptr as *const u32) };

                // DROPEFFECT_MOVE = 2
                is_cut = value == 2;

                let _ = unsafe { GlobalUnlock(hglobal) };
            }
        }
    }

    unsafe {
        let _ = CloseClipboard();
    }

    is_cut
}

pub fn copy_text_to_clipboard(hwnd: Option<HWND>, text: &str) -> bool {
    let Some(hwnd) = hwnd else {
        return false;
    };

    if !open_clipboard_for_write(Some(hwnd)) {
        return false;
    }

    if unsafe { EmptyClipboard() }.is_err() {
        unsafe {
            let _ = CloseClipboard();
        }
        return false;
    }

    // Convert string to null-terminated UTF-16.
    let wide_text: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();

    let text_size = wide_text.len() * size_of::<u16>();

    let hglobal = match unsafe { GlobalAlloc(GMEM_MOVEABLE, text_size) } {
        Ok(h) => h,
        Err(_) => {
            unsafe {
                let _ = CloseClipboard();
            }
            return false;
        }
    };

    unsafe {
        let ptr = GlobalLock(hglobal);

        if ptr.is_null() {
            let _ = GlobalFree(Some(hglobal));
            let _ = CloseClipboard();
            return false;
        }

        std::ptr::copy_nonoverlapping(wide_text.as_ptr(), ptr as *mut u16, wide_text.len());

        let _ = GlobalUnlock(hglobal);
    }

    // SetClipboardData takes ownership of hglobal on success.
    if unsafe { SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(hglobal.0))) }.is_err() {
        unsafe {
            let _ = GlobalFree(Some(hglobal));
            let _ = CloseClipboard();
        }
        return false;
    }

    unsafe {
        let _ = CloseClipboard();
    }

    true
}
