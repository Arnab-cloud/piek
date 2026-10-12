use crate::constants::{DISPLAY_WIDTH, PAGE_GAP};

pub struct RenderedPage {
    pub page: usize,
    /// (width, height, BGRA bytes); `None` if rendering failed.
    pub pixels: Option<(u32, u32, Vec<u8>)>,
}

pub struct PageLayout {
    /// vertical pos from where the page starts, including the top gap
    pub tops: Vec<f32>,
    /// height of each page
    pub heights: Vec<f32>,
    /// vertical pos till the page + PAGE_GAP
    pub bottoms: Vec<f32>,
    /// total scroll height
    pub total: f32,
}

impl PageLayout {
    pub fn new(sizes: &[(f32, f32)]) -> Self {
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

    pub fn len(&self) -> usize {
        self.tops.len()
    }

    /// Inclusive range of pages intersecting [top, bottom). Requires len() > 0.
    pub fn visible_range(&self, top: f32, bottom: f32) -> (usize, usize) {
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
