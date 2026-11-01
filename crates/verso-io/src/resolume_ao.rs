//! Resolume Arena Advanced Output presets (`.xml`), in both directions.
//!
//! [`import`] reads screens, slices, polygons and DMX slices into an
//! [`OutputSetup`]. [`export`] writes a preset Arena loads. Given the file the
//! setup was imported from, it patches that document, so what Verso does not
//! model (guides, masks, DMX fixtures, device ids) is kept as it was. Screens
//! and slices added in Verso are written from a template.
//!
//! Tested with presets saved by Arena 7.28.

use verso_core::output::{
    ColorAdjust, Contour, Device, DeviceKind, OutputSetup, Quad, Screen, Slice, SliceKind,
    SoftEdge, Warp,
};

use crate::xml::Element;
use crate::{Error, Result};

/// A parsed preset, with what could not be carried over.
#[derive(Debug, Clone, PartialEq)]
pub struct Import {
    /// Preset name, from the root element.
    pub name: String,
    pub setup: OutputSetup,
    /// Human-readable notes on unsupported content.
    pub warnings: Vec<String>,
}

fn error(message: impl Into<String>) -> Error {
    Error::Format(format!("Advanced Output: {}", message.into()))
}

pub fn import(text: &str) -> Result<Import> {
    let root = Element::parse(text).map_err(error)?;
    if root.name != "XmlState" {
        return Err(error(format!("unexpected root <{}>", root.name)));
    }
    let setup_el = root
        .child("ScreenSetup")
        .ok_or_else(|| error("no <ScreenSetup>: not an Advanced Output preset"))?;
    let mut warnings = Vec::new();
    let mut setup = OutputSetup::default();
    if let Some(size) = setup_el.child("CurrentCompositionTextureSize") {
        setup.canvas = (
            parse_or(size.attr("width"), 1920),
            parse_or(size.attr("height"), 1080),
        );
    }
    if let Some(soft) = setup_el.child("SoftEdging") {
        let s = &mut setup.soft_edge;
        s.gamma_rgb = [
            float_param(soft, "Gamma Red").unwrap_or(s.gamma_rgb[0]),
            float_param(soft, "Gamma Green").unwrap_or(s.gamma_rgb[1]),
            float_param(soft, "Gamma Blue").unwrap_or(s.gamma_rgb[2]),
        ];
        s.gamma = float_param(soft, "Gamma").unwrap_or(s.gamma);
        s.luminance = float_param(soft, "Luminance").unwrap_or(s.luminance);
        s.power = float_param(soft, "Power").unwrap_or(s.power);
    }
    for el in setup_el
        .child("screens")
        .into_iter()
        .flat_map(|s| &s.children)
    {
        match el.name.as_str() {
            "Screen" | "DmxScreen" => setup.screens.push(read_screen(el, &mut warnings)?),
            other => warnings.push(format!("skipped unknown screen type <{other}>")),
        }
    }
    Ok(Import {
        name: root.attr("name").unwrap_or_default().to_owned(),
        setup,
        warnings,
    })
}

fn read_screen(el: &Element, warnings: &mut Vec<String>) -> Result<Screen> {
    let name = string_param(el, "Name")
        .or(el.attr("name"))
        .unwrap_or("Screen")
        .to_owned();
    let mut screen = Screen {
        id: id_of(el)?,
        enabled: bool_param(el, "Enabled").unwrap_or(true),
        hidden: bool_param(el, "Hidden").unwrap_or(false),
        device: read_device(el),
        color: read_color(el),
        slices: Vec::new(),
        dmx: el.name == "DmxScreen",
        name,
    };
    for layer in el.child("layers").into_iter().flat_map(|l| &l.children) {
        let kind = match layer.name.as_str() {
            "Slice" => SliceKind::Slice,
            "Polygon" => SliceKind::Polygon,
            "DmxSlice" => SliceKind::Dmx,
            other => {
                warnings.push(format!("{}: skipped unknown layer <{other}>", screen.name));
                continue;
            }
        };
        let slice = read_slice(layer, kind)?;
        let known = ["0:", "1:", "3:"];
        if !known.iter().any(|k| slice.source.starts_with(k)) {
            warnings.push(format!(
                "{} / {}: input source {} is shown as the composition",
                screen.name, slice.name, slice.source
            ));
        }
        screen.slices.push(slice);
    }
    if screen.dmx {
        warnings.push(format!(
            "{}: DMX output is kept in the file but not sent by Verso",
            screen.name
        ));
    }
    Ok(screen)
}

fn read_device(screen: &Element) -> Device {
    let el = screen
        .child("OutputDevice")
        .and_then(|d| d.children.first());
    let Some(el) = el else {
        return Device {
            kind: DeviceKind::None,
            name: String::new(),
            size: None,
        };
    };
    let kind = match el.name.as_str() {
        "OutputDeviceDisplay" => DeviceKind::Display,
        "OutputDeviceNDI" => DeviceKind::Ndi,
        "OutputDeviceSpout" => DeviceKind::Spout,
        "OutputDeviceDmx" => DeviceKind::Dmx,
        _ => DeviceKind::Virtual,
    };
    let size = match (el.attr("width"), el.attr("height")) {
        (Some(w), Some(h)) => w.parse().ok().zip(h.parse().ok()),
        _ => None,
    };
    Device {
        kind,
        name: el.attr("name").unwrap_or_default().to_owned(),
        size,
    }
}

fn read_color(el: &Element) -> ColorAdjust {
    let d = ColorAdjust::default();
    ColorAdjust {
        opacity: float_param(el, "Opacity").unwrap_or(d.opacity),
        brightness: float_param(el, "Brightness").unwrap_or(d.brightness),
        contrast: float_param(el, "Contrast").unwrap_or(d.contrast),
        rgb: [
            float_param(el, "Red").unwrap_or(0.0),
            float_param(el, "Green").unwrap_or(0.0),
            float_param(el, "Blue").unwrap_or(0.0),
        ],
    }
}

fn read_slice(el: &Element, kind: SliceKind) -> Result<Slice> {
    let input_el = el.child("InputRect");
    let input = quad(input_el).ok_or_else(|| error("slice without <InputRect>"))?;
    let output = quad(el.child("OutputRect")).unwrap_or(input);
    let warp = el.child("Warper").map(|warper| {
        let bezier = warper.child("BezierWarper");
        let grid = bezier.map_or((0, 0), |b| {
            (
                parse_or(b.attr("controlWidth"), 0),
                parse_or(b.attr("controlHeight"), 0),
            )
        });
        let homography = warper.child("Homography");
        Warp {
            pin_from: quad(homography.and_then(|h| h.child("src"))).unwrap_or(output),
            corners: quad(homography.and_then(|h| h.child("dst"))).unwrap_or(output),
            grid,
            points: bezier
                .and_then(|b| b.child("vertices"))
                .map(points)
                .unwrap_or_default(),
        }
    });
    let contour = match (el.child("InputContour"), el.child("OutputContour")) {
        (Some(i), Some(o)) => Some(Contour {
            input: i.child("points").map(points).unwrap_or_default(),
            output: o.child("points").map(points).unwrap_or_default(),
        }),
        _ => None,
    };
    let mut color = read_color(el);
    color.opacity = 1.0;
    Ok(Slice {
        id: id_of(el)?,
        name: string_param(el, "Name").unwrap_or("Slice").to_owned(),
        enabled: bool_param(el, "Enabled").unwrap_or(true),
        kind,
        source: find_param(el, "Input Source")
            .and_then(|p| p.attr("value"))
            .unwrap_or("0:1")
            .to_owned(),
        input,
        input_rotation: parse_or(input_el.and_then(|i| i.attr("orientation")), 0.0),
        output,
        warp,
        contour,
        flip: parse_or(find_param(el, "Flip").and_then(|p| p.attr("value")), 0),
        color,
        soft_edge: bool_param(el, "SoftEdgeEnable").unwrap_or(false),
    })
}

fn id_of(el: &Element) -> Result<u64> {
    el.attr("uniqueId")
        .and_then(|id| id.parse().ok())
        .ok_or_else(|| error(format!("<{}> without a uniqueId", el.name)))
}

fn parse_or<T: std::str::FromStr>(value: Option<&str>, default: T) -> T {
    value.and_then(|v| v.trim().parse().ok()).unwrap_or(default)
}

fn points(el: &Element) -> Vec<[f32; 2]> {
    el.children_named("v")
        .map(|v| [parse_or(v.attr("x"), 0.0), parse_or(v.attr("y"), 0.0)])
        .collect()
}

fn quad(el: Option<&Element>) -> Option<Quad> {
    points(el?).try_into().ok()
}

/// A parameter of `el` by name, in any of its direct `<Params>` groups.
fn find_param<'a>(el: &'a Element, name: &str) -> Option<&'a Element> {
    el.children_named("Params")
        .flat_map(|p| &p.children)
        .find(|p| p.attr("name") == Some(name))
}

fn string_param<'a>(el: &'a Element, name: &str) -> Option<&'a str> {
    find_param(el, name)?.attr("value")
}

fn bool_param(el: &Element, name: &str) -> Option<bool> {
    Some(find_param(el, name)?.attr("value")? != "0")
}

fn float_param(el: &Element, name: &str) -> Option<f64> {
    find_param(el, name)?.attr("value")?.parse().ok()
}

// --- Export -----------------------------------------------------------------

/// Writes `setup` as a preset named `name`. `base` is the preset it was
/// imported from, if any: its unmodeled content is kept.
pub fn export(name: &str, setup: &OutputSetup, base: Option<&str>) -> Result<String> {
    let mut root = match base {
        Some(text) => Element::parse(text).map_err(error)?,
        None => Element::new("XmlState"),
    };
    root.set_attr("name", name);
    if root.child("versionInfo").is_none() {
        root.children.insert(
            0,
            Element::new("versionInfo")
                .with("name", "Resolume Arena")
                .with("majorVersion", 7)
                .with("minorVersion", 28)
                .with("microVersion", 1)
                .with("revision", 26535),
        );
    }
    let setup_el = root.ensure_child("ScreenSetup");
    setup_el.set_attr("name", "ScreenSetup");
    if setup_el.child("Params").is_none() {
        setup_el
            .children
            .insert(0, Element::new("Params").with("name", "ScreenSetupParams"));
    }
    setup_el
        .ensure_child("CurrentCompositionTextureSize")
        .attrs
        .clone_from(&vec![
            ("width".into(), setup.canvas.0.to_string()),
            ("height".into(), setup.canvas.1.to_string()),
        ]);

    let old: Vec<Element> = setup_el
        .child_mut("screens")
        .map(|s| std::mem::take(&mut s.children))
        .unwrap_or_default();
    let mut screens = Vec::with_capacity(setup.screens.len());
    for screen in &setup.screens {
        screens.push(write_screen(screen, take_by_id(&old, screen.id))?);
    }
    setup_el.ensure_child("screens").children = screens;

    write_soft_edge(setup_el.ensure_child("SoftEdging"), &setup.soft_edge);
    Ok(root.to_document().replace('\n', "\r\n"))
}

fn take_by_id(elements: &[Element], id: u64) -> Option<Element> {
    let id = id.to_string();
    elements
        .iter()
        .find(|e| e.attr("uniqueId") == Some(&id))
        .cloned()
}

fn write_soft_edge(el: &mut Element, soft: &SoftEdge) {
    for (name, default, value, max) in [
        ("Gamma Red", 2.0, soft.gamma_rgb[0], 4.0),
        ("Gamma Green", 2.0, soft.gamma_rgb[1], 4.0),
        ("Gamma Blue", 2.0, soft.gamma_rgb[2], 4.0),
        ("Gamma", 1.0, soft.gamma, 4.0),
        ("Luminance", 0.5, soft.luminance, 1.0),
        ("Power", 2.0, soft.power, 8.0),
    ] {
        set_range(el, "Soft Edge", name, default, value, (0.0, max));
    }
}

fn write_screen(screen: &Screen, base: Option<Element>) -> Result<Element> {
    let tag = if screen.dmx { "DmxScreen" } else { "Screen" };
    let mut el = match base {
        Some(el) if el.name == tag => el,
        _ if screen.dmx => {
            return Err(error(format!(
                "DMX screen \"{}\" can only be written back to the preset it came from",
                screen.name
            )));
        }
        _ => Element::new(tag),
    };
    el.set_attr("name", &screen.name);
    el.set_attr("uniqueId", screen.id);

    set_value(&mut el, "Params", "Name", "STRING", "", &screen.name);
    set_value(
        &mut el,
        "Params",
        "Enabled",
        "BOOL",
        "1",
        bool_text(screen.enabled),
    );
    set_value(
        &mut el,
        "Params",
        "Hidden",
        "BOOL",
        "0",
        bool_text(screen.hidden),
    );
    set_range(
        &mut el,
        "Output",
        "Opacity",
        1.0,
        screen.color.opacity,
        (0.0, 1.0),
    );
    write_color(&mut el, &screen.color);

    let old: Vec<Element> = el
        .child_mut("layers")
        .map(|l| std::mem::take(&mut l.children))
        .unwrap_or_default();
    let mut layers = Vec::with_capacity(screen.slices.len());
    for slice in &screen.slices {
        layers.push(write_slice(slice, take_by_id(&old, slice.id))?);
    }
    el.ensure_child("layers").children = layers;

    let device = el.ensure_child("OutputDevice");
    let unchanged = !device.children.is_empty() && {
        let mut probe = Element::new("Screen");
        probe.children.push(device.clone());
        read_device(&probe) == screen.device
    };
    if !unchanged {
        device.children = device_element(&screen.device).into_iter().collect();
    }
    Ok(el)
}

fn device_element(device: &Device) -> Option<Element> {
    let tag = match device.kind {
        DeviceKind::None => return None,
        DeviceKind::Display => "OutputDeviceDisplay",
        DeviceKind::Ndi => "OutputDeviceNDI",
        DeviceKind::Spout => "OutputDeviceSpout",
        DeviceKind::Virtual => "OutputDeviceVirtual",
        DeviceKind::Dmx => "OutputDeviceDmx",
    };
    let mut el = Element::new(tag).with("name", &device.name);
    if device.kind == DeviceKind::Display {
        el.set_attr("fullscreen", 1);
    }
    if let Some((w, h)) = device.size {
        el.set_attr("width", w);
        el.set_attr("height", h);
    }
    Some(el)
}

fn write_slice(slice: &Slice, base: Option<Element>) -> Result<Element> {
    let tag = match slice.kind {
        SliceKind::Slice => "Slice",
        SliceKind::Polygon => "Polygon",
        SliceKind::Dmx => "DmxSlice",
    };
    let mut el = match base {
        Some(el) if el.name == tag => el,
        _ if slice.kind == SliceKind::Dmx => {
            return Err(error(format!(
                "DMX slice \"{}\" can only be written back to the preset it came from",
                slice.name
            )));
        }
        _ => Element::new(tag),
    };
    el.set_attr("uniqueId", slice.id);

    set_value(&mut el, "Common", "Name", "STRING", "Layer", &slice.name);
    set_value(
        &mut el,
        "Common",
        "Enabled",
        "BOOL",
        "1",
        bool_text(slice.enabled),
    );
    set_choice(&mut el, "Input", "Input Source", "0:1", &slice.source);
    if slice.kind == SliceKind::Slice {
        let soft_edge = bool_text(slice.soft_edge);
        set_value(&mut el, "Input", "SoftEdgeEnable", "BOOL", "0", soft_edge);
    }
    set_value(
        &mut el,
        "Output",
        "Flip",
        "UINT8",
        "0",
        &slice.flip.to_string(),
    );
    write_color(&mut el, &slice.color);

    let input_rect = set_quad(&mut el, "InputRect", &slice.input);
    if parse_or(input_rect.attr("orientation"), f32::NAN) != slice.input_rotation {
        input_rect.set_attr("orientation", slice.input_rotation);
    }
    let output_rect = set_quad(&mut el, "OutputRect", &slice.output);
    if output_rect.attr("orientation").is_none() {
        output_rect.set_attr("orientation", 0);
    }

    if let Some(contour) = &slice.contour {
        for (tag, points) in [
            ("InputContour", &contour.input),
            ("OutputContour", &contour.output),
        ] {
            let c = el.ensure_child(tag);
            if c.attr("closed").is_none() {
                c.set_attr("closed", 1);
            }
            set_points(c.ensure_child("points"), points);
            let segments = c.ensure_child("segments");
            if segments.text.chars().count() != points.len() {
                segments.text = "L".repeat(points.len());
            }
        }
    }

    if let Some(warp) = &slice.warp {
        let warper = el.ensure_child("Warper");
        let bezier = warper.ensure_child("BezierWarper");
        bezier.set_attr("controlWidth", warp.grid.0);
        bezier.set_attr("controlHeight", warp.grid.1);
        set_points(bezier.ensure_child("vertices"), &warp.points);
        let homography = warper.ensure_child("Homography");
        set_points(homography.ensure_child("src"), &warp.pin_from);
        set_points(homography.ensure_child("dst"), &warp.corners);
    }
    Ok(el)
}

fn write_color(el: &mut Element, color: &ColorAdjust) {
    set_range(
        el,
        "Output",
        "Brightness",
        0.0,
        color.brightness,
        (-1.0, 1.0),
    );
    set_range(el, "Output", "Contrast", 0.0, color.contrast, (-1.0, 1.0));
    for (name, value) in ["Red", "Green", "Blue"].iter().zip(color.rgb) {
        set_range(el, "Output", name, 0.0, value, (-1.0, 1.0));
    }
}

fn bool_text(value: bool) -> &'static str {
    if value { "1" } else { "0" }
}

fn vertices(points: &[[f32; 2]]) -> Vec<Element> {
    points
        .iter()
        .map(|[x, y]| Element::new("v").with("x", x).with("y", y))
        .collect()
}

/// Replaces the `<v>` children of `el`, unless they already hold `points`:
/// Arena writes more digits than an `f32` needs, and an unedited preset
/// should export unchanged.
fn set_points(el: &mut Element, points: &[[f32; 2]]) {
    if self::points(el) != points {
        el.children = vertices(points);
    }
}

fn set_quad<'a>(el: &'a mut Element, tag: &str, quad: &Quad) -> &'a mut Element {
    let rect = el.ensure_child(tag);
    set_points(rect, quad);
    rect
}

/// A parameter of `el` by name, in any of its direct `<Params>` groups.
fn find_param_mut<'a>(el: &'a mut Element, name: &str) -> Option<&'a mut Element> {
    el.children
        .iter_mut()
        .filter(|c| c.name == "Params")
        .flat_map(|p| &mut p.children)
        .find(|p| p.attr("name") == Some(name))
}

/// Adds `param` to the `<Params name="group">` of `el`, creating the group
/// after the existing ones, as Arena orders them.
fn add_param(el: &mut Element, group: &str, param: Element) {
    let index = el
        .children
        .iter()
        .position(|c| c.name == "Params" && c.attr("name") == Some(group));
    let index = index.unwrap_or_else(|| {
        let at = el
            .children
            .iter()
            .rposition(|c| c.name == "Params")
            .map_or(0, |i| i + 1);
        el.children
            .insert(at, Element::new("Params").with("name", group));
        at
    });
    el.children[index].children.push(param);
}

// Arena leaves out parameters at their default value, and so does Verso:
// the setters below only add a missing parameter that differs from its
// default.

fn set_value(el: &mut Element, group: &str, name: &str, kind: &str, default: &str, value: &str) {
    match find_param_mut(el, name) {
        Some(param) => param.set_attr("value", value),
        None if value == default => {}
        None => add_param(
            el,
            group,
            Element::new("Param")
                .with("name", name)
                .with("T", kind)
                .with("default", default)
                .with("value", value),
        ),
    }
}

fn set_choice(el: &mut Element, group: &str, name: &str, default: &str, value: &str) {
    match find_param_mut(el, name) {
        Some(param) => param.set_attr("value", value),
        None if value == default => {}
        None => add_param(
            el,
            group,
            Element::new("ParamChoice")
                .with("name", name)
                .with("default", default)
                .with("value", value)
                .with("storeChoices", 0),
        ),
    }
}

/// Sets a numeric parameter, created in Arena's full form when missing.
fn set_range(
    el: &mut Element,
    group: &str,
    name: &str,
    default: f64,
    value: f64,
    (min, max): (f64, f64),
) {
    if let Some(param) = find_param_mut(el, name) {
        // Keep the digits Arena wrote when the value did not change.
        if param.attr("value").and_then(|v| v.parse::<f64>().ok()) != Some(value) {
            param.set_attr("value", value);
        }
        return;
    }
    if value == default {
        return;
    }
    let range = |kind: &str| {
        Element::new("ValueRange")
            .with("name", kind)
            .with("min", min)
            .with("max", max)
    };
    add_param(
        el,
        group,
        Element::new("ParamRange")
            .with("name", name)
            .with("T", "DOUBLE")
            .with("default", default)
            .with("value", value)
            .child_of(Element::new("PhaseSourceStatic").with("name", "PhaseSourceStatic"))
            .child_of(Element::new("BehaviourDouble").with("name", "BehaviourDouble"))
            .child_of(range("defaultRange"))
            .child_of(range("minMax"))
            .child_of(range("startStop")),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    const HSS: &str = include_str!("../tests/fixtures/HSS 4 ecrans.xml");
    const POLYGON: &str = include_str!("../tests/fixtures/harboat.xml");

    #[test]
    fn imports_hss_preset() {
        let import = import(HSS).unwrap();
        assert_eq!(import.name, "HSS 4 ecrans");
        let setup = &import.setup;
        assert_eq!(setup.canvas, (1920, 1920));
        let neon = &setup.screens[0];
        assert_eq!(neon.name, "NEON 1920x1080");
        assert_eq!(neon.id, 1793100000001);
        assert_eq!(neon.device.kind, DeviceKind::Display);
        assert_eq!(neon.device.size, Some((1920, 1080)));
        assert!((neon.color.brightness - 0.169397573).abs() < 1e-6);
        let first = &neon.slices[0];
        assert_eq!(first.name, "Left Screen Out");
        assert_eq!(first.input, verso_core::output::rect(400.0, 800.0));
        let warp = first.warp.as_ref().unwrap();
        assert_eq!(warp.grid, (4, 4));
        assert_eq!(warp.points.len(), 16);
        assert!(
            setup
                .screens
                .iter()
                .any(|s| s.device.kind == DeviceKind::Ndi)
        );
    }

    #[test]
    fn imports_polygon() {
        let setup = import(POLYGON).unwrap().setup;
        let polygon = setup
            .screens
            .iter()
            .flat_map(|s| &s.slices)
            .find(|s| s.kind == SliceKind::Polygon)
            .unwrap();
        let contour = polygon.contour.as_ref().unwrap();
        assert_eq!(contour.input.len(), 4);
        assert_eq!(contour.output[0], [1920.0, 0.0]);
    }

    #[test]
    fn unedited_preset_exports_unchanged() {
        for text in [HSS, POLYGON] {
            let import = import(text).unwrap();
            let out = export(&import.name, &import.setup, Some(text)).unwrap();
            // Same tree as the source, ignoring formatting.
            assert_eq!(Element::parse(&out).unwrap(), Element::parse(text).unwrap());
        }
    }

    #[test]
    fn edits_round_trip() {
        let mut setup = import(HSS).unwrap().setup;
        setup.screens[0].slices[1].name = "Renamed & <moved>".into();
        setup.screens[0].slices[1].input[0] = [12.5, 3.0];
        setup.screens[0].slices.remove(0);
        setup.screens[1].device.size = Some((1280, 720));
        setup.add_screen("Added", (800, 600));
        let out = export("Edited", &setup, Some(HSS)).unwrap();
        assert_eq!(import(&out).unwrap().setup, setup);
    }

    #[test]
    fn new_setup_round_trips() {
        let mut setup = OutputSetup::default();
        setup.add_screen("Projector", (1920, 1200));
        setup.screens[0].device.kind = DeviceKind::Display;
        setup.screens[0].slices[0].flip = 1;
        setup.screens[0].slices[0].source = "3:2".into();
        setup.screens[0].slices[0].set_output_point(2, [1800.0, 1100.0]);
        let id = setup.next_id();
        setup.screens[0].slices.push(Slice::polygon(
            id,
            "Drawn",
            vec![[0.0, 0.0], [100.0, 0.0], [50.0, 80.0]],
            vec![[10.0, 10.0], [300.0, 10.0], [150.0, 240.0]],
        ));
        setup.add_screen("Stream", (1280, 720));
        let out = export("Fresh", &setup, None).unwrap();
        let back = import(&out).unwrap();
        assert_eq!(back.name, "Fresh");
        assert_eq!(back.setup, setup);
        assert!(back.warnings.is_empty());
    }

    /// Every preset in `VERSO_AO_DIR` imports and exports unchanged. Run with
    /// `cargo test -p verso-io -- --ignored` against Arena's preset folder.
    #[test]
    #[ignore]
    fn presets_in_folder_round_trip() {
        let dir = std::env::var("VERSO_AO_DIR").expect("set VERSO_AO_DIR");
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_none_or(|e| e != "xml") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            let imported = import(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            let out = export(&imported.name, &imported.setup, Some(&text)).unwrap();
            if let Ok(dump) = std::env::var("VERSO_AO_DUMP") {
                std::fs::write(
                    std::path::Path::new(&dump).join(path.file_name().unwrap()),
                    &out,
                )
                .unwrap();
            }
            assert_eq!(
                Element::parse(&out).unwrap(),
                Element::parse(&text).unwrap(),
                "{}",
                path.display()
            );
            assert_eq!(import(&out).unwrap().setup, imported.setup);
            for slice in imported.setup.screens.iter().flat_map(|s| &s.slices) {
                let mesh = slice.mesh(imported.setup.canvas);
                assert!(
                    mesh.iter()
                        .all(|v| v.position.iter().chain(&v.uv).all(|c| c.is_finite())),
                    "{}: {}",
                    path.display(),
                    slice.name
                );
                if slice.kind == SliceKind::Slice {
                    assert!(!mesh.is_empty(), "{}: {}", path.display(), slice.name);
                }
            }
        }
    }

    #[test]
    fn rejects_other_documents() {
        assert!(import("<XmlState name=\"x\"><Composition/></XmlState>").is_err());
        assert!(import("not xml").is_err());
    }
}
