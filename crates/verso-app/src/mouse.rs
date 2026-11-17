//! The mouse as a control source: position, button and speed over the
//! Preview and Output monitors (and the output window when allowed), for
//! modulation and the `iMouse` input of ShaderToy shaders.

use eframe::egui::{Rect, Response};

/// Speed that reads 1: four monitor widths per second.
const FULL_SPEED: f32 = 4.0;

#[derive(Debug, Clone, Default)]
pub struct MouseControl {
    /// Pointer position, 0..1 from left to right and bottom to top. Kept
    /// when the pointer leaves.
    pos: [f32; 2],
    /// Position while the button was last down, as ShaderToy keeps it.
    drag_pos: [f32; 2],
    down: bool,
    /// Where the button last went down.
    click: [f32; 2],
    /// The button went down since the last frame.
    pressed: bool,
    /// Monitor widths per second, smoothed.
    speed: f32,
    /// Position at the previous frame, while the pointer is over an area.
    last: Option<[f32; 2]>,
    /// Distance moved this frame.
    moved: f32,
    seen: bool,
}

impl MouseControl {
    /// Follows the pointer over `image`, the part of `response` showing the
    /// frame. Call for each area the mouse may control.
    pub fn track(&mut self, response: &Response, image: Rect) {
        let held = response.is_pointer_button_down_on();
        let pointer = if held {
            response.interact_pointer_pos()
        } else {
            response.hover_pos()
        };
        let Some(p) = pointer.filter(|p| held || image.contains(*p)) else {
            return;
        };
        if image.width() <= 0.0 || image.height() <= 0.0 {
            return;
        }
        let pos = [
            ((p.x - image.left()) / image.width()).clamp(0.0, 1.0),
            1.0 - ((p.y - image.top()) / image.height()).clamp(0.0, 1.0),
        ];
        if let Some(last) = self.last {
            self.moved += (pos[0] - last[0]).hypot(pos[1] - last[1]);
        }
        self.last = Some(pos);
        self.pos = pos;
        if held {
            if !self.down {
                self.pressed = true;
                self.click = pos;
            }
            self.drag_pos = pos;
        }
        self.down = held;
        self.seen = true;
    }

    /// Ends a frame of tracking: updates the speed and forgets the pointer
    /// when no area saw it. Call once per frame, before the areas track.
    pub fn end_frame(&mut self, dt: f32) {
        let dt = dt.max(1e-3);
        let instant = self.moved / dt;
        self.speed += (instant - self.speed) * (dt * 10.0).min(1.0);
        if !self.seen {
            self.last = None;
            self.down = false;
        }
        self.seen = false;
        self.moved = 0.0;
    }

    /// Clears the press of this frame once it has been rendered.
    pub fn rendered(&mut self) {
        self.pressed = false;
    }

    /// X, Y, button and speed, each in 0..1, in `verso_core::modulation::MouseAxis` order.
    pub fn signals(&self) -> [f32; 4] {
        [
            self.pos[0],
            self.pos[1],
            if self.down { 1.0 } else { 0.0 },
            (self.speed / FULL_SPEED).clamp(0.0, 1.0),
        ]
    }

    /// ShaderToy's `iMouse` as fractions of the frame size: the position
    /// while the button was down, then the click position, positive while
    /// the button is down (z) and on the frame it went down (w).
    pub fn shadertoy(&self) -> [f32; 4] {
        let sign = |on: bool| if on { 1.0 } else { -1.0 };
        [
            self.drag_pos[0],
            self.drag_pos[1],
            self.click[0] * sign(self.down),
            self.click[1] * sign(self.pressed),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui;

    /// Runs one UI frame with the pointer at `pos` and the primary button
    /// `down`, tracking a 100×100 area at the origin.
    fn frame(
        ctx: &egui::Context,
        mouse: &mut MouseControl,
        pos: egui::Pos2,
        events: Vec<egui::Event>,
    ) {
        let input = egui::RawInput {
            events: std::iter::once(egui::Event::PointerMoved(pos))
                .chain(events)
                .collect(),
            ..Default::default()
        };
        mouse.end_frame(1.0 / 60.0);
        let mut output = ctx.run_ui(input, |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                let rect = Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(100.0, 100.0));
                let response = ui.interact(
                    rect,
                    ui.make_persistent_id("area"),
                    egui::Sense::click_and_drag(),
                );
                mouse.track(&response, rect);
            });
        });
        output.textures_delta.clear();
    }

    fn button(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        }
    }

    #[test]
    fn position_button_and_imouse() {
        let ctx = egui::Context::default();
        let mut mouse = MouseControl::default();
        // egui knows what is hovered from the frame before.
        frame(&ctx, &mut mouse, egui::pos2(25.0, 75.0), vec![]);
        frame(&ctx, &mut mouse, egui::pos2(25.0, 75.0), vec![]);
        let [x, y, b, _] = mouse.signals();
        assert!(
            (x - 0.25).abs() < 0.02 && (y - 0.25).abs() < 0.02,
            "{x} {y}"
        );
        assert_eq!(b, 0.0);
        assert!(mouse.shadertoy()[2] <= 0.0);

        let at = egui::pos2(50.0, 50.0);
        frame(&ctx, &mut mouse, at, vec![button(at, true)]);
        frame(&ctx, &mut mouse, at, vec![]);
        assert_eq!(mouse.signals()[2], 1.0);
        let imouse = mouse.shadertoy();
        assert!(imouse[2] > 0.0 && imouse[3] > 0.0, "{imouse:?}");
        mouse.rendered();
        assert!(mouse.shadertoy()[3] < 0.0);

        frame(&ctx, &mut mouse, at, vec![button(at, false)]);
        frame(&ctx, &mut mouse, at, vec![]);
        assert_eq!(mouse.signals()[2], 0.0);
        assert!(mouse.shadertoy()[2] < 0.0);
    }

    #[test]
    fn moving_raises_the_speed_and_stopping_drops_it() {
        let ctx = egui::Context::default();
        let mut mouse = MouseControl::default();
        for i in 0..20 {
            frame(
                &ctx,
                &mut mouse,
                egui::pos2(5.0 * (i % 2) as f32 * 10.0 + 20.0, 50.0),
                vec![],
            );
        }
        mouse.end_frame(1.0 / 60.0);
        assert!(mouse.signals()[3] > 0.5, "{:?}", mouse.signals());
        for _ in 0..60 {
            frame(&ctx, &mut mouse, egui::pos2(20.0, 50.0), vec![]);
        }
        mouse.end_frame(1.0 / 60.0);
        assert!(mouse.signals()[3] < 0.05, "{:?}", mouse.signals());
    }
}
