use std::collections::{HashMap, HashSet, VecDeque};
use std::env;
use std::sync::{Arc, Condvar, Mutex, mpsc};

use anyhow::{Context, Result};
use gpui::{
    AnyElement, AppContext, Application, Bounds, InteractiveElement, IntoElement, ParentElement,
    Render, RenderImage, ScrollHandle, StatefulInteractiveElement, Styled, Window, WindowBounds,
    WindowOptions, div, img, px, rgb, size,
};
use image::{Frame, RgbaImage};
use pdfium_render::prelude::{PdfRenderConfig, Pdfium};

// Hardcoded for now. Change this to a PDF on your machine.
const PDF_PATH: &str = "test_pdfs/test.pdf";
const PDFIUM_DLL: &str = "./pdfium-win-x64/bin";
// Render at 2x the displayed width so it stays sharp on HiDPI screens.
const DISPLAY_WIDTH: f32 = 800.0;
const RENDER_SCALE: f32 = 2.0;
const RENDER_WIDTH_PX: i32 = (DISPLAY_WIDTH * RENDER_SCALE) as i32;
const PAGE_GAP: f32 = 12.0;

// Preferred window size; it is clamped to the screen so the title bar
// can never end up above the top edge of the desktop.
const PREFERRED_WINDOW: (f32, f32) = (900.0, 1000.0);
const SCREEN_MARGIN: f32 = 0.85; // use at most 85% of the screen

// Pages rendered ahead of / behind the viewport.
const PREFETCH_PAGES: usize = 2;
// Max rendered pages kept in memory. At 1600px wide a page is ~13 MB of
// pixels (plus a GPU copy), so tune this to your memory budget.
const MAX_CACHED_PAGES: usize = 16;

struct Shared {
    queue: Mutex<VecDeque<usize>>,
    wake: Condvar,
}

impl Shared {
    fn next_job(&self) -> usize {
        let mut queue = self.queue.lock().unwrap();
        loop {
            if let Some(page) = queue.pop_front() {
                return page;
            }
            queue = self.wake.wait(queue).unwrap();
        }
    }
}

struct PageImage {
    image: Arc<RenderImage>,
    display_w: f32,
    display_h: f32,
}

struct PdfView {
    layout: PageLayout,
    cache: HashMap<usize, CachedPage>,
    /// Pages queued or currently rendering on the worker (not yet received).
    requested: HashSet<usize>,
    /// The priority list last sent to the worker; avoids redundant scheduling.
    last_wanted: Vec<usize>,
    shared: Arc<Shared>,
    scroll: ScrollHandle,
    tick: u64,
}

impl PdfView {
    fn new(
        sizes: Vec<(f32, f32)>,
        shared: Arc<Shared>,
        results: async_channel::Receiver<RenderedPage>,
        cx: &mut gpui::Context<Self>,
    ) -> Self {
        // Receive finished pages from the worker and hand them to the view.
        cx.spawn(async move |this, cx| {
            while let Ok(msg) = results.recv().await {
                if this
                    .update(cx, |view, cx| view.on_rendered(msg, cx))
                    .is_err()
                {
                    break; // view was dropped
                }
            }
        })
        .detach();

        Self {
            layout: PageLayout::new(&sizes),
            cache: HashMap::new(),
            requested: HashSet::new(),
            last_wanted: Vec::new(),
            shared,
            scroll: ScrollHandle::new(),
            tick: 0,
        }
    }

    fn on_rendered(&mut self, msg: RenderedPage, cx: &mut gpui::Context<Self>) {
        self.requested.remove(&msg.page);

        let Some((w, h, bytes)) = msg.pixels else {
            return;
        };
        let Some(buffer) = RgbaImage::from_raw(w, h, bytes) else {
            return;
        };

        self.tick += 1;
        self.cache.insert(
            msg.page,
            CachedPage {
                image: Arc::new(RenderImage::new(vec![Frame::new(buffer)])),
                last_used: self.tick,
            },
        );
        cx.notify();
    }

    /// Replace the worker's queue with `wanted` (highest priority first),
    /// skipping pages that are already cached or already being rendered.
    fn schedule(&mut self, wanted: &[usize]) {
        let mut queue = self.shared.queue.lock().unwrap();

        // Anything we requested that is no longer in the queue has been picked
        // up by the worker, so its result is on the way.
        let queued: HashSet<usize> = queue.iter().copied().collect();
        let in_flight: HashSet<usize> = self.requested.difference(&queued).copied().collect();

        queue.clear();
        let mut requested = in_flight.clone();
        for &page in wanted {
            if self.cache.contains_key(&page) || in_flight.contains(&page) {
                continue;
            }
            queue.push_back(page);
            requested.insert(page);
        }
        drop(queue);

        self.requested = requested;
        self.shared.wake.notify_one();
    }
}

impl Render for PdfView {
    fn render(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        let n = self.layout.len();
        if n == 0 {
            return div()
                .size_full()
                .bg(rgb(0x3a3a3a))
                .flex()
                .items_center()
                .justify_center()
                .text_color(rgb(0xcccccc))
                .child("This PDF has no pages")
                .into_any_element();
        }

        // --- What is on screen? ---
        let viewport_h = f32::from(window.viewport_size().height);
        let scroll_y = (-f32::from(self.scroll.offset().y)).max(0.0);
        let (first_vis, last_vis) = self.layout.visible_range(scroll_y, scroll_y + viewport_h);

        // Pages we want rendered (visible + prefetch), nearest to the viewport first.
        let wanted_lo = first_vis.saturating_sub(PREFETCH_PAGES);
        let wanted_hi = (last_vis + PREFETCH_PAGES).min(n - 1);
        let mut wanted: Vec<usize> = (wanted_lo..=wanted_hi).collect();
        wanted.sort_by_key(|&p| {
            if p < first_vis {
                first_vis - p
            } else if p > last_vis {
                p - last_vis
            } else {
                0
            }
        }); // stable: visible pages stay in top-to-bottom order

        if wanted != self.last_wanted {
            self.schedule(&wanted);
            self.last_wanted = wanted;
        }

        // --- LRU bookkeeping and eviction ---
        self.tick += 1;
        let tick = self.tick;
        let keep = wanted_lo..=wanted_hi;
        for page in keep.clone() {
            if let Some(entry) = self.cache.get_mut(&page) {
                entry.last_used = tick;
            }
        }
        while self.cache.len() > MAX_CACHED_PAGES {
            let victim = self
                .cache
                .iter()
                .filter(|(page, _)| !keep.contains(*page))
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(page, _)| *page);
            let Some(victim) = victim else { break };
            if let Some(entry) = self.cache.remove(&victim) {
                // Also free the GPU-side texture, not just the CPU pixels.
                let _ = window.drop_image(entry.image);
            }
        }

        // --- Build elements for just the pages near the viewport ---
        let draw_lo = first_vis.saturating_sub(1);
        let draw_hi = (last_vis + 1).min(n - 1);

        let mut pages: Vec<AnyElement> = Vec::with_capacity(draw_hi - draw_lo + 1);
        for i in draw_lo..=draw_hi {
            let h = self.layout.heights[i];
            let element = match self.cache.get(&i) {
                Some(entry) => img(entry.image.clone())
                    .w(px(DISPLAY_WIDTH))
                    .h(px(h))
                    .flex_none()
                    .mb(px(PAGE_GAP))
                    .into_any_element(),
                None => div()
                    .w(px(DISPLAY_WIDTH))
                    .h(px(h))
                    .flex_none()
                    .mb(px(PAGE_GAP))
                    .bg(rgb(0xffffff))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(rgb(0x999999))
                    .child(format!("Loading page {}…", i + 1))
                    .into_any_element(),
            };
            pages.push(element);
        }

        // Spacers stand in for the pages we skipped, so the scrollbar range and
        // scroll offsets match the full document.
        let top_spacer = self.layout.tops[draw_lo];
        let bottom_spacer =
            (self.layout.total - (self.layout.bottoms[draw_hi] + PAGE_GAP)).max(0.0);

        div()
            .id("viewport")
            .size_full()
            .bg(rgb(0x3a3a3a))
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            // Make sure we re-render (and schedule pages) on every scroll tick.
            .on_scroll_wheel(cx.listener(|_, _, _, cx| cx.notify()))
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .flex_none()
                    .items_center()
                    .child(div().h(px(top_spacer)).flex_none())
                    .children(pages)
                    .child(div().h(px(bottom_spacer)).flex_none()),
            )
            .into_any_element()
    }
}

struct RenderedPage {
    page: usize,
    /// (width, height, BGRA bytes); `None` if rendering failed.
    pixels: Option<(u32, u32, Vec<u8>)>,
}

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

struct PageLayout {
    tops: Vec<f32>,
    heights: Vec<f32>,
    bottoms: Vec<f32>,
    total: f32,
}

impl PageLayout {
    fn new(sizes: &[(f32, f32)]) -> Self {
        let mut tops = Vec::with_capacity(sizes.len());
        let mut heights = Vec::with_capacity(sizes.len());
        let mut bottoms = Vec::with_capacity(sizes.len());
        let mut y = PAGE_GAP;
        for &(w, h) in sizes {
            let display_h = if w > 0.0 {
                DISPLAY_WIDTH * h / w
            } else {
                DISPLAY_WIDTH * 1.4
            };
            tops.push(y);
            heights.push(display_h);
            bottoms.push(y + display_h);
            y += display_h + PAGE_GAP;
        }
        Self {
            tops,
            heights,
            bottoms,
            total: y,
        }
    }

    fn len(&self) -> usize {
        self.tops.len()
    }

    /// Inclusive range of pages intersecting [top, bottom). Requires len() > 0.
    fn visible_range(&self, top: f32, bottom: f32) -> (usize, usize) {
        let last_idx = self.len() - 1;
        let first = self.bottoms.partition_point(|&b| b <= top).min(last_idx);
        let last = self
            .tops
            .partition_point(|&t| t < bottom)
            .saturating_sub(1)
            .min(last_idx)
            .max(first);
        (first, last)
    }
}

struct CachedPage {
    image: Arc<RenderImage>,
    last_used: u64,
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
