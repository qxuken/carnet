//! Where the OS pointer is: while files are dragged over the window, and
//! while the system hides it for typing.
//!
//! On Windows a file drag is the OS's own loop: winit hears it enter and
//! drop, at a stale point, and never hears it move — so the page asks the
//! OS directly, every frame the drag lasts, to draw its drop line under
//! the pointer. macOS reports the drag moving (kui-native forwards it);
//! there and in tests this says nothing.

use kui_native::Vec2;

/// The pointer in this window's logical coordinates, when it is over a
/// window of this process; `None` otherwise.
#[cfg(all(windows, not(test)))]
pub fn over_window(scale: f32) -> Option<Vec2> {
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Point {
        x: i32,
        y: i32,
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetCursorPos(p: *mut Point) -> i32;
        fn WindowFromPoint(p: Point) -> isize;
        fn GetAncestor(h: isize, flags: u32) -> isize;
        fn GetWindowThreadProcessId(h: isize, pid: *mut u32) -> u32;
        fn ScreenToClient(h: isize, p: *mut Point) -> i32;
    }
    const GA_ROOT: u32 = 2;
    // SAFETY: plain Win32 calls on values owned here; a null window or a
    // failed call returns before anything is read from it.
    unsafe {
        let mut p = Point { x: 0, y: 0 };
        if GetCursorPos(&mut p) == 0 {
            return None;
        }
        let under = WindowFromPoint(p);
        if under == 0 {
            return None;
        }
        let top = GetAncestor(under, GA_ROOT);
        let mut pid = 0u32;
        GetWindowThreadProcessId(top, &mut pid);
        if pid != std::process::id() {
            return None;
        }
        if ScreenToClient(top, &mut p) == 0 {
            return None;
        }
        let s = if scale > 0.0 { scale } else { 1.0 };
        Some(Vec2::new(p.x as f32 / s, p.y as f32 / s))
    }
}

#[cfg(not(all(windows, not(test))))]
pub fn over_window(_scale: f32) -> Option<Vec2> {
    None
}

/// The pointer anywhere on screen, in the OS's own units: what tells a
/// hand that went to the mouse while the system hid the pointer for the
/// typing (macOS does, and the window then hears no move). Only ever
/// compared with itself. `None` where the OS is not asked (and in tests).
#[cfg(all(target_os = "macos", not(test)))]
pub fn on_screen() -> Option<Vec2> {
    use std::ffi::c_void;
    #[repr(C)]
    struct CGPoint {
        x: f64,
        y: f64,
    }
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGEventCreate(source: *const c_void) -> *mut c_void;
        fn CGEventGetLocation(event: *mut c_void) -> CGPoint;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRelease(cf: *const c_void);
    }
    // SAFETY: an event made here with no source, read, and released; a
    // null one returns before it is touched.
    unsafe {
        let e = CGEventCreate(std::ptr::null());
        if e.is_null() {
            return None;
        }
        let p = CGEventGetLocation(e);
        CFRelease(e);
        Some(Vec2::new(p.x as f32, p.y as f32))
    }
}

#[cfg(all(windows, not(test)))]
pub fn on_screen() -> Option<Vec2> {
    #[repr(C)]
    struct Point {
        x: i32,
        y: i32,
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetCursorPos(p: *mut Point) -> i32;
    }
    let mut p = Point { x: 0, y: 0 };
    // SAFETY: one Win32 call writing into a point owned here.
    (unsafe { GetCursorPos(&mut p) } != 0).then(|| Vec2::new(p.x as f32, p.y as f32))
}

#[cfg(any(test, not(any(target_os = "macos", windows))))]
pub fn on_screen() -> Option<Vec2> {
    None
}
