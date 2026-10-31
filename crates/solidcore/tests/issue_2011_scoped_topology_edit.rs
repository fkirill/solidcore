//! Public topology edits restore state and caches on successful and failed returns.

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use std::sync::{Arc, Mutex};

use solidcore::geom::attribute::{Attribute, Behavior, ObjectKey};
use solidcore::geom::context::{MarkType, NotifyCallback};
use solidcore::geom::object::{NotifyOperation, ObjectRef};
use solidcore::primitive::api_primitives::create_box;
use solidcore::topology::brep::{p3, p9};
use solidcore::topology::topology_traverser::Entity;
use solidcore::topology::{edit_brep, integrity, Brep, EditScope, FaceId};
use solidcore::{EntityRef, Kernel, Point3, Result, SolidError};

const STOP: SolidError = SolidError::InvalidInput {
    operation: "issue_2011::stop",
    reason: "deliberate error after a completed mutation",
};

fn cube(k: &mut Kernel) -> Brep {
    create_box(k, Point3::ZERO, 10.0, 10.0, 10.0).unwrap()
}

fn remove(k: &mut Kernel, b: &mut Brep, scope: &mut EditScope<'_>, f: FaceId) -> Result<()> {
    let (env, marks) = scope.parts();
    p3::delete_face(k, b, env, marks, f, p3::DeleteFaceOptions::default())?;
    Ok(())
}

#[test]
fn delete_face_refreshes_caches_and_restores_either_editing_state() {
    for enabled in [false, true] {
        let mut k = Kernel::default();
        let mut b = cube(&mut k);
        b.set_key(ObjectKey(2011));
        b.set_editing_enabled(enabled);
        let f = b.face_list()[0];
        let value = edit_brep(&mut k, &mut b, |k, b, scope| {
            assert!(b.editing_enabled());
            remove(k, b, scope, f)?;
            Ok(73)
        })
        .unwrap();
        assert_eq!(value, 73);
        assert_eq!(b.face_list().len(), 5);
        assert!(b.face(f).is_err());
        assert_eq!(b.editing_enabled(), enabled);
        assert_eq!(b.key(), ObjectKey(2011));
        integrity::check(&b).unwrap();
    }
}

#[test]
fn unscoped_delete_leaves_the_cached_topology_stale() {
    // Negative control for #2011: the existing callback helper alone does not
    // restore editing state or refresh the cached shell/face links.
    let mut k = Kernel::default();
    let mut b = cube(&mut k);
    let f = b.face_list()[0];
    b.set_editing_enabled(true);
    p3::with_kernel_callbacks(&mut k, &mut b, |k, b, env| {
        let mut marks = Default::default();
        p3::delete_face(k, b, env, &mut marks, f, p3::DeleteFaceOptions::default())
    })
    .unwrap();
    assert!(integrity::check(&b).is_err());
    b.refresh_cached_topology();
    integrity::check(&b).unwrap();
}

#[derive(Debug)]
struct Recorder(Arc<Mutex<Vec<(NotifyOperation, ObjectRef)>>>);

impl NotifyCallback for Recorder {
    fn execute(
        &mut self,
        operation: NotifyOperation,
        caller: ObjectRef,
        _data1: Option<ObjectRef>,
        _data2: Option<ObjectRef>,
        _data3: Option<ObjectRef>,
    ) -> Result<()> {
        self.0.lock().unwrap().push((operation, caller));
        Ok(())
    }
}

#[test]
fn failed_edit_preserves_mutations_attributes_and_callbacks() {
    for enabled in [false, true] {
        let mut k = Kernel::default();
        let mut b = cube(&mut k);
        b.set_editing_enabled(enabled);
        let faces = b.face_list().to_vec();
        let shared = b
            .add_attribute(
                EntityRef::Face(faces[0]),
                Attribute::long(10007, 91, Behavior::Reference),
            )
            .unwrap();
        b.attach_attribute(EntityRef::Face(faces[1]), shared)
            .unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        k.context
            .set_user_notify_callback(Some(Box::new(Recorder(Arc::clone(&seen)))));
        let result = edit_brep(&mut k, &mut b, |k, b, scope| {
            let object = (scope.env().key_of)(EntityRef::Face(faces[0]));
            let env = scope.env();
            let h = env.attributes.find_attribute(object.key, 10007).unwrap();
            assert_eq!(env.attributes.get(h).unwrap().long_value(), Some(91));
            remove(k, b, scope, faces[0])?;
            Err::<(), _>(STOP)
        });
        assert_eq!(result, Err(STOP));
        assert_eq!(b.editing_enabled(), enabled);
        assert_eq!(b.face_list().len(), 5);
        integrity::check(&b).unwrap();
        assert!(b.entity_attributes(EntityRef::Face(faces[0])).is_empty());
        assert_eq!(
            b.find_attribute(EntityRef::Face(faces[1]), 10007)
                .unwrap()
                .long_value(),
            Some(91)
        );
        assert_eq!(b.attribute(shared).unwrap().users().len(), 1);
        let before = seen.lock().unwrap().len();
        assert!(before > 0);
        edit_brep(&mut k, &mut b, |k, b, scope| remove(k, b, scope, faces[1])).unwrap();
        assert!(seen.lock().unwrap().len() > before);
        assert!(b.attribute(shared).is_none());
        integrity::check(&b).unwrap();
    }
}

#[test]
fn split_propagates_attributes_to_both_pieces() {
    let mut k = Kernel::default();
    let mut b = cube(&mut k);
    let e = b.edge_list()[0];
    b.add_attribute(
        EntityRef::Edge(e),
        Attribute::long(10008, 7, Behavior::Copy),
    )
    .unwrap();
    let pieces = edit_brep(&mut k, &mut b, |k, b, scope| {
        // Both pieces get their keys from this edit's notification identities.
        let key_of = scope.env().key_of;
        p9::make_vertex_split_edge(k, b, e, 0.5, scope.env(), &|edge, kind| ObjectRef {
            key: key_of(EntityRef::Edge(edge)).key,
            type_code: kind,
        })
    })
    .unwrap()
    .edges
    .unwrap();
    for piece in pieces {
        assert_eq!(
            b.find_attribute(EntityRef::Edge(piece), 10008)
                .unwrap()
                .long_value(),
            Some(7)
        );
    }
    assert!(!b.editing_enabled());
    integrity::check(&b).unwrap();
}

#[test]
fn nested_edits_share_marks_and_restore_the_outer_state_after_error() {
    let mut k = Kernel::default();
    let mut b = cube(&mut k);
    let faces = b.face_list().to_vec();
    edit_brep(&mut k, &mut b, |k, b, scope| {
        scope
            .marks()
            .mark(k, Entity::Face(faces[2]), MarkType::MarkIo);
        let result = scope.edit(k, b, |k, b, inner| {
            assert!(inner
                .marks()
                .is_marked(k, Entity::Face(faces[2]), MarkType::MarkIo));
            remove(k, b, inner, faces[0])?;
            Err::<(), _>(STOP)
        });
        assert_eq!(result, Err(STOP));
        assert!(b.editing_enabled());
        integrity::check(b)?;
        assert!(scope
            .marks()
            .is_marked(k, Entity::Face(faces[2]), MarkType::MarkIo));
        scope.edit(k, b, |k, b, inner| remove(k, b, inner, faces[1]))?;
        assert!(b.editing_enabled());
        integrity::check(b)
    })
    .unwrap();
    assert!(!b.editing_enabled());
    assert_eq!(b.face_list().len(), 4);
    integrity::check(&b).unwrap();
}

#[test]
fn a_second_context_lend_and_another_body_are_rejected_before_their_closures() {
    let mut k = Kernel::default();
    let mut b = cube(&mut k);
    let mut other = cube(&mut k);
    edit_brep(&mut k, &mut b, |k, b, scope| {
        let mut ran = false;
        let result = edit_brep(k, b, |_, _, _| {
            ran = true;
            Ok(())
        });
        assert!(matches!(result, Err(SolidError::InvalidInput { .. })));
        assert!(!ran);
        let result = scope.edit(k, &mut other, |_, _, _| {
            ran = true;
            Ok(())
        });
        assert!(matches!(result, Err(SolidError::InvalidInput { .. })));
        assert!(!ran);
        assert!(b.editing_enabled());
        let f = b.face_list()[0];
        remove(k, b, scope, f)
    })
    .unwrap();
    assert!(!other.editing_enabled());
    assert_eq!(other.face_list().len(), 6);
    integrity::check(&other).unwrap();
    integrity::check(&b).unwrap();
}
