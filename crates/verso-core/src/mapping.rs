//! Projection mapping: how the composition reaches the screens of a room.
//!
//! The setup is saved apart from the composition (it belongs to the venue):
//! a list of [`Screen`]s, each a window/projector with its own resolution,
//! showing [`Slice`]s. A slice takes a rectangle of the composition and
//! places it on the screen through a [`Warp`] grid, with optional soft edges
//! ([`EdgeBlend`]) for overlapping projectors and a polygon [`Mask`].
//!
//! Coordinates are normalised: 0..1 across the composition (input) or the
//! screen (output), origin top left.

use serde::{Deserialize, Serialize};

/// Mesh subdivisions per warp grid cell when tessellating.
pub const SUBDIVISIONS: usize = 16;
/// Largest warp grid on either side.
pub const MAX_GRID: usize = 16;

/// A normalised rectangle.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const FULL: Rect = Rect {
        x: 0.0,
        y: 0.0,
        w: 1.0,
        h: 1.0,
    };

    /// Point at `(u, v)` inside the rectangle.
    pub fn at(&self, u: f32, v: f32) -> [f32; 2] {
        [self.x + u * self.w, self.y + v * self.h]
    }
}

/// Output shape of a slice: a grid of control points, `columns × rows`,
/// row by row from the top left. A 2×2 grid is a corner pin (perspective
/// correct); larger grids bend smoothly through every point.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Warp {
    pub columns: usize,
    pub rows: usize,
    pub points: Vec<[f32; 2]>,
}

impl Warp {
    /// A flat grid covering `rect`.
    pub fn grid(rect: Rect, columns: usize, rows: usize) -> Self {
        let (columns, rows) = (columns.clamp(2, MAX_GRID), rows.clamp(2, MAX_GRID));
        let points = (0..rows)
            .flat_map(|r| {
                (0..columns).map(move |c| {
                    rect.at(
                        c as f32 / (columns - 1) as f32,
                        r as f32 / (rows - 1) as f32,
                    )
                })
            })
            .collect();
        Self {
            columns,
            rows,
            points,
        }
    }

    pub fn point(&self, column: usize, row: usize) -> [f32; 2] {
        let c = column.min(self.columns - 1);
        let r = row.min(self.rows - 1);
        self.points[r * self.columns + c]
    }

    /// Changes the grid size, resampling the current shape.
    pub fn resized(&self, columns: usize, rows: usize) -> Self {
        let (columns, rows) = (columns.clamp(2, MAX_GRID), rows.clamp(2, MAX_GRID));
        let points = (0..rows)
            .flat_map(|r| {
                (0..columns).map(move |c| {
                    self.map(
                        c as f32 / (columns - 1) as f32,
                        r as f32 / (rows - 1) as f32,
                    )
                })
            })
            .collect();
        Self {
            columns,
            rows,
            points,
        }
    }

    /// Whether the grid has the right number of finite points.
    pub fn valid(&self) -> bool {
        (2..=MAX_GRID).contains(&self.columns)
            && (2..=MAX_GRID).contains(&self.rows)
            && self.points.len() == self.columns * self.rows
            && self.points.iter().flatten().all(|v| v.is_finite())
    }

    /// Output position of slice point `(u, v)` in 0..1.
    pub fn map(&self, u: f32, v: f32) -> [f32; 2] {
        if self.columns == 2 && self.rows == 2 {
            return corner_pin(
                [
                    self.point(0, 0),
                    self.point(1, 0),
                    self.point(1, 1),
                    self.point(0, 1),
                ],
                u,
                v,
            );
        }
        // Catmull-Rom surface through the control points.
        let fx = u.clamp(0.0, 1.0) * (self.columns - 1) as f32;
        let fy = v.clamp(0.0, 1.0) * (self.rows - 1) as f32;
        let (cx, cy) = (
            (fx.floor() as usize).min(self.columns - 2),
            (fy.floor() as usize).min(self.rows - 2),
        );
        let (tx, ty) = (fx - cx as f32, fy - cy as f32);
        let point = |c: isize, r: isize| {
            // Edge points are extended linearly past the grid.
            let clamp_c = c.clamp(0, self.columns as isize - 1) as usize;
            let clamp_r = r.clamp(0, self.rows as isize - 1) as usize;
            let p = self.point(clamp_c, clamp_r);
            let (dc, dr) = (c - clamp_c as isize, r - clamp_r as isize);
            let along = |dc: isize, dr: isize| {
                let inner = self.point(
                    (clamp_c as isize - dc).clamp(0, self.columns as isize - 1) as usize,
                    (clamp_r as isize - dr).clamp(0, self.rows as isize - 1) as usize,
                );
                [p[0] - inner[0], p[1] - inner[1]]
            };
            let ext_c = if dc != 0 {
                along(dc.signum(), 0)
            } else {
                [0.0, 0.0]
            };
            let ext_r = if dr != 0 {
                along(0, dr.signum())
            } else {
                [0.0, 0.0]
            };
            [p[0] + ext_c[0] + ext_r[0], p[1] + ext_c[1] + ext_r[1]]
        };
        let row = |r: isize| {
            catmull_rom(
                point(cx as isize - 1, r),
                point(cx as isize, r),
                point(cx as isize + 1, r),
                point(cx as isize + 2, r),
                tx,
            )
        };
        catmull_rom(
            row(cy as isize - 1),
            row(cy as isize),
            row(cy as isize + 1),
            row(cy as isize + 2),
            ty,
        )
    }

    /// Triangle mesh: positions on the screen and matching 0..1 slice
    /// coordinates, three vertices per triangle.
    pub fn mesh(&self) -> Vec<([f32; 2], [f32; 2])> {
        let (nx, ny) = if self.columns == 2 && self.rows == 2 {
            // A corner pin is drawn finely enough for the perspective.
            (SUBDIVISIONS * 2, SUBDIVISIONS * 2)
        } else {
            (
                (self.columns - 1) * SUBDIVISIONS,
                (self.rows - 1) * SUBDIVISIONS,
            )
        };
        let vertex = |i: usize, j: usize| {
            let (u, v) = (i as f32 / nx as f32, j as f32 / ny as f32);
            (self.map(u, v), [u, v])
        };
        let mut out = Vec::with_capacity(nx * ny * 6);
        for j in 0..ny {
            for i in 0..nx {
                let (a, b, c, d) = (
                    vertex(i, j),
                    vertex(i + 1, j),
                    vertex(i + 1, j + 1),
                    vertex(i, j + 1),
                );
                out.extend([a, b, c, a, c, d]);
            }
        }
        out
    }
}

fn catmull_rom(p0: [f32; 2], p1: [f32; 2], p2: [f32; 2], p3: [f32; 2], t: f32) -> [f32; 2] {
    let (t2, t3) = (t * t, t * t * t);
    let f = |a: f32, b: f32, c: f32, d: f32| {
        0.5 * (2.0 * b
            + (c - a) * t
            + (2.0 * a - 5.0 * b + 4.0 * c - d) * t2
            + (3.0 * b - a - 3.0 * c + d) * t3)
    };
    [f(p0[0], p1[0], p2[0], p3[0]), f(p0[1], p1[1], p2[1], p3[1])]
}

/// Maps `(u, v)` of the unit square onto the quad `q` (top left, top right,
/// bottom right, bottom left) with the perspective a projector produces.
fn corner_pin(q: [[f32; 2]; 4], u: f32, v: f32) -> [f32; 2] {
    // Homography of the unit square onto the quad (Heckbert).
    let [p0, p1, p2, p3] = q.map(|p| [p[0] as f64, p[1] as f64]);
    let sx = p0[0] - p1[0] + p2[0] - p3[0];
    let sy = p0[1] - p1[1] + p2[1] - p3[1];
    let (dx1, dy1) = (p1[0] - p2[0], p1[1] - p2[1]);
    let (dx2, dy2) = (p3[0] - p2[0], p3[1] - p2[1]);
    let det = dx1 * dy2 - dx2 * dy1;
    let (g, h) = if det.abs() < 1e-12 {
        (0.0, 0.0)
    } else {
        ((sx * dy2 - dx2 * sy) / det, (dx1 * sy - sx * dy1) / det)
    };
    let a = p1[0] - p0[0] + g * p1[0];
    let b = p3[0] - p0[0] + h * p3[0];
    let d = p1[1] - p0[1] + g * p1[1];
    let e = p3[1] - p0[1] + h * p3[1];
    let (u, v) = (u as f64, v as f64);
    let w = g * u + h * v + 1.0;
    let w = if w.abs() < 1e-9 { 1e-9 } else { w };
    [
        ((a * u + b * v + p0[0]) / w) as f32,
        ((d * u + e * v + p0[1]) / w) as f32,
    ]
}

/// Soft edges for overlapping projectors: the width of each fade (fraction
/// of the slice, 0 for none) and the gamma of the ramp.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct EdgeBlend {
    pub left: f32,
    pub right: f32,
    pub top: f32,
    pub bottom: f32,
    /// Ramp exponent; 2.2 compensates a typical projector gamma.
    pub gamma: f32,
}

impl Default for EdgeBlend {
    fn default() -> Self {
        Self {
            left: 0.0,
            right: 0.0,
            top: 0.0,
            bottom: 0.0,
            gamma: 2.2,
        }
    }
}

impl EdgeBlend {
    /// Brightness factor at slice point `(u, v)`. Matches the shader.
    pub fn factor(&self, u: f32, v: f32) -> f32 {
        let ramp = |distance: f32, width: f32| {
            if width <= 0.0 {
                1.0
            } else {
                // Smooth ramp so two overlapping fades sum to 1 in light.
                let t = (distance / width).clamp(0.0, 1.0);
                let s = t * t * (3.0 - 2.0 * t);
                s.powf(1.0 / self.gamma.max(0.1))
            }
        };
        ramp(u, self.left)
            * ramp(1.0 - u, self.right)
            * ramp(v, self.top)
            * ramp(1.0 - v, self.bottom)
    }
}

/// A polygon on the screen; outside it (or inside, inverted) is black.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Mask {
    /// Screen points, in order around the shape.
    pub points: Vec<[f32; 2]>,
    #[serde(default)]
    pub invert: bool,
}

impl Mask {
    /// Triangles filling the polygon (ear clipping; works for any simple
    /// polygon, convex or not).
    pub fn triangles(&self) -> Vec<[f32; 2]> {
        triangulate(&self.points)
    }
}

fn cross(o: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
}

fn inside_triangle(p: [f32; 2], a: [f32; 2], b: [f32; 2], c: [f32; 2]) -> bool {
    let (d1, d2, d3) = (cross(a, b, p), cross(b, c, p), cross(c, a, p));
    let negative = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let positive = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(negative && positive)
}

/// Ear-clipping triangulation of a simple polygon.
pub fn triangulate(points: &[[f32; 2]]) -> Vec<[f32; 2]> {
    let n = points.len();
    if n < 3 || points.iter().flatten().any(|v| !v.is_finite()) {
        return Vec::new();
    }
    // Work counter-clockwise (positive area in a y-down frame is clockwise
    // on screen; only consistency matters).
    let area: f32 = (0..n)
        .map(|i| {
            let (a, b) = (points[i], points[(i + 1) % n]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum();
    let mut index: Vec<usize> = if area > 0.0 {
        (0..n).collect()
    } else {
        (0..n).rev().collect()
    };
    let mut out = Vec::with_capacity((n - 2) * 3);
    let mut guard = 0;
    while index.len() > 3 && guard < n * n {
        guard += 1;
        let m = index.len();
        let mut clipped = false;
        for i in 0..m {
            let (ia, ib, ic) = (index[(i + m - 1) % m], index[i], index[(i + 1) % m]);
            let (a, b, c) = (points[ia], points[ib], points[ic]);
            if cross(a, b, c) <= 0.0 {
                continue; // Reflex corner.
            }
            let blocked = index
                .iter()
                .filter(|&&j| j != ia && j != ib && j != ic)
                .any(|&j| inside_triangle(points[j], a, b, c));
            if !blocked {
                out.extend([a, b, c]);
                index.remove(i);
                clipped = true;
                break;
            }
        }
        if !clipped {
            break; // Self-intersecting: keep what was found.
        }
    }
    if index.len() == 3 {
        out.extend(index.iter().map(|&i| points[i]));
    }
    out
}

/// One region of the composition placed on a screen.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Slice {
    pub name: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Part of the composition shown.
    pub input: Rect,
    /// Where it lands on the screen.
    pub warp: Warp,
    #[serde(default)]
    pub blend: EdgeBlend,
    #[serde(default)]
    pub mask: Option<Mask>,
    #[serde(default = "one")]
    pub opacity: f32,
}

fn yes() -> bool {
    true
}

fn one() -> f32 {
    1.0
}

impl Slice {
    /// The whole composition on the whole screen.
    pub fn full(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            enabled: true,
            input: Rect::FULL,
            warp: Warp::grid(Rect::FULL, 2, 2),
            blend: EdgeBlend::default(),
            mask: None,
            opacity: 1.0,
        }
    }
}

/// A projector or display: a window rendered at `size`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Screen {
    pub name: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Output resolution in pixels.
    pub size: (u32, u32),
    pub slices: Vec<Slice>,
    /// Last window position and size on the desktop, to reopen it on the
    /// same projector.
    #[serde(default)]
    pub window: Option<[f32; 4]>,
    #[serde(default)]
    pub fullscreen: bool,
}

impl Screen {
    pub fn new(name: impl Into<String>, size: (u32, u32)) -> Self {
        Self {
            name: name.into(),
            enabled: true,
            size,
            slices: vec![Slice::full("Slice 1")],
            window: None,
            fullscreen: false,
        }
    }
}

/// The screens of a venue. Saved as `*.versomap`, apart from compositions.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct MappingSetup {
    pub screens: Vec<Screen>,
}

impl MappingSetup {
    pub fn to_ron(&self) -> crate::Result<String> {
        Ok(ron::ser::to_string_pretty(self, Default::default())?)
    }

    /// Parses a setup and makes it safe to render: sizes bounded, grids
    /// consistent, numbers finite.
    pub fn from_ron(text: &str) -> crate::Result<Self> {
        let mut setup: Self = ron::from_str(text)?;
        setup.repair();
        Ok(setup)
    }

    pub fn repair(&mut self) {
        let finite = |v: f32, d: f32| if v.is_finite() { v } else { d };
        for screen in &mut self.screens {
            screen.size = (
                screen.size.0.clamp(crate::MIN_SIZE, crate::MAX_SIZE),
                screen.size.1.clamp(crate::MIN_SIZE, crate::MAX_SIZE),
            );
            if screen
                .window
                .is_some_and(|w| w.iter().any(|v| !v.is_finite()))
            {
                screen.window = None;
            }
            for slice in &mut screen.slices {
                let r = &mut slice.input;
                r.x = finite(r.x, 0.0).clamp(-1.0, 2.0);
                r.y = finite(r.y, 0.0).clamp(-1.0, 2.0);
                r.w = finite(r.w, 1.0).clamp(0.001, 3.0);
                r.h = finite(r.h, 1.0).clamp(0.001, 3.0);
                if !slice.warp.valid() {
                    slice.warp = Warp::grid(Rect::FULL, 2, 2);
                }
                let b = &mut slice.blend;
                for edge in [&mut b.left, &mut b.right, &mut b.top, &mut b.bottom] {
                    *edge = finite(*edge, 0.0).clamp(0.0, 0.5);
                }
                b.gamma = finite(b.gamma, 2.2).clamp(0.1, 5.0);
                slice.opacity = finite(slice.opacity, 1.0).clamp(0.0, 1.0);
                if let Some(mask) = &mut slice.mask {
                    mask.points.retain(|p| p.iter().all(|v| v.is_finite()));
                    mask.points.truncate(256);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f32; 2], b: [f32; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-4 && (a[1] - b[1]).abs() < 1e-4
    }

    #[test]
    fn corner_pin_hits_its_corners_with_perspective() {
        let mut warp = Warp::grid(Rect::FULL, 2, 2);
        // Row by row: top left, top right, bottom left, bottom right.
        warp.points = vec![[0.1, 0.0], [0.9, 0.2], [0.0, 1.0], [0.8, 0.9]];
        assert!(close(warp.map(0.0, 0.0), [0.1, 0.0]));
        assert!(close(warp.map(1.0, 0.0), [0.9, 0.2]));
        assert!(close(warp.map(1.0, 1.0), [0.8, 0.9]));
        assert!(close(warp.map(0.0, 1.0), [0.0, 1.0]));
        // Perspective: the middle is where the diagonals cross, not the
        // average of the corners.
        let m = warp.map(0.5, 0.5);
        let avg = [(0.1 + 0.9 + 0.8 + 0.0) / 4.0, (0.0 + 0.2 + 0.9 + 1.0) / 4.0];
        assert!(!close(m, avg));
        assert!(
            cross([0.1, 0.0], [0.8, 0.9], m).abs() < 1e-4,
            "on diagonal 1"
        );
        assert!(
            cross([0.9, 0.2], [0.0, 1.0], m).abs() < 1e-4,
            "on diagonal 2"
        );
    }

    #[test]
    fn flat_grids_are_the_identity_and_curves_pass_through_points() {
        let warp = Warp::grid(Rect::FULL, 4, 3);
        for (u, v) in [(0.0, 0.0), (0.3, 0.7), (1.0, 1.0), (0.5, 0.25)] {
            assert!(close(warp.map(u, v), [u, v]), "{u},{v}");
        }
        let mut bent = warp.clone();
        bent.points[5] = [0.4, 0.6];
        assert!(close(bent.map(1.0 / 3.0, 0.5), [0.4, 0.6]));
        // Resizing keeps the shape.
        let resized = bent.resized(7, 5);
        assert!(close(resized.map(0.0, 0.0), bent.map(0.0, 0.0)));
        assert!(close(resized.map(1.0, 1.0), bent.map(1.0, 1.0)));
    }

    #[test]
    fn mesh_covers_the_slice() {
        let warp = Warp::grid(Rect::FULL, 3, 3);
        let mesh = warp.mesh();
        assert_eq!(mesh.len(), 2 * SUBDIVISIONS * 2 * SUBDIVISIONS * 6);
        let area: f32 = mesh
            .chunks(3)
            .map(|t| cross(t[0].0, t[1].0, t[2].0).abs() / 2.0)
            .sum();
        assert!((area - 1.0).abs() < 1e-3, "{area}");
    }

    #[test]
    fn edge_blend_ramps_and_overlaps_sum_to_one() {
        let blend = EdgeBlend {
            right: 0.2,
            gamma: 1.0,
            ..Default::default()
        };
        assert_eq!(blend.factor(0.5, 0.5), 1.0);
        assert_eq!(blend.factor(1.0, 0.5), 0.0);
        // Two projectors overlapping by 20 %: left one fades out, right one
        // fades in over the same area.
        let right = EdgeBlend {
            left: 0.2,
            gamma: 1.0,
            ..Default::default()
        };
        for i in 0..=10 {
            let t = i as f32 / 10.0 * 0.2;
            let sum = blend.factor(0.8 + t, 0.5) + right.factor(t, 0.5);
            assert!((sum - 1.0).abs() < 1e-5, "{t}: {sum}");
        }
    }

    #[test]
    fn concave_masks_triangulate_to_their_area() {
        // An L shape: area 0.75 of the unit square.
        let l = [
            [0.0, 0.0],
            [1.0, 0.0],
            [1.0, 0.5],
            [0.5, 0.5],
            [0.5, 1.0],
            [0.0, 1.0],
        ];
        for points in [l.to_vec(), l.iter().rev().copied().collect()] {
            let triangles = triangulate(&points);
            assert_eq!(triangles.len(), 4 * 3);
            let area: f32 = triangles
                .chunks(3)
                .map(|t| cross(t[0], t[1], t[2]).abs() / 2.0)
                .sum();
            assert!((area - 0.75).abs() < 1e-5);
        }
        assert!(triangulate(&[[0.0, 0.0], [1.0, 1.0]]).is_empty());
    }

    #[test]
    fn setups_round_trip_and_are_repaired() {
        let mut setup = MappingSetup {
            screens: vec![Screen::new("Projector 1", (1920, 1080))],
        };
        setup.screens[0].slices[0].blend.right = 0.15;
        let text = setup.to_ron().unwrap();
        assert_eq!(MappingSetup::from_ron(&text).unwrap(), setup);

        setup.screens[0].size = (0, 100_000);
        setup.screens[0].slices[0].warp.points.pop();
        setup.screens[0].slices[0].blend.left = f32::NAN;
        let repaired = MappingSetup::from_ron(&setup.to_ron().unwrap()).unwrap();
        let screen = &repaired.screens[0];
        assert_eq!(screen.size, (crate::MIN_SIZE, crate::MAX_SIZE));
        assert!(screen.slices[0].warp.valid());
        assert_eq!(screen.slices[0].blend.left, 0.0);
    }
}
