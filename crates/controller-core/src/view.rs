//! Controller views: the SVG drawings of a controller and the button shapes in them.
//!
//! A view SVG marks each button's shape with a `data-button` attribute holding the
//! button's canonical name. Those shapes are the hotspots, so the drawing is the only
//! copy of the geometry.

use kurbo::{BezPath, Circle, Point, Rect, RoundedRect, Shape};
use roxmltree::{Document, Node};
use serde::Deserialize;

/// The outline colour of every view SVG. The UI swaps it for the theme's colour.
pub const VIEW_LINE_COLOR: &str = "#000000";

/// The body fill colour of every view SVG. The UI swaps it for the theme's colour.
pub const VIEW_FILL_COLOR: &str = "#ffffff";

/// The attribute that marks an element as a button's shape.
pub const BUTTON_ATTRIBUTE: &str = "data-button";

/// Paint values a view SVG may use: the two view colours, or no paint.
const ALLOWED_PAINT: [&str; 3] = [VIEW_LINE_COLOR, VIEW_FILL_COLOR, "none"];

/// Elements a view SVG may contain. No text, images, gradients or styles. `defs` is
/// here because vector editors write an empty one; anything inside it is checked too.
const ALLOWED_ELEMENTS: [&str; 7] = ["svg", "g", "defs", "path", "circle", "rect", "ellipse"];

/// Elements skipped with everything inside them: they hold text, not drawing.
const SKIPPED_ELEMENTS: [&str; 2] = ["metadata", "title"];

/// Namespaces of the editor data Inkscape saves, such as `sodipodi:namedview`.
/// Elements in them are skipped with everything inside them.
const EDITOR_NAMESPACES: [&str; 2] = [
    "http://sodipodi.sourceforge.net/DTD/sodipodi-0.dtd",
    "http://www.inkscape.org/namespaces/inkscape",
];

/// Attributes a view SVG must not use. `class` could paint in other colours;
/// `transform` would move a shape away from its hotspot.
const FORBIDDEN_ATTRIBUTES: [&str; 2] = ["class", "transform"];

/// How to save from Inkscape without a `transform`, shown when one is found.
const TRANSFORM_HINT: &str = "in Inkscape, keep Preferences > Behavior > Transforms > \
    Store transformation on Optimized, ungroup a moved group, and turn a rotated shape \
    into a path (Path > Object to Path) and nudge it";

/// How far, in viewBox units, a curve may stray when flattened.
const CURVE_TOLERANCE: f64 = 0.1;

/// One drawing of the controller, such as the front or the back.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct View {
    /// View name, unique within the description. Never shown.
    pub id: String,
    /// File name of the SVG, next to `description.json`.
    pub svg: String,
    /// The SVG's file contents, drawn in [`VIEW_LINE_COLOR`] and [`VIEW_FILL_COLOR`] only.
    #[serde(skip)]
    pub svg_data: &'static str,
    /// Width and height of the SVG `viewBox`, which starts at 0 0.
    #[serde(skip)]
    pub view_box: [u16; 2],
    /// The button shapes in this view, in document order.
    #[serde(skip)]
    pub hotspots: Vec<Hotspot>,
}

/// The shape of one button in a view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hotspot {
    /// The button's canonical name.
    pub button: String,
    /// The shape's outline as absolute SVG path data in viewBox units, fit for a
    /// Slint `Path`'s `commands`.
    pub path: String,
}

impl Hotspot {
    /// Whether the point (`x`, `y`), in viewBox units, lies inside the shape.
    #[must_use]
    pub fn contains(&self, x: f64, y: f64) -> bool {
        BezPath::from_svg(&self.path).is_ok_and(|p| p.contains(Point::new(x, y)))
    }

    /// The shape's bounding box as `[x0, y0, x1, y1]`, in viewBox units.
    #[must_use]
    pub fn bounds(&self) -> [f64; 4] {
        BezPath::from_svg(&self.path).map_or([0.0; 4], |p| {
            let b = p.bounding_box();
            [b.x0, b.y0, b.x1, b.y1]
        })
    }
}

impl View {
    /// Attaches the SVG contents, reads the viewBox and the button shapes, and checks
    /// that the SVG paints in the two view colours only.
    pub(crate) fn attach(&mut self, svg_data: &'static str) -> Result<(), String> {
        let name = &self.svg;
        let doc = Document::parse(svg_data).map_err(|e| format!("'{name}': {e}"))?;
        self.view_box = parse_view_box(doc.root_element().attribute("viewBox"))
            .ok_or_else(|| format!("'{name}' needs viewBox=\"0 0 <width> <height>\""))?;
        let [width, height] = self.view_box;
        let bounds = Rect::new(0.0, 0.0, f64::from(width), f64::from(height));
        self.hotspots.clear();
        let drawn = |n: &Node<'_, '_>| n.is_element() && !n.ancestors().any(is_skipped);
        for node in doc.descendants().filter(drawn) {
            check_element(node).map_err(|e| format!("'{name}': {e}"))?;
            let Some(button) = node.attribute(BUTTON_ATTRIBUTE) else { continue };
            let path = outline(node)
                .filter(|p| bounds.contains_rect(p.bounding_box()))
                .ok_or_else(|| format!("'{name}': the shape of '{button}' is not inside it"))?;
            self.hotspots.push(Hotspot { button: button.to_owned(), path: path.to_svg() });
        }
        self.svg_data = svg_data;
        Ok(())
    }
}

/// Parses `"0 0 500 356"` into `[500, 356]`.
fn parse_view_box(value: Option<&str>) -> Option<[u16; 2]> {
    let numbers: Vec<&str> = value?.split([' ', ',']).filter(|s| !s.is_empty()).collect();
    match numbers.as_slice() {
        ["0", "0", w, h] => Some([w.parse().ok()?, h.parse().ok()?]),
        _ => None,
    }
}

/// Whether the element is editor data or text, skipped with everything inside it.
fn is_skipped(node: Node<'_, '_>) -> bool {
    let tag = node.tag_name();
    SKIPPED_ELEMENTS.contains(&tag.name())
        || tag.namespace().is_some_and(|ns| EDITOR_NAMESPACES.contains(&ns))
}

/// The element is allowed and paints in view colours only, in its `fill` and
/// `stroke` attributes and in its `style`.
fn check_element(node: Node<'_, '_>) -> Result<(), String> {
    let tag = node.tag_name().name();
    if !ALLOWED_ELEMENTS.contains(&tag) {
        return Err(format!("element <{tag}> is not allowed"));
    }
    if let Some(attr) = FORBIDDEN_ATTRIBUTES.iter().find(|a| node.has_attribute(**a)) {
        let hint = if *attr == "transform" { format!("; {TRANSFORM_HINT}") } else { String::new() };
        return Err(format!("attribute '{attr}' is not allowed{hint}"));
    }
    let style = node.attribute("style").unwrap_or("").split(';').filter_map(|d| d.split_once(':'));
    let attributes = ["fill", "stroke"].into_iter().filter_map(|a| Some((a, node.attribute(a)?)));
    for (property, paint) in attributes.chain(style) {
        let (property, paint) = (property.trim(), paint.trim());
        if ["fill", "stroke"].contains(&property) && !ALLOWED_PAINT.contains(&paint) {
            return Err(format!("{property} '{paint}' is not a view colour"));
        }
    }
    Ok(())
}

/// The outline of a `path`, `circle` or `rect` element.
fn outline(node: Node<'_, '_>) -> Option<BezPath> {
    let num = |name: &str| node.attribute(name)?.parse::<f64>().ok();
    match node.tag_name().name() {
        "path" => BezPath::from_svg(node.attribute("d")?).ok(),
        "circle" => Some(Circle::new((num("cx")?, num("cy")?), num("r")?).to_path(CURVE_TOLERANCE)),
        "rect" => {
            let (x, y) = (num("x").unwrap_or(0.0), num("y").unwrap_or(0.0));
            let rect = Rect::new(x, y, x + num("width")?, y + num("height")?);
            let radius = num("rx").unwrap_or(0.0);
            Some(RoundedRect::from_rect(rect, radius).to_path(CURVE_TOLERANCE))
        }
        _ => None,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn attach(svg: &'static str) -> Result<View, String> {
        let mut view = View {
            id: "front".to_owned(),
            svg: "front.svg".to_owned(),
            svg_data: "",
            view_box: [0, 0],
            hotspots: Vec::new(),
        };
        view.attach(svg).map(|()| view)
    }

    #[test]
    fn reads_the_view_box_and_each_button_shape() {
        let view = attach(
            r##"<?xml version="1.0"?><svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 50">
                <path d="M 0 0 L 100 0 L 100 50 Z" fill="#ffffff" stroke="#000000"/>
                <circle data-button="a" cx="20" cy="20" r="10" fill="#ffffff"/>
                <rect data-button="b" x="40" y="10" width="20" height="10" rx="5" fill="none"/>
                <path data-button="c" d="M 70 10 L 90 10 L 80 30 Z" stroke="none"/>
            </svg>"##,
        )
        .unwrap();
        assert_eq!(view.view_box, [100, 50]);
        let buttons: Vec<&str> = view.hotspots.iter().map(|h| h.button.as_str()).collect();
        assert_eq!(buttons, ["a", "b", "c"]);
        let [a, b, c] = [&view.hotspots[0], &view.hotspots[1], &view.hotspots[2]];
        assert!(a.contains(20.0, 20.0) && a.contains(28.0, 20.0) && !a.contains(29.0, 29.0));
        assert!(b.contains(50.0, 15.0) && !b.contains(40.5, 10.5), "rounded corner");
        assert!(c.contains(80.0, 15.0) && !c.contains(72.0, 28.0));
    }

    #[test]
    fn rejects_a_bad_view_box_paint_or_shape() {
        let bad: [&'static str; 11] = [
            r#"<svg viewBox="0 0 100"/>"#,
            r#"<svg viewBox="10 0 100 50"/>"#,
            r##"<svg viewBox="0 0 100 50"><path fill="#ff0000"/></svg>"##,
            r#"<svg viewBox="0 0 100 50"><path stroke="url(#g)"/></svg>"#,
            r#"<svg viewBox="0 0 100 50"><path style="fill:red"/></svg>"#,
            r#"<svg viewBox="0 0 100 50"><path style="fill:#ffffff; stroke: #ff0000"/></svg>"#,
            r#"<svg viewBox="0 0 100 50"><defs><linearGradient/></defs></svg>"#,
            r#"<svg viewBox="0 0 100 50"><g transform="scale(2)"/></svg>"#,
            r#"<svg viewBox="0 0 100 50"><text>A</text></svg>"#,
            r#"<svg viewBox="0 0 100 50"><circle data-button="a" cx="95" cy="5" r="10"/></svg>"#,
            r#"<svg viewBox="0 0 100 50"><ellipse data-button="a" cx="5" cy="5" rx="1" ry="1"/></svg>"#,
        ];
        for (i, svg) in bad.iter().enumerate() {
            assert!(attach(svg).is_err(), "svg {i} should fail");
        }
    }

    #[test]
    fn accepts_an_svg_saved_from_inkscape() {
        let view = attach(include_str!("../../../fixtures/views/inkscape-saved.svg")).unwrap();
        assert_eq!(view.view_box, [100, 50]);
        assert_eq!(view.hotspots.len(), 1);
        assert_eq!(view.hotspots[0].button, "a");
        assert!(view.hotspots[0].contains(51.0, 15.0));
    }

    #[test]
    fn a_transform_names_the_inkscape_fix() {
        let err = attach(r#"<svg viewBox="0 0 100 50"><g transform="scale(2)"/></svg>"#);
        assert!(err.unwrap_err().contains("Store transformation on Optimized"));
    }
}
