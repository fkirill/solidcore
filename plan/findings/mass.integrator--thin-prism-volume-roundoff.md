# mass.integrator: tiny planar prism volume loses digits (2026-10-31)

- **Unit:** `mass.integrator`, `brep_volume`.
- **PR:** https://github.com/fkirill/solidcore/pull/2
- **Inputs:** the slope=0.1, d=2e-6, reversed-operand intersection in
  `crates/solidcore/tests/issue_1794_planar_strips.rs`, with
  `boolean_refine_planar_strips` and `boolean_vertex_by_best_crossing` enabled
  during development (the final Boolean geometry test needs only the former). Translate the result by (-2,-20,0)
  before querying `brep_volume` at tolerance 1e-10. Original operands are
  documented by the integration test's `operands` helper.
- **Values:** analytic volume from the represented depth is
  8.07999998790584863e-11. The triangular prism's boundary vertices give
  8.079999987923612e-11. The mass query gives 8.07999997737823696e-11
  (about 1.30e-9 relative error). The body passes level-2 validation.
- **Suspected cause:** cancellation in trimmed planar surface integration;
  recentering the result alone does not remove the error. This is separate
  from the Boolean topology failure and its line-root placement error.
- **Native comparison:** not established for the corrected body; pinned C++
  does not construct this corrected intersection. This is analytic evidence,
  not a confirmed Rust/C++ mismatch or a confirmed upstream defect.
- **Follow-up:** add a standalone prism mass-query reproduction and compare
  identical valid geometry with native before changing integration formulas.
  No tolerance was loosened or native case parked for this finding.
