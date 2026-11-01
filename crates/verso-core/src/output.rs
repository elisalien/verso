//! Output mapping: the composition is cut into slices, and each slice is
//! warped onto a screen (a monitor, an NDI or Spout stream, …).
//!
//! The model follows Resolume Arena's Advanced Output so that its presets
//! import without loss; see `verso_io::resolume_ao`. Coordinates are pixels:
//! slice inputs in composition pixels, outputs in screen pixels.

use serde::{Deserialize, Serialize};

/// Four corners, clockwise from top left.
pub type Quad = [[f32; 2]; 4];

/// The quad covering `width` × `height` from the origin.
pub fn rect(width: f32, height: f32) -> Quad {
    [[0.0, 0.0], [width, 0.0], [width, height], [0.0, height]]
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutputSetup {
    /// Size of the composition the slice inputs refer to.
    pub canvas: (u32, u32),
    pub screens: Vec<Screen>,
    /// Shared by every soft-edged slice.
    pub soft_edge: SoftEdge,
}

impl Default for OutputSetup {
    fn default() -> Self {
        Self {
            canvas: (1920, 1080),
            screens: Vec::new(),
            soft_edge: SoftEdge::default(),
        }
    }
}

impl OutputSetup {
    /// A free id for a new screen or slice. Ids are stable across saves,
    /// since other documents (Chaser sequences, StageFlow looks) refer to
    /// slices by id.
    pub fn next_id(&self) -> u64 {
        self.screens
            .iter()
            .flat_map(|s| std::iter::once(s.id).chain(s.slices.iter().map(|l| l.id)))
            .max()
            .map_or(1, |id| id + 1)
    }

    pub fn slice_count(&self) -> usize {
        self.screens.iter().map(|s| s.slices.len()).sum()
    }

    /// Adds a screen of `size` with one slice showing the whole composition.
    pub fn add_screen(&mut self, name: &str, size: (u32, u32)) -> u64 {
        let id = self.next_id();
        let (w, h) = (size.0 as f32, size.1 as f32);
        let input = rect(self.canvas.0 as f32, self.canvas.1 as f32);
        self.screens.push(Screen {
            id,
            name: name.to_owned(),
            enabled: true,
            hidden: false,
            device: Device {
                kind: DeviceKind::Virtual,
                name: name.to_owned(),
                size: Some(size),
            },
            color: ColorAdjust::default(),
            slices: vec![Slice::new(id + 1, "Slice", input, rect(w, h))],
            dmx: false,
        });
        id
    }
}

/// Curve of the soft edge blend between overlapping projectors.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SoftEdge {
    pub gamma_rgb: [f64; 3],
    pub gamma: f64,
    pub luminance: f64,
    pub power: f64,
}

impl Default for SoftEdge {
    fn default() -> Self {
        Self {
            gamma_rgb: [2.0; 3],
            gamma: 1.0,
            luminance: 0.5,
            power: 2.0,
        }
    }
}

/// Opacity and color correction; offsets are in -1..1, 0 is neutral.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ColorAdjust {
    pub opacity: f64,
    pub brightness: f64,
    pub contrast: f64,
    pub rgb: [f64; 3],
}

impl Default for ColorAdjust {
    fn default() -> Self {
        Self {
            opacity: 1.0,
            brightness: 0.0,
            contrast: 0.0,
            rgb: [0.0; 3],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeviceKind {
    /// Not assigned to any output yet.
    None,
    /// A monitor or projector, full screen.
    Display,
    Ndi,
    Spout,
    /// Rendered but not sent anywhere.
    Virtual,
    /// DMX pixel mapping (Art-Net / sACN).
    Dmx,
}

impl DeviceKind {
    pub fn label(self) -> &'static str {
        match self {
            DeviceKind::None => "No output",
            DeviceKind::Display => "Display",
            DeviceKind::Ndi => "NDI",
            DeviceKind::Spout => "Spout",
            DeviceKind::Virtual => "Virtual",
            DeviceKind::Dmx => "DMX",
        }
    }
}

/// Where a screen is sent. Imported devices keep their name, but the user
/// picks the actual monitor again: monitor ids differ between machines.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Device {
    pub kind: DeviceKind,
    pub name: String,
    /// Output resolution, when the device sets one.
    pub size: Option<(u32, u32)>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Screen {
    pub id: u64,
    pub name: String,
    pub enabled: bool,
    /// Hidden from the editor, still rendered.
    pub hidden: bool,
    pub device: Device,
    pub color: ColorAdjust,
    pub slices: Vec<Slice>,
    /// A DMX lumiverse rather than a video screen.
    pub dmx: bool,
}

impl Screen {
    /// Output resolution: the device's, else the extent of the slices.
    pub fn size(&self, canvas: (u32, u32)) -> (u32, u32) {
        if let Some(size) = self.device.size {
            return size;
        }
        let mut max = [0.0f32; 2];
        for slice in &self.slices {
            for p in slice.outline() {
                max = [max[0].max(p[0]), max[1].max(p[1])];
            }
        }
        if max[0] < 1.0 || max[1] < 1.0 {
            return canvas;
        }
        (max[0].ceil() as u32, max[1].ceil() as u32)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SliceKind {
    /// Rectangle in, warped quad out.
    Slice,
    /// Free polygon in and out.
    Polygon,
    /// Pixels sampled for a DMX fixture.
    Dmx,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Slice {
    pub id: u64,
    pub name: String,
    pub enabled: bool,
    pub kind: SliceKind,
    /// What the slice shows, as a Resolume input source: `0:1` is the
    /// composition, `1:n` layer n, `3:n` group n.
    pub source: String,
    /// Area of the composition, in composition pixels.
    pub input: Quad,
    /// Rotation of the input area in radians.
    pub input_rotation: f32,
    /// Placement on the screen, in screen pixels, before warping.
    pub output: Quad,
    /// Corner pin and grid applied after `output`, when the slice has them.
    pub warp: Option<Warp>,
    /// Outlines of a polygon slice.
    pub contour: Option<Contour>,
    /// Bit 0 flips horizontally, bit 1 vertically.
    pub flip: u8,
    /// `opacity` is unused on slices.
    pub color: ColorAdjust,
    pub soft_edge: bool,
}

impl Slice {
    /// A slice with a neutral warp.
    pub fn new(id: u64, name: &str, input: Quad, output: Quad) -> Self {
        Self {
            id,
            name: name.to_owned(),
            enabled: true,
            kind: SliceKind::Slice,
            source: "0:1".into(),
            input,
            input_rotation: 0.0,
            output,
            warp: Some(Warp::identity(output, 4, 4)),
            contour: None,
            flip: 0,
            color: ColorAdjust::default(),
            soft_edge: false,
        }
    }

    /// A polygon slice cutting `input` (composition pixels) and drawing it
    /// at `output` (screen pixels), point for point.
    pub fn polygon(id: u64, name: &str, input: Vec<[f32; 2]>, output: Vec<[f32; 2]>) -> Self {
        Self {
            kind: SliceKind::Polygon,
            warp: None,
            ..Self::new(id, name, bounds_quad(&input), bounds_quad(&output))
        }
        .with_contour(Contour { input, output })
    }

    fn with_contour(mut self, contour: Contour) -> Self {
        self.contour = Some(contour);
        self
    }

    /// Outline drawn on the screen: the corner pin, the output quad, or the
    /// polygon.
    pub fn screen_outline(&self) -> Vec<[f32; 2]> {
        self.outline()
    }

    /// Outline cut from the composition.
    pub fn input_outline(&self) -> Vec<[f32; 2]> {
        match (&self.kind, &self.contour) {
            (SliceKind::Polygon, Some(contour)) => contour.input.clone(),
            _ => rotate(&self.input, self.input_rotation).to_vec(),
        }
    }

    /// Moves the slice on its screen by `delta` pixels.
    pub fn translate_output(&mut self, delta: [f32; 2]) {
        let shift = |p: &mut [f32; 2]| {
            p[0] += delta[0];
            p[1] += delta[1];
        };
        self.output.iter_mut().for_each(shift);
        if let Some(warp) = &mut self.warp {
            warp.corners.iter_mut().for_each(shift);
        }
        if let Some(contour) = &mut self.contour {
            contour.output.iter_mut().for_each(shift);
        }
    }

    /// Moves the area cut from the composition by `delta` pixels.
    pub fn translate_input(&mut self, delta: [f32; 2]) {
        let shift = |p: &mut [f32; 2]| {
            p[0] += delta[0];
            p[1] += delta[1];
        };
        self.input.iter_mut().for_each(shift);
        if let Some(contour) = &mut self.contour {
            contour.input.iter_mut().for_each(shift);
        }
    }

    /// Moves point `index` of the screen outline to `p`. Corner pins keep
    /// their output rectangle as their bounds, as Arena saves them.
    pub fn set_output_point(&mut self, index: usize, p: [f32; 2]) {
        match (&self.kind, &mut self.contour, &mut self.warp) {
            (SliceKind::Polygon, Some(contour), _) => {
                if let Some(point) = contour.output.get_mut(index) {
                    *point = p;
                }
            }
            (_, _, Some(warp)) if index < 4 => {
                warp.corners[index] = p;
                self.output = bounds_quad(&warp.corners);
            }
            _ if index < 4 => self.output[index] = p,
            _ => {}
        }
    }

    /// Moves point `index` of the input outline to `p`. A rectangle input
    /// stays a rectangle: the opposite corner holds.
    pub fn set_input_point(&mut self, index: usize, p: [f32; 2]) {
        match (&self.kind, &mut self.contour) {
            (SliceKind::Polygon, Some(contour)) => {
                if let Some(point) = contour.input.get_mut(index) {
                    *point = p;
                }
            }
            _ if index < 4 => {
                let fixed = self.input[(index + 2) % 4];
                self.input = bounds_quad(&[fixed, p]);
            }
            _ => {}
        }
    }
}

/// Axis-aligned quad around `points`, clockwise from top left.
fn bounds_quad(points: &[[f32; 2]]) -> Quad {
    let (mut min, mut max) = ([f32::MAX; 2], [f32::MIN; 2]);
    for p in points {
        min = [min[0].min(p[0]), min[1].min(p[1])];
        max = [max[0].max(p[0]), max[1].max(p[1])];
    }
    [min, [max[0], min[1]], max, [min[0], max[1]]]
}

/// Image a slice cuts from. Indices start at 0; Resolume numbers layers
/// from 1 at the bottom, as Verso's `Composition::layers` are ordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InputSource {
    Composition,
    Layer(usize),
    Group(usize),
}

impl InputSource {
    /// The Resolume source string, inverse of [`Slice::input_source`].
    pub fn code(self) -> String {
        match self {
            InputSource::Composition => "0:1".into(),
            InputSource::Layer(i) => format!("1:{}", i + 1),
            InputSource::Group(i) => format!("3:{}", i + 1),
        }
    }
}

/// A vertex of a slice mesh: position in screen pixels, and where it samples
/// the composition, from 0 to 1 with the origin at the top left.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeshVertex {
    pub position: [f32; 2],
    pub uv: [f32; 2],
}

/// Cells per side of a warped slice mesh, at least.
const MESH_CELLS: u32 = 16;

impl Slice {
    /// Triangles drawing the slice on its screen; empty for DMX slices and
    /// broken contours. `canvas` is the composition size the input refers to.
    ///
    /// A slice maps its input rectangle onto the Bézier grid of its warper,
    /// whose points lie in the space of the corner pin source, then the
    /// corner pin maps that space onto the screen. Grid points are joined
    /// linearly, as Arena's default point mode does.
    pub fn mesh(&self, canvas: (u32, u32)) -> Vec<MeshVertex> {
        let scale = [1.0 / canvas.0.max(1) as f32, 1.0 / canvas.1.max(1) as f32];
        match self.kind {
            SliceKind::Dmx => Vec::new(),
            SliceKind::Polygon => {
                let Some(contour) = &self.contour else {
                    return Vec::new();
                };
                if contour.input.len() != contour.output.len() {
                    return Vec::new();
                }
                triangulate(&contour.output)
                    .into_iter()
                    .map(|i| MeshVertex {
                        position: contour.output[i],
                        uv: [
                            contour.input[i][0] * scale[0],
                            contour.input[i][1] * scale[1],
                        ],
                    })
                    .collect()
            }
            SliceKind::Slice => {
                let input = rotate(&self.input, self.input_rotation);
                // Grid points fall on mesh vertices.
                let cells = |grid: u32| match grid.saturating_sub(1) {
                    0 => MESH_CELLS,
                    spans => spans * MESH_CELLS.div_ceil(spans),
                };
                let (columns, rows) = match &self.warp {
                    Some(w) => (cells(w.grid.0), cells(w.grid.1)),
                    None => (1, 1),
                };
                let pin = self
                    .warp
                    .as_ref()
                    .map(|w| Homography::new(&w.pin_from, &w.corners));
                let point = |u: f32, v: f32| {
                    let su = if self.flip & 1 != 0 { 1.0 - u } else { u };
                    let sv = if self.flip & 2 != 0 { 1.0 - v } else { v };
                    let source = bilinear(&input, su, sv);
                    let position = match (&self.warp, &pin) {
                        (Some(warp), Some(pin)) => pin.apply(warp.eval(u, v)),
                        _ => bilinear(&self.output, u, v),
                    };
                    MeshVertex {
                        position,
                        uv: [source[0] * scale[0], source[1] * scale[1]],
                    }
                };
                let mut vertices = Vec::with_capacity((columns * rows * 6) as usize);
                let (du, dv) = (1.0 / columns as f32, 1.0 / rows as f32);
                for row in 0..rows {
                    for column in 0..columns {
                        let (u0, v0) = (column as f32 * du, row as f32 * dv);
                        let (u1, v1) = (u0 + du, v0 + dv);
                        let (a, b, c, d) =
                            (point(u0, v0), point(u1, v0), point(u1, v1), point(u0, v1));
                        vertices.extend([a, b, c, a, c, d]);
                    }
                }
                vertices
            }
        }
    }

    /// What the slice shows, from [`Slice::source`]. Unknown sources show
    /// the composition.
    pub fn input_source(&self) -> InputSource {
        let parse = |n: &str| n.parse::<usize>().ok().and_then(|n| n.checked_sub(1));
        match self.source.split_once(':') {
            Some(("1", n)) => parse(n).map_or(InputSource::Composition, InputSource::Layer),
            Some(("3", n)) => parse(n).map_or(InputSource::Composition, InputSource::Group),
            _ => InputSource::Composition,
        }
    }

    /// Outline on the screen, to size it.
    fn outline(&self) -> Vec<[f32; 2]> {
        match (&self.kind, &self.contour, &self.warp) {
            (SliceKind::Polygon, Some(contour), _) => contour.output.clone(),
            (_, _, Some(warp)) => warp.corners.to_vec(),
            _ => self.output.to_vec(),
        }
    }
}

fn lerp(a: [f32; 2], b: [f32; 2], t: f32) -> [f32; 2] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
}

/// Point at `u`, `v` (0 to 1) of a quad, clockwise from top left.
fn bilinear(quad: &Quad, u: f32, v: f32) -> [f32; 2] {
    lerp(lerp(quad[0], quad[1], u), lerp(quad[3], quad[2], u), v)
}

/// `quad` turned by `angle` radians about its center.
fn rotate(quad: &Quad, angle: f32) -> Quad {
    if angle == 0.0 {
        return *quad;
    }
    let center = quad
        .iter()
        .fold([0.0, 0.0], |c, p| [c[0] + p[0] / 4.0, c[1] + p[1] / 4.0]);
    let (sin, cos) = angle.sin_cos();
    quad.map(|[x, y]| {
        let (dx, dy) = (x - center[0], y - center[1]);
        [
            center[0] + dx * cos - dy * sin,
            center[1] + dx * sin + dy * cos,
        ]
    })
}

/// Projective map taking one quad onto another (a corner pin).
struct Homography([f64; 9]);

impl Homography {
    fn new(from: &Quad, to: &Quad) -> Self {
        let forward = square_to_quad(to);
        match invert(&square_to_quad(from)) {
            Some(inverse) => Self(multiply(&forward, &inverse)),
            // A degenerate source: leave the points where they are.
            None => Self([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]),
        }
    }

    fn apply(&self, [x, y]: [f32; 2]) -> [f32; 2] {
        let m = &self.0;
        let (x, y) = (x as f64, y as f64);
        let w = m[6] * x + m[7] * y + m[8];
        let w = if w.abs() < 1e-12 { 1e-12 } else { w };
        [
            ((m[0] * x + m[1] * y + m[2]) / w) as f32,
            ((m[3] * x + m[4] * y + m[5]) / w) as f32,
        ]
    }
}

/// Row-major 3×3 matrix mapping the unit square onto `q` (Heckbert).
fn square_to_quad(q: &Quad) -> [f64; 9] {
    let [x0, y0] = q[0].map(f64::from);
    let [x1, y1] = q[1].map(f64::from);
    let [x2, y2] = q[2].map(f64::from);
    let [x3, y3] = q[3].map(f64::from);
    let (sx, sy) = (x0 - x1 + x2 - x3, y0 - y1 + y2 - y3);
    if sx.abs() < 1e-9 && sy.abs() < 1e-9 {
        // Parallelogram: affine.
        return [x1 - x0, x3 - x0, x0, y1 - y0, y3 - y0, y0, 0.0, 0.0, 1.0];
    }
    let (dx1, dx2, dy1, dy2) = (x1 - x2, x3 - x2, y1 - y2, y3 - y2);
    let det = dx1 * dy2 - dx2 * dy1;
    let det = if det.abs() < 1e-12 { 1e-12 } else { det };
    let g = (sx * dy2 - dx2 * sy) / det;
    let h = (dx1 * sy - sx * dy1) / det;
    [
        x1 - x0 + g * x1,
        x3 - x0 + h * x3,
        x0,
        y1 - y0 + g * y1,
        y3 - y0 + h * y3,
        y0,
        g,
        h,
        1.0,
    ]
}

fn multiply(a: &[f64; 9], b: &[f64; 9]) -> [f64; 9] {
    std::array::from_fn(|i| {
        let (row, column) = (i / 3, i % 3);
        (0..3).map(|k| a[row * 3 + k] * b[k * 3 + column]).sum()
    })
}

fn invert(m: &[f64; 9]) -> Option<[f64; 9]> {
    let [a, b, c, d, e, f, g, h, i] = *m;
    let det = a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g);
    if det.abs() < 1e-12 {
        return None;
    }
    let inv = 1.0 / det;
    Some([
        (e * i - f * h) * inv,
        (c * h - b * i) * inv,
        (b * f - c * e) * inv,
        (f * g - d * i) * inv,
        (a * i - c * g) * inv,
        (c * d - a * f) * inv,
        (d * h - e * g) * inv,
        (b * g - a * h) * inv,
        (a * e - b * d) * inv,
    ])
}

/// Triangles of a simple polygon by ear clipping, as indices into `points`.
fn triangulate(points: &[[f32; 2]]) -> Vec<usize> {
    let n = points.len();
    if n < 3 {
        return Vec::new();
    }
    let cross = |o: [f32; 2], a: [f32; 2], b: [f32; 2]| {
        (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
    };
    let area: f32 = (0..n)
        .map(|i| {
            let (a, b) = (points[i], points[(i + 1) % n]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum();
    // Ears turn the same way as the polygon.
    let sign = if area >= 0.0 { 1.0 } else { -1.0 };
    let mut remaining: Vec<usize> = (0..n).collect();
    let mut triangles = Vec::with_capacity((n - 2) * 3);
    while remaining.len() > 3 {
        let m = remaining.len();
        let corner = |k: usize| {
            (
                remaining[(k + m - 1) % m],
                remaining[k],
                remaining[(k + 1) % m],
            )
        };
        let ear = (0..m).find(|&k| {
            let (a, b, c) = corner(k);
            let (pa, pb, pc) = (points[a], points[b], points[c]);
            if cross(pa, pb, pc) * sign <= 0.0 {
                return false;
            }
            remaining.iter().all(|&p| {
                p == a
                    || p == b
                    || p == c
                    || cross(pa, pb, points[p]) * sign < 0.0
                    || cross(pb, pc, points[p]) * sign < 0.0
                    || cross(pc, pa, points[p]) * sign < 0.0
            })
        });
        // Self-intersecting outline: clip anyway rather than loop forever.
        let k = ear.unwrap_or(0);
        let (a, b, c) = corner(k);
        triangles.extend([a, b, c]);
        remaining.remove(k);
    }
    triangles.extend_from_slice(&remaining);
    triangles
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Warp {
    /// Corner pin: `pin_from` mapped onto `corners`. `pin_from` is usually
    /// the slice output, but Arena keeps it when the output is moved.
    pub pin_from: Quad,
    pub corners: Quad,
    /// Grid columns and rows of the Bézier warp.
    pub grid: (u32, u32),
    /// Grid points, row by row, in screen pixels.
    pub points: Vec<[f32; 2]>,
}

impl Warp {
    /// No deformation: the corners are `output` and the grid is spread
    /// evenly over it.
    pub fn identity(output: Quad, columns: u32, rows: u32) -> Self {
        let lerp =
            |a: [f32; 2], b: [f32; 2], t: f32| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
        let mut points = Vec::with_capacity((columns * rows) as usize);
        for row in 0..rows {
            let v = row as f32 / (rows - 1).max(1) as f32;
            let left = lerp(output[0], output[3], v);
            let right = lerp(output[1], output[2], v);
            for column in 0..columns {
                points.push(lerp(
                    left,
                    right,
                    column as f32 / (columns - 1).max(1) as f32,
                ));
            }
        }
        Self {
            pin_from: output,
            corners: output,
            grid: (columns, rows),
            points,
        }
    }
}

impl Warp {
    /// Point of the grid at `u`, `v` (0 to 1), joining grid points linearly.
    /// Without a usable grid, the corner pin source.
    fn eval(&self, u: f32, v: f32) -> [f32; 2] {
        let (columns, rows) = (self.grid.0 as usize, self.grid.1 as usize);
        if columns < 2 || rows < 2 || self.points.len() != columns * rows {
            return bilinear(&self.pin_from, u, v);
        }
        let (fx, fy) = (u * (columns - 1) as f32, v * (rows - 1) as f32);
        let (cx, cy) = (
            (fx.floor() as usize).min(columns - 2),
            (fy.floor() as usize).min(rows - 2),
        );
        let at = |x: usize, y: usize| self.points[y * columns + x];
        let cell = [
            at(cx, cy),
            at(cx + 1, cy),
            at(cx + 1, cy + 1),
            at(cx, cy + 1),
        ];
        bilinear(&cell, fx - cx as f32, fy - cy as f32)
    }

    /// Where a point of the grid space lands on the screen.
    pub fn to_screen(&self, p: [f32; 2]) -> [f32; 2] {
        Homography::new(&self.pin_from, &self.corners).apply(p)
    }

    /// The grid space point that lands on screen point `p`.
    pub fn from_screen(&self, p: [f32; 2]) -> [f32; 2] {
        Homography::new(&self.corners, &self.pin_from).apply(p)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Contour {
    pub input: Vec<[f32; 2]>,
    pub output: Vec<[f32; 2]>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_stay_unique() {
        let mut setup = OutputSetup::default();
        let a = setup.add_screen("A", (1920, 1080));
        let b = setup.add_screen("B", (1280, 720));
        let ids: Vec<u64> = setup
            .screens
            .iter()
            .flat_map(|s| std::iter::once(s.id).chain(s.slices.iter().map(|l| l.id)))
            .collect();
        assert_eq!(ids, [a, a + 1, b, b + 1]);
        assert!(b > a + 1);
        assert_eq!(setup.slice_count(), 2);
    }

    #[test]
    fn identity_warp_spreads_grid() {
        let warp = Warp::identity(rect(300.0, 30.0), 4, 2);
        assert_eq!(warp.points.len(), 8);
        assert_eq!(warp.points[1], [100.0, 0.0]);
        assert_eq!(warp.points[7], [300.0, 30.0]);
    }

    fn close(a: [f32; 2], b: [f32; 2]) -> bool {
        (a[0] - b[0]).abs() < 0.01 && (a[1] - b[1]).abs() < 0.01
    }

    #[test]
    fn plain_slice_maps_input_onto_output() {
        let slice = Slice::new(1, "S", rect(960.0, 1080.0), rect(1920.0, 1080.0));
        let mesh = slice.mesh((1920, 1080));
        // 4×4 grid: 3 spans of 6 cells per side.
        assert_eq!(mesh.len(), 18 * 18 * 6);
        let corner = mesh
            .iter()
            .find(|v| close(v.position, [1920.0, 1080.0]))
            .unwrap();
        assert!(close(corner.uv, [0.5, 1.0]));
    }

    #[test]
    fn corner_pin_moves_corners_from_stale_source() {
        // As Arena saves it: grid and pin source in an older space.
        let source = [
            [0.0, -11.0],
            [1920.0, -11.0],
            [1920.0, 1189.0],
            [0.0, 1189.0],
        ];
        let corners = [
            [257.5, 251.6],
            [1512.6, 251.3],
            [1509.3, 1140.1],
            [318.4, 1157.4],
        ];
        let mut slice = Slice::new(1, "S", rect(1920.0, 1200.0), rect(100.0, 100.0));
        let mut warp = Warp::identity(source, 4, 4);
        warp.corners = corners;
        slice.warp = Some(warp);
        let mesh = slice.mesh((1920, 1200));
        let uvs = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        for (corner, uv) in corners.iter().zip(uvs) {
            let v = mesh.iter().find(|v| close(v.position, *corner)).unwrap();
            assert!(close(v.uv, uv), "{v:?}");
        }
        assert_eq!(setup_size(&slice), (1513, 1158));
    }

    fn setup_size(slice: &Slice) -> (u32, u32) {
        let mut setup = OutputSetup::default();
        setup.add_screen("A", (1, 1));
        setup.screens[0].device.size = None;
        setup.screens[0].slices = vec![slice.clone()];
        setup.screens[0].size((1920, 1080))
    }

    #[test]
    fn grid_warp_moves_inner_points() {
        let mut slice = Slice::new(1, "S", rect(300.0, 300.0), rect(300.0, 300.0));
        let warp = slice.warp.as_mut().unwrap();
        warp.points[5] = [150.0, 150.0]; // was (100, 100)
        let mesh = slice.mesh((300, 300));
        let v = mesh
            .iter()
            .find(|v| close(v.uv, [1.0 / 3.0, 1.0 / 3.0]))
            .unwrap();
        assert!(close(v.position, [150.0, 150.0]), "{v:?}");
    }

    #[test]
    fn flip_mirrors_the_input() {
        let mut slice = Slice::new(1, "S", rect(100.0, 100.0), rect(100.0, 100.0));
        slice.flip = 1;
        let mesh = slice.mesh((100, 100));
        let v = mesh.iter().find(|v| close(v.position, [0.0, 0.0])).unwrap();
        assert!(close(v.uv, [1.0, 0.0]));
    }

    #[test]
    fn concave_polygon_triangulates() {
        // An L shape: 6 points, 4 triangles, area preserved.
        let points = [
            [0.0, 0.0],
            [2.0, 0.0],
            [2.0, 1.0],
            [1.0, 1.0],
            [1.0, 2.0],
            [0.0, 2.0],
        ];
        let triangles = triangulate(&points);
        assert_eq!(triangles.len(), 12);
        let area: f32 = triangles
            .chunks(3)
            .map(|t| {
                let (a, b, c) = (points[t[0]], points[t[1]], points[t[2]]);
                ((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])).abs() / 2.0
            })
            .sum();
        assert!((area - 3.0).abs() < 1e-5);
    }

    #[test]
    fn screen_size_from_device_or_slices() {
        let mut setup = OutputSetup::default();
        setup.add_screen("A", (1280, 720));
        let screen = &mut setup.screens[0];
        assert_eq!(screen.size((1920, 1080)), (1280, 720));
        screen.device.size = None;
        assert_eq!(screen.size((1920, 1080)), (1280, 720));
    }

    #[test]
    fn editing_keeps_shapes_consistent() {
        let mut slice = Slice::new(1, "S", rect(100.0, 50.0), rect(200.0, 100.0));
        // A rectangle input stays a rectangle, anchored at the far corner.
        slice.set_input_point(2, [60.0, 40.0]);
        assert_eq!(slice.input, rect(60.0, 40.0));
        slice.translate_input([10.0, 5.0]);
        assert_eq!(slice.input[0], [10.0, 5.0]);

        // Output corners pin the warp; the output rectangle bounds them.
        slice.set_output_point(1, [250.0, -10.0]);
        let warp = slice.warp.clone().unwrap();
        assert_eq!(warp.corners[1], [250.0, -10.0]);
        assert_eq!(slice.output[0], [0.0, -10.0]);
        assert_eq!(slice.output[2], [250.0, 100.0]);
        let p = [50.0, 30.0];
        let back = warp.from_screen(warp.to_screen(p));
        assert!(close(back, p), "{back:?}");
        slice.translate_output([5.0, 5.0]);
        assert_eq!(slice.warp.unwrap().corners[1], [255.0, -5.0]);

        let mut polygon = Slice::polygon(
            2,
            "P",
            vec![[0.0, 0.0], [10.0, 0.0], [5.0, 8.0]],
            vec![[0.0, 0.0], [20.0, 0.0], [10.0, 16.0]],
        );
        assert_eq!(polygon.input[2], [10.0, 8.0]);
        polygon.set_output_point(2, [10.0, 30.0]);
        assert_eq!(polygon.screen_outline()[2], [10.0, 30.0]);
        assert_eq!(polygon.mesh((10, 8)).len(), 3);
    }

    #[test]
    fn input_sources_parse() {
        let mut slice = Slice::new(1, "S", rect(1.0, 1.0), rect(1.0, 1.0));
        assert_eq!(slice.input_source(), InputSource::Composition);
        for (code, source) in [
            ("1:3", InputSource::Layer(2)),
            ("3:1", InputSource::Group(0)),
            ("1:0", InputSource::Composition),
            ("2:4", InputSource::Composition),
        ] {
            slice.source = code.into();
            assert_eq!(slice.input_source(), source, "{code}");
        }
        assert_eq!(InputSource::Layer(2).code(), "1:3");
    }

    #[test]
    fn serializes() {
        let mut setup = OutputSetup::default();
        setup.add_screen("Main", (1920, 1080));
        let text = ron::to_string(&setup).unwrap();
        assert_eq!(ron::from_str::<OutputSetup>(&text).unwrap(), setup);
    }
}
