//! SVG shapes as polygon slices, like Arena's "Import SVG": every closed
//! outline of the drawing becomes a polygon. Curves are flattened.

use usvg::tiny_skia_path::{PathSegment, Point};

use crate::{Error, Result};

/// Line segments per curve.
const CURVE_STEPS: usize = 12;

/// Outlines of an SVG drawing, in its own pixels, with its size.
#[derive(Debug, Clone, PartialEq)]
pub struct Drawing {
    pub size: (f32, f32),
    pub polygons: Vec<Vec<[f32; 2]>>,
}

pub fn read(text: &str) -> Result<Drawing> {
    let tree = usvg::Tree::from_str(text, &usvg::Options::default())
        .map_err(|e| Error::Format(format!("SVG: {e}")))?;
    let mut polygons = Vec::new();
    collect(tree.root(), &mut polygons);
    if polygons.is_empty() {
        return Err(Error::Format("SVG: no shapes found".into()));
    }
    let size = tree.size();
    Ok(Drawing {
        size: (size.width(), size.height()),
        polygons,
    })
}

fn collect(group: &usvg::Group, polygons: &mut Vec<Vec<[f32; 2]>>) {
    for node in group.children() {
        match node {
            usvg::Node::Group(group) => collect(group, polygons),
            usvg::Node::Path(path) if path.is_visible() => {
                let transform = path.abs_transform();
                for mut outline in outlines(path.data()) {
                    transform.map_points(&mut outline);
                    let points: Vec<[f32; 2]> = outline.iter().map(|p| [p.x, p.y]).collect();
                    if points.len() >= 3 {
                        polygons.push(points);
                    }
                }
            }
            _ => {}
        }
    }
}

/// Each subpath as a list of points, without repeating the first one.
fn outlines(path: &usvg::tiny_skia_path::Path) -> Vec<Vec<Point>> {
    let mut all = Vec::new();
    let mut current: Vec<Point> = Vec::new();
    let mut finish = |current: &mut Vec<Point>| {
        if current.len() > 1 && current.first() == current.last() {
            current.pop();
        }
        if !current.is_empty() {
            all.push(std::mem::take(current));
        }
    };
    for segment in path.segments() {
        let last = current.last().copied().unwrap_or_default();
        match segment {
            PathSegment::MoveTo(p) => {
                finish(&mut current);
                current.push(p);
            }
            PathSegment::LineTo(p) => current.push(p),
            PathSegment::QuadTo(c, p) => current.extend((1..=CURVE_STEPS).map(|i| {
                let t = i as f32 / CURVE_STEPS as f32;
                let u = 1.0 - t;
                Point::from_xy(
                    u * u * last.x + 2.0 * u * t * c.x + t * t * p.x,
                    u * u * last.y + 2.0 * u * t * c.y + t * t * p.y,
                )
            })),
            PathSegment::CubicTo(c1, c2, p) => current.extend((1..=CURVE_STEPS).map(|i| {
                let t = i as f32 / CURVE_STEPS as f32;
                let u = 1.0 - t;
                let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
                Point::from_xy(
                    a * last.x + b * c1.x + c * c2.x + d * p.x,
                    a * last.y + b * c1.y + c * c2.y + d * p.y,
                )
            })),
            PathSegment::Close => finish(&mut current),
        }
    }
    finish(&mut current);
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes_become_polygons() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100">
            <rect x="10" y="10" width="50" height="30" fill="red"/>
            <g transform="translate(100 0)">
                <polygon points="0,0 40,0 20,40" fill="blue"/>
            </g>
            <circle cx="150" cy="70" r="20" fill="green"/>
        </svg>"#;
        let drawing = read(svg).unwrap();
        assert_eq!(drawing.size, (200.0, 100.0));
        assert_eq!(drawing.polygons.len(), 3);
        assert_eq!(drawing.polygons[0].len(), 4);
        assert!(drawing.polygons[0].contains(&[60.0, 40.0]));
        assert_eq!(
            drawing.polygons[1],
            [[100.0, 0.0], [140.0, 0.0], [120.0, 40.0]]
        );
        // The circle is flattened, and stays on its radius.
        let circle = &drawing.polygons[2];
        assert!(circle.len() > 16);
        for p in circle {
            let r = ((p[0] - 150.0).powi(2) + (p[1] - 70.0).powi(2)).sqrt();
            assert!((r - 20.0).abs() < 0.5, "{p:?}");
        }
    }

    #[test]
    fn empty_drawing_is_an_error() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"/>"#;
        assert!(read(svg).is_err());
        assert!(read("not svg").is_err());
    }
}
