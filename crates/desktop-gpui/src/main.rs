//! The GPUI front-end for the Oxide desktop app.
//!
//! The Electron app in `crates/desktop` is untouched: it keeps its own window,
//! its own release train and its own harness. This is a second window over the
//! same core — the same `config.json`, `auth.json`, `settings.json`, project
//! registry, `trust.json`, sessions and shadow snapshots — painted with Zed's
//! GPUI instead of a webview. It links `oxide-core` for the run and
//! `oxide-desktop`'s library for the projects, the sessions and the `@path`
//! completion, so the two windows cannot disagree about what a project or a turn
//! is.

mod editor;
mod runner;
mod state;
mod theme;
mod transcript;
mod update;
mod view;

use gpui::{
    px, size, App, AppContext as _, Application, Bounds, TitlebarOptions, WindowBounds,
    WindowOptions,
};
use oxide_desktop::manager::DesktopManager;
use runner::Runner;
use view::Root;

fn main() {
    // A turn streams on a task of its own and an approval is answered later
    // still, so the runtime lives for the window's whole life and is never
    // dropped while a turn is in flight.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("a tokio runtime");
    let handle = runtime.handle().clone();
    std::mem::forget(runtime);

    Application::new().run(move |cx: &mut App| {
        let (sender, events) = tokio::sync::mpsc::unbounded_channel();
        let runner = Runner::new(handle.clone(), sender);
        let manager = DesktopManager::load_lossy();

        let bounds = Bounds::centered(None, size(px(1120.), px(780.)), cx);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(TitlebarOptions {
                        title: Some("Oxide".into()),
                        ..Default::default()
                    }),
                    window_min_size: Some(size(px(680.), px(420.))),
                    ..Default::default()
                },
                move |window, cx| cx.new(|cx| Root::new(runner, events, manager, window, cx)),
            )
            .expect("the window");
        let _ = window;
        cx.activate(true);
    });
}
