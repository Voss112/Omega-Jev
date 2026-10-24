//! Paint effects: gradients, frosted glass, glows, the aurora backdrop, spinners and shimmering text.

use std::{f32::consts::TAU, sync::Arc};

use eframe::egui::{
    Color32, Context, CornerRadius, FontId, LayerId, Mesh, Painter, Pos2, Rect, Shadow, Shape, StrokeKind, Vec2,
    epaint::{ColorMode, PathStroke, RectShape, TessellationOptions, Tessellator},
    text::{LayoutJob, TextFormat},
    vec2,
};

use super::theme::{ACCENT, BG, CYAN, INDIGO, PINK};

/// Seconds since start, for time-driven animation.
pub fn now(ctx: &Context) -> f64 {
    ctx.input(|i| i.time)
}

/// Smooth 0→1→0 wave of the given period.
pub fn pulse(t: f64, period: f64) -> f32 {
    (0.5 - 0.5 * (t * std::f64::consts::TAU / period).cos()) as f32
}

/// Light-emitting version of a colour: adds to what is below instead of covering it.
pub fn additive(c: Color32) -> Color32 {
    Color32::from_rgba_premultiplied(c.r(), c.g(), c.b(), 0)
}

pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    a.lerp_to_gamma(b, t.clamp(0.0, 1.0))
}

/// Tessellates `shape` and colours every vertex with `color(pos)`, keeping anti-aliasing coverage.
pub fn tinted(painter: &Painter, shape: impl Into<Shape>, color: impl Fn(Pos2) -> Color32) -> Shape {
    let mut tess = Tessellator::new(painter.pixels_per_point(), TessellationOptions::default(), [1, 1], Vec::new());
    let mut mesh = Mesh::default();
    tess.tessellate_shape(shape.into(), &mut mesh);
    for v in &mut mesh.vertices {
        let coverage = f32::from(v.color.a()) / 255.0;
        v.color = color(v.pos).gamma_multiply(coverage);
    }
    Shape::mesh(mesh)
}

pub fn paint_with(painter: &Painter, shape: impl Into<Shape>, color: impl Fn(Pos2) -> Color32) {
    painter.add(tinted(painter, shape, color));
}

/// Linear gradient across `rect` along `dir`.
pub fn linear(rect: Rect, from: Color32, to: Color32, dir: Vec2) -> impl Fn(Pos2) -> Color32 + Send + Sync + 'static {
    let d = dir.normalized();
    let half = ((rect.width() * d.x.abs() + rect.height() * d.y.abs()) / 2.0).max(1.0);
    let c = rect.center();
    move |p| mix(from, to, (p - c).dot(d) / half * 0.5 + 0.5)
}

/// Rounded rectangle filled with a diagonal gradient.
pub fn gradient_rect(painter: &Painter, rect: Rect, radius: impl Into<CornerRadius>, from: Color32, to: Color32) {
    painter.add(gradient_fill(painter, rect, radius, from, to));
}

pub fn gradient_fill(painter: &Painter, rect: Rect, radius: impl Into<CornerRadius>, from: Color32, to: Color32) -> Shape {
    tinted(painter, RectShape::filled(rect, radius, Color32::WHITE), linear(rect, from, to, vec2(1.0, 0.6)))
}

/// Densely sampled rounded-rect outline (clockwise from the top edge), so per-vertex colours follow any gradient.
fn outline(rect: Rect, radius: CornerRadius) -> Vec<Pos2> {
    const STEP: f32 = 6.0;
    let max = rect.width().min(rect.height()) / 2.0;
    let r = |v: u8| f32::from(v).min(max);
    let corners = [
        (rect.right_top() + vec2(-r(radius.ne), r(radius.ne)), r(radius.ne), -90.0_f32),
        (rect.right_bottom() + vec2(-r(radius.se), -r(radius.se)), r(radius.se), 0.0),
        (rect.left_bottom() + vec2(r(radius.sw), -r(radius.sw)), r(radius.sw), 90.0),
        (rect.left_top() + vec2(r(radius.nw), r(radius.nw)), r(radius.nw), 180.0),
    ];
    let on = |c: Pos2, r: f32, a: f32| c + vec2(a.to_radians().cos(), a.to_radians().sin()) * r;
    let mut pts = Vec::new();
    for (i, &(c, rad, a0)) in corners.iter().enumerate() {
        for k in 0..=8 {
            pts.push(on(c, rad, a0 + 90.0 * k as f32 / 8.0));
        }
        let (nc, nr, na) = corners[(i + 1) % 4];
        let (from, to) = (on(c, rad, a0 + 90.0), on(nc, nr, na));
        let n = ((to - from).length() / STEP) as usize;
        pts.extend((1..n).map(|k| from + (to - from) * (k as f32 / n as f32)));
    }
    pts
}

/// Rounded border whose colour follows `color(pos)`, drawn inside `rect`.
pub fn border(
    rect: Rect,
    radius: impl Into<CornerRadius>,
    width: f32,
    color: impl Fn(Pos2) -> Color32 + Send + Sync + 'static,
) -> Shape {
    let radius = radius.into();
    let inset = (width / 2.0).round() as u8;
    let inner = CornerRadius {
        nw: radius.nw.saturating_sub(inset),
        ne: radius.ne.saturating_sub(inset),
        sw: radius.sw.saturating_sub(inset),
        se: radius.se.saturating_sub(inset),
    };
    let path = outline(rect.shrink(width / 2.0), inner);
    let color = ColorMode::UV(Arc::new(move |_: Rect, p: Pos2| color(p)));
    Shape::closed_line(path, PathStroke { width, color, kind: StrokeKind::Middle })
}

pub fn gradient_border(
    painter: &Painter,
    rect: Rect,
    radius: impl Into<CornerRadius>,
    width: f32,
    color: impl Fn(Pos2) -> Color32 + Send + Sync + 'static,
) {
    painter.add(border(rect, radius, width, color));
}

/// Edge light: bright along the top, fading over the first `fade` points.
pub fn edge_light(rect: Rect, radius: impl Into<CornerRadius>, top_alpha: f32, bottom_alpha: f32) -> Shape {
    let (top, h) = (rect.top(), rect.height().clamp(1.0, 220.0));
    border(rect, radius, 1.0, move |p| {
        let t = ((p.y - top) / h).clamp(0.0, 1.0);
        Color32::from_white_alpha((top_alpha + (bottom_alpha - top_alpha) * t) as u8)
    })
}

/// Border sweeping through violet → cyan → pink as it turns around the centre.
pub fn conic(rect: Rect, t: f64, alpha: f32) -> impl Fn(Pos2) -> Color32 + Send + Sync + 'static {
    let c = rect.center();
    let phase = (t * 0.6) as f32;
    move |p| {
        let a = ((p - c).angle() + phase).rem_euclid(TAU) / TAU;
        let col = match a {
            a if a < 1.0 / 3.0 => mix(ACCENT, CYAN, a * 3.0),
            a if a < 2.0 / 3.0 => mix(CYAN, PINK, a * 3.0 - 1.0),
            a => mix(PINK, ACCENT, a * 3.0 - 2.0),
        };
        col.gamma_multiply(alpha)
    }
}

/// Frosted glass: translucent fill, a soft top sheen and an edge light brighter at the top.
pub fn glass(painter: &Painter, rect: Rect, radius: f32, fill: Color32) {
    painter.rect_filled(rect, radius, fill);
    let sheen = Rect::from_min_size(rect.min, vec2(rect.width(), rect.height().min(120.0)));
    let r = radius as u8;
    let corners = CornerRadius { nw: r, ne: r, sw: 0, se: 0 };
    let light = Color32::from_white_alpha(8);
    paint_with(
        painter,
        RectShape::filled(sheen, corners, Color32::WHITE),
        linear(sheen, light, Color32::TRANSPARENT, Vec2::DOWN),
    );
    painter.add(edge_light(rect, radius, 38.0, 14.0));
}

/// Soft drop shadow under a floating surface.
pub fn shadow(painter: &Painter, rect: Rect, radius: f32, strength: u8) {
    let s = Shadow { offset: [0, 10], blur: 34, spread: 0, color: Color32::from_black_alpha(strength) };
    painter.add(s.as_shape(rect, radius));
}

/// Coloured halo around a rect: light, so it brightens what is under it.
pub fn glow(painter: &Painter, rect: Rect, radius: f32, color: Color32, blur: u8) {
    painter.add(halo(rect, radius, color, blur));
}

pub fn halo(rect: Rect, radius: impl Into<CornerRadius>, color: Color32, blur: u8) -> Shape {
    Shape::Rect(Shadow { offset: [0, 0], blur, spread: 0, color: additive(color) }.as_shape(rect, radius))
}

/// Horizontal hairline fading out at both ends.
pub fn hairline(painter: &Painter, left: f32, right: f32, y: f32, color: Color32) {
    let mid = (left + right) / 2.0;
    let l = Rect::from_min_max(Pos2::new(left, y), Pos2::new(mid, y + 1.0));
    let r = Rect::from_min_max(Pos2::new(mid, y), Pos2::new(right, y + 1.0));
    paint_with(painter, RectShape::filled(l, 0.0, Color32::WHITE), linear(l, Color32::TRANSPARENT, color, Vec2::RIGHT));
    paint_with(painter, RectShape::filled(r, 0.0, Color32::WHITE), linear(r, color, Color32::TRANSPARENT, Vec2::RIGHT));
}

/// Radial light blob: Gaussian falloff to fully transparent at `r`.
fn radial(mesh: &mut Mesh, c: Pos2, r: f32, color: Color32, vignette: bool) {
    const RINGS: u32 = 14;
    const SEG: u32 = 56;
    let base = mesh.vertices.len() as u32;
    let center = if vignette { Color32::TRANSPARENT } else { color };
    mesh.colored_vertex(c, center);
    for k in 1..=RINGS {
        let f = k as f32 / RINGS as f32;
        let w = if vignette {
            let x = ((f - 0.45) / 0.55).clamp(0.0, 1.0);
            x * x * (3.0 - 2.0 * x)
        } else {
            (-3.2 * f * f).exp() * (1.0 - f * f)
        };
        let col = color.gamma_multiply(w);
        for s in 0..SEG {
            let a = s as f32 / SEG as f32 * TAU;
            mesh.colored_vertex(c + vec2(a.cos(), a.sin()) * r * f, col);
        }
    }
    for s in 0..SEG {
        mesh.add_triangle(base, base + 1 + s, base + 1 + (s + 1) % SEG);
    }
    for k in 1..RINGS {
        let (inner, outer) = (base + 1 + (k - 1) * SEG, base + 1 + k * SEG);
        for s in 0..SEG {
            let n = (s + 1) % SEG;
            mesh.add_triangle(inner + s, outer + s, outer + n);
            mesh.add_triangle(inner + s, outer + n, inner + n);
        }
    }
}

/// Aurora light source, positions and radius relative to the window.
struct Light {
    x: f32,
    y: f32,
    drift: Vec2,
    radius: f32,
    color: Color32,
    intensity: f32,
    speed: f64,
    phase: f64,
}

const LIGHTS: [Light; 5] = [
    Light { x: 0.10, y: 0.08, drift: vec2(0.07, 0.06), radius: 0.62, color: ACCENT, intensity: 0.30, speed: 0.050, phase: 0.0 },
    Light { x: 0.55, y: -0.05, drift: vec2(0.10, 0.05), radius: 0.50, color: INDIGO, intensity: 0.20, speed: 0.037, phase: 1.7 },
    Light { x: 0.95, y: 0.90, drift: vec2(0.06, 0.08), radius: 0.60, color: CYAN, intensity: 0.19, speed: 0.043, phase: 3.1 },
    Light { x: 0.78, y: 0.30, drift: vec2(0.08, 0.10), radius: 0.42, color: PINK, intensity: 0.10, speed: 0.031, phase: 4.4 },
    Light {
        x: 0.28,
        y: 1.00,
        drift: vec2(0.10, 0.04),
        radius: 0.55,
        color: Color32::from_rgb(0x5B, 0x21, 0xB6),
        intensity: 0.24,
        speed: 0.047,
        phase: 2.2,
    },
];

/// Window background: deep night, drifting aurora, vignette.
pub fn backdrop(ctx: &Context, rect: Rect, t: f64) {
    let painter = ctx.layer_painter(LayerId::background());
    painter.rect_filled(rect, 0.0, BG);
    let size = rect.width().max(rect.height());
    let mut mesh = Mesh::default();
    for l in &LIGHTS {
        let p = t * l.speed * std::f64::consts::TAU + l.phase;
        let c = rect.min
            + vec2((l.x + l.drift.x * p.sin() as f32) * rect.width(), (l.y + l.drift.y * (p * 0.8).cos() as f32) * rect.height());
        let breathe = 0.85 + 0.15 * (p * 1.3).sin() as f32;
        radial(&mut mesh, c, l.radius * size, additive(l.color.gamma_multiply(l.intensity * breathe)), false);
    }
    radial(&mut mesh, rect.center(), rect.size().length() * 0.62, Color32::from_black_alpha(150), true);
    painter.add(Shape::mesh(mesh));
}

/// Rotating gradient arc.
pub fn spinner(painter: &Painter, center: Pos2, r: f32, width: f32, t: f64) {
    let start = (t * 5.2) as f32 % TAU;
    let sweep = 4.4;
    let points: Vec<Pos2> =
        (0..=32).map(|i| start + sweep * i as f32 / 32.0).map(|a| center + vec2(a.cos(), a.sin()) * r).collect();
    let color = ColorMode::UV(Arc::new(move |_: Rect, p: Pos2| {
        let along = ((p - center).angle() - start).rem_euclid(TAU) / sweep;
        mix(ACCENT, CYAN, along).gamma_multiply(along.clamp(0.0, 1.0).powf(0.7))
    }));
    painter.add(Shape::line(points, PathStroke { width, color, kind: StrokeKind::Middle }));
}

/// Text with a light band sweeping across it.
pub fn shimmer(text: &str, font: FontId, base: Color32, light: Color32, t: f64) -> LayoutJob {
    let n = text.chars().count().max(1) as f32;
    let head = ((t * 0.55).fract() as f32) * (n + 12.0) - 6.0;
    let mut job = LayoutJob::default();
    for (i, ch) in text.chars().enumerate() {
        let k = (1.0 - (i as f32 - head).abs() / 5.0).clamp(0.0, 1.0);
        let format = TextFormat { font_id: font.clone(), color: mix(base, light, k * k * (3.0 - 2.0 * k)), ..Default::default() };
        job.append(ch.encode_utf8(&mut [0; 4]), 0.0, format);
    }
    job
}

/// Text coloured character by character from `from` to `to`.
pub fn gradient_text(text: &str, font: FontId, from: Color32, to: Color32) -> LayoutJob {
    let n = (text.chars().count().max(2) - 1) as f32;
    let mut job = LayoutJob::default();
    for (i, ch) in text.chars().enumerate() {
        let format = TextFormat { font_id: font.clone(), color: mix(from, to, i as f32 / n), ..Default::default() };
        job.append(ch.encode_utf8(&mut [0; 4]), 0.0, format);
    }
    job
}

/// Uppercase label with extra letter spacing.
pub fn spaced(text: &str, font: FontId, color: Color32, spacing: f32) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.append(text, 0.0, TextFormat { font_id: font, color, extra_letter_spacing: spacing, ..Default::default() });
    job
}
