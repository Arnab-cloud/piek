use std::sync::Arc;
 use std::env;

use gpui::{
    AppContext, Application, Bounds, Point, Render, RenderImage, Size, WindowBounds, WindowOptions,
    div,Styled, ParentElement, InteractiveElement, StatefulInteractiveElement, px, img, size, rgb};
use pdfium_render::prelude::{Pdfium,PdfRenderConfig};
use image::{RgbaImage, Frame};
use anyhow::{Context, Result};

// Hardcoded for now. Change this to a PDF on your machine.
const PDF_PATH: &str = "test_pdfs/test.pdf";
const PDFIUM_DLL: &str = "./pdfium-win-x64/bin";
// Render at 2x the displayed width so it stays sharp on HiDPI screens.
const DISPLAY_WIDTH: f32 = 800.0;
const RENDER_SCALE: f32 = 2.0;
const PAGE_GAP: f32 = 12.0;

// Preferred window size; it is clamped to the screen so the title bar
// can never end up above the top edge of the desktop.
const PREFERRED_WINDOW: (f32, f32) = (900.0, 1000.0);
const SCREEN_MARGIN: f32 = 0.85; // use at most 85% of the screen


struct PageImage {
    image: Arc<RenderImage>,
    display_w: f32,
    display_h: f32,
}

struct PdfView {
    pages: Vec<PageImage>,
}

impl Render for PdfView {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        _cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        div()
            .id("viewport")
            .size_full()
            .bg(rgb(0x3a3a3a))
            .overflow_y_scroll()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(PAGE_GAP))
                    .py(px(PAGE_GAP))
                    .children(self.pages.iter().map(|p| {
                        img(p.image.clone())
                            .w(px(p.display_w))
                            .h(px(p.display_h))
                            // Don't let the flex container squash pages
                            // (this is what would otherwise break scrolling)
                            .flex_none()
                    })),
            )
    }
}

/// Load the PDF with pdfium and render every page into a GPUI-compatible image.
fn render_all_pages(path: &str) -> Result<Vec<PageImage>> {
    // Looks for libpdfium in the cwd first, then falls back to a system copy.
    let bindings = Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path(PDFIUM_DLL))
        .or_else(|_| Pdfium::bind_to_system_library())
        .context("could not find the pdfium shared library")?;
    let pdfium = Pdfium::new(bindings);

    let document = pdfium
        .load_pdf_from_file(path, None)
        .with_context(|| format!("failed to open {path}"))?;

    let target_width = (DISPLAY_WIDTH * RENDER_SCALE) as i32;
    let config = PdfRenderConfig::new().set_target_width(target_width);

    let mut pages = Vec::new();
    for (index, page) in document.pages().iter().enumerate() {
        let bitmap = page
            .render_with_config(&config)
            .with_context(|| format!("failed to render page {}", index + 1))?;

        let (w, h) = (bitmap.width() as u32, bitmap.height() as u32);

        // pdfium renders BGRA, which is what GPUI's RenderImage expects.
        let bytes = bitmap.as_raw_bytes().to_vec();
        let buffer = RgbaImage::from_raw(w, h, bytes).context("unexpected bitmap size")?;

        pages.push(PageImage {
            image: Arc::new(RenderImage::new(vec![Frame::new(buffer)])),
            display_w: DISPLAY_WIDTH,
            display_h: DISPLAY_WIDTH * (h as f32 / w as f32),
        });
    }

    Ok(pages)
}

fn main() -> Result<()>{
    let envs: Vec<String> = env::args().collect();
    let pdf_path: String;
    if envs.len() < 2 {
        pdf_path = PDF_PATH.to_string();
        eprintln!("NO pdf path provided, using the default one: {}", pdf_path);
    } else {pdf_path = envs[1].clone();}
    let pages = render_all_pages(&pdf_path)?;

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

        app.open_window(window_options, |_win, cx| cx.new(|_| PdfView{pages}))
            .unwrap();
    });
    Ok(())
}
