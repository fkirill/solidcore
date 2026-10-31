# general: ARM64 validation during the #1794 repair (2026-10-31)

- **PR:** https://github.com/fkirill/solidcore/pull/2
- **Environment:** aarch64-apple-darwin, pinned Rust 1.98.0, pinned SMLib
  1.0.1 (`3fd5b95`) built with AppleClang and contraction disabled. Exact upstream
  base: `9dde25e97b05dd986b2ba49b8a03622b3ed572f9`; baseline artifact
  `13038075739` from successful upstream CI run `41187083336`.
- **Owned-path evidence:** `boolean.api_brep`, `boolean.merge_p4`,
  `boolean.topology_intersector_p1`, and `topology.proto_topology_p2` pass all
  4,020 cases and their exact-base coverage checks. The opt-in planar-strip
  regression passes 72 union/intersection/difference combinations and level-2
  validation. This does not certify the full native gate on this platform.

## Full native sweep

`--green --jobs 2 --baseline <exact-base-reports>` completed all 280 units:
268 passed, 12 failed the gate. The reports contain 587,012 cases. No waiver,
comparison tolerance, fixture, platform guard or unit status was changed.

Two units have comparison failures:

- `curve.conic`, `gen0316` (generated seed 15876535056392152770):
  `values.r[4]` is C++ `1.61599991421391e-10`, Rust
  `1.6200880971914413e-10`, relative error `0.0025234325124778557`.
- `sweep.topology_sweep`, `whole_merge_rotation_y_90`
  (`fixtures/sweep.topology_sweep/whole_brep.json#23`):
  `ints.whole[1..=4]` is C++ `[3,3,20,20]`, Rust `[2,2,19,19]`.

Five whole-unit x86-only reference skips fail the exact-base selection gate:
`geom.hash_table`, `heal.heal_data_p1`, `heal.heal_data_p2`,
`heal.heal_data_p3`, and `surface.twist_volume` (UB-35 / #448 conversions).
Five other units fail retention because existing platform-skipped cases passed
in the Linux baseline: `curve.curve_p6`, `nurbs.b_spline_curve_n_lib`,
`surface.srf_in_volume`, `topology.brep_data`, and `topology.database_io`.
These skips are explicit coverage loss, not passes.

The two native mismatches are in unmodified functions. Their numerical causes
have not been isolated; the known platform differences do not waive them.
Reproduce individually with the listed unit/case filters, without a baseline
filter masquerading as a gate. Use an x86-64 Linux run for the full baseline gate.

## Rust test suite and exact-base controls

The full optimized suite (`cargo test --profile difftest -p solidcore
--no-fail-fast`) reports 3,407 passed, three failed, 25 ignored. All 2,742
library unit tests pass. The three failing integration targets reproduce with
identical output on the untouched exact upstream base in the same optimized
profile on this machine:

- `defeature_imprint_without_vertex_sector`: patch face pairs
  `[(4,4),(3,3)]`, expected `[(4,4),(0,0)]`.
- `issue_398_shell_box_rounds_chamfer_fillet`,
  `issue_398_reference_without_fix`: open-body counts `(51,313,265)`,
  expected `(72,307,236)`.
- `wr_407_swept_arc_walls_join_without_fold`,
  `wr_407_default_join_loses_the_folded_cover`: volume
  `227280.03602318905` versus closed form `223932.72434788046`; the test
  expects the known default defect to reduce volume by over 1000 instead.

The planar-strip flag is off in these controls. An independent detached checkout
of the exact base, its own Cargo target directory, the same pinned fixture tree,
and the same optimized profile were used. No test expectation was changed.

## x86-64 Linux native result

The same Rust source passes the complete native sweep in Ubuntu 24.04 x86-64,
with GCC 13.3.0, glibc 2.39, Rust 1.98.0 and verified prebuilt reference
`v1-3fd5b9548d19-726643fb17a52b7d`: **280 units passed, zero partial, zero failed**.
All 591,143 baseline cases are retained: 590,442 passed, 18 existing C++ defect
cases parked, 683 existing guard rejections. These counts equal the original
baseline exactly. Both ARM comparison failures and all ARM-skipped units pass
here; no waiver or numerical threshold changed.

The focused strengthened planar-strip regression passes on Linux too. The full
Rust suite reports 3,410 passed, zero failed, 25 ignored. Formatting, wasm and the
final whole-report ratchet pass (280 reports, zero regressions). The complete
runner/FFI suite passes: 1,043 passed, zero failed, 38 ignored, including all
27,440 isolated geometry/operation/flag probes. Workspace all-target Clippy with
`difftest/cpp` and `-D warnings` passes. Initial parallel runner and Clippy
compilations exceeded the container memory limit; both passed with one Cargo job,
retaining every test and lint. Native execution uses two workers, Cargo at most
two jobs, and the containers at most four logical CPUs combined / 10 GiB each. The container's local dev
profile uses opt-level 3 and no debug symbols to share C++ compilation flags with
the difftest profile; the difftest glue crate remains at opt-level 0 for Clippy,
with debug assertions and overflow checking enabled. Ubuntu's old sccache reused
invalid Rust jobserver descriptors, so Rust/Clippy run directly and only C++ uses
the cache. These are validation-environment changes, not source or gate changes.

The validated Rust source is commit `15a2f68b9fa4c382ccfacf0b83d84a6e3de0dceb`;
the final subsequent change only records these results. Hosted fork CI remains
queued for Blacksmith runners; these local results do not bypass its merge gate.
