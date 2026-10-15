//! Windows host for the Browser mode webview.
//!
//! GPUI renders through a DirectComposition target created as *topmost*, so a
//! WebView2 embedded as a child window is drawn underneath GPUI's content and
//! never visible. Instead the webview lives in a borderless popup window owned
//! by the app window (owned popups stay above their owner, minimise with it and
//! are not covered by its composition layer), positioned over the response area.

use std::num::NonZeroIsize;

use raw_window_handle::{
    HandleError, HasWindowHandle, RawWindowHandle, Win32WindowHandle, WindowHandle,
};
use windows_sys::Win32::Foundation::{HWND, POINT};
use windows_sys::Win32::Graphics::Gdi::ClientToScreen;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, SW_HIDE, SWP_NOACTIVATE, SWP_SHOWWINDOW, SetWindowPos,
    ShowWindow, WS_CLIPCHILDREN, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP,
};

/// HWND of a GPUI window, if it is a Win32 window.
pub fn owner_hwnd(window: &impl HasWindowHandle) -> Option<HWND> {
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get() as HWND),
        _ => None,
    }
}

pub struct PopupHost {
    hwnd: HWND,
    owner: HWND,
}

impl PopupHost {
    pub fn new(owner: HWND) -> Option<Self> {
        let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                class.as_ptr(),
                std::ptr::null(),
                WS_POPUP | WS_CLIPCHILDREN,
                0,
                0,
                1,
                1,
                owner,
                std::ptr::null_mut(),
                GetModuleHandleW(std::ptr::null()),
                std::ptr::null(),
            )
        };
        (!hwnd.is_null()).then_some(Self { hwnd, owner })
    }

    /// Places the popup over the owner's client-area rectangle (physical px)
    /// and shows it.
    pub fn place(&self, x: i32, y: i32, width: i32, height: i32) {
        let mut origin = POINT { x, y };
        unsafe {
            ClientToScreen(self.owner, &mut origin);
            SetWindowPos(
                self.hwnd,
                std::ptr::null_mut(),
                origin.x,
                origin.y,
                width.max(1),
                height.max(1),
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
        }
    }

    pub fn hide(&self) {
        unsafe {
            ShowWindow(self.hwnd, SW_HIDE);
        }
    }

    /// Gives keyboard focus back to the app window (the popup or the page
    /// may have it after a click into the page).
    pub fn focus_owner(&self) {
        unsafe {
            SetFocus(self.owner);
        }
    }
}

impl Drop for PopupHost {
    fn drop(&mut self) {
        unsafe {
            DestroyWindow(self.hwnd);
        }
    }
}

impl HasWindowHandle for PopupHost {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let hwnd = NonZeroIsize::new(self.hwnd as isize).ok_or(HandleError::Unavailable)?;
        let raw = RawWindowHandle::Win32(Win32WindowHandle::new(hwnd));
        // SAFETY: the popup outlives every borrow of this handle (it is owned
        // by the BrowserPane together with the webview built on it).
        Ok(unsafe { WindowHandle::borrow_raw(raw) })
    }
}
