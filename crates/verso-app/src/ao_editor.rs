//! Slice editor of the Advanced Output window, in two views as in Arena:
//!
//! - Input selection: the slices over the composition. Drag a slice to move
//!   the area it cuts, a corner to resize it, a polygon point to reshape it.
//! - Output transformation: the slices on the screen. Drag a slice to place
//!   it, a corner to pin it, and with "Warp grid" the points of its grid.
//!
//! Polygons are drawn point by point in either view, or imported from SVG.

use eframe::egui::{self, Color32, Pos2, Rect, Sense, Shape, Stroke};
use verso_core::output::{Slice, SliceKind};

use crate::advanced_output_ui::EditorView;
use crate::app::VersoApp;
use crate::dialogs::DialogKind;
use crate::theme;

/// Pointer distance, in UI points, that grabs a point.
const GRAB: f32 = 8.0;

/// What the pointer is moving.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Drag {
    /// A corner or polygon point of the outline in the current view.
    Point { slice: u64, index: usize },
    /// A point of the warp grid, in the output view.
    Grid { slice: u64, index: usize },
    /// The whole slice; `last` is the previous pointer position in pixels.
    Body { slice: u64, last: [f32; 2] },
}

/// Pixels of the edited space to UI points, fitted in the editor area.
struct Mapping {
    origin: Pos2,
    scale: f32,
}

impl Mapping {
    fn fit(area: Rect, size: (u32, u32)) -> Self {
        let (w, h) = (size.0.max(1) as f32, size.1.max(1) as f32);
        let scale = ((area.width() - 24.0) / w)
            .min((area.height() - 24.0) / h)
            .max(0.001);
        Self {
            origin: area.center() - egui::vec2(w * scale, h * scale) / 2.0,
            scale,
        }
    }

    fn to_point(&self, p: [f32; 2]) -> Pos2 {
        self.origin + egui::vec2(p[0], p[1]) * self.scale
    }

    fn to_pixels(&self, p: Pos2) -> [f32; 2] {
        let v = (p - self.origin) / self.scale;
        [v.x, v.y]
    }
}

impl VersoApp {
    pub(crate) fn ao_editor(&mut self, ui: &mut egui::Ui) {
        self.ao_editor_bar(ui);
        let ao = &self.advanced;
        let view = ao.view;
        let space = match view {
            EditorView::Input => ao.setup.canvas,
            EditorView::Output => ao.screen_size(),
        };
        let background = match view {
            EditorView::Input => Some(self.output_texture),
            EditorView::Output if ao.live => ao
                .screen
                .and_then(|id| self.outputs.screen_textures.get(&id).copied()),
            EditorView::Output => None,
        };

        let (area, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        let painter = ui.painter_at(area);
        painter.rect_filled(area, 0.0, theme::bg_deep());
        let map = Mapping::fit(area, space);
        let frame = Rect::from_min_max(
            map.to_point([0.0, 0.0]),
            map.to_point([space.0 as f32, space.1 as f32]),
        );
        match background {
            Some(texture) => {
                painter.image(
                    texture,
                    frame,
                    Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
            None => {
                painter.rect_filled(frame, 0.0, Color32::BLACK);
            }
        }
        painter.rect_stroke(
            frame,
            0.0,
            Stroke::new(1.0, theme::line()),
            egui::StrokeKind::Outside,
        );

        // Outlines of every slice of the screen, in this view.
        let slices: Vec<(u64, String, Vec<[f32; 2]>)> = ao
            .selected_screen()
            .map(|screen| {
                screen
                    .slices
                    .iter()
                    .map(|s| (s.id, s.name.clone(), outline(s, view)))
                    .collect()
            })
            .unwrap_or_default();
        let selected = ao.slice;
        for (id, name, points) in slices.iter().rev() {
            let picked = Some(*id) == selected;
            let color = if picked {
                theme::accent()
            } else {
                theme::text_dim()
            };
            let ui_points: Vec<Pos2> = points.iter().map(|&p| map.to_point(p)).collect();
            painter.add(Shape::closed_line(
                ui_points.clone(),
                Stroke::new(if picked { 2.0 } else { 1.0 }, color),
            ));
            if !ui_points.is_empty() {
                let center = ui_points
                    .iter()
                    .fold(egui::Vec2::ZERO, |c, p| c + p.to_vec2())
                    / ui_points.len() as f32;
                painter.text(
                    center.to_pos2(),
                    egui::Align2::CENTER_CENTER,
                    name,
                    egui::FontId::proportional(12.0),
                    color,
                );
            }
        }

        // Handles of the selected slice.
        let selected_slice = selected.and_then(|id| {
            ao.selected_screen()?
                .slices
                .iter()
                .find(|s| s.id == id)
                .cloned()
        });
        let mut handles: Vec<(Drag, Pos2)> = Vec::new();
        if let Some(slice) = &selected_slice {
            if view == EditorView::Output
                && ao.edit_grid
                && let Some(warp) = &slice.warp
            {
                let (columns, rows) = (warp.grid.0 as usize, warp.grid.1 as usize);
                let grid: Vec<Pos2> = warp
                    .points
                    .iter()
                    .map(|&p| map.to_point(warp.to_screen(p)))
                    .collect();
                if grid.len() == columns * rows {
                    let stroke = Stroke::new(1.0, theme::warn().gamma_multiply(0.6));
                    for row in 0..rows {
                        painter.add(Shape::line(
                            grid[row * columns..(row + 1) * columns].to_vec(),
                            stroke,
                        ));
                    }
                    for column in 0..columns {
                        let line = (0..rows).map(|r| grid[r * columns + column]).collect();
                        painter.add(Shape::line(line, stroke));
                    }
                }
                for (index, &p) in grid.iter().enumerate() {
                    painter.circle(p, 3.5, theme::warn(), Stroke::new(1.0, Color32::BLACK));
                    handles.push((
                        Drag::Grid {
                            slice: slice.id,
                            index,
                        },
                        p,
                    ));
                }
            }
            for (index, &p) in outline(slice, view).iter().enumerate() {
                let p = map.to_point(p);
                painter.rect_filled(
                    Rect::from_center_size(p, egui::vec2(8.0, 8.0)),
                    1.0,
                    theme::accent(),
                );
                handles.push((
                    Drag::Point {
                        slice: slice.id,
                        index,
                    },
                    p,
                ));
            }
        }

        let pointer = response.interact_pointer_pos().or(response.hover_pos());
        if self.advanced.drawing.is_some() {
            self.draw_polygon(ui, &response, &painter, &map, pointer, space);
            return;
        }

        // Corners first, then grid points, then the topmost slice under the
        // pointer.
        let hit = |pos: Pos2| -> Option<Drag> {
            let handle = handles
                .iter()
                .rev()
                .filter(|(_, p)| p.distance(pos) <= GRAB)
                .min_by(|a, b| a.1.distance(pos).total_cmp(&b.1.distance(pos)))
                .map(|(drag, _)| *drag);
            handle.or_else(|| {
                let p = map.to_pixels(pos);
                slices
                    .iter()
                    .find(|(_, _, outline)| contains(outline, p))
                    .map(|(id, _, _)| Drag::Body {
                        slice: *id,
                        last: p,
                    })
            })
        };
        // A drag is recognized once the pointer has moved: hit-test where the
        // button went down.
        let press = ui.input(|i| i.pointer.press_origin());
        if response.drag_started()
            && let Some(pos) = press.or(response.interact_pointer_pos())
        {
            self.advanced.drag = hit(pos);
            if let Some(Drag::Body { slice, .. }) = self.advanced.drag {
                self.advanced.slice = Some(slice);
            }
        } else if response.clicked()
            && let Some(pos) = response.interact_pointer_pos()
        {
            self.advanced.slice = match hit(pos) {
                Some(Drag::Body { slice, .. }) => Some(slice),
                Some(_) => self.advanced.slice,
                None => None,
            };
        }
        if response.dragged()
            && let (Some(drag), Some(pos)) = (self.advanced.drag, response.interact_pointer_pos())
        {
            let p = map.to_pixels(pos);
            self.apply_drag(drag, p, view);
        }
        if response.drag_stopped() {
            self.advanced.drag = None;
        }
        if let Some(pos) = pointer
            && response.hovered()
        {
            let [x, y] = map.to_pixels(pos);
            painter.text(
                area.left_bottom() + egui::vec2(6.0, -6.0),
                egui::Align2::LEFT_BOTTOM,
                format!("{x:.0}, {y:.0}"),
                egui::FontId::monospace(11.0),
                theme::text_dim(),
            );
        }
    }

    fn ao_editor_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let ao = &mut self.advanced;
            ui.selectable_value(&mut ao.view, EditorView::Input, "Input Selection")
                .on_hover_text("What each slice cuts from the composition");
            ui.selectable_value(&mut ao.view, EditorView::Output, "Output Transformation")
                .on_hover_text("Where each slice goes on the screen");
            if ao.view == EditorView::Output {
                ui.checkbox(&mut ao.edit_grid, "Warp grid")
                    .on_hover_text("Show and move the points of the selected slice's grid");
            }
            ui.separator();
            if ui
                .button("+ Slice")
                .on_hover_text("The whole composition on the whole screen")
                .clicked()
            {
                self.add_full_slice();
            }
            let drawing = self.advanced.drawing.is_some();
            if ui
                .selectable_label(drawing, "✏ Draw Polygon")
                .on_hover_text(
                    "Click to add points; click the first point, double-click or press \
                     Enter to close the shape; Esc cancels",
                )
                .clicked()
            {
                self.advanced.drawing = if drawing { None } else { Some(Vec::new()) };
            }
            if ui
                .button("Import SVG…")
                .on_hover_text("Each shape of the drawing becomes a polygon slice")
                .clicked()
            {
                self.dialogs.open(DialogKind::AdvancedSvg, || {
                    rfd::FileDialog::new()
                        .add_filter("SVG drawing", &["svg"])
                        .pick_file()
                });
            }
        });
    }

    fn apply_drag(&mut self, drag: Drag, p: [f32; 2], view: EditorView) {
        let id = match drag {
            Drag::Point { slice, .. } | Drag::Grid { slice, .. } | Drag::Body { slice, .. } => {
                slice
            }
        };
        let Some(slice) = self
            .advanced
            .selected_screen_mut()
            .and_then(|s| s.slices.iter_mut().find(|l| l.id == id))
        else {
            return;
        };
        match (drag, view) {
            (Drag::Point { index, .. }, EditorView::Input) => slice.set_input_point(index, p),
            (Drag::Point { index, .. }, EditorView::Output) => slice.set_output_point(index, p),
            (Drag::Grid { index, .. }, _) => {
                if let Some(warp) = &mut slice.warp {
                    let q = warp.from_screen(p);
                    if let Some(point) = warp.points.get_mut(index) {
                        *point = q;
                    }
                }
            }
            (Drag::Body { slice: id, last }, view) => {
                let delta = [p[0] - last[0], p[1] - last[1]];
                match view {
                    EditorView::Input => slice.translate_input(delta),
                    EditorView::Output => slice.translate_output(delta),
                }
                self.advanced.drag = Some(Drag::Body { slice: id, last: p });
            }
        }
    }

    fn draw_polygon(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        painter: &egui::Painter,
        map: &Mapping,
        pointer: Option<Pos2>,
        space: (u32, u32),
    ) {
        let (enter, escape) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::Enter),
                i.key_pressed(egui::Key::Escape),
            )
        });
        if escape {
            self.advanced.drawing = None;
            return;
        }
        let Some(points) = self.advanced.drawing.as_mut() else {
            return;
        };
        let mut finish = enter || response.double_clicked();
        if response.clicked()
            && !response.double_clicked()
            && let Some(pos) = response.interact_pointer_pos()
        {
            let closes = points.len() >= 3
                && points
                    .first()
                    .is_some_and(|&first| map.to_point(first).distance(pos) <= GRAB);
            if closes {
                finish = true;
            } else {
                points.push(map.to_pixels(pos));
            }
        }

        let ui_points: Vec<Pos2> = points.iter().map(|&p| map.to_point(p)).collect();
        let mut preview = ui_points.clone();
        if let Some(pos) = pointer {
            preview.push(pos);
        }
        painter.add(Shape::line(preview, Stroke::new(1.5, theme::warn())));
        for p in &ui_points {
            painter.circle_filled(*p, 3.5, theme::warn());
        }

        if !finish {
            return;
        }
        let mut points = self.advanced.drawing.take().unwrap_or_default();
        points.dedup_by(|a, b| (a[0] - b[0]).abs() < 0.5 && (a[1] - b[1]).abs() < 0.5);
        if points.len() < 3 {
            self.status = "A polygon needs at least 3 points".into();
            return;
        }
        // The shape is drawn in one view and placed at the same spot,
        // relative to the size, in the other.
        let ao = &mut self.advanced;
        let other = match ao.view {
            EditorView::Input => ao.screen_size(),
            EditorView::Output => ao.setup.canvas,
        };
        let mapped: Vec<[f32; 2]> = points
            .iter()
            .map(|p| {
                [
                    p[0] / space.0.max(1) as f32 * other.0 as f32,
                    p[1] / space.1.max(1) as f32 * other.1 as f32,
                ]
            })
            .collect();
        let (input, output) = match ao.view {
            EditorView::Input => (points, mapped),
            EditorView::Output => (mapped, points),
        };
        let id = ao.setup.next_id();
        let count = ao.selected_screen().map_or(0, |s| {
            s.slices
                .iter()
                .filter(|l| l.kind == SliceKind::Polygon)
                .count()
        });
        ao.add_slice(Slice::polygon(
            id,
            &format!("Polygon {}", count + 1),
            input,
            output,
        ));
    }
}

fn outline(slice: &Slice, view: EditorView) -> Vec<[f32; 2]> {
    match view {
        EditorView::Input => slice.input_outline(),
        EditorView::Output => slice.screen_outline(),
    }
}

/// Whether `p` is inside the polygon `outline` (even-odd rule).
fn contains(outline: &[[f32; 2]], p: [f32; 2]) -> bool {
    let mut inside = false;
    let n = outline.len();
    for i in 0..n {
        let (a, b) = (outline[i], outline[(i + n - 1) % n]);
        if (a[1] > p[1]) != (b[1] > p[1])
            && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0]
        {
            inside = !inside;
        }
    }
    inside
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn point_in_polygon() {
        let square = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        assert!(contains(&square, [5.0, 5.0]));
        assert!(!contains(&square, [15.0, 5.0]));
        let l_shape = [
            [0.0, 0.0],
            [2.0, 0.0],
            [2.0, 1.0],
            [1.0, 1.0],
            [1.0, 2.0],
            [0.0, 2.0],
        ];
        assert!(contains(&l_shape, [0.5, 1.5]));
        assert!(!contains(&l_shape, [1.5, 1.5]));
    }

    #[test]
    fn mapping_round_trips() {
        let map = Mapping::fit(
            Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(400.0, 300.0)),
            (1920, 1080),
        );
        let p = [960.0, 540.0];
        let back = map.to_pixels(map.to_point(p));
        assert!((back[0] - p[0]).abs() < 0.01 && (back[1] - p[1]).abs() < 0.01);
        // Centered in the area.
        assert!(map.to_point(p).distance(egui::pos2(210.0, 170.0)) < 0.01);
    }
}
