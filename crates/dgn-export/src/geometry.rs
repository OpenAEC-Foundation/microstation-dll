//! Curves, B-splines, triangulation and ribbons.

use dgn_model::{BSplineCurve, BSplineSurface, Point};
use glam::{DVec3, DVec4};

pub(crate) fn v(p: Point) -> DVec3 { DVec3::from_array(p) }

/// Points of an elliptical arc: `center + a cos t + b sin t`.
pub(crate) fn ellipse(center: DVec3, a: DVec3, b: DVec3, start: f64, sweep: f64) -> Vec<DVec3> {
    let segments = ((sweep.abs() / std::f64::consts::TAU * 72.0).ceil() as usize).clamp(4, 72);
    (0..=segments).map(|k| {
        let t = start + sweep * k as f64 / segments as f64;
        center + a * t.cos() + b * t.sin()
    }).collect()
}

/// The ISFF curve (type 11): piecewise cubics through points 2..n-3 with Akima-style tangents;
/// the first two and last two points only shape the ends.
pub(crate) fn akima(points: &[DVec3]) -> Vec<DVec3> {
    let n = points.len();
    if n < 6 { return points.to_vec(); }
    let lengths: Vec<f64> = points.windows(2).map(|p| p[0].distance(p[1])).collect();
    let slope = |k: usize| if lengths[k] > 0.0 { (points[k + 1] - points[k]) / lengths[k] } else { DVec3::ZERO };
    let tangent = |k: usize| {
        let (m2, m1, m0, p1) = (slope(k - 2), slope(k - 1), slope(k), slope(k + 1));
        let axis = |a: f64, b: f64, c: f64, d: f64| {
            let (w1, w2) = ((d - c).abs(), (b - a).abs());
            if w1 + w2 > 0.0 { (b * w1 + c * w2) / (w1 + w2) } else { (b + c) / 2.0 }
        };
        DVec3::new(axis(m2.x, m1.x, m0.x, p1.x), axis(m2.y, m1.y, m0.y, p1.y), axis(m2.z, m1.z, m0.z, p1.z))
    };
    let mut out = vec![points[2]];
    for k in 2..n - 3 {
        let length = lengths[k];
        if length <= 0.0 { continue; }
        let (t0, t1, p0, p1) = (tangent(k), tangent(k + 1), points[k], points[k + 1]);
        let chord = (p1 - p0) / length;
        let b = (chord * 3.0 - t0 * 2.0 - t1) / length;
        let a = (t0 + t1 - chord * 2.0) / (length * length);
        for s in 1..=8 {
            let d = length * f64::from(s) / 8.0;
            out.push(p0 + t0 * d + b * d * d + a * d * d * d);
        }
    }
    out
}

/// De Boor's algorithm on homogeneous poles.
fn de_boor(poles: &[DVec4], knots: &[f64], order: usize, t: f64) -> DVec4 {
    let n = poles.len();
    let mut s = order - 1;
    while s + 1 < n && knots[s + 1] <= t { s += 1; }
    let mut d: Vec<DVec4> = (0..order).map(|j| poles[j + s + 1 - order]).collect();
    for r in 1..order {
        for j in (r..order).rev() {
            let i = j + s + 1 - order;
            let denominator = knots[i + order - r] - knots[i];
            let alpha = if denominator.abs() < 1e-15 { 0.0 } else { (t - knots[i]) / denominator };
            d[j] = d[j - 1] * (1.0 - alpha) + d[j] * alpha;
        }
    }
    d[order - 1]
}

/// Samples a B-spline over its domain; `poles` already include a periodic wrap.
fn sample(poles: &[DVec4], knots: &[f64], order: usize, samples: usize) -> Vec<DVec4> {
    let (lo, hi) = (knots[order - 1], knots[poles.len()]);
    (0..=samples).map(|k| de_boor(poles, knots, order, lo + (hi - lo) * k as f64 / samples as f64)).collect()
}

fn wrap(mut poles: Vec<DVec4>, order: usize, periodic: bool) -> Vec<DVec4> {
    if periodic { for k in 0..order - 1 { poles.push(poles[k % poles.len()]); } }
    poles
}

fn homogeneous(poles: &[Point], weights: Option<&[f64]>) -> Vec<DVec4> {
    poles.iter().enumerate().map(|(k, &p)| {
        let w = weights.and_then(|w| w.get(k)).copied().unwrap_or(1.0);
        (v(p) * w).extend(w)
    }).collect()
}

/// A knot vector that fits `poles` (after any wrap) of `order`.
fn fits(knots: &[f64], poles: usize, order: usize) -> bool { order >= 1 && knots.len() == poles + order }

/// Points along a B-spline curve; with fewer poles than the order, the control polygon.
pub(crate) fn bspline_points(curve: &BSplineCurve) -> Vec<DVec3> {
    let poles = homogeneous(&curve.poles, curve.weights.as_deref());
    if poles.len() < curve.order || poles.is_empty() { return poles.iter().map(|p| p.truncate() / p.w).collect(); }
    let poles = wrap(poles, curve.order, curve.periodic);
    if !fits(&curve.knots, poles.len(), curve.order) { return curve.poles.iter().map(|&p| v(p)).collect(); }
    let samples = (poles.len() * 8).clamp(16, 400);
    sample(&poles, &curve.knots, curve.order, samples).into_iter().map(|p| p.truncate() / p.w).collect()
}

/// Triangles of a B-spline surface, sampled on a grid.
pub(crate) fn bspline_triangles(s: &BSplineSurface) -> Vec<[DVec3; 3]> {
    let (nu, nv) = (s.rows.first().map_or(0, Vec::len), s.rows.len());
    let wrapped_u = nu + if s.periodic_u { s.order_u - 1 } else { 0 };
    let wrapped_v = nv + if s.periodic_v { s.order_v - 1 } else { 0 };
    if nu < s.order_u || nv < s.order_v || s.rows.iter().any(|r| r.len() != nu)
        || !fits(&s.knots_u, wrapped_u, s.order_u) || !fits(&s.knots_v, wrapped_v, s.order_v) {
        return Vec::new();
    }
    let (su, sv) = ((nu * 4).clamp(8, 64), (nv * 4).clamp(8, 64));
    let along_u: Vec<Vec<DVec4>> = s.rows.iter().enumerate().map(|(k, row)| {
        let weights = s.weights.as_ref().and_then(|w| w.get(k)).map(Vec::as_slice);
        sample(&wrap(homogeneous(row, weights), s.order_u, s.periodic_u), &s.knots_u, s.order_u, su)
    }).collect();
    let grid: Vec<Vec<DVec3>> = (0..=su).map(|k| {
        let column: Vec<DVec4> = along_u.iter().map(|row| row[k]).collect();
        sample(&wrap(column, s.order_v, s.periodic_v), &s.knots_v, s.order_v, sv).into_iter().map(|p| p.truncate() / p.w).collect()
    }).collect();
    let mut triangles = Vec::new();
    for a in 0..su {
        for b in 0..sv {
            let (p, q, r, t) = (grid[a][b], grid[a + 1][b], grid[a + 1][b + 1], grid[a][b + 1]);
            triangles.push([p, q, r]);
            triangles.push([p, r, t]);
        }
    }
    triangles
}

/// Triangulates a planar polygon (closing point optional).
pub(crate) fn triangulate(points: &[DVec3]) -> Vec<[DVec3; 3]> {
    let mut points = points.to_vec();
    points.dedup();
    if points.len() > 1 && points.first() == points.last() { points.pop(); }
    if points.len() < 3 { return Vec::new(); }
    let origin = points[0];
    let normal = (0..points.len()).fold(DVec3::ZERO, |sum, i| sum + (points[i] - origin).cross(points[(i + 1) % points.len()] - origin));
    if normal.length_squared() <= 1e-24 { return Vec::new(); }
    let axis = if normal.x.abs() >= normal.y.abs() && normal.x.abs() >= normal.z.abs() { 0 }
        else if normal.y.abs() >= normal.z.abs() { 1 } else { 2 };
    let coords: Vec<f64> = points.iter().flat_map(|p| {
        let p = *p - origin;
        match axis { 0 => [p.y, p.z], 1 => [p.x, p.z], _ => [p.x, p.y] }
    }).collect();
    let Ok(indices) = earcutr::earcut(&coords, &[], 2) else { return Vec::new() };
    indices.chunks_exact(3).map(|t| [points[t[0]], points[t[1]], points[t[2]]]).collect()
}

/// Half the width of a weight-0 line: a fixed share of the drawing extent, measured between the
/// 2nd and 98th percentile so a few stray far-away elements do not make every line fat.
pub(crate) fn line_half_width(points: &[DVec3]) -> f64 {
    let step = (points.len() / 100_000).max(1);
    let mut extent = DVec3::ZERO;
    for axis in 0..3 {
        let mut values: Vec<f64> = points.iter().step_by(step).map(|p| p[axis]).collect();
        if values.is_empty() { return 0.01; }
        values.sort_by(f64::total_cmp);
        let at = |f: f64| values[((values.len() - 1) as f64 * f).round() as usize];
        extent[axis] = at(0.98) - at(0.02);
    }
    let diagonal = extent.length();
    if diagonal > 0.0 { diagonal / 3000.0 } else { 0.01 }
}

/// Line colours as ink on paper: very light colours (white is colour 0) draw dark grey.
pub(crate) fn ink(color: [u8; 3]) -> [u8; 3] {
    let luma = 0.2126 * f64::from(color[0]) + 0.7152 * f64::from(color[1]) + 0.0722 * f64::from(color[2]);
    if luma > 215.0 { [45, 45, 48] } else { color }
}

/// A polyline as thin quads with square ends: flat (facing +Z) in 2D designs, two crossed quads
/// per segment in 3D designs. A single point becomes a small square.
pub(crate) fn ribbons(points: &[DVec3], half: f64, three_d: bool) -> Vec<([DVec3; 3], DVec3)> {
    let mut out = Vec::new();
    let mut quad = |a: DVec3, b: DVec3, offset: DVec3, normal: DVec3| {
        let (p, q, r, s) = (a + offset, b + offset, b - offset, a - offset);
        out.push(([p, q, r], normal));
        out.push(([p, r, s], normal));
    };
    let mut points = points.to_vec();
    points.dedup();
    if points.len() == 1 {
        let p = points[0];
        quad(p - DVec3::X * half * 2.0, p + DVec3::X * half * 2.0, DVec3::Y * half * 2.0, DVec3::Z);
        if three_d { quad(p - DVec3::X * half * 2.0, p + DVec3::X * half * 2.0, DVec3::Z * half * 2.0, DVec3::Y); }
        return out;
    }
    for pair in points.windows(2) {
        let direction = (pair[1] - pair[0]).normalize();
        let (a, b) = (pair[0] - direction * half, pair[1] + direction * half);
        if three_d {
            let side = if direction.cross(DVec3::Z).length_squared() > 1e-6 { direction.cross(DVec3::Z).normalize() } else { DVec3::X };
            let up = direction.cross(side).normalize();
            quad(a, b, side * half, up);
            quad(a, b, up * half, side);
        } else {
            quad(a, b, DVec3::new(-direction.y, direction.x, 0.0) * half, DVec3::Z);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn akima_curves_run_through_the_inner_points() {
        let curve = akima(&(0..8).map(|k| DVec3::new(k as f64, (k as f64 * 0.7).sin(), 0.0)).collect::<Vec<_>>());
        assert_eq!(curve.first().unwrap().x, 2.0);
        assert!((curve.last().unwrap().x - 5.0).abs() < 1e-9);
    }

    #[test]
    fn clamped_cubic_ends_on_its_end_poles() {
        let curve = BSplineCurve {
            order: 4, closed: false, periodic: false, rational: false,
            poles: vec![[0.0, 0.0, 0.0], [1.0, 2.0, 0.0], [3.0, 2.0, 0.0], [4.0, 0.0, 0.0]],
            weights: None, knots: vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0],
        };
        let points = bspline_points(&curve);
        assert_eq!(points.first().copied(), Some(DVec3::ZERO));
        assert!((points.last().unwrap().x - 4.0).abs() < 1e-12);
        assert!(points.iter().all(|p| p.y <= 1.5 + 1e-9), "a cubic stays inside its control polygon");
    }

    #[test]
    fn polygons_triangulate_and_lines_become_ribbons() {
        let square = [DVec3::ZERO, DVec3::X, DVec3::new(1.0, 1.0, 0.0), DVec3::Y, DVec3::ZERO];
        assert_eq!(triangulate(&square).len(), 2);
        assert_eq!(ribbons(&square[..2], 0.1, false).len(), 2);
        assert_eq!(ribbons(&square[..2], 0.1, true).len(), 4);
        assert_eq!(ribbons(&square[..1], 0.1, true).len(), 4, "a point is a crossed square");
        assert_eq!(ink([255, 255, 255]), [45, 45, 48]);
        assert_eq!(ink([0, 0, 255]), [0, 0, 255]);
    }
}
