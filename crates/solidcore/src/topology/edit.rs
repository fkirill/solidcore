//! Scoped composition of topology edits with notifications and refreshed caches.
//!
//! Upstream: `source/SMLib/inc/SmBrep.h`, `source/SMLib/inc/SmTopology.h`.
//!
//! | Rust | Upstream |
//! |------|----------|
//! | [`edit_brep`], [`EditScope`] | (Rust composition of editing state, context notifications and topology marks) |
//!
//! Deviations: a new closure API composes the existing notification lender and
//! restores editing state and derived topology caches on both `Ok` and `Err`.
//! This is not a transaction: completed mutations survive an error. Nested edits
//! reuse the scope rather than lending the kernel's notification context again.

use crate::geom::attribute::ObjectKey;
use crate::kernel::Kernel;
use crate::topology::brep::p3::{topology_ref, with_kernel_callbacks};
use crate::topology::edge::NotifyEnv;
use crate::topology::topology_traverser::Marks;
use crate::topology::{Brep, BrepId};
use crate::{EntityRef, Result, SolidError};

/// Notification environment and traversal marks shared by one B-rep edit.
///
/// Obtain a scope through [`edit_brep`]. Pass it to helpers composing low-level
/// operations; use [`Self::parts`] when an operation takes both an environment
/// and marks. Nested helpers can use [`Self::edit`] with the same kernel and B-rep.
/// Attributes are lent to [`Self::env`] during the outer edit and returned to the
/// B-rep when [`edit_brep`] returns, including when the closure returns an error.
#[derive(Debug)]
pub struct EditScope<'a> {
    env: NotifyEnv<'a>,
    marks: Marks,
    body: ObjectKey,
}

impl<'a> EditScope<'a> {
    /// The edit's notifications, B-rep attributes and kernel callbacks.
    pub fn env(&mut self) -> &mut NotifyEnv<'a> {
        &mut self.env
    }

    /// Traversal marks retained across operations and nested edits in this scope.
    pub fn marks(&mut self) -> &mut Marks {
        &mut self.marks
    }

    /// Borrows notifications and marks together for a low-level topology operation.
    pub fn parts(&mut self) -> (&mut NotifyEnv<'a>, &mut Marks) {
        (&mut self.env, &mut self.marks)
    }

    /// Runs a nested edit using the outer scope's notifications and marks.
    ///
    /// Use the same kernel and B-rep supplied to [`edit_brep`]. Editing is enabled
    /// for the closure; the previous editing state is restored and cached topology
    /// is refreshed on either `Ok` or `Err`. Attributes remain in the shared
    /// environment until the outermost edit returns. Mutations are not rolled back.
    ///
    /// # Errors
    ///
    /// Those of `edit`; [`SolidError::InvalidInput`] before invoking it if `b` is
    /// not the B-rep named by this scope. Calling [`edit_brep`] again inside an
    /// edit is refused by the notification lender; use this method instead.
    pub fn edit<R>(
        &mut self,
        k: &mut Kernel,
        b: &mut Brep,
        edit: impl FnOnce(&mut Kernel, &mut Brep, &mut Self) -> Result<R>,
    ) -> Result<R> {
        // Clones retain the temporary key but have no issued identity. Check
        // the issuing domain too: key equality alone could lend the source's
        // attributes and records to another B-rep.
        let issued_body = self
            .env
            .context
            .as_deref()
            .and_then(|context| context.issued_brep_serial(b.notify_identity()))
            .map(|serial| topology_ref(serial, EntityRef::Brep(BrepId::from_index(0))).key);
        if b.key() != self.body || issued_body != Some(self.body) {
            return Err(SolidError::invalid_input(
                "topology::edit::EditScope::edit",
                "nested edit requires the scope's B-rep",
            ));
        }
        let editing = b.editing_enabled();
        b.set_editing_enabled(true);
        let result = edit(k, b, self);
        b.set_editing_enabled(editing);
        b.refresh_cached_topology();
        result
    }
}

/// Composes topology operations with editing enabled, notifications and marks.
///
/// The kernel's notification callbacks and the B-rep's own attributes are lent to
/// the scope under the existing kernel identity scheme. On either `Ok` or `Err`,
/// the original editing flag, B-rep key, callbacks and surviving attributes are
/// restored, and cached topology is refreshed. An error preserves edits already
/// performed; this function does not roll back geometry or repair invalid links.
/// These guarantees apply to normal `Result` returns, not panic unwinding.
///
/// Helpers inside the closure should accept the same [`EditScope`] and use
/// [`EditScope::edit`] for nested scopes. Starting another [`edit_brep`] or
/// [`with_kernel_callbacks`] while this scope is active is refused before its
/// closure runs. Use [`EditScope::env`] to access attributes during the edit;
/// their lists are temporarily absent from the B-rep itself.
///
/// # Example
///
/// ```
/// use solidcore::{Kernel, Point3, Result};
/// use solidcore::primitive::api_primitives::create_box;
/// use solidcore::topology::{edit_brep, integrity};
/// use solidcore::topology::brep::p3::{delete_face, DeleteFaceOptions};
/// # fn main() -> Result<()> {
/// let mut kernel = Kernel::default();
/// let mut body = create_box(&mut kernel, Point3::ZERO, 1.0, 1.0, 1.0)?;
/// if let Some(face) = body.face_list().first().copied() {
///     edit_brep(&mut kernel, &mut body, |k, b, scope| {
///         let (env, marks) = scope.parts();
///         delete_face(k, b, env, marks, face, DeleteFaceOptions::default())?;
///         Ok(())
///     })?;
/// }
/// integrity::check(&body)?;
/// # Ok(())
/// # }
/// ```
///
/// # Errors
///
/// Those of `edit` or [`with_kernel_callbacks`], including a nested context lend
/// or an exhausted identity domain.
pub fn edit_brep<R>(
    k: &mut Kernel,
    b: &mut Brep,
    edit: impl FnOnce(&mut Kernel, &mut Brep, &mut EditScope<'_>) -> Result<R>,
) -> Result<R> {
    with_kernel_callbacks(k, b, |k, b, env| {
        let mut scope = EditScope {
            env: NotifyEnv {
                records: &mut *env.records,
                attributes: &mut *env.attributes,
                context: env.context.as_deref_mut(),
                key_of: env.key_of,
            },
            marks: Marks::default(),
            body: b.key(),
        };
        scope.edit(k, b, edit)
    })
}
