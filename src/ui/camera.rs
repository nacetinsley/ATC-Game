//! Maps sim coordinates (NM, y north) to screen pixels (y down), inside a
//! viewport: the whole scope area, or an inset within it.

use crate::sim::airspace::Airport;
use macroquad::prelude as mq;

/// Width of the side panel, pixels. The scope fills the rest of the window.
pub const PANEL_WIDTH: f32 = 420.0;
const MIN_SCALE: f32 = 2.0;
const MAX_SCALE: f32 = 6000.0;
/// Extra room around what's being framed, as a share of its size.
const VIEW_MARGIN: f32 = 1.12;
/// Inset size as a share of the scope area's shorter side.
const INSET_SHARE: f32 = 0.38;
const INSET_MIN_PX: f32 = 240.0;
const INSET_MARGIN_PX: f32 = 12.0;

#[derive(Debug, Clone)]
pub struct Camera {
    pub center: glam::Vec2,
    /// Pixels per NM.
    pub scale: f32,
    /// Where on screen this camera draws.
    pub viewport: mq::Rect,
}

impl Camera {
    /// Frame a set of points in a viewport.
    pub fn fit(points: impl IntoIterator<Item = glam::Vec2>, viewport: mq::Rect) -> Camera {
        let (mut min, mut max) = (glam::Vec2::splat(f32::MAX), glam::Vec2::splat(f32::MIN));
        for p in points {
            min = min.min(p);
            max = max.max(p);
        }
        let size = (max - min).max_element().max(0.1) * VIEW_MARGIN;
        let side = viewport.w.min(viewport.h);
        Camera { center: (min + max) / 2.0, scale: (side / size).clamp(MIN_SCALE, MAX_SCALE), viewport }
    }

    /// Frame the whole sector, whatever its size.
    pub fn fit_sector(airport: &Airport) -> Camera {
        Camera::fit(airport.sector.iter().map(|p| glam::Vec2::new(p[0], p[1])), scope_area())
    }

    /// Frame the airport surface.
    pub fn fit_surface(airport: &Airport) -> Camera {
        let points: Vec<glam::Vec2> = match &airport.surface {
            Some(map) => map.nodes.iter().map(|n| n.pos).collect(),
            None => airport.runways.iter().flat_map(|r| [r.threshold(), r.far_end()]).collect(),
        };
        Camera::fit(points, scope_area())
    }

    /// Centred on the field, showing this many miles to the edge.
    pub fn range(center: glam::Vec2, range_nm: f32, viewport: mq::Rect) -> Camera {
        let side = viewport.w.min(viewport.h);
        Camera { center, scale: side / 2.0 / range_nm, viewport }
    }

    fn origin(&self) -> mq::Vec2 {
        self.viewport.center()
    }

    pub fn to_screen(&self, p: glam::Vec2) -> mq::Vec2 {
        let origin = self.origin();
        mq::vec2(
            origin.x + (p.x - self.center.x) * self.scale,
            origin.y - (p.y - self.center.y) * self.scale,
        )
    }

    pub fn to_world(&self, s: mq::Vec2) -> glam::Vec2 {
        let origin = self.origin();
        glam::Vec2::new(
            (s.x - origin.x) / self.scale + self.center.x,
            -(s.y - origin.y) / self.scale + self.center.y,
        )
    }

    /// Move the view by a mouse drag, in pixels.
    pub fn pan_pixels(&mut self, delta: mq::Vec2) {
        self.center.x -= delta.x / self.scale;
        self.center.y += delta.y / self.scale;
    }

    /// Zoom keeping the point under the cursor fixed.
    pub fn zoom_at(&mut self, cursor: mq::Vec2, factor: f32) {
        let before = self.to_world(cursor);
        self.scale = (self.scale * factor).clamp(MIN_SCALE, MAX_SCALE);
        let after = self.to_world(cursor);
        self.center += before - after;
    }

    /// Miles from the centre to the nearest edge.
    pub fn range_nm(&self) -> f32 {
        self.viewport.w.min(self.viewport.h) / 2.0 / self.scale
    }

    pub fn contains_mouse(&self) -> bool {
        self.viewport.contains(mq::Vec2::from(mq::mouse_position()))
    }
}

pub fn scope_width() -> f32 {
    (mq::screen_width() - PANEL_WIDTH).max(100.0)
}

/// Everything left of the side panel.
pub fn scope_area() -> mq::Rect {
    mq::Rect::new(0.0, 0.0, scope_width(), mq::screen_height())
}

/// A square in the bottom-right corner of the scope area.
pub fn inset_area() -> mq::Rect {
    let area = scope_area();
    let side = (area.w.min(area.h) * INSET_SHARE).max(INSET_MIN_PX).min(area.w.min(area.h));
    mq::Rect::new(
        area.x + area.w - side - INSET_MARGIN_PX,
        area.y + area.h - side - INSET_MARGIN_PX,
        side,
        side,
    )
}

pub fn mouse_in_scope() -> bool {
    mq::mouse_position().0 < scope_width()
}

/// Limit drawing to a rectangle, or lift the limit with `None`.
pub fn clip(rect: Option<mq::Rect>) {
    // SAFETY: macroquad's internal GL context is only touched from the main thread,
    // between frames' draw calls, which is where all drawing happens.
    let mut gl = unsafe { macroquad::window::get_internal_gl() };
    gl.flush();
    let dpi = macroquad::window::screen_dpi_scale();
    gl.quad_gl.scissor(rect.map(|r| {
        (
            (r.x * dpi) as i32,
            (r.y * dpi) as i32,
            (r.w * dpi) as i32,
            (r.h * dpi) as i32,
        )
    }));
}
