mod constants;

mod page;
mod pdfview;
mod shared;

use crate::constants::{PDF_PATH, PDFIUM_DLL, PREFERRED_WINDOW, RENDER_WIDTH_PX, SCREEN_MARGIN};
use crate::pdfview::PdfView;
use crate::shared::Shared;
use crate::page::{RenderedPage};

use std::collections::VecDeque;
use std::env;
use std::sync::{Arc, Condvar, Mutex, mpsc};

use anyhow::{Context, Result};
use gpui::{AppContext, Application, Bounds, WindowBounds, WindowOptions, px, size};
use pdfium_render::prelude::{PdfRenderConfig, Pdfium};

fn worker_main(
    path: &str,
    shared: &Shared,
    init_tx: &mpsc::Sender<Result<Vec<(f32, f32)>>>,
    results: &async_channel::Sender<RenderedPage>,
) -> Result<()> {
    let bindings =
        Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path(PDFIUM_DLL))
            .or_else(|_| Pdfium::bind_to_system_library())
            .context("could not find the pdfium shared library")?;
    let pdfium = Pdfium::new(bindings);

    let document = pdfium
        .load_pdf_from_file(path, None)
        .with_context(|| format!("failed to open {path}"))?;
    let pages = document.pages();

    // Cheap up-front pass: page sizes in points, so the UI can lay out the
    // whole document before any page is rendered.
    let sizes: Vec<(f32, f32)> = pages
        .iter()
        .map(|p| (p.width().value, p.height().value))
        .collect();
    let _ = init_tx.send(Ok(sizes));

    let config = PdfRenderConfig::new().set_target_width(RENDER_WIDTH_PX);

    loop {
        let index = shared.next_job();

        let pixels = (|| -> Result<(u32, u32, Vec<u8>)> {
            let page_index = u16::try_from(index).context("page index too large")?;
            let page = pages.get(page_index as i32)?;
            let bitmap = page.render_with_config(&config)?;
            let (w, h) = (bitmap.width() as u32, bitmap.height() as u32);
            // pdfium renders BGRA, which is what GPUI's RenderImage expects.
            let bytes: Vec<u8> = bitmap.as_raw_bytes().into();
            Ok((w, h, bytes))
        })()
        .map_err(|e| eprintln!("failed to render page {}: {e:#}", index + 1))
        .ok();

        // The UI side is gone (window closed): stop the thread.
        if results
            .send_blocking(RenderedPage {
                page: index,
                pixels,
            })
            .is_err()
        {
            return Ok(());
        }
    }
}

/// Spawns the worker and blocks until it reports the page sizes.
fn start_worker(
    path: &str,
) -> Result<(
    Vec<(f32, f32)>,
    Arc<Shared>,
    async_channel::Receiver<RenderedPage>,
)> {
    let shared = Arc::new(Shared {
        queue: Mutex::new(VecDeque::new()),
        wake: Condvar::new(),
    });
    let (result_tx, result_rx) = async_channel::unbounded();
    let (init_tx, init_rx) = mpsc::channel();

    let worker_shared = shared.clone();
    let path = path.to_owned();
    std::thread::Builder::new()
        .name("pdfium-worker".into())
        .spawn(move || {
            if let Err(err) = worker_main(&path, &worker_shared, &init_tx, &result_tx) {
                let _ = init_tx.send(Err(err));
            }
        })?;

    let sizes = init_rx
        .recv()
        .context("pdf worker exited before loading the document")??;
    Ok((sizes, shared, result_rx))
}

fn parse_env() -> String {
    let envs: Vec<String> = env::args().collect();
    let pdf_path: String;
    if envs.len() < 2 {
        pdf_path = PDF_PATH.to_string();
        eprintln!("NO pdf path provided, using the default one: {}", pdf_path);
    } else {
        pdf_path = envs[1].clone();
    }
    pdf_path
}

fn main() -> Result<()> {
    let pdf_path = parse_env();
    let (sizes, shared, results) = start_worker(&pdf_path)?;

    Application::new().run(move |app| {
        println!("HI from piek");
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
        let window_bounds = Some(WindowBounds::Windowed(bounds));

        let titlebar = Some(gpui::TitlebarOptions {
            title: Some("Piek".into()),
            appears_transparent: false,
            traffic_light_position: None,
        });

        let window_options = WindowOptions {
            kind: gpui::WindowKind::Normal,
            window_min_size: Some(size(px(400.0), px(300.0))),
            titlebar,
            window_bounds,
            ..Default::default()
        };

        app.open_window(window_options, |_win, cx| {
            cx.new(|cx| PdfView::new(sizes, shared, results, cx))
        })
        .unwrap();
    });
    Ok(())
}
