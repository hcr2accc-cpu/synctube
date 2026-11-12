//! A native child window inside the app window that mpv draws the video into
//! (`--wid`), so the video plays in the app itself, not in a window of its own.
//!
//! The window is disabled: mouse input over it goes to the app (hover,
//! double-click for full screen) and it never takes the keyboard focus.

use eframe::egui::Rect;

pub struct VideoHost {
    #[cfg_attr(not(windows), allow(dead_code))]
    hwnd: isize,
    /// Last placement in physical pixels (x, y, w, h, hidden bottom
    /// strip), `None` while hidden.
    placed: Option<[i32; 5]>,
}

impl VideoHost {
    /// Creates the (hidden) video area inside the app window.
    pub fn new(parent: &impl raw_window_handle::HasWindowHandle) -> Option<Self> {
        let raw = parent.window_handle().ok()?.as_raw();
        imp::create(raw).map(|hwnd| Self { hwnd, placed: None })
    }

    /// Window id for mpv's `--wid`.
    pub fn wid(&self) -> i64 {
        self.hwnd as i64
    }

    /// Shows the video over `rect` (egui points), or hides it on `None`.
    /// The bottom `cut` points are left to the app (controls drawn there).
    pub fn place(&mut self, rect: Option<Rect>, cut: f32, pixels_per_point: f32) {
        let px = rect.map(|r| {
            let x0 = (r.min.x * pixels_per_point).round() as i32;
            let y0 = (r.min.y * pixels_per_point).round() as i32;
            let x1 = (r.max.x * pixels_per_point).round() as i32;
            let y1 = (r.max.y * pixels_per_point).round() as i32;
            let (w, h) = ((x1 - x0).max(1), (y1 - y0).max(1));
            let cut = ((cut * pixels_per_point).round() as i32).clamp(0, h - 1);
            [x0, y0, w, h, cut]
        });
        if px == self.placed {
            return;
        }
        self.placed = px;
        imp::place(self.hwnd, px);
    }
}

impl Drop for VideoHost {
    fn drop(&mut self) {
        imp::destroy(self.hwnd);
    }
}

#[cfg(windows)]
mod imp {
    use raw_window_handle::RawWindowHandle;
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::Graphics::Gdi::{
        CreateRectRgn, RedrawWindow, SetWindowRgn, RDW_ALLCHILDREN, RDW_INVALIDATE,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos,
        ShowWindow, GWL_STYLE, HWND_TOP, SWP_NOACTIVATE, SWP_NOOWNERZORDER, SWP_SHOWWINDOW,
        SW_HIDE, WS_CHILD, WS_CLIPCHILDREN, WS_CLIPSIBLINGS, WS_DISABLED,
    };

    /// `SS_BLACKRECT`: the area is black until the first video frame.
    const SS_BLACKRECT: u32 = 0x4;

    pub fn create(parent: RawWindowHandle) -> Option<isize> {
        let RawWindowHandle::Win32(h) = parent else {
            return None;
        };
        let parent = h.hwnd.get() as HWND;
        let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
        // SAFETY: plain Win32 calls with a valid parent window of this thread.
        unsafe {
            // The app's own drawing must leave the video area alone.
            let style = GetWindowLongPtrW(parent, GWL_STYLE);
            SetWindowLongPtrW(parent, GWL_STYLE, style | WS_CLIPCHILDREN as isize);
            let hwnd = CreateWindowExW(
                0,
                class.as_ptr(),
                std::ptr::null(),
                WS_CHILD | WS_DISABLED | WS_CLIPSIBLINGS | WS_CLIPCHILDREN | SS_BLACKRECT,
                0,
                0,
                1,
                1,
                parent,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            );
            (!hwnd.is_null()).then_some(hwnd as isize)
        }
    }

    pub fn place(hwnd: isize, px: Option<[i32; 5]>) {
        let hwnd = hwnd as HWND;
        // SAFETY: `hwnd` is our own child window.
        unsafe {
            match px {
                Some([x, y, w, h, cut]) => {
                    // The region is owned by the window after this call.
                    let rgn = if cut > 0 {
                        CreateRectRgn(0, 0, w, h - cut)
                    } else {
                        std::ptr::null_mut()
                    };
                    SetWindowRgn(hwnd, rgn, 1);
                    // Let mpv repaint the part that was uncovered (also when paused).
                    RedrawWindow(
                        hwnd,
                        std::ptr::null(),
                        std::ptr::null_mut(),
                        RDW_INVALIDATE | RDW_ALLCHILDREN,
                    );
                    SetWindowPos(
                        hwnd,
                        HWND_TOP,
                        x,
                        y,
                        w,
                        h,
                        SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_SHOWWINDOW,
                    );
                }
                None => {
                    ShowWindow(hwnd, SW_HIDE);
                }
            }
        }
    }

    pub fn destroy(hwnd: isize) {
        // SAFETY: `hwnd` is our own child window.
        unsafe {
            DestroyWindow(hwnd as HWND);
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn create(_: raw_window_handle::RawWindowHandle) -> Option<isize> {
        None
    }
    pub fn place(_: isize, _: Option<[i32; 5]>) {}
    pub fn destroy(_: isize) {}
}
