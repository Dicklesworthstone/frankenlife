//! `flife render --camera`: a keyframed pan and zoom for pre-rendered output
//! (FL-020's [`Viewport::with_view`], plan §13.5). Presentation only: the
//! camera changes which part of the torus each frame shows and nothing the
//! world computes (INV-09), so a render with a camera reaches the same
//! digest as one without.
//!
//! A path is a JSON object `{"keys": [{"tick": t, "center": [x, y],
//! "zoom": z}, …]}` with ticks strictly increasing, centers in world units
//! (wrapped onto the torus) and zooms in `[1, MAXIMUM_ZOOM]`, where zoom 1
//! fits the whole torus. Before the first key and after the last the camera
//! holds still. Between two keys it eases with smoothstep `s² (3 − 2s)` of
//! the tick's fraction `s` of the way: the zoom geometrically (its log
//! linearly in the eased fraction), and the center along the shorter way
//! round the torus on each axis (the minimum image), so a path may cross a
//! seam.

use flife_core::CoreError;
use flife_manim::{MAXIMUM_ZOOM, Viewport};
use serde::{Deserialize, Serialize};

/// One key of a camera path.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CameraKey {
    /// The world tick the camera is here.
    pub tick: u64,
    /// The view's center in world units, y down as the world's.
    pub center: [f64; 2],
    /// The zoom: 1 fits the whole torus.
    pub zoom: f64,
}

/// A validated camera path: at least one key, ticks strictly increasing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CameraPath {
    /// The keys, in tick order.
    pub keys: Vec<CameraKey>,
}

/// Where a camera looks at one tick.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct CameraView {
    /// The view's center in world units, wrapped onto the torus.
    pub center: [f64; 2],
    /// The zoom.
    pub zoom: f64,
}

impl CameraPath {
    /// The path `text` holds.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for JSON that is not a path, no keys,
    /// ticks that do not strictly increase, a center that is not finite or a
    /// zoom outside `[1, MAXIMUM_ZOOM]`.
    pub fn from_json(text: &str) -> Result<Self, CoreError> {
        let path: Self = serde_json::from_str(text).map_err(|error| invalid(error.to_string()))?;
        if path.keys.is_empty() {
            return Err(invalid("a camera path needs at least one key".into()));
        }
        for pair in path.keys.windows(2) {
            if pair[1].tick <= pair[0].tick {
                return Err(invalid(format!(
                    "key ticks must strictly increase: {} then {}",
                    pair[0].tick, pair[1].tick
                )));
            }
        }
        for key in &path.keys {
            if !key.center.iter().all(|value| value.is_finite()) {
                return Err(invalid(format!(
                    "the center {:?} at tick {} is not finite",
                    key.center, key.tick
                )));
            }
            if !(1.0..=MAXIMUM_ZOOM).contains(&key.zoom) {
                return Err(invalid(format!(
                    "the zoom {} at tick {} is outside [1, {MAXIMUM_ZOOM}]",
                    key.zoom, key.tick
                )));
            }
        }
        Ok(path)
    }

    /// Where the camera looks at `tick` on a torus of `extent` world units.
    #[must_use]
    pub fn at(&self, tick: u64, extent: [f64; 2]) -> CameraView {
        let view = |center: [f64; 2], zoom: f64| CameraView {
            center: [wrap(center[0], extent[0]), wrap(center[1], extent[1])],
            zoom,
        };
        let next = self.keys.partition_point(|key| key.tick <= tick);
        let (Some(from), Some(to)) = (
            next.checked_sub(1).and_then(|at| self.keys.get(at)),
            self.keys.get(next),
        ) else {
            // Before the first key or at or after the last: hold.
            let key = if next == 0 {
                self.keys.first()
            } else {
                self.keys.last()
            };
            return key.map_or_else(
                || view([extent[0] / 2.0, extent[1] / 2.0], 1.0),
                |key| view(key.center, key.zoom),
            );
        };
        #[expect(
            clippy::cast_precision_loss,
            reason = "tick spans far below 2^52 ticks; the fraction is presentation only"
        )]
        let fraction = (tick - from.tick) as f64 / (to.tick - from.tick) as f64;
        let eased = fraction * fraction * (3.0 - 2.0 * fraction);
        let zoom = (from.zoom.ln() + eased * (to.zoom.ln() - from.zoom.ln()))
            .exp()
            .clamp(1.0, MAXIMUM_ZOOM);
        let mut center = [0.0; 2];
        for axis in 0..2 {
            let length = extent[axis];
            let mut step = wrap(to.center[axis] - from.center[axis], length);
            if step > length / 2.0 {
                step -= length;
            }
            center[axis] = from.center[axis] + eased * step;
        }
        view(center, zoom)
    }

    /// `base` (the view's own viewport) looking where the camera does at
    /// `tick`.
    ///
    /// # Errors
    /// As [`Viewport::with_view`].
    pub fn viewport(
        &self,
        base: &Viewport,
        tick: u64,
    ) -> Result<(Viewport, CameraView), CoreError> {
        let view = self.at(tick, base.extent());
        Ok((base.with_view(view.center, view.zoom)?, view))
    }
}

/// `value` wrapped into `[0, length)`.
fn wrap(value: f64, length: f64) -> f64 {
    let wrapped = value.rem_euclid(length);
    if wrapped >= length { 0.0 } else { wrapped }
}

fn invalid(detail: String) -> CoreError {
    CoreError::NumericalInvalid {
        quantity: "camera path",
        detail,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXTENT: [f64; 2] = [256.0, 128.0];

    fn path(keys: &[(u64, [f64; 2], f64)]) -> CameraPath {
        CameraPath {
            keys: keys
                .iter()
                .map(|&(tick, center, zoom)| CameraKey { tick, center, zoom })
                .collect(),
        }
    }

    #[test]
    fn holds_before_the_first_key_and_after_the_last() {
        let camera = path(&[(100, [10.0, 20.0], 2.0), (200, [30.0, 40.0], 4.0)]);
        for tick in [0, 99, 100] {
            assert_eq!(
                camera.at(tick, EXTENT),
                CameraView {
                    center: [10.0, 20.0],
                    zoom: 2.0
                }
            );
        }
        for tick in [200, 201, 10_000] {
            assert_eq!(
                camera.at(tick, EXTENT),
                CameraView {
                    center: [30.0, 40.0],
                    zoom: 4.0
                }
            );
        }
    }

    #[test]
    fn eases_halfway_to_the_geometric_mean_zoom_and_the_midpoint() {
        let camera = path(&[(0, [10.0, 20.0], 1.0), (100, [30.0, 60.0], 4.0)]);
        // smoothstep(1/2) = 1/2: the midpoint, and zoom √(1 · 4) = 2.
        let half = camera.at(50, EXTENT);
        assert!((half.center[0] - 20.0).abs() < 1e-12);
        assert!((half.center[1] - 40.0).abs() < 1e-12);
        assert!((half.zoom - 2.0).abs() < 1e-12);
        // smoothstep(1/4) = 5/32 of the way.
        let quarter = camera.at(25, EXTENT);
        assert!((quarter.center[0] - (10.0 + 20.0 * 5.0 / 32.0)).abs() < 1e-12);
        assert!((quarter.zoom - 4.0_f64.powf(5.0 / 32.0)).abs() < 1e-12);
    }

    #[test]
    fn crosses_a_seam_the_shorter_way() {
        // 250 to 6 on a 256-wide torus is 12 units east, not 244 west.
        let camera = path(&[(0, [250.0, 64.0], 1.0), (100, [6.0, 64.0], 1.0)]);
        let half = camera.at(50, EXTENT);
        assert!((half.center[0] - 0.0).abs() < 1e-12, "{half:?}");
        let quarter = camera.at(25, EXTENT);
        assert!((quarter.center[0] - (250.0 + 12.0 * 5.0 / 32.0)).abs() < 1e-12);
    }

    #[test]
    fn refuses_what_is_not_a_path() {
        for text in [
            r#"{"keys": []}"#,
            r#"{"keys": [{"tick": 5, "center": [0, 0], "zoom": 1}, {"tick": 5, "center": [0, 0], "zoom": 1}]}"#,
            r#"{"keys": [{"tick": 5, "center": [0, 0], "zoom": 0.5}]}"#,
            r#"{"keys": [{"tick": 5, "center": [0, 0], "zoom": 1, "roll": 3}]}"#,
            r#"{"keys": [{"tick": 5, "center": [0, 0]}]}"#,
            "not json",
        ] {
            assert!(CameraPath::from_json(text).is_err(), "{text}");
        }
        let camera =
            CameraPath::from_json(r#"{"keys": [{"tick": 5, "center": [300, -4], "zoom": 3}]}"#)
                .expect("a path");
        assert_eq!(
            camera.at(0, EXTENT),
            CameraView {
                center: [44.0, 124.0],
                zoom: 3.0
            }
        );
    }
}
