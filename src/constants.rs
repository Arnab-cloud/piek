pub const PAGE_GAP: f32 = 12.0;

// Pages rendered ahead of / behind the viewport.
pub const PREFETCH_PAGES: usize = 2;
// Max rendered pages kept in memory. At 1600px wide a page is ~13 MB of
// pixels (plus a GPU copy), so tune this to your memory budget.
pub const MAX_CACHED_PAGES: usize = 16;

// Hardcoded for now. Change this to a PDF on your machine.
pub const PDF_PATH: &str = "test_pdfs/test.pdf";
pub const PDFIUM_DLL: &str = "./pdfium-win-x64/bin";
// Render at 2x the displayed width so it stays sharp on HiDPI screens.
pub const DISPLAY_WIDTH: f32 = 800.0;
pub const RENDER_SCALE: f32 = 2.0;
pub const RENDER_WIDTH_PX: i32 = (DISPLAY_WIDTH * RENDER_SCALE) as i32;

// Preferred window size; it is clamped to the screen so the title bar
// can never end up above the top edge of the desktop.
pub const PREFERRED_WINDOW: (f32, f32) = (900.0, 1000.0);
pub const SCREEN_MARGIN: f32 = 0.85; // use at most 85% of the screen
