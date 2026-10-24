//! Windows touches for Graft's own window frame: the icon in every size from
//! the exe's icon resource, rounded corners, and a border matching the theme.

use egui::Color32;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

/// Apply to the main window once it exists.
pub fn style_window(window: &impl HasWindowHandle) {
    let Ok(handle) = window.window_handle() else {
        return;
    };
    #[cfg(windows)]
    if let RawWindowHandle::Win32(h) = handle.as_raw() {
        win::style(h.hwnd.get() as _);
    }
    #[cfg(not(windows))]
    let _ = handle;
}

/// Color the window border, e.g. after a theme switch.
pub fn set_border(color: Color32) {
    #[cfg(windows)]
    win::set_border(color);
    #[cfg(not(windows))]
    let _ = color;
}

#[cfg(windows)]
mod win {
    use std::sync::atomic::{AtomicPtr, Ordering};

    use egui::Color32;
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::Graphics::Dwm::{
        DWMWA_BORDER_COLOR, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute,
    };
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        ICON_BIG, ICON_SMALL, IMAGE_ICON, LR_DEFAULTCOLOR, LoadImageW, SM_CXICON, SM_CXSMICON,
        SendMessageW, WM_SETICON,
    };

    /// `winresource` embeds `graft.ico` under this id (see build.rs).
    const ICON_ID: usize = 1;

    /// The main window, once styled.
    static WINDOW: AtomicPtr<core::ffi::c_void> = AtomicPtr::new(std::ptr::null_mut());

    pub fn style(hwnd: HWND) {
        WINDOW.store(hwnd, Ordering::Relaxed);
        // SAFETY: plain Win32 calls on our own, live window. Failures (e.g.
        // no icon resource, or Windows 10 without these DWM attributes) only
        // leave the defaults in place.
        unsafe {
            let dpi = GetDpiForWindow(hwnd).max(96);
            let module = GetModuleHandleW(std::ptr::null());
            // Windows picks the best-matching image from the .ico for each
            // size, instead of shrinking one big bitmap (blurry title bar,
            // thin taskbar icon).
            for (kind, metric) in [(ICON_SMALL, SM_CXSMICON), (ICON_BIG, SM_CXICON)] {
                let size = GetSystemMetricsForDpi(metric, dpi);
                let icon = LoadImageW(
                    module,
                    ICON_ID as *const u16,
                    IMAGE_ICON,
                    size,
                    size,
                    LR_DEFAULTCOLOR,
                );
                if !icon.is_null() {
                    SendMessageW(hwnd, WM_SETICON, kind as usize, icon as isize);
                }
            }
            let corners = DWMWCP_ROUND;
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE as u32,
                (&raw const corners).cast(),
                size_of_val(&corners) as u32,
            );
        }
        set_border(crate::theme::color::border());
    }

    pub fn set_border(color: Color32) {
        let hwnd = WINDOW.load(Ordering::Relaxed);
        if hwnd.is_null() {
            return;
        }
        // A COLORREF is 0x00BBGGRR.
        let border = u32::from_le_bytes([color.r(), color.g(), color.b(), 0]);
        // SAFETY: as in `style`; the window lives as long as the app.
        unsafe {
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_BORDER_COLOR as u32,
                (&raw const border).cast(),
                size_of_val(&border) as u32,
            );
        }
    }
}

/// Wait (up to 10 seconds) for the process `pid` to end.
pub fn wait_for_exit(pid: u32) {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
        };
        // SAFETY: plain Win32 calls; the handle is checked and closed.
        unsafe {
            let handle = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
            if !handle.is_null() {
                WaitForSingleObject(handle, 10_000);
                CloseHandle(handle);
            }
        }
    }
    #[cfg(not(windows))]
    let _ = pid;
}
