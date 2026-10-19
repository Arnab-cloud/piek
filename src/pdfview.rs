use gpui::{
    AnyElement, InteractiveElement, IntoElement, ParentElement, Render, RenderImage, ScrollHandle,
    StatefulInteractiveElement, Styled, Window, div, img, px, rgb,
};

use image::{Frame, RgbaImage};
use std::sync::Arc;

use std::collections::{HashMap, HashSet};

use crate::constants::{DISPLAY_WIDTH, MAX_CACHED_PAGES, PAGE_GAP, PREFETCH_PAGES};
use crate::page::{PageLayout, RenderedPage};
use crate::shared::Shared;

struct CachedPage {
    image: Arc<RenderImage>,
    last_used: u64,
}

pub struct PdfView {
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
    pub fn new(
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

impl Drop for PdfView {
    fn drop(&mut self) {
        // Lets the pdfium worker thread exit when this document is closed.
        self.shared.shutdown();
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
