//! The mouse cursor's position, asked from the OS. Needed when a file is
//! dropped on the window: during the drag the source app holds the mouse,
//! so the window gets no cursor events, and winit 0.30 doesn't report where
//! the drop happened.

use glam::Vec2;
use winit::window::Window;

/// The cursor in window pixels (relative to the client area), if the OS
/// can tell.
pub fn position_in(window: &Window) -> Option<Vec2> {
    let screen = screen_position()?;
    let origin = window.inner_position().ok()?;
    Some(screen - Vec2::new(origin.x as f32, origin.y as f32))
}

#[cfg(windows)]
fn screen_position() -> Option<Vec2> {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;
    let mut point = POINT { x: 0, y: 0 };
    // SAFETY: GetCursorPos only writes to the POINT it is given.
    let ok = unsafe { GetCursorPos(&mut point) } != 0;
    ok.then(|| Vec2::new(point.x as f32, point.y as f32))
}

#[cfg(not(windows))]
fn screen_position() -> Option<Vec2> {
    None
}
