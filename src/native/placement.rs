//! Restore the Windows window and keep its maximized frame inside the work area.
use super::{WindowState, egui};
use egui_winit::winit::dpi::{PhysicalPosition, PhysicalSize};

pub(super) fn restore(cc: &eframe::CreationContext<'_>, state: Option<WindowState>) -> bool {
    let Some(window) = cc.winit_window() else {
        return false;
    };
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    if let Ok(handle) = window.window_handle()
        && let RawWindowHandle::Win32(handle) = handle.as_raw()
    {
        // SAFETY: creation runs on the window's thread. The callback stores no borrowed
        // data and removes itself when Windows destroys the window.
        unsafe {
            windows_sys::Win32::UI::Shell::SetWindowSubclass(
                handle.hwnd.get() as _,
                Some(window_proc),
                0,
                0,
            );
        }
    }
    let Some(state) = state else {
        return true;
    };
    let monitors = window.available_monitors().collect::<Vec<_>>();
    let bounds = |monitor: &egui_winit::winit::monitor::MonitorHandle| {
        let pos = monitor.position();
        let size = monitor.size();
        egui::Rect::from_min_size(
            egui::pos2(pos.x as f32, pos.y as f32),
            egui::vec2(size.width as f32, size.height as f32),
        )
    };
    let saved_pos = egui::pos2(state.x as f32, state.y as f32);
    let saved_rect = egui::Rect::from_min_size(
        saved_pos,
        egui::vec2(state.width as f32, state.height as f32),
    );
    let monitor = monitors
        .iter()
        .find(|m| bounds(m).contains(saved_pos))
        .or_else(|| {
            monitors
                .iter()
                .filter(|m| bounds(m).intersect(saved_rect).area() > 0.0)
                .max_by(|a, b| {
                    bounds(a)
                        .intersect(saved_rect)
                        .area()
                        .total_cmp(&bounds(b).intersect(saved_rect).area())
                })
        })
        .cloned()
        .or_else(|| window.primary_monitor())
        .or_else(|| monitors.first().cloned());
    if let Some(monitor) = monitor {
        let scale = monitor.scale_factor() as f32 * cc.egui_ctx.zoom_factor();
        let size = state
            .size
            .filter(|s| s.iter().all(|v| v.is_finite() && *v > 0.0))
            .map_or_else(
                || egui::vec2(state.width as f32, state.height as f32),
                |s| egui::vec2(s[0], s[1]) * scale,
            );
        let (pos, size) = fit(saved_pos, size, bounds(&monitor), scale);
        // Physical coordinates also work on monitors left of the primary display,
        // and do not depend on the DPI of the monitor used during window creation.
        window.set_outer_position(PhysicalPosition::new(pos.x as i32, pos.y as i32));
        let _ = window.request_inner_size(PhysicalSize::new(size.x as u32, size.y as u32));
    }
    // This helper cloaks winit's brief show/hide when maximizing a hidden window.
    egui_winit::set_maximized(window, state.maximized);
    true
}

// SAFETY: installed only on our window's thread; message pointers are used only for
// the corresponding Windows message and never retained beyond the call.
unsafe extern "system" fn window_proc(
    hwnd: windows_sys::Win32::Foundation::HWND,
    message: u32,
    wparam: usize,
    lparam: isize,
    subclass: usize,
    _data: usize,
) -> isize {
    use windows_sys::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
    };
    use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass};
    use windows_sys::Win32::UI::WindowsAndMessaging::{MINMAXINFO, WM_GETMINMAXINFO, WM_NCDESTROY};
    // SAFETY: forwarding the original message to the next procedure preserves winit's
    // minimum tracking size and all other window handling.
    let result = unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
    if message == WM_GETMINMAXINFO {
        let mut monitor = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        // winit confines the client area, but the acrylic backdrop covers the outer
        // frame too. Without a caption Windows can maximize that frame over the taskbar.
        // SAFETY: hwnd is live, monitor is writable, and Windows supplies MINMAXINFO
        // for this message. A failed monitor query leaves winit's defaults intact.
        unsafe {
            if GetMonitorInfoW(
                MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST),
                &mut monitor,
            ) != 0
            {
                maximize_in_work_area(&mut *(lparam as *mut MINMAXINFO), &monitor);
            }
        }
    } else if message == WM_NCDESTROY {
        // SAFETY: remove this callback using the same pair that installed it.
        unsafe { RemoveWindowSubclass(hwnd, Some(window_proc), subclass) };
    }
    result
}

fn maximize_in_work_area(
    info: &mut windows_sys::Win32::UI::WindowsAndMessaging::MINMAXINFO,
    monitor: &windows_sys::Win32::Graphics::Gdi::MONITORINFO,
) {
    use windows_sys::Win32::Foundation::POINT;
    let work = monitor.rcWork;
    info.ptMaxPosition = POINT {
        x: work.left - monitor.rcMonitor.left,
        y: work.top - monitor.rcMonitor.top,
    };
    info.ptMaxSize = POINT {
        x: work.right - work.left,
        y: work.bottom - work.top,
    };
}

fn fit(
    pos: egui::Pos2,
    size: egui::Vec2,
    monitor: egui::Rect,
    scale: f32,
) -> (egui::Pos2, egui::Vec2) {
    let size = size
        .max(egui::vec2(640.0, 420.0) * scale)
        .min(monitor.size());
    let pos = egui::pos2(
        pos.x.clamp(monitor.left(), monitor.right() - size.x),
        pos.y.clamp(monitor.top(), monitor.bottom() - size.y),
    );
    (pos, size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_maximize_message_uses_the_monitors_work_area() {
        use windows_sys::Win32::Graphics::Gdi::{
            GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
        };
        use windows_sys::Win32::UI::Shell::SetWindowSubclass;
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DestroyWindow, MINMAXINFO, SendMessageW, WM_GETMINMAXINFO,
            WS_THICKFRAME,
        };

        // SAFETY: this hidden test window is created, subclassed, messaged and destroyed
        // on this thread. The synchronous message borrows info only until it returns.
        let (installed, queried, info, monitor) = unsafe {
            let hwnd = CreateWindowExW(
                0,
                windows_sys::w!("STATIC"),
                std::ptr::null(),
                WS_THICKFRAME,
                0,
                0,
                800,
                600,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            );
            assert!(!hwnd.is_null(), "create hidden test window");
            let installed = SetWindowSubclass(hwnd, Some(window_proc), 0, 0);
            let mut info = MINMAXINFO::default();
            SendMessageW(hwnd, WM_GETMINMAXINFO, 0, (&raw mut info) as isize);
            let mut monitor = MONITORINFO {
                cbSize: size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            let queried = GetMonitorInfoW(
                MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST),
                &mut monitor,
            );
            DestroyWindow(hwnd);
            (installed, queried, info, monitor)
        };
        assert_ne!(installed, 0, "install frame callback");
        assert_ne!(queried, 0, "query test monitor");
        assert_eq!(
            info.ptMaxPosition.x,
            monitor.rcWork.left - monitor.rcMonitor.left
        );
        assert_eq!(
            info.ptMaxPosition.y,
            monitor.rcWork.top - monitor.rcMonitor.top
        );
        assert_eq!(info.ptMaxSize.x, monitor.rcWork.right - monitor.rcWork.left);
        assert_eq!(info.ptMaxSize.y, monitor.rcWork.bottom - monitor.rcWork.top);
    }

    #[test]
    fn maximized_frame_excludes_taskbars_on_every_edge() {
        use windows_sys::Win32::Foundation::{POINT, RECT};
        use windows_sys::Win32::Graphics::Gdi::MONITORINFO;
        use windows_sys::Win32::UI::WindowsAndMessaging::MINMAXINFO;

        for (x, y) in [(0, 0), (-1920, -1080), (2560, 200)] {
            let screen = RECT {
                left: x,
                top: y,
                right: x + 1920,
                bottom: y + 1080,
            };
            for (left, top, right, bottom) in [
                (0, 0, 0, 48),
                (48, 0, 0, 0),
                (0, 48, 0, 0),
                (0, 0, 48, 0),
                (0, 0, 0, 0),
            ] {
                let monitor = MONITORINFO {
                    rcMonitor: screen,
                    rcWork: RECT {
                        left: x + left,
                        top: y + top,
                        right: screen.right - right,
                        bottom: screen.bottom - bottom,
                    },
                    ..Default::default()
                };
                let mut info = MINMAXINFO {
                    ptMinTrackSize: POINT { x: 640, y: 420 },
                    ptMaxTrackSize: POINT { x: 8000, y: 4000 },
                    ..Default::default()
                };
                maximize_in_work_area(&mut info, &monitor);
                assert_eq!((info.ptMaxPosition.x, info.ptMaxPosition.y), (left, top));
                assert_eq!(
                    (info.ptMaxSize.x, info.ptMaxSize.y),
                    (1920 - left - right, 1080 - top - bottom)
                );
                assert_eq!((info.ptMinTrackSize.x, info.ptMinTrackSize.y), (640, 420));
                assert_eq!((info.ptMaxTrackSize.x, info.ptMaxTrackSize.y), (8000, 4000));
            }
        }
    }

    #[test]
    fn placement_keeps_negative_monitor_coordinates_and_fits_disconnected_displays() {
        let left = egui::Rect::from_min_size(egui::pos2(-1920.0, 0.0), egui::vec2(1920.0, 1080.0));
        let size = egui::vec2(1100.0, 720.0);
        assert_eq!(
            fit(egui::pos2(-1800.0, 100.0), size, left, 1.0).0,
            egui::pos2(-1800.0, 100.0)
        );
        let primary = left.translate(egui::vec2(1920.0, 0.0));
        assert_eq!(
            fit(egui::pos2(-1800.0, 100.0), size, primary, 1.0).0,
            egui::pos2(0.0, 100.0)
        );
        assert_eq!(
            fit(egui::pos2(1900.0, 1000.0), size * 2.0, primary, 2.0),
            (primary.min, primary.size())
        );
    }
}
