//! WR-614 / WR-619: preserve a true planar intersection thinner than entity tolerance.
//!
//! A 4 mm cube on [-2,2] x [16,20] and a rotated 4 mm cube overlap in a triangular
//! prism. Its section has height d and width d*(m+1/m), so V = 2*d*d*(m+1/m).
//! Every fixed result must be a manifold solid, pass the level-2 B-rep check and
//! agree with that closed form within 1e-9 relative. Default C++ parity is retained.

#![allow(clippy::unwrap_used, clippy::panic)]

use solidcore::boolean::api_brep::boolean;
use solidcore::geom::a_object::BooleanOperation;
use solidcore::geom::attribute::{AttributeStore, ObjectKey};
use solidcore::mass::integrator::properties::{brep_volume, PropertyAttributes};
use solidcore::math::{Point3, Vec3};
use solidcore::primitive::api_primitives::create_box;
use solidcore::topology::brep::p2::is_manifold_solid;
use solidcore::topology::brep::validation::{check_brep, CheckLevel};
use solidcore::topology::proto_topology::p1::api_transform::{
    api_rotate, api_translate, TransformObject,
};
use solidcore::{Brep, Kernel};

fn operands(k: &mut Kernel, d: f64, slope: f64) -> (Brep, Brep) {
    let a = create_box(k, Point3::new(-2.0, 16.0, 0.0), 4.0, 4.0, 4.0).unwrap();
    let mut b = create_box(k, Point3::ZERO, 4.0, 4.0, 4.0).unwrap();
    api_rotate(
        k,
        Some(TransformObject::Brep(&mut b)),
        Point3::ZERO,
        Vec3::Z,
        slope.atan().to_degrees(),
    )
    .unwrap();
    api_translate(
        k,
        Some(TransformObject::Brep(&mut b)),
        Vec3::new(2.0 - 2.0 * d / slope, 20.0 - d, 0.0),
    )
    .unwrap();
    (a, b)
}

fn volume(k: &mut Kernel, b: &mut Brep) -> f64 {
    // Measure tiny features in a local frame to avoid adding their micrometre
    // coordinates to the 20 mm translation at every integrand evaluation.
    api_translate(
        k,
        Some(TransformObject::Brep(b)),
        Vec3::new(-2.0, -20.0, 0.0),
    )
    .unwrap();
    let store = AttributeStore::default();
    brep_volume(
        k,
        b,
        1e-10,
        &PropertyAttributes {
            store: &store,
            key_of: &|_| ObjectKey(0),
        },
    )
    .unwrap()
}

#[test]
fn exact_planar_strips_build_valid_solids_with_their_analytic_volumes() {
    for slope in [0.01, 0.1, 0.3] {
        for d in [2e-6, 5e-6, 1e-5, 1.5e-5] {
            for reverse in [false, true] {
                for operation in [
                    BooleanOperation::Union,
                    BooleanOperation::Intersection,
                    BooleanOperation::Difference,
                ] {
                    let mut k = Kernel::default();
                    k.fixes.boolean_refine_planar_strips = true;
                    let (a, b) = operands(&mut k, d, slope);
                    let (a, b) = if reverse { (b, a) } else { (a, b) };
                    let mut result = boolean(&mut k, a, b, operation)
                        .unwrap_or_else(|e| {
                            panic!("slope={slope} d={d} reverse={reverse} op={operation:?}: {e:?}")
                        })
                        .result;
                    let case = format!("slope={slope} d={d} reverse={reverse} op={operation:?}");
                    assert!(is_manifold_solid(&result).unwrap(), "{case}");
                    let check = check_brep(&mut k, &mut result, CheckLevel::Level2).unwrap();
                    assert!(check.is_valid(), "{case}: {:?}", check.lines());
                    // The input translation stores 20-d in f64; use its actual
                    // representable depth rather than an ideal real-number d.
                    let represented_d = 20.0 - (20.0 - d);
                    let overlap = 2.0 * represented_d * represented_d * (slope + 1.0 / slope);
                    let expected = match operation {
                        BooleanOperation::Union => 128.0 - overlap,
                        BooleanOperation::Intersection => overlap,
                        BooleanOperation::Difference => 64.0 - overlap,
                        _ => unreachable!(),
                    };
                    let measured = volume(&mut k, &mut result);
                    assert!(
                        (measured - expected).abs() <= 1e-9 * measured.abs().max(expected.abs()),
                        "{case}: expected {expected:.17e}, measured {measured:.17e}"
                    );
                }
            }
        }
    }
}

#[test]
fn default_still_has_the_pinned_reference_failure() {
    let mut k = Kernel::default();
    assert!(!k.fixes.boolean_refine_planar_strips);
    let (a, b) = operands(&mut k, 1e-5, 0.1);
    assert!(boolean(&mut k, a, b, BooleanOperation::Union).is_err());
    let (a, b) = operands(&mut k, 5e-6, 0.1);
    let result = boolean(&mut k, a, b, BooleanOperation::Union)
        .unwrap()
        .result;
    assert!(!is_manifold_solid(&result).unwrap());
}
