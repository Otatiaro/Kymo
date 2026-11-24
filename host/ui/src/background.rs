//! The window's background on Windows. Its class has none: until Vulkan's or
//! OpenGL's first image reaches the screen, the compositor shows the window
//! white. DX12 presents to the compositor directly and shows the image.

use eframe::egui::Color32;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows_sys::Win32::Graphics::Gdi::CreateSolidBrush;
use windows_sys::Win32::UI::WindowsAndMessaging::{GCLP_HBRBACKGROUND, SetClassLongPtrW};

/// Gives the window's class a background of `color`, before the window is
/// shown.
pub fn set(window: &impl HasWindowHandle, color: Color32) {
    let Ok(handle) = window.window_handle() else {
        return;
    };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return;
    };
    let rgb = u32::from(color.r()) | (u32::from(color.g()) << 8) | (u32::from(color.b()) << 16);
    // SAFETY: the handle is the live window eframe made; the brush stays the
    // class's for the life of the process.
    unsafe {
        let brush = CreateSolidBrush(rgb);
        if !brush.is_null() {
            SetClassLongPtrW(handle.hwnd.get() as _, GCLP_HBRBACKGROUND, brush as isize);
        }
    }
}
