//! Maps sim coordinates (NM, y north) to screen pixels (y down).

use crate::sim::airspace::Airport;
use macroquad::prelude as mq;

/// Width of the side panel, pixels. The scope fills the rest of the window.
pub const PANEL_WIDTH: f32 = 420.0;
const MIN_SCALE: f32 = 2.0;
const MAX_SCALE: f32 = 60.0;
/// Extra room around the sector at startup, as a share of its size.
const VIEW_MARGIN: f32 = 1.12;

pub struct Camera {
    pub center: glam::Vec2,
    /// Pixels per NM.
    pub scale: f32,
}

impl Camera {
    /// Frame the whole sector, whatever its size.
    pub fn fit_sector(airport: &Airport) -> Camera {
        let (mut min, mut max) = (glam::Vec2::splat(f32::MAX), glam::Vec2::splat(f32::MIN));
        for p in &airport.sector {
            let p = glam::Vec2::new(p[0], p[1]);
            min = min.min(p);
            max = max.max(p);
        }
        let size = (max - min).max_element().max(1.0) * VIEW_MARGIN;
        let side = scope_width().min(mq::screen_height());
        Camera {
            center: (min + max) / 2.0,
            scale: (side / size).clamp(MIN_SCALE, MAX_SCALE),
        }
    }

    pub fn to_screen(&self, p: glam::Vec2) -> mq::Vec2 {
        let origin = scope_center();
        mq::vec2(
            origin.x + (p.x - self.center.x) * self.scale,
            origin.y - (p.y - self.center.y) * self.scale,
        )
    }

    pub fn to_world(&self, s: mq::Vec2) -> glam::Vec2 {
        let origin = scope_center();
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
}

pub fn scope_width() -> f32 {
    (mq::screen_width() - PANEL_WIDTH).max(100.0)
}

fn scope_center() -> mq::Vec2 {
    mq::vec2(scope_width() / 2.0, mq::screen_height() / 2.0)
}

pub fn mouse_in_scope() -> bool {
    mq::mouse_position().0 < scope_width()
}
