//! Analytic placement for an opt-in refined planar Boolean (WR-614 / WR-619).
//!
//! Upstream: `source/SMLib/src/SmProtoTopology.cpp` (no upstream fix).
//!
//! | Rust | Upstream |
//! |------|----------|
//! | `prepare_planar_vertices` (crate) | own precision policy before imprinting |
//!
//! Deviations: only exact plane/line operands accepted by Boolean refinement use
//! analytic line/plane and coplanar line/line roots. Roots outside their edges,
//! parallel lines and movement beyond the definition tolerance retain the original
//! placement. No recursion; no differential tolerance changes.

use super::{ProtoBrep, ProtoTopologyManager, VertexDefId};
use crate::error::Result;
use crate::intersect::point_class::{ClassObject, PointClassType, PointClassification};
use crate::kernel::Kernel;
use crate::math::{Point2, Point3, Vec3};

impl<O: ClassObject> ProtoTopologyManager<O> {
    /// Prepares analytic crossings against the unmodified source geometry, before
    /// any vertex imprint splits or deforms an edge (WR-614/619 opt-in policy).
    pub(crate) fn prepare_planar_vertices(
        &mut self,
        k: &Kernel,
        brep: &impl ProtoBrep<O>,
    ) -> Result<()> {
        let mut definitions = Vec::new();
        for &pv in self.proto_vertices() {
            if let Some(vertex) = self.proto_vertex(pv) {
                definitions.extend_from_slice(vertex.vertex_definitions());
            }
        }
        for definition in definitions {
            self.place_line_plane_vertex(k, brep, definition)?;
        }
        Ok(())
    }

    fn place_line_plane_vertex(
        &mut self,
        k: &Kernel,
        brep: &impl ProtoBrep<O>,
        vd: VertexDefId,
    ) -> Result<()> {
        let definition = self.vd_ref(vd)?;
        let mut pcs = [
            *definition.brep_point_classif(1),
            *definition.brep_point_classif(2),
        ];
        if pcs
            .iter()
            .all(|pc| pc.point_class() == PointClassType::Edge)
        {
            return self.place_line_line_vertex(k, brep, vd);
        }
        let (edge_side, face_side) = match (pcs.first(), pcs.get(1)) {
            (Some(a), Some(b))
                if a.point_class() == PointClassType::Edge
                    && b.point_class() == PointClassType::Face =>
            {
                (0, 1)
            }
            (Some(a), Some(b))
                if a.point_class() == PointClassType::Face
                    && b.point_class() == PointClassType::Edge =>
            {
                (1, 0)
            }
            _ => return Ok(()),
        };
        let Some(edge) = pcs.get(edge_side).and_then(PointClassification::object) else {
            return Ok(());
        };
        let Some(face) = pcs.get(face_side).and_then(PointClassification::object) else {
            return Ok(());
        };
        let Some(curve) = brep.edge_curve(edge).flatten().and_then(|c| brep.curve(c)) else {
            return Ok(());
        };
        let crate::curve::curve::p2::CurveKind::Line(line) = curve.kind() else {
            return Ok(());
        };
        let Some(surface) = brep
            .face_surface(face)
            .flatten()
            .and_then(|s| brep.surface(s))
        else {
            return Ok(());
        };
        let crate::surface::surface::p2::SurfaceKind::Plane(plane) = surface.kind() else {
            return Ok(());
        };
        let canonical = line.canonical();
        let frame = plane.position();
        let normal = frame.z_axis();
        let denominator = canonical.vector.dot(normal);
        if !denominator.is_finite()
            || denominator.abs() <= 128.0 * f64::EPSILON * canonical.vector.length()
        {
            return Ok(());
        }
        let step = (frame.origin() - canonical.point).dot(normal) / denominator;
        let parameter = line.convert_t_from_step_to_nurbs(step);
        let Some(interval) = brep.edge_interval(edge) else {
            return Ok(());
        };
        if !parameter.is_finite() || parameter < interval.min() || parameter > interval.max() {
            return Ok(());
        }
        let point = canonical.point + canonical.vector * step;
        let movement = point.distance(definition.position());
        let tolerance = definition.tolerance(k);
        if !point.x.is_finite()
            || !point.y.is_finite()
            || !point.z.is_finite()
            || !movement.is_finite()
            || !tolerance.is_finite()
            || movement > tolerance
        {
            return Ok(());
        }
        let scale = plane.uv_scale();
        if !scale.x.is_finite() || !scale.y.is_finite() || scale.x == 0.0 || scale.y == 0.0 {
            return Ok(());
        }
        let offset = point - frame.origin();
        let uv = Point2::new(
            offset.dot(frame.x_axis()) / scale.x,
            offset.dot(frame.y_axis()) / scale.y,
        );
        if !uv.x.is_finite() || !uv.y.is_finite() {
            return Ok(());
        }
        if let Some(pc) = pcs.get_mut(edge_side) {
            pc.set_t_param(parameter, false);
        }
        if let Some(pc) = pcs.get_mut(face_side) {
            pc.set_uv_param(uv);
        }
        for pc in &mut pcs {
            pc.set_sss_point(point);
        }
        let [pc1, pc2] = pcs;
        let definition = self.vd_mut(vd)?;
        definition.set_position(point);
        *definition.brep_point_classif_mut(1) = pc1;
        *definition.brep_point_classif_mut(2) = pc2;
        Ok(())
    }

    fn place_line_line_vertex(
        &mut self,
        k: &Kernel,
        brep: &impl ProtoBrep<O>,
        vd: VertexDefId,
    ) -> Result<()> {
        let definition = self.vd_ref(vd)?;
        let [mut pc1, mut pc2] = [
            *definition.brep_point_classif(1),
            *definition.brep_point_classif(2),
        ];
        let (Some(e1), Some(e2)) = (pc1.edge_object(), pc2.edge_object()) else {
            return Ok(());
        };
        let line = |edge| {
            let curve = brep
                .edge_curve(edge)
                .flatten()
                .and_then(|c| brep.curve(c))?;
            match curve.kind() {
                crate::curve::curve::p2::CurveKind::Line(line) if curve.dim() == 3 => Some(line),
                _ => None,
            }
        };
        let (Some(l1), Some(l2)) = (line(e1), line(e2)) else {
            return Ok(());
        };
        let (a, b) = (l1.canonical(), l2.canonical());
        let cross = a.vector.cross(b.vector);
        let coordinate = if cross.x.abs() >= cross.y.abs() && cross.x.abs() >= cross.z.abs() {
            0
        } else if cross.y.abs() >= cross.z.abs() {
            1
        } else {
            2
        };
        let component = |v: Vec3| match coordinate {
            0 => v.x,
            1 => v.y,
            _ => v.z,
        };
        let denominator = component(cross);
        if !denominator.is_finite()
            || denominator.abs() <= 128.0 * f64::EPSILON * a.vector.length() * b.vector.length()
        {
            return Ok(());
        }
        let difference = b.point - a.point;
        let s1 = component(difference.cross(b.vector)) / denominator;
        let s2 = component(difference.cross(a.vector)) / denominator;
        let t1 = l1.convert_t_from_step_to_nurbs(s1);
        let t2 = l2.convert_t_from_step_to_nurbs(s2);
        for (edge, t) in [(e1, t1), (e2, t2)] {
            let Some(interval) = brep.edge_interval(edge) else {
                return Ok(());
            };
            if !t.is_finite() || t < interval.min() || t > interval.max() {
                return Ok(());
            }
        }
        let (delta1, delta2) = (a.vector * s1, b.vector * s2);
        let (p1, p2) = (a.point + delta1, b.point + delta2);
        let scale =
            p1.x.abs()
                .max(p1.y.abs())
                .max(p1.z.abs())
                .max(p2.x.abs())
                .max(p2.y.abs())
                .max(p2.z.abs());
        let gap = p1.distance(p2);
        if !gap.is_finite() || gap > 128.0 * f64::EPSILON * scale {
            return Ok(());
        }
        // Each coordinate uses the nearer analytic anchor. In particular an
        // axis-aligned boundary retains its exact constant coordinate instead of
        // averaging it with a rounded evaluation of the crossing line.
        let point = Point3::new(
            if delta1.x.abs() <= delta2.x.abs() {
                p1.x
            } else {
                p2.x
            },
            if delta1.y.abs() <= delta2.y.abs() {
                p1.y
            } else {
                p2.y
            },
            if delta1.z.abs() <= delta2.z.abs() {
                p1.z
            } else {
                p2.z
            },
        );
        let movement = point.distance(definition.position());
        let tolerance = definition.tolerance(k);
        if !movement.is_finite() || !tolerance.is_finite() || movement > tolerance {
            return Ok(());
        }
        pc1.set_t_param(t1, false);
        pc2.set_t_param(t2, false);
        pc1.set_sss_point(point);
        pc2.set_sss_point(point);
        let definition = self.vd_mut(vd)?;
        definition.set_position(point);
        *definition.brep_point_classif_mut(1) = pc1;
        *definition.brep_point_classif_mut(2) = pc2;
        Ok(())
    }
}
