use std::sync::{Arc, mpsc};

use anyhow::{Context, Result};
use pdfium_render::prelude::{PdfPages, PdfRenderConfig, Pdfium};

use crate::constants::{PDFIUM_DLL, RENDER_WIDTH_PX};
use crate::page::RenderedPage;
use crate::shared::Shared;

pub type PageSizes = Vec<(f32, f32)>;

pub struct OpenedDocument {
    pub sizes: PageSizes,
    pub shared: Arc<Shared>,
    pub results: async_channel::Receiver<RenderedPage>,
}

/// Spawns the worker and blocks until it reports the page sizes.
/// Safe to call from a background thread.
pub fn start_worker(path: &str) -> Result<OpenedDocument> {
    let shared = Arc::new(Shared::new());
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

    // The init channel carries a Result because loading can fail (missing
    // pdfium library, bad file, ...) and the caller needs to know why.
    let sizes = init_rx
        .recv()
        .context("pdf worker exited before loading the document")??;
    Ok(OpenedDocument {
        sizes,
        shared,
        results: result_rx,
    })
}

fn worker_main(
    path: &str,
    shared: &Shared,
    init_tx: &mpsc::Sender<Result<PageSizes>>,
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

    let sizes: PageSizes = pages
        .iter()
        .map(|p| (p.width().value, p.height().value))
        .collect();
    let _ = init_tx.send(Ok(sizes));

    let config = PdfRenderConfig::new().set_target_width(RENDER_WIDTH_PX);
    // `next_job` returns None when the document is closed.
    while let Some(index) = shared.next_job() {
        let rendered = RenderedPage {
            page: index,
            pixels: get_page_pixels(pages, &config, index),
        };
        // The UI side is gone: stop the thread.
        if results.send_blocking(rendered).is_err() {
            break;
        }
    }
    Ok(())
}

fn get_page_pixels(
    pages: &PdfPages,
    config: &PdfRenderConfig,
    index: usize,
) -> Option<(u32, u32, Vec<u8>)> {
    (|| -> Result<(u32, u32, Vec<u8>)> {
        let page_index = u16::try_from(index).context("page index too large")?;
        let page = pages.get(page_index as i32)?;
        let bitmap = page.render_with_config(config)?;
        let (w, h) = (bitmap.width() as u32, bitmap.height() as u32);
        // pdfium renders BGRA, which is what GPUI's RenderImage expects.
        let bytes: Vec<u8> = bitmap.as_raw_bytes().into();
        Ok((w, h, bytes))
    })()
    .map_err(|e| eprintln!("failed to render page {}: {e:#}", index + 1))
    .ok()
}
