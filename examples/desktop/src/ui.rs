// The controls over the map, in egui, after the web demo's
// (examples/web/web/index.html and page.css): the style picker, zoom, a
// compass, the sources' attributions, the renderer's backend, which a click
// switches where there are two, and a panel with the gestures, the debug
// overlays and the renderer's status. `Controls::show` lays them out and
// returns what the user asked of the map; the view (view.rs) carries it out.
//
// The WebGPU logo (assets/webgpu.svg) is W3C's, under CC BY 4.0, which the
// about panel credits; the Metal logo (assets/metal.svg) is Apple's, from
// Wikimedia Commons, where it is in the public domain. The OpenGL logo
// (assets/opengl.svg) is from Wikimedia Commons too, vectorised by Vulphere:
// too simple for copyright, so in the public domain, and a trademark of
// Hewlett Packard Enterprise that the Khronos Group uses under licence.

use egui::{
    Align2, Color32, CornerRadius, CursorIcon, Frame, Id, Margin, Order, Rect, Response, RichText, Sense, Shadow,
    Shape, Stroke, Ui, Vec2, pos2, vec2,
};
use mln_map::MapDebugOptions;

use crate::backend::Kind;
use crate::styles::Style;

/// What the controls ask of the map.
pub(crate) enum Action {
    ZoomIn,
    ZoomOut,
    /// Ease the bearing and pitch back to zero.
    ResetNorth,
    /// Load the style at this index of `Shown::styles`.
    PickStyle(usize),
    SetDebug(MapDebugOptions),
    /// Draw the map on the globe, or on Mercator.
    SetGlobe(bool),
    /// Empty the cache database.
    ResetCache,
    /// Render with this backend from the next frame on.
    SetBackend(Kind),
}

/// What the controls show, as of this frame.
pub(crate) struct Shown<'a> {
    pub styles: &'a [Style],
    pub style: &'a Style,
    /// The loaded style's own name, for a style from a URL.
    pub style_name: &'a str,
    pub loading: bool,
    pub bearing: f64,
    pub pitch: f64,
    /// The sources' attributions, as HTML.
    pub attributions: &'a [String],
    pub debug: MapDebugOptions,
    /// Whether the style's projection is the globe.
    pub globe: bool,
    /// The backend the map renders with.
    pub backend: Kind,
    /// Lines about the renderer: its adapter, its frame time.
    pub status: &'a [String],
}

/// The controls' own state: which panels are open.
#[derive(Default)]
pub(crate) struct Controls {
    styles_open: bool,
    about_open: bool,
}

const GAP: f32 = 10.0;
const BUTTON: f32 = 32.0;
const PANEL_WIDTH: f32 = 300.0;

/// The web demo's colour tokens (page.css), light or dark.
struct Palette {
    surface: Color32,
    ink: Color32,
    muted: Color32,
    line: Color32,
    hover: Color32,
    north: Color32,
    accent: Color32,
    shadow: Color32,
}

impl Palette {
    fn new(dark: bool) -> Self {
        let hex = |s: &str| Color32::from_hex(s).unwrap_or(Color32::MAGENTA);
        if dark {
            Palette {
                surface: hex("#222a33"),
                ink: hex("#e6eaee"),
                muted: hex("#9ba6b1"),
                line: Color32::from_white_alpha(36),
                hover: Color32::from_white_alpha(20),
                north: hex("#ff6a5a"),
                accent: hex("#7eaaf2"),
                shadow: Color32::from_black_alpha(128),
            }
        } else {
            Palette {
                surface: hex("#ffffff"),
                ink: hex("#1d2733"),
                muted: hex("#5d6874"),
                line: Color32::from_rgba_unmultiplied(29, 39, 51, 36),
                hover: Color32::from_rgba_unmultiplied(29, 39, 51, 18),
                north: hex("#d2392b"),
                accent: hex("#2f6fd6"),
                shadow: Color32::from_black_alpha(40),
            }
        }
    }

    fn apply(&self, ctx: &egui::Context, dark: bool) {
        let mut visuals = if dark { egui::Visuals::dark() } else { egui::Visuals::light() };
        visuals.override_text_color = Some(self.ink);
        visuals.hyperlink_color = self.accent;
        visuals.selection.bg_fill = self.accent;
        visuals.widgets.hovered.weak_bg_fill = self.hover;
        visuals.widgets.hovered.bg_fill = self.hover;
        visuals.widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
        ctx.set_visuals(visuals);
    }

    /// A control's frame: the buttons and the style button.
    fn control(&self) -> Frame {
        Frame::new()
            .fill(self.surface)
            .stroke(Stroke::new(1.0, self.line))
            .corner_radius(CornerRadius::same(6))
            .shadow(Shadow { offset: [0, 1], blur: 3, spread: 0, color: self.shadow })
    }

    /// A panel's frame: the style list and the about panel.
    fn panel(&self) -> Frame {
        Frame::new()
            .fill(self.surface)
            .stroke(Stroke::new(1.0, self.line))
            .corner_radius(CornerRadius::same(8))
            .shadow(Shadow { offset: [0, 6], blur: 24, spread: 0, color: self.shadow })
            .inner_margin(Margin::same(6))
    }
}

/// A layer over the map, at a corner of the window.
fn layer(id: &str, anchor: Align2, offset: Vec2) -> egui::Area {
    egui::Area::new(Id::unique(id)).anchor(anchor, offset).order(Order::Foreground).constrain(true)
}

/// A square button with a painted icon.
fn icon_button(
    ui: &mut Ui,
    palette: &Palette,
    hint: &str,
    icon: impl FnOnce(&egui::Painter, Rect, &Palette),
) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(BUTTON), Sense::click());
    if response.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(6), palette.hover);
    }
    icon(ui.painter(), rect, palette);
    response.on_hover_cursor(CursorIcon::PointingHand).on_hover_text(hint)
}

impl Controls {
    /// Lays the controls out over the map for one frame; returns what the
    /// user asked of the map.
    pub(crate) fn show(&mut self, ctx: &egui::Context, shown: &Shown<'_>) -> Vec<Action> {
        let palette = Palette::new(shown.style.dark);
        palette.apply(ctx, shown.style.dark);
        let mut actions = Vec::new();

        // A press on the map, or Escape, closes the panels, as a popover's do.
        let pressed_map = ctx.input(|i| i.pointer.any_pressed()) && !ctx.is_pointer_over_egui();
        if pressed_map || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.styles_open = false;
            self.about_open = false;
        }

        self.style_button(ctx, &palette, shown);
        if self.styles_open {
            self.style_list(ctx, &palette, shown, &mut actions);
        }
        self.about_button(ctx, &palette);
        if self.about_open {
            about_panel(ctx, &palette, shown, &mut actions);
        }
        camera_controls(ctx, &palette, shown, &mut actions);
        if Kind::ALL.len() > 1 {
            backend_label(ctx, &palette, shown.backend, &mut actions);
        }
        attribution(ctx, &palette, shown.attributions);
        actions
    }

    fn style_button(&mut self, ctx: &egui::Context, palette: &Palette, shown: &Shown<'_>) {
        let response = layer("style-button", Align2::LEFT_TOP, vec2(GAP, GAP)).show(ctx, |ui| {
            palette
                .control()
                .inner_margin(Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    ui.set_min_width(132.0);
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 0.0;
                            let name = style_name(shown.style, shown.style_name);
                            ui.label(RichText::new(name).size(14.0).strong());
                            let detail = if shown.loading { "Loading…" } else { shown.style.provider.as_str() };
                            ui.label(RichText::new(detail).size(12.0).color(palette.muted));
                        });
                        let (chevron, _) = ui.allocate_exact_size(vec2(16.0, 30.0), Sense::hover());
                        let c = chevron.right_center() - vec2(6.0, 0.0);
                        ui.painter().add(Shape::line(
                            vec![c + vec2(-4.0, -2.0), c + vec2(0.0, 2.0), c + vec2(4.0, -2.0)],
                            Stroke::new(1.6, palette.muted),
                        ));
                    });
                })
                .response
                .interact(Sense::click())
        });
        if response.inner.on_hover_cursor(CursorIcon::PointingHand).clicked() {
            self.styles_open = !self.styles_open;
            self.about_open = false;
        }
    }

    fn style_list(&mut self, ctx: &egui::Context, palette: &Palette, shown: &Shown<'_>, actions: &mut Vec<Action>) {
        layer("styles", Align2::LEFT_TOP, vec2(GAP, GAP + 52.0)).show(ctx, |ui| {
            palette.panel().show(ui, |ui| {
                ui.set_width(PANEL_WIDTH - 40.0);
                ui.spacing_mut().item_spacing.y = 2.0;
                for (index, style) in shown.styles.iter().enumerate() {
                    if style_row(ui, palette, style, style == shown.style).clicked() {
                        self.styles_open = false;
                        if style != shown.style {
                            actions.push(Action::PickStyle(index));
                        }
                    }
                }
                ui.add_space(4.0);
                ui.separator();
                ui.label(RichText::new("Or open any style with --style and its URL.").size(12.0).color(palette.muted));
            });
        });
    }

    fn about_button(&mut self, ctx: &egui::Context, palette: &Palette) {
        layer("about-button", Align2::RIGHT_TOP, vec2(-GAP, GAP)).show(ctx, |ui| {
            palette.control().show(ui, |ui| {
                let response = icon_button(ui, palette, "About this map", |p, rect, palette| {
                    p.text(rect.center(), Align2::CENTER_CENTER, "?", egui::FontId::proportional(16.0), palette.ink);
                });
                if response.clicked() {
                    self.about_open = !self.about_open;
                    self.styles_open = false;
                }
            });
        });
    }
}

/// The style's name: a style from a URL goes by the name in it, once loaded.
fn style_name<'a>(style: &'a Style, loaded_name: &'a str) -> &'a str {
    match (style.name.as_str(), loaded_name) {
        ("", "") => "Custom style",
        ("", name) => name,
        (name, _) => name,
    }
}

fn style_row(ui: &mut Ui, palette: &Palette, style: &Style, current: bool) -> Response {
    let inner =
        Frame::new().inner_margin(Margin::symmetric(10, 5)).corner_radius(CornerRadius::same(6)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.label(RichText::new(&style.name).size(14.0).strong());
            ui.label(RichText::new(&style.provider).size(12.0).color(palette.muted));
        });
    let rect = inner.response.rect;
    let response = inner.response.interact(Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
    if current || response.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(6), palette.hover);
    }
    if current {
        let bar = Rect::from_min_size(rect.min, vec2(3.0, rect.height()));
        ui.painter().rect_filled(bar, CornerRadius::same(2), palette.accent);
    }
    response
}

/// The debug overlays the panel switches.
const DEBUG_OVERLAYS: [(&str, MapDebugOptions); 5] = [
    ("Tile borders", MapDebugOptions::TILE_BORDERS),
    ("Parse status", MapDebugOptions::PARSE_STATUS),
    ("Timestamps", MapDebugOptions::TIMESTAMPS),
    ("Collision boxes", MapDebugOptions::COLLISION),
    ("Overdraw", MapDebugOptions::OVERDRAW),
];

const GESTURES: [(&str, &str); 5] = [
    ("Pan", "Drag, or the arrow keys"),
    ("Zoom", "Scroll, pinch, double-click, or + and −"),
    ("Zoom out", "Shift-double-click"),
    ("Zoom to a box", "Shift-drag"),
    ("Rotate and tilt", "Right-drag, Ctrl-drag, two fingers, or Shift and the arrow keys"),
];

fn about_panel(ctx: &egui::Context, palette: &Palette, shown: &Shown<'_>, actions: &mut Vec<Action>) {
    layer("about", Align2::RIGHT_TOP, vec2(-GAP, GAP + BUTTON + 8.0)).show(ctx, |ui| {
        palette.panel().inner_margin(Margin::symmetric(16, 12)).show(ui, |ui| {
            ui.set_width(PANEL_WIDTH);
            ui.label(RichText::new("mln-rs").size(16.0).strong());
            let backends: Vec<&str> = Kind::ALL.iter().map(|kind| kind.name()).collect();
            ui.label(format!("A map renderer in Rust, drawing MapLibre styles with {}.", backends.join(" or ")));
            ui.add_space(8.0);
            egui::Grid::new("gestures").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
                for (gesture, how) in GESTURES {
                    ui.label(RichText::new(gesture).strong());
                    ui.add(egui::Label::new(RichText::new(how).color(palette.muted)).wrap());
                    ui.end_row();
                }
            });
            ui.add_space(8.0);
            ui.label(RichText::new("Debug overlays").strong());
            let mut debug = shown.debug;
            for (name, option) in DEBUG_OVERLAYS {
                let mut on = debug.contains(option);
                if ui.checkbox(&mut on, name).changed() {
                    debug = if on { debug | option } else { MapDebugOptions(debug.0 & !option.0) };
                }
            }
            if debug != shown.debug {
                actions.push(Action::SetDebug(debug));
            }
            let mut globe = shown.globe;
            if ui.checkbox(&mut globe, "Globe").on_hover_text("Draw the map on the globe").changed() {
                actions.push(Action::SetGlobe(globe));
            }
            let reset = egui::Button::new("Reset the cache").stroke(Stroke::new(1.0, palette.line));
            if ui.add(reset).on_hover_text("Empty the cache database").clicked() {
                actions.push(Action::ResetCache);
            }
            ui.add_space(8.0);
            for line in shown.status {
                ui.label(RichText::new(line).size(12.0).color(palette.muted));
            }
            ui.add_space(4.0);
            ui.hyperlink_to("Source on GitHub", "https://github.com/louwers/mln-rs");
            if Kind::ALL.len() > 1 {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    ui.label(RichText::new("WebGPU logo by ").size(11.0).color(palette.muted));
                    ui.hyperlink_to(RichText::new("W3C").size(11.0), "https://www.w3.org/2023/02/webgpu-logos.html");
                    ui.label(RichText::new(", ").size(11.0).color(palette.muted));
                    ui.hyperlink_to(
                        RichText::new("CC BY 4.0").size(11.0),
                        "https://creativecommons.org/licenses/by/4.0/",
                    );
                });
            }
        });
    });
}

fn camera_controls(ctx: &egui::Context, palette: &Palette, shown: &Shown<'_>, actions: &mut Vec<Action>) {
    // Above the attribution's line.
    layer("camera", Align2::RIGHT_BOTTOM, vec2(-GAP, -(GAP + 26.0))).show(ctx, |ui| {
        ui.spacing_mut().item_spacing.y = GAP;
        palette.control().show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            if icon_button(ui, palette, "Zoom in", |p, r, palette| plus_minus(p, r, palette.ink, true)).clicked() {
                actions.push(Action::ZoomIn);
            }
            let line = ui.cursor().min.y;
            ui.painter().hline(ui.min_rect().x_range(), line, Stroke::new(1.0, palette.line));
            if icon_button(ui, palette, "Zoom out", |p, r, palette| plus_minus(p, r, palette.ink, false)).clicked() {
                actions.push(Action::ZoomOut);
            }
        });
        palette.control().show(ui, |ui| {
            let (bearing, pitch) = (shown.bearing, shown.pitch);
            let compass =
                icon_button(ui, palette, "Reset north and tilt", |p, r, palette| needle(p, r, palette, bearing, pitch));
            if compass.clicked() {
                actions.push(Action::ResetNorth);
            }
        });
    });
}

fn plus_minus(p: &egui::Painter, rect: Rect, ink: Color32, plus: bool) {
    let c = rect.center();
    let stroke = Stroke::new(1.6, ink);
    p.line_segment([c + vec2(-5.0, 0.0), c + vec2(5.0, 0.0)], stroke);
    if plus {
        p.line_segment([c + vec2(0.0, -5.0), c + vec2(0.0, 5.0)], stroke);
    }
}

/// The compass needle: north turned by the bearing, and flattened by the
/// tilt, less than the map is, so that it stays legible.
fn needle(p: &egui::Painter, rect: Rect, palette: &Palette, bearing: f64, pitch: f64) {
    let c = rect.center();
    let angle = (-bearing as f32).to_radians();
    let flatten = (pitch as f32 * 0.7).to_radians().cos();
    let at = |x: f32, y: f32| {
        let (x, y) = (x * angle.cos() - y * angle.sin(), x * angle.sin() + y * angle.cos());
        c + vec2(x, y * flatten)
    };
    p.add(Shape::convex_polygon(vec![at(0.0, -7.5), at(3.0, 0.0), at(-3.0, 0.0)], palette.north, Stroke::NONE));
    p.add(Shape::convex_polygon(vec![at(0.0, 7.5), at(-3.0, 0.0), at(3.0, 0.0)], palette.muted, Stroke::NONE));
}

/// A piece of an attribution: text, or a link.
#[derive(Clone, Debug, PartialEq)]
enum Piece {
    Text(String),
    Link { text: String, url: String },
}

/// The backend's logo and name, at the bottom left; a click switches to the
/// next backend.
fn backend_label(ctx: &egui::Context, palette: &Palette, backend: Kind, actions: &mut Vec<Action>) {
    // The logos are 18 points high; OpenGL's is a wordmark of 12 by 5.
    let (logo, width) = match backend {
        #[cfg(target_os = "macos")]
        Kind::Metal => (egui::include_image!("../assets/metal.svg"), 18.0),
        Kind::WebGpu => (egui::include_image!("../assets/webgpu.svg"), 18.0),
        #[cfg(target_os = "linux")]
        Kind::OpenGl => (egui::include_image!("../assets/opengl.svg"), 43.2),
    };
    let index = Kind::ALL.iter().position(|&kind| kind == backend).unwrap_or(0);
    let next = Kind::ALL[(index + 1) % Kind::ALL.len()];
    layer("backend", Align2::LEFT_BOTTOM, vec2(GAP, -GAP)).show(ctx, |ui| {
        let response = palette
            .control()
            .inner_margin(Margin::symmetric(8, 4))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    ui.add(egui::Image::new(logo).fit_to_exact_size(vec2(width, 18.0)));
                    ui.add(
                        egui::Label::new(RichText::new(backend.name()).size(12.0).color(palette.ink)).selectable(false),
                    );
                });
            })
            .response
            .interact(Sense::click());
        if response.on_hover_cursor(CursorIcon::PointingHand).clicked() {
            actions.push(Action::SetBackend(next));
        }
    });
}

fn attribution(ctx: &egui::Context, palette: &Palette, attributions: &[String]) {
    let credits = credits(attributions);
    if credits.is_empty() {
        return;
    }
    layer("attribution", Align2::RIGHT_BOTTOM, vec2(-GAP, -GAP)).show(ctx, |ui| {
        Frame::new()
            .fill(palette.surface.gamma_multiply(0.85))
            .corner_radius(CornerRadius::same(9))
            .inner_margin(Margin::symmetric(8, 2))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.horizontal(|ui| {
                    for (i, credit) in credits.iter().enumerate() {
                        if i > 0 {
                            ui.label(RichText::new(" ").size(11.0));
                        }
                        for piece in credit {
                            match piece {
                                Piece::Text(text) => {
                                    ui.label(RichText::new(text).size(11.0).color(palette.ink));
                                }
                                Piece::Link { text, url } => {
                                    ui.hyperlink_to(RichText::new(text).size(11.0).color(palette.ink).underline(), url);
                                }
                            }
                        }
                    }
                });
            });
    });
}

/// The sources' attributions, each once: an attribution is left out when a
/// longer one, or an equal one before it, contains its text.
fn credits(attributions: &[String]) -> Vec<Vec<Piece>> {
    let all: Vec<(Vec<Piece>, String)> = attributions
        .iter()
        .map(|html| {
            let pieces = pieces(html);
            let text = pieces
                .iter()
                .map(|piece| match piece {
                    Piece::Text(text) | Piece::Link { text, .. } => text.as_str(),
                })
                .collect::<String>()
                .trim()
                .to_owned();
            (pieces, text)
        })
        .collect();
    let repeated = |i: usize, text: &str| {
        all.iter()
            .enumerate()
            .any(|(j, (_, other))| j != i && other.contains(text) && (other.len() > text.len() || j < i))
    };
    all.iter()
        .enumerate()
        .filter(|(i, (_, text))| !text.is_empty() && !repeated(*i, text))
        .map(|(_, (pieces, _))| pieces.clone())
        .collect()
}

/// An attribution's HTML as text and its http(s) links; other tags are
/// left out, and their text kept.
fn pieces(html: &str) -> Vec<Piece> {
    let mut pieces = Vec::new();
    let mut text = String::new();
    let mut link: Option<String> = None;
    let mut rest = html;
    while let Some(start) = rest.find('<') {
        text.push_str(&rest[..start]);
        let Some(end) = rest[start..].find('>') else {
            rest = &rest[start..];
            break;
        };
        let tag = &rest[start + 1..start + end];
        rest = &rest[start + end + 1..];
        let name = tag.split_whitespace().next().unwrap_or("").to_ascii_lowercase();
        if name == "a" {
            push_text(&mut pieces, &mut text, None);
            link = href(tag).filter(|url| url.starts_with("http://") || url.starts_with("https://"));
        } else if name == "/a" {
            push_text(&mut pieces, &mut text, link.take());
        }
    }
    text.push_str(rest);
    push_text(&mut pieces, &mut text, link);
    pieces
}

/// Ends the text so far: a link to `url`, or text, which joins the text
/// before it.
fn push_text(pieces: &mut Vec<Piece>, text: &mut String, url: Option<String>) {
    let decoded = decode_entities(&std::mem::take(text));
    if decoded.is_empty() {
        return;
    }
    match (url, pieces.last_mut()) {
        (Some(url), _) => pieces.push(Piece::Link { text: decoded, url }),
        (None, Some(Piece::Text(before))) => before.push_str(&decoded),
        (None, _) => pieces.push(Piece::Text(decoded)),
    }
}

/// An `<a>` tag's `href`.
fn href(tag: &str) -> Option<String> {
    let at = tag.find("href=")? + "href=".len();
    let value = &tag[at..];
    let quote = value.chars().next()?;
    let url = if quote == '"' || quote == '\'' {
        value[1..].split(quote).next()?
    } else {
        value.split_whitespace().next()?
    };
    Some(decode_entities(url))
}

fn decode_entities(text: &str) -> String {
    text.replace("&copy;", "©")
        .replace("&#169;", "©")
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&amp;", "&")
}

/// Whether `pos`, in the window's points, is over a control: a layer above
/// egui's background, which covers the window under the map's controls.
pub(crate) fn over_controls(ctx: &egui::Context, x: f32, y: f32) -> bool {
    ctx.layer_id_at(pos2(x, y)).is_some_and(|layer| layer.order != Order::Background)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pieces_keep_text_and_web_links() {
        let html = r#"<a href="https://openfreemap.org" target="_blank">OpenFreeMap</a> <a href="https://www.openmaptiles.org/">&copy; OpenMapTiles</a> Data from <a href="https://www.openstreetmap.org/copyright">OpenStreetMap</a>"#;
        assert_eq!(
            pieces(html),
            vec![
                Piece::Link { text: "OpenFreeMap".into(), url: "https://openfreemap.org".into() },
                Piece::Text(" ".into()),
                Piece::Link { text: "© OpenMapTiles".into(), url: "https://www.openmaptiles.org/".into() },
                Piece::Text(" Data from ".into()),
                Piece::Link { text: "OpenStreetMap".into(), url: "https://www.openstreetmap.org/copyright".into() },
            ]
        );
        assert_eq!(pieces(r#"<a href="javascript:x">x</a> <b>y</b>"#), vec![Piece::Text("x y".into())]);
    }

    #[test]
    fn credits_leave_out_repeats() {
        let attributions = ["© OpenStreetMap contributors".to_owned(), "© OpenStreetMap contributors · ESA".to_owned()];
        assert_eq!(credits(&attributions), vec![vec![Piece::Text("© OpenStreetMap contributors · ESA".into())]]);
        let same = ["A".to_owned(), "A".to_owned()];
        assert_eq!(credits(&same).len(), 1);
    }
}
