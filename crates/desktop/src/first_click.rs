//! macOS spends the first click on a window that is not key by activating the
//! app, and only hands that press to the view under the pointer when the view
//! declares `acceptsFirstMouse`. Wry does declare it, and the desktop's window
//! config asks for it, so the press is still eaten before the page sees it: a
//! click aimed at the composer, a sidebar row or an attachment thumbnail has to
//! be made twice whenever another app had focus. A window that is already key
//! never takes that path, so the press is spent making it key before AppKit
//! dispatches it — the monitor below runs ahead of the dispatch, and the click
//! then lands on what the pointer was aimed at.

use std::ptr::NonNull;

use block2::RcBlock;
use objc2::runtime::NSObjectProtocol;
use objc2::{sel, MainThreadMarker};
use objc2_app_kit::{NSApplication, NSEvent, NSEventMask, NSWindow};

/// Make the window under the pointer key before a press reaches it, so the press
/// is delivered rather than consumed by activating the app.
pub fn install() {
    // The block's return type is the binding's own — `Fn(NonNull<NSEvent>) ->
    // *mut NSEvent`, whose doc reads "block's return must be a valid pointer or
    // null": a pointer keeps the event, and null would swallow it. This monitor
    // hands every press back.
    let handler = RcBlock::new(|event: NonNull<NSEvent>| -> *mut NSEvent {
        // SAFETY: AppKit hands the block a live event for the duration of the
        // call, and the pointer is only read while it is valid.
        let press = unsafe { event.as_ref() };
        if let Some(mtm) = MainThreadMarker::new() {
            if let Some(window) = press.window(mtm) {
                if !window.isKeyWindow() {
                    take_key(&window, mtm);
                }
            }
        }
        event.as_ptr()
    });
    // SAFETY: the mask names two mouse-down types, and both the block and the
    // monitor AppKit hands back are kept for the rest of the process.
    let monitor = unsafe {
        NSEvent::addLocalMonitorForEventsMatchingMask_handler(
            NSEventMask::LeftMouseDown | NSEventMask::RightMouseDown,
            &handler,
        )
    };
    // Neither handle may be dropped: the monitor reads the block on every press,
    // and the monitor itself is only removed when the object AppKit returned is
    // released — so a local, dropped when this function returns, would leave the
    // first click taken again with nothing to show for it.
    std::mem::forget(handler);
    std::mem::forget(monitor);
}

/// Activate the app if it is not already active, then make `window` key.
fn take_key(window: &NSWindow, mtm: MainThreadMarker) {
    let app = NSApplication::sharedApplication(mtm);
    if !app.isActive() {
        if app.respondsToSelector(sel!(activate)) {
            app.activate();
        } else {
            #[allow(deprecated)]
            app.activateIgnoringOtherApps(true);
        }
    }
    window.makeKeyAndOrderFront(None);
}
