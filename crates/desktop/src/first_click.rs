//! macOS spends the first click on a window that is not key by activating the
//! app, and only hands that press to the view under the pointer when the view
//! declares `acceptsFirstMouse`. That part is wired up: the window config's
//! `acceptFirstMouse` becomes `WebviewAttributes::accept_first_mouse`, and wry's
//! web view answers `acceptsFirstMouse:` with it. What an activation still
//! decides is the moment the press arrives — AppKit will not dispatch a press to
//! a window it is in the middle of making key — so the monitor below runs ahead
//! of that dispatch and makes the window under the pointer key first, which
//! leaves it key by the time the press is delivered.
//!
//! Every control in the page answers the gesture for itself (`pressActivated` in
//! `ui/app.js`): a first press can still arrive as the one that takes focus, with
//! no click behind it, and a press that starts a drag loses its click too.

use std::ptr::NonNull;

use block2::RcBlock;
use objc2::runtime::NSObjectProtocol;
use objc2::{sel, MainThreadMarker};
use objc2_app_kit::{NSApplication, NSEvent, NSEventMask, NSWindow};

/// Make the window under the pointer key — activating the app if it is not
/// active — before a press reaches it, so the press is delivered rather than
/// consumed by activating the app.
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
                // Only an activation can change whether this press is delivered:
                // the view already accepts a first mouse, so a press into a
                // window of the active app lands as it stands, and taking the key
                // away from whatever holds it mid-press is what would drop it.
                if !window.isKeyWindow() && !NSApplication::sharedApplication(mtm).isActive() {
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
        // The press being answered is still in flight, so the activation has to
        // have happened by the time AppKit decides whether to hand that press to
        // the view: `activate` may be deferred to the next pass of the event
        // loop, and the call it replaced took effect at once.
        #[allow(deprecated)]
        app.activateIgnoringOtherApps(true);
        if app.respondsToSelector(sel!(activate)) {
            app.activate();
        }
    }
    window.makeKeyAndOrderFront(None);
}
