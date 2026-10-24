//! Vector icons drawn on a 24-unit grid (Lucide-like strokes): crisp at any scale, no font glyph lottery.

use std::f32::consts::PI;

use eframe::egui::{Color32, Mesh, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, Vec2, vec2};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Icon {
    Plus,
    Search,
    Close,
    ArrowUp,
    Stop,
    Folder,
    Gear,
    Trash,
    Pencil,
    Undo,
    Copy,
    Refresh,
    Image,
    Clock,
    Lock,
    Unlock,
    Download,
    PanelLeft,
    PanelRight,
    Sparkle,
    Shield,
    Globe,
    Flask,
    Layers,
    Check,
    Alert,
    Bolt,
    Bubble,
    Eye,
    EyeOff,
    Terminal,
    File,
    Cube,
    Graph,
    Clipboard,
    Book,
    Key,
    Star,
    Minimize,
    Maximize,
    Restore,
}

/// Paints `icon` centred in `rect`, scaled to its smaller side.
pub fn paint(p: &Painter, rect: Rect, icon: Icon, color: Color32) {
    let s = rect.width().min(rect.height()) / 24.0;
    let o = rect.center() - vec2(12.0 * s, 12.0 * s);
    let width = (1.9 * s).max(1.1);
    Pen { p, o, s, stroke: Stroke::new(width, color) }.draw(icon);
}

/// Small icon at a point, the size given in points.
pub fn at(p: &Painter, center: Pos2, size: f32, icon: Icon, color: Color32) {
    paint(p, Rect::from_center_size(center, vec2(size, size)), icon, color);
}

/// Chevron rotated by `turn` (0 = pointing right, 1 = pointing down), for animated disclosure.
pub fn chevron(p: &Painter, center: Pos2, size: f32, turn: f32, color: Color32) {
    let angle = turn * PI / 2.0;
    let rot = |v: Vec2| vec2(v.x * angle.cos() - v.y * angle.sin(), v.x * angle.sin() + v.y * angle.cos());
    let k = size / 24.0;
    let pts = [vec2(-3.0, -6.0), vec2(3.0, 0.0), vec2(-3.0, 6.0)].map(|v| center + rot(v * k));
    p.add(Shape::line(pts.to_vec(), Stroke::new((2.0 * k).max(1.2), color)));
}

struct Pen<'a> {
    p: &'a Painter,
    o: Pos2,
    s: f32,
    stroke: Stroke,
}

impl Pen<'_> {
    fn pt(&self, x: f32, y: f32) -> Pos2 {
        self.o + vec2(x * self.s, y * self.s)
    }

    /// Open polyline with round caps.
    fn line(&self, pts: &[(f32, f32)]) {
        let pts: Vec<Pos2> = pts.iter().map(|&(x, y)| self.pt(x, y)).collect();
        let cap = self.stroke.width / 2.0;
        if let (Some(a), Some(b)) = (pts.first(), pts.last()) {
            self.p.circle_filled(*a, cap, self.stroke.color);
            self.p.circle_filled(*b, cap, self.stroke.color);
        }
        self.p.add(Shape::line(pts, self.stroke));
    }

    fn closed(&self, pts: &[(f32, f32)]) {
        self.p.add(Shape::closed_line(pts.iter().map(|&(x, y)| self.pt(x, y)).collect(), self.stroke));
    }

    /// Star-shaped polygon filled from its first point's centroid.
    fn fill(&self, pts: &[(f32, f32)]) {
        let pts: Vec<Pos2> = pts.iter().map(|&(x, y)| self.pt(x, y)).collect();
        let c = pts.iter().fold(Vec2::ZERO, |acc, p| acc + p.to_vec2()) / pts.len() as f32;
        let mut mesh = Mesh::default();
        mesh.colored_vertex(c.to_pos2(), self.stroke.color);
        for p in &pts {
            mesh.colored_vertex(*p, self.stroke.color);
        }
        let n = pts.len() as u32;
        for i in 0..n {
            mesh.add_triangle(0, 1 + i, 1 + (i + 1) % n);
        }
        self.p.add(Shape::mesh(mesh));
        self.p.add(Shape::closed_line(pts, Stroke::new(self.stroke.width * 0.6, self.stroke.color)));
    }

    fn circle(&self, x: f32, y: f32, r: f32) {
        self.p.circle_stroke(self.pt(x, y), r * self.s, self.stroke);
    }

    fn dot(&self, x: f32, y: f32, r: f32) {
        self.p.circle_filled(self.pt(x, y), r * self.s, self.stroke.color);
    }

    fn rect(&self, x0: f32, y0: f32, x1: f32, y1: f32, r: f32) {
        let rect = Rect::from_min_max(self.pt(x0, y0), self.pt(x1, y1));
        self.p.rect_stroke(rect, r * self.s, self.stroke, StrokeKind::Middle);
    }

    /// Arc points, angles in degrees, clockwise on screen.
    fn arc(&self, cx: f32, cy: f32, r: f32, from: f32, to: f32) -> Vec<(f32, f32)> {
        let n = ((to - from).abs() / 12.0).ceil().max(2.0) as usize;
        (0..=n)
            .map(|i| (from + (to - from) * i as f32 / n as f32).to_radians())
            .map(|a| (cx + r * a.cos(), cy + r * a.sin()))
            .collect()
    }

    fn draw(&self, icon: Icon) {
        match icon {
            Icon::Plus => {
                self.line(&[(12.0, 5.0), (12.0, 19.0)]);
                self.line(&[(5.0, 12.0), (19.0, 12.0)]);
            }
            Icon::Search => {
                self.circle(11.0, 11.0, 7.0);
                self.line(&[(16.2, 16.2), (20.5, 20.5)]);
            }
            Icon::Close => {
                self.line(&[(6.0, 6.0), (18.0, 18.0)]);
                self.line(&[(18.0, 6.0), (6.0, 18.0)]);
            }
            Icon::ArrowUp => {
                self.line(&[(12.0, 19.0), (12.0, 5.5)]);
                self.line(&[(5.5, 11.5), (12.0, 5.0), (18.5, 11.5)]);
            }
            Icon::Stop => {
                let rect = Rect::from_min_max(self.pt(7.0, 7.0), self.pt(17.0, 17.0));
                self.p.rect_filled(rect, 2.5 * self.s, self.stroke.color);
            }
            Icon::Folder => self.closed(&[
                (4.0, 4.5),
                (9.0, 4.5),
                (11.0, 7.0),
                (20.0, 7.0),
                (21.5, 8.5),
                (21.5, 18.0),
                (20.0, 19.5),
                (4.0, 19.5),
                (2.5, 18.0),
                (2.5, 6.0),
            ]),
            Icon::Gear => {
                let mut pts = Vec::new();
                for i in 0..8 {
                    let a = i as f32 * 45.0;
                    for (r, da) in [(7.4, -16.0), (9.6, -9.0), (9.6, 9.0), (7.4, 16.0)] {
                        let t = (a + da).to_radians();
                        pts.push((12.0 + r * t.cos(), 12.0 + r * t.sin()));
                    }
                }
                self.closed(&pts);
                self.circle(12.0, 12.0, 3.0);
            }
            Icon::Trash => {
                self.line(&[(3.5, 6.5), (20.5, 6.5)]);
                self.line(&[(18.5, 6.5), (17.6, 19.2), (16.2, 20.5), (7.8, 20.5), (6.4, 19.2), (5.5, 6.5)]);
                self.line(&[(9.0, 6.5), (9.0, 4.5), (10.0, 3.5), (14.0, 3.5), (15.0, 4.5), (15.0, 6.5)]);
                self.line(&[(10.0, 11.0), (10.0, 16.5)]);
                self.line(&[(14.0, 11.0), (14.0, 16.5)]);
            }
            Icon::Pencil => {
                self.closed(&[(16.5, 3.5), (20.5, 7.5), (8.0, 20.0), (3.5, 20.5), (4.0, 16.0)]);
                self.line(&[(14.5, 5.5), (18.5, 9.5)]);
            }
            Icon::Undo => {
                self.line(&[(9.0, 14.0), (4.0, 9.0), (9.0, 4.0)]);
                let mut path = vec![(4.0, 9.0)];
                path.extend(self.arc(14.5, 14.5, 5.5, -90.0, 90.0));
                path.push((11.0, 20.0));
                self.line(&path);
            }
            Icon::Copy => {
                self.rect(8.0, 8.0, 21.0, 21.0, 2.0);
                self.line(&[(4.5, 16.0), (3.0, 14.5), (3.0, 4.5), (4.5, 3.0), (14.5, 3.0), (16.0, 4.5)]);
            }
            Icon::Refresh => {
                self.line(&self.arc(12.0, 12.0, 8.5, 30.0, 320.0));
                self.line(&[(19.8, 2.8), (19.8, 7.6), (15.0, 7.6)]);
            }
            Icon::Image => {
                self.rect(3.0, 3.0, 21.0, 21.0, 2.5);
                self.circle(9.0, 9.0, 2.0);
                self.line(&[(21.0, 15.0), (16.0, 10.0), (5.0, 21.0)]);
            }
            Icon::Clock => {
                self.circle(12.0, 12.0, 9.5);
                self.line(&[(12.0, 7.0), (12.0, 12.0), (15.5, 14.0)]);
            }
            Icon::Lock | Icon::Unlock => {
                self.rect(4.5, 11.0, 19.5, 21.0, 2.5);
                let end = if icon == Icon::Lock { 360.0 } else { 325.0 };
                let mut path = vec![(8.0, 11.0)];
                path.extend(self.arc(12.0, 7.5, 4.0, 180.0, end));
                if icon == Icon::Lock {
                    path.push((16.0, 11.0));
                }
                self.line(&path);
            }
            Icon::Download => {
                self.line(&[(3.5, 15.0), (3.5, 19.0), (5.0, 20.5), (19.0, 20.5), (20.5, 19.0), (20.5, 15.0)]);
                self.line(&[(7.5, 10.5), (12.0, 15.0), (16.5, 10.5)]);
                self.line(&[(12.0, 15.0), (12.0, 3.5)]);
            }
            Icon::PanelLeft | Icon::PanelRight => {
                self.rect(3.0, 3.5, 21.0, 20.5, 2.5);
                let x = if icon == Icon::PanelLeft { 9.0 } else { 15.0 };
                self.line(&[(x, 3.5), (x, 20.5)]);
            }
            Icon::Sparkle => {
                self.fill(&[
                    (12.0, 2.5),
                    (13.9, 10.1),
                    (21.5, 12.0),
                    (13.9, 13.9),
                    (12.0, 21.5),
                    (10.1, 13.9),
                    (2.5, 12.0),
                    (10.1, 10.1),
                ]);
            }
            Icon::Shield => self.closed(&[
                (12.0, 21.5),
                (7.5, 19.0),
                (4.6, 15.6),
                (4.0, 12.0),
                (4.0, 5.5),
                (12.0, 2.5),
                (20.0, 5.5),
                (20.0, 12.0),
                (19.4, 15.6),
                (16.5, 19.0),
            ]),
            Icon::Globe => {
                self.circle(12.0, 12.0, 9.5);
                self.line(&[(2.5, 12.0), (21.5, 12.0)]);
                let ellipse: Vec<(f32, f32)> = (0..=36)
                    .map(|i| (i as f32 * 10.0).to_radians())
                    .map(|a| (12.0 + 4.2 * a.cos(), 12.0 + 9.5 * a.sin()))
                    .collect();
                self.closed(&ellipse);
            }
            Icon::Flask => {
                self.line(&[
                    (9.5, 3.0),
                    (9.5, 9.0),
                    (4.5, 18.5),
                    (5.5, 20.5),
                    (18.5, 20.5),
                    (19.5, 18.5),
                    (14.5, 9.0),
                    (14.5, 3.0),
                ]);
                self.line(&[(8.0, 3.0), (16.0, 3.0)]);
                self.line(&[(7.0, 15.0), (17.0, 15.0)]);
            }
            Icon::Layers => {
                self.closed(&[(12.0, 3.0), (21.5, 8.0), (12.0, 13.0), (2.5, 8.0)]);
                self.line(&[(2.5, 12.5), (12.0, 17.5), (21.5, 12.5)]);
                self.line(&[(2.5, 16.5), (12.0, 21.5), (21.5, 16.5)]);
            }
            Icon::Check => self.line(&[(5.0, 12.5), (10.0, 17.5), (19.5, 7.0)]),
            Icon::Alert => {
                self.closed(&[(12.0, 3.5), (21.5, 20.0), (2.5, 20.0)]);
                self.line(&[(12.0, 9.5), (12.0, 14.0)]);
                self.dot(12.0, 17.0, 1.2);
            }
            Icon::Bolt => self.closed(&[(13.0, 2.5), (4.0, 13.5), (12.0, 13.5), (11.0, 21.5), (20.0, 10.5), (12.0, 10.5)]),
            Icon::Bubble => self.closed(&[
                (4.5, 4.5),
                (19.5, 4.5),
                (21.0, 6.0),
                (21.0, 15.0),
                (19.5, 16.5),
                (10.5, 16.5),
                (6.5, 20.0),
                (6.5, 16.5),
                (4.5, 16.5),
                (3.0, 15.0),
                (3.0, 6.0),
            ]),
            Icon::Eye | Icon::EyeOff => {
                let mut lens = self.arc(12.0, 19.0, 12.0, 215.0, 325.0);
                lens.extend(self.arc(12.0, 5.0, 12.0, 35.0, 145.0));
                self.closed(&lens);
                self.circle(12.0, 12.0, 3.0);
                if icon == Icon::EyeOff {
                    self.line(&[(3.0, 3.0), (21.0, 21.0)]);
                }
            }
            Icon::Terminal => {
                self.line(&[(4.0, 17.0), (10.0, 11.0), (4.0, 5.0)]);
                self.line(&[(12.0, 19.0), (20.0, 19.0)]);
            }
            Icon::File => {
                self.closed(&[(6.0, 2.5), (14.0, 2.5), (19.5, 8.0), (19.5, 21.5), (6.0, 21.5), (4.5, 20.0), (4.5, 4.0)]);
                self.line(&[(14.0, 2.5), (14.0, 8.0), (19.5, 8.0)]);
            }
            Icon::Cube => {
                self.closed(&[(12.0, 2.5), (20.5, 7.0), (20.5, 17.0), (12.0, 21.5), (3.5, 17.0), (3.5, 7.0)]);
                self.line(&[(3.5, 7.0), (12.0, 11.5), (20.5, 7.0)]);
                self.line(&[(12.0, 11.5), (12.0, 21.5)]);
            }
            Icon::Graph => {
                self.line(&[(8.0, 7.5), (16.0, 8.0)]);
                self.line(&[(7.0, 8.5), (9.5, 16.0)]);
                self.line(&[(16.5, 10.0), (12.0, 16.5)]);
                self.circle(6.0, 6.0, 2.6);
                self.circle(18.0, 8.0, 2.6);
                self.circle(10.5, 18.5, 2.6);
            }
            Icon::Clipboard => {
                self.rect(5.0, 4.5, 19.0, 21.5, 2.0);
                self.rect(9.0, 2.5, 15.0, 6.5, 1.0);
            }
            Icon::Book => {
                self.closed(&[
                    (3.0, 4.5),
                    (9.0, 4.5),
                    (12.0, 7.0),
                    (15.0, 4.5),
                    (21.0, 4.5),
                    (21.0, 18.5),
                    (15.0, 18.5),
                    (12.0, 21.0),
                    (9.0, 18.5),
                    (3.0, 18.5),
                ]);
                self.line(&[(12.0, 7.0), (12.0, 21.0)]);
            }
            Icon::Key => {
                self.circle(8.0, 15.5, 4.5);
                self.line(&[(11.3, 12.2), (20.0, 3.5)]);
                self.line(&[(17.0, 6.5), (19.5, 9.0)]);
                self.line(&[(14.5, 9.0), (16.5, 11.0)]);
            }
            Icon::Star => {
                let pts: Vec<(f32, f32)> = (0..10)
                    .map(|i| {
                        let a = (-90.0 + i as f32 * 36.0).to_radians();
                        let r = if i % 2 == 0 { 9.8 } else { 4.4 };
                        (12.0 + r * a.cos(), 12.6 + r * a.sin())
                    })
                    .collect();
                self.closed(&pts);
            }
            Icon::Minimize => self.line(&[(6.0, 12.0), (18.0, 12.0)]),
            Icon::Maximize => self.rect(6.0, 6.0, 18.0, 18.0, 1.5),
            Icon::Restore => {
                self.rect(6.0, 9.0, 15.0, 18.0, 1.5);
                self.line(&[(9.0, 9.0), (9.0, 6.0), (18.0, 6.0), (18.0, 15.0), (15.0, 15.0)]);
            }
        }
    }
}
