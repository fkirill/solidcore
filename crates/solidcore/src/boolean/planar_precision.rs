//! Opt-in precision refinement for exact polyhedral operands crossing in a narrow band.
//!
//! Upstream: `source/SMLib/src/SmTopologyIntersector.cpp`,
//! `source/SMLib/src/SmProtoTopology.cpp` (WR-614 / WR-619; no upstream fix).
//!
//! | Rust | Upstream |
//! |------|----------|
//! | `refine` (crate) | none: own precision policy before the ordinary Boolean |
//!
//! Deviations: with `boolean_refine_planar_strips`, exact plane/line solids may use
//! smaller entity tolerances when nonparallel faces cross within their declared
//! tolerance. Geometry, identities and attributes are unchanged; the ordinary merge
//! then constructs the thin feature. Curved geometry and operands with gaps larger
//! than a conservative roundoff allowance keep their original tolerances. No
//! recursion, global state, retries or change to differential comparison tolerances.

use crate::curve::curve::p2::CurveKind;
use crate::error::Result;
use crate::geom::extent3d::Extent3d;
use crate::kernel::Kernel;
use crate::math::{Point2, Point3, Vec3};
use crate::surface::surface::p2::SurfaceKind;
use crate::topology::brep::p2::is_manifold_solid;
use crate::topology::face::{p3 as face_edit, p4 as face_query};
use crate::topology::{edgeuse::p1 as edgeuse, Brep, FaceId};

/// Coordinate-scaled allowance for evaluating a plane or a line's endpoints.
const ROUNDOFF_FACTOR: f64 = 128.0 * f64::EPSILON;

struct PlanarFace {
    origin: Point3,
    normal: Vec3,
    points: Vec<Point3>,
    bounds: Extent3d,
    tolerance: f64,
}

fn finite(p: Point3) -> bool {
    p.x.is_finite() && p.y.is_finite() && p.z.is_finite()
}

fn coordinate_scale(b: &Brep) -> Option<f64> {
    let mut scale: f64 = 0.0;
    for (_, vertex) in b.vertices().iter() {
        let p = vertex.point;
        if !finite(p) {
            return None;
        }
        scale = scale.max(p.x.abs()).max(p.y.abs()).max(p.z.abs());
    }
    (scale > 0.0).then_some(scale)
}

// All tests finish before either operand changes. A caller's declared tolerance is
// not evidence that its geometry is accurate enough to permit a smaller one.
fn exact_faces(k: &Kernel, b: &Brep, roundoff: f64) -> Result<Option<Vec<PlanarFace>>> {
    if !is_manifold_solid(b)? {
        return Ok(None);
    }
    for (_, edge) in b.edges().iter() {
        let Some(curve) = edge.curve.as_ref() else {
            return Ok(None);
        };
        if curve.dim() != 3 || !matches!(curve.kind(), CurveKind::Line(_)) {
            return Ok(None);
        }
        let [v0, v1] = edge.vertices;
        let (p0, p1) = (b.vertex(v0)?.point, b.vertex(v1)?.point);
        let q0 = curve.evaluate_point(k, edge.interval.min())?;
        let q1 = curve.evaluate_point(k, edge.interval.max())?;
        if !finite(q0) || !finite(q1) {
            return Ok(None);
        }
        let same = p0.distance(q0).max(p1.distance(q1));
        let opposite = p0.distance(q1).max(p1.distance(q0));
        if same.min(opposite) > roundoff {
            return Ok(None);
        }
    }
    // Existing UV trims are another geometric representation of each edge. Do
    // not erase the tolerance of a real trim/edge gap by inspecting 3D alone.
    for (id, record) in b.edgeuses().iter() {
        let (Some(trim), _) = edgeuse::uv_trim_curve(b, id)? else {
            continue;
        };
        if trim.dim() != 2 || !matches!(trim.kind(), CurveKind::Line(_)) {
            return Ok(None);
        }
        let Some(face) = edgeuse::face(b, id)? else {
            return Ok(None);
        };
        let Some(surface) = b.face(face)?.surface.as_ref() else {
            return Ok(None);
        };
        let Some(edge) = record.edge else {
            return Ok(None);
        };
        let edge = b.edge(edge)?;
        let Some(curve) = edge.curve.as_ref() else {
            return Ok(None);
        };
        for fraction in [0.0, 0.5, 1.0] {
            let parameter = edge.interval.evaluate(fraction);
            let uv = trim.evaluate_point(k, parameter)?;
            let point = surface.evaluate_point(k, Point2::new(uv.x, uv.y))?;
            if !finite(point) || point.distance(curve.evaluate_point(k, parameter)?) > roundoff {
                return Ok(None);
            }
        }
    }
    let mut faces = Vec::new();
    for (id, face) in b.faces().iter() {
        let Some(surface) = face.surface.as_ref() else {
            return Ok(None);
        };
        let SurfaceKind::Plane(plane) = surface.kind() else {
            return Ok(None);
        };
        let origin = plane.position().origin();
        let normal = plane.position().z_axis();
        if !finite(origin) || !finite(normal) || (normal.length() - 1.0).abs() > ROUNDOFF_FACTOR {
            return Ok(None);
        }
        let mut points = Vec::new();
        let mut bounds = Extent3d::new();
        for vertex in face_query::vertices(b, id, None, false, None)? {
            let point = b.vertex(vertex)?.point;
            if ((point - origin).dot(normal)).abs() > roundoff {
                return Ok(None);
            }
            bounds.add_point(point);
            points.push(point);
        }
        if points.len() < 3 {
            return Ok(None);
        }
        faces.push(PlanarFace {
            origin,
            normal,
            points,
            bounds,
            tolerance: face.tolerance,
        });
    }
    Ok(Some(faces))
}

// A true crossing has vertices on both sides; parallel, coincident and merely
// touching planes must retain the caller's tolerance semantics.
fn crossing_gap(a: &PlanarFace, b: &PlanarFace, roundoff: f64) -> Option<f64> {
    let mut negative = false;
    let mut positive = false;
    let mut gap = f64::INFINITY;
    for &point in &a.points {
        let distance = (point - b.origin).dot(b.normal);
        negative |= distance < -roundoff;
        positive |= distance > roundoff;
        if distance.abs() > 32.0 * roundoff {
            gap = gap.min(distance.abs());
        }
    }
    (negative && positive && gap.is_finite()).then_some(gap)
}

fn minimum_tolerance(b: &Brep) -> Option<f64> {
    let mut tolerance = b.tolerance();
    if !tolerance.is_finite() || tolerance <= 0.0 {
        return None;
    }
    for (_, face) in b.faces().iter() {
        if !face.tolerance.is_finite() || face.tolerance <= 0.0 {
            return None;
        }
        tolerance = tolerance.min(face.tolerance);
    }
    for (_, edge) in b.edges().iter() {
        if !edge.tolerance.is_finite() || edge.tolerance <= 0.0 {
            return None;
        }
        tolerance = tolerance.min(edge.tolerance);
    }
    for (_, vertex) in b.vertices().iter() {
        if !vertex.tolerance.is_finite() || vertex.tolerance <= 0.0 {
            return None;
        }
        tolerance = tolerance.min(vertex.tolerance);
    }
    Some(tolerance)
}

fn set_precision(b: &mut Brep, tolerance: f64) -> Result<()> {
    b.set_tolerance(b.tolerance().min(tolerance));
    let faces: Vec<FaceId> = b.faces().ids().collect();
    for face in faces {
        let tolerance = b.face(face)?.tolerance.min(tolerance);
        face_edit::set_tolerance(b, face, tolerance, false, false)?;
    }
    let edges: Vec<_> = b.edges().ids().collect();
    for edge in edges {
        b.edge_mut(edge)?.tolerance = b.edge(edge)?.tolerance.min(tolerance);
    }
    let vertices: Vec<_> = b.vertices().ids().collect();
    for vertex in vertices {
        b.vertex_mut(vertex)?.tolerance = b.vertex(vertex)?.tolerance.min(tolerance);
    }
    Ok(())
}

/// Refines only geometrically exact plane/line solids whose crossing is ambiguous
/// at their current precision. Returns whether the operands' tolerances changed.
pub(crate) fn refine(k: &Kernel, a: &mut Brep, b: &mut Brep) -> Result<bool> {
    let (Some(sa), Some(sb)) = (coordinate_scale(a), coordinate_scale(b)) else {
        return Ok(false);
    };
    let roundoff = sa.max(sb) * ROUNDOFF_FACTOR;
    let (Some(ta), Some(tb)) = (minimum_tolerance(a), minimum_tolerance(b)) else {
        return Ok(false);
    };
    let original = ta.min(tb);
    if !roundoff.is_finite() || roundoff <= 0.0 || original <= 4.0 * roundoff {
        return Ok(false);
    }
    let (Some(faces_a), Some(faces_b)) =
        (exact_faces(k, a, roundoff)?, exact_faces(k, b, roundoff)?)
    else {
        return Ok(false);
    };
    let mut precision = original;
    for fa in &faces_a {
        for fb in &faces_b {
            let tolerance = fa.tolerance + fb.tolerance;
            if fa.bounds.are_disjoint(&fb.bounds, tolerance) {
                continue;
            }
            let sine = fa.normal.cross(fb.normal).length();
            if sine <= ROUNDOFF_FACTOR {
                continue;
            }
            let (Some(ga), Some(gb)) = (
                crossing_gap(fa, fb, roundoff),
                crossing_gap(fb, fa, roundoff),
            ) else {
                continue;
            };
            let gap = ga.min(gb);
            // Preserve the signed separation from a crossing plane instead of
            // collapsing its narrow interval into the boundary tolerance band.
            if gap < tolerance {
                precision = precision.min((gap / 16.0).max(4.0 * roundoff));
            }
        }
    }
    if precision >= original {
        return Ok(false);
    }
    set_precision(a, precision)?;
    set_precision(b, precision)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitive::api_primitives::{create_box, create_cylinder};
    use crate::topology::proto_topology::p1::api_transform::{
        api_rotate, api_translate, TransformObject,
    };

    fn strip(k: &mut Kernel) -> (Brep, Brep) {
        let a = create_box(k, Point3::new(-2.0, 16.0, 0.0), 4.0, 4.0, 4.0).unwrap();
        let mut b = create_box(k, Point3::ZERO, 4.0, 4.0, 4.0).unwrap();
        api_rotate(
            k,
            Some(TransformObject::Brep(&mut b)),
            Point3::ZERO,
            Vec3::Z,
            0.1_f64.atan().to_degrees(),
        )
        .unwrap();
        api_translate(
            k,
            Some(TransformObject::Brep(&mut b)),
            Vec3::new(1.9999, 19.999995, 0.0),
        )
        .unwrap();
        (a, b)
    }

    // Observable values the refinement is allowed to change: all entity tolerances.
    // Keep these snapshots exact so eligibility failure cannot partially shrink a body.
    fn tolerances(b: &Brep) -> Vec<f64> {
        let mut values = vec![b.tolerance()];
        values.extend(b.faces().iter().map(|(_, v)| v.tolerance));
        values.extend(b.edges().iter().map(|(_, v)| v.tolerance));
        values.extend(b.vertices().iter().map(|(_, v)| v.tolerance));
        values
    }

    #[test]
    fn a_real_vertex_gap_keeps_both_operands_original_tolerances() {
        let mut k = Kernel::default();
        let (mut a, mut b) = strip(&mut k);
        let v = a.vertices().ids().next().unwrap();
        a.vertex_mut(v).unwrap().point.z += 1e-7;
        let before = (tolerances(&a), tolerances(&b));
        assert!(!refine(&k, &mut a, &mut b).unwrap());
        assert_eq!((tolerances(&a), tolerances(&b)), before);
    }

    #[test]
    fn curved_geometry_keeps_both_operands_original_tolerances() {
        let mut k = Kernel::default();
        let (mut a, _) = strip(&mut k);
        let mut b = create_cylinder(&mut k, Point3::new(2.0, 20.0, 0.0), 1.0, 4.0).unwrap();
        let before = (tolerances(&a), tolerances(&b));
        assert!(!refine(&k, &mut a, &mut b).unwrap());
        assert_eq!((tolerances(&a), tolerances(&b)), before);
    }

    #[test]
    fn ordinary_overlap_and_exact_touch_keep_original_precision() {
        let mut k = Kernel::default();
        for origin in [Point3::new(2.0, 1.0, 1.0), Point3::new(4.0, 0.0, 0.0)] {
            let mut a = create_box(&mut k, Point3::ZERO, 4.0, 4.0, 4.0).unwrap();
            let mut b = create_box(&mut k, origin, 4.0, 4.0, 4.0).unwrap();
            let before = (tolerances(&a), tolerances(&b));
            assert!(!refine(&k, &mut a, &mut b).unwrap());
            assert_eq!((tolerances(&a), tolerances(&b)), before);
        }
    }

    #[test]
    fn invalid_declared_tolerance_is_not_replaced() {
        let mut k = Kernel::default();
        let (mut a, mut b) = strip(&mut k);
        a.set_tolerance(f64::INFINITY);
        assert!(!refine(&k, &mut a, &mut b).unwrap());
        assert_eq!(a.tolerance(), f64::INFINITY);
        assert_eq!(b.tolerance(), 1e-5);
    }
}
