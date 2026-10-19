use std::path::PathBuf;

use gpui::{
    AppContext, Context, Entity, InteractiveElement, IntoElement, ParentElement, PathPromptOptions,
    Render, SharedString, StatefulInteractiveElement, Styled, Window, div, px, rgb,
};

use crate::pdfview::PdfView;
use crate::worker::start_worker;

/// Root view: shows the empty "open a PDF" screen until a document is loaded.
pub struct PiekApp {
    viewer: Option<Entity<PdfView>>,
    loading: bool,
    error: Option<SharedString>,
}

impl PiekApp {
    pub fn new(initial: Option<PathBuf>, cx: &mut Context<Self>) -> Self {
        let mut app = Self {
            viewer: None,
            loading: false,
            error: None,
        };
        if let Some(path) = initial {
            app.open_path(path, cx);
        }
        app
    }

    /// Show the native file picker, then open whatever was chosen.
    fn prompt_open(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Open PDF".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = picked.await {
                if let Some(path) = paths.into_iter().next() {
                    this.update(cx, |app, cx| app.open_path(path, cx)).ok();
                }
            }
        })
        .detach();
    }

    fn open_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.loading = true;
        self.error = None;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let path_str = path.to_string_lossy().into_owned();
            // Loading the document talks to pdfium, so keep it off the UI thread.
            let result = cx
                .background_executor()
                .spawn(async move { start_worker(&path_str) })
                .await;

            this.update(cx, |app, cx| {
                app.loading = false;
                match result {
                    Ok(doc) => {
                        // Replacing the old viewer drops it, which shuts down
                        // its worker thread.
                        app.viewer =
                            Some(cx.new(|cx| PdfView::new(doc.sizes, doc.shared, doc.results, cx)));
                    }
                    Err(err) => {
                        eprintln!("failed to open pdf: {err:#}");
                        app.error = Some(format!("{err:#}").into());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn empty_state(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let label = if self.loading {
            "Opening…"
        } else {
            "Open PDF"
        };

        let mut column = div()
            .size_full()
            .bg(rgb(0x21252b))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(12.0))
            .child(
                div()
                    .text_size(px(15.0))
                    .text_color(rgb(0xabb2bf))
                    .child("No PDF open."),
            )
            .child(
                div()
                    .id("open-pdf")
                    .px(px(16.0))
                    .py(px(8.0))
                    .rounded(px(6.0))
                    .bg(rgb(0x2c313a))
                    .text_size(px(15.0))
                    .text_color(rgb(0xffffff))
                    .cursor_pointer()
                    .hover(|style| style.bg(rgb(0x353b45)).text_color(rgb(0xe5c07b)))
                    .on_click(cx.listener(|this, _, _, cx| {
                        if !this.loading {
                            this.prompt_open(cx);
                        }
                    }))
                    .child(label),
            );

        if let Some(error) = &self.error {
            column = column.child(
                div()
                    .max_w(px(500.0))
                    .text_size(px(13.0))
                    .text_color(rgb(0xf87171))
                    .child(error.clone()),
            );
        }
        column
    }
}

impl Render for PiekApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match &self.viewer {
            Some(viewer) => div().size_full().child(viewer.clone()).into_any_element(),
            None => self.empty_state(cx).into_any_element(),
        }
    }
}
