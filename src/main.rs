mod app;
mod constants;
mod page;
mod pdfview;
mod shared;
mod worker;

use std::path::PathBuf;

use anyhow::Result;
use gpui::{AppContext, Application, Bounds, WindowBounds, WindowOptions, px, size};

use crate::app::PiekApp;
use crate::constants::{PREFERRED_WINDOW, SCREEN_MARGIN};

fn main() -> Result<()> {
    // `piek [file.pdf]`: with no argument the app starts on the empty screen.
    let initial_pdf: Option<PathBuf> = std::env::args_os().nth(1).map(PathBuf::from);

    Application::new().run(move |app| {
        let (screen_w, screen_h) = app
            .primary_display()
            .map(|d| {
                let s = d.bounds().size;
                (f32::from(s.width), f32::from(s.height))
            })
            .unwrap_or((1920.0, 1080.0));

        let win_w = PREFERRED_WINDOW.0.min(screen_w * SCREEN_MARGIN);
        let win_h = PREFERRED_WINDOW.1.min(screen_h * SCREEN_MARGIN);
        let bounds = Bounds::centered(None, size(px(win_w), px(win_h)), app);

        let window_options = WindowOptions {
            kind: gpui::WindowKind::Normal,
            window_min_size: Some(size(px(400.0), px(300.0))),
            titlebar: Some(gpui::TitlebarOptions {
                title: Some("Piek".into()),
                appears_transparent: false,
                traffic_light_position: None,
            }),
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            ..Default::default()
        };

        app.open_window(window_options, |_win, cx| {
            cx.new(|cx| PiekApp::new(initial_pdf, cx))
        })
        .unwrap();

        app.activate(true);
    });
    Ok(())
}
