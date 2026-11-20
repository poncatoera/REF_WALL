//! Glyph outlines as quadratic Bézier curves, in em units (1.0 = font size).

/// One quadratic Bézier: start, control, end.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quad {
    pub p0: [f32; 2],
    pub p1: [f32; 2],
    pub p2: [f32; 2],
}

impl Quad {
    pub fn line(a: [f32; 2], b: [f32; 2]) -> Self {
        Self { p0: a, p1: [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5], p2: b }
    }

    pub fn min(&self, axis: usize) -> f32 {
        self.p0[axis].min(self.p1[axis]).min(self.p2[axis])
    }

    pub fn max(&self, axis: usize) -> f32 {
        self.p0[axis].max(self.p1[axis]).max(self.p2[axis])
    }

    /// Curves flat along `axis` never cross a ray along that axis; skip them.
    pub fn is_flat(&self, axis: usize) -> bool {
        self.p0[axis] == self.p1[axis] && self.p1[axis] == self.p2[axis]
    }
}

/// Collects a glyph outline from `ttf-parser`, converting lines and cubics
/// (CFF / OpenType fonts) into quadratics.
pub struct OutlineCollector {
    pub curves: Vec<Quad>,
    scale: f32,
    start: [f32; 2],
    last: [f32; 2],
}

impl OutlineCollector {
    pub fn new(units_per_em: u16) -> Self {
        Self { curves: Vec::new(), scale: 1.0 / units_per_em as f32, start: [0.0; 2], last: [0.0; 2] }
    }

    fn pt(&self, x: f32, y: f32) -> [f32; 2] {
        [x * self.scale, y * self.scale]
    }

    fn push(&mut self, q: Quad) {
        // Drop degenerate curves (zero length).
        if q.p0 != q.p2 || q.p0 != q.p1 {
            self.curves.push(q);
        }
    }
}

/// Cubic → quadratics. Splits until each piece is close to one quadratic.
fn cubic_to_quads(p0: [f32; 2], c1: [f32; 2], c2: [f32; 2], p3: [f32; 2], out: &mut Vec<Quad>, depth: u32) {
    // Error of approximating with a single quad is proportional to
    // |p3 - 3c2 + 3c1 - p0|.
    let ex = p3[0] - 3.0 * c2[0] + 3.0 * c1[0] - p0[0];
    let ey = p3[1] - 3.0 * c2[1] + 3.0 * c1[1] - p0[1];
    let err = (ex * ex + ey * ey).sqrt() * (3.0f32.sqrt() / 36.0);
    if err < 1e-4 || depth >= 6 {
        let q1 = [
            (3.0 * (c1[0] + c2[0]) - p0[0] - p3[0]) * 0.25,
            (3.0 * (c1[1] + c2[1]) - p0[1] - p3[1]) * 0.25,
        ];
        out.push(Quad { p0, p1: q1, p2: p3 });
        return;
    }
    let mid = |a: [f32; 2], b: [f32; 2]| [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5];
    let (ab, bc, cd) = (mid(p0, c1), mid(c1, c2), mid(c2, p3));
    let (abc, bcd) = (mid(ab, bc), mid(bc, cd));
    let m = mid(abc, bcd);
    cubic_to_quads(p0, ab, abc, m, out, depth + 1);
    cubic_to_quads(m, bcd, cd, p3, out, depth + 1);
}

impl ttf_parser::OutlineBuilder for OutlineCollector {
    fn move_to(&mut self, x: f32, y: f32) {
        self.start = self.pt(x, y);
        self.last = self.start;
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let p = self.pt(x, y);
        self.push(Quad::line(self.last, p));
        self.last = p;
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let (c, p) = (self.pt(x1, y1), self.pt(x, y));
        self.push(Quad { p0: self.last, p1: c, p2: p });
        self.last = p;
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let (c1, c2, p) = (self.pt(x1, y1), self.pt(x2, y2), self.pt(x, y));
        let mut quads = Vec::new();
        cubic_to_quads(self.last, c1, c2, p, &mut quads, 0);
        for q in quads {
            self.push(q);
        }
        self.last = p;
    }

    fn close(&mut self) {
        if self.last != self.start {
            self.push(Quad::line(self.last, self.start));
        }
        self.last = self.start;
    }
}
