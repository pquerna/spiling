# Deposition scheduling

The shared Rust scheduler used by manufacturing and the offline algorithm-edit
lab. It chooses task order, permitted entry/direction, authored wall seams and
per-barrier tool sequence. Manufacturing supplies already admitted deposition;
the scheduler never adds a connector, splits a path or changes material,
width, occupied height or construction ownership.

`Task` borrows entry vertices and prerequisite indices and retains its actual
physical Z interval. Barrier indices express caller-supplied precedence; they
are not layer heights. Today's production adapter retains a barrier per actual
fine layer, including coarser interior paths ending there. Cross-layer
production scheduling is not enabled.

`Machine` declares known start/tool state, travel and retraction estimates,
station endpoints, mandatory service cadence, modeled selection time and
optional prewarm lead. `ToolOrder::Free` minimizes selection count exactly over
the barriers for any tool count: per first/last tool pair it builds the smallest
prerequisite-respecting order, so no inventory falls back to a fixed order. `Fixed` retains ascending tool groups. Every
required service group remains present even without model paths. The bounded
fixed-dual manufacturing backend retains both supported waste strokes per fine
interval while avoiding a redundant selection between adjacent same-tool
groups. Other tool changers are synthetic lab models, not enabled printers.

The editable algorithm lives in `src/search.rs` and `src/search/`. It constructs
a prerequisite-respecting nearest-entry route, improves it with bounded local
search, and returns a permutation over the original tasks. Authored nearest,
preferred-direction and distributed wall seams are selected in the crate using
actual command coordinates and per-tool history. The frozen checker rejects
missing/duplicate tasks, forbidden direction/seams, prerequisite/barrier/group
violations and forged cost/clock annotations. Production independently decodes
emitted bytes and checks construction and tool/frame/E/debt/thermal/waste state.

The cost is a declared scheduling estimate: straight travel, modeled lift,
thresholded retract/recover, supplied deposition duration, service and swap
seconds. A prewarm lead that cannot fit before selection adds modeled startup
waiting; its annotation is not a measured temperature or readiness proof.
Station endpoints establish where model work begins; station approach motion,
thermal dynamics, acceleration and finalization are not represented by this lab
model. Use actual-program accounting for product comparisons; do not label the
lab total as physical print time. Zero-gap starts are a continuity proxy, not
permission for new extrusion or calibrated pressure behavior.

The crate has no kernel, storage, renderer, G-code or material authority. It does
not perform collision or support proof; the former research-only declared-box
router is removed. Manufacturing retains its existing declared fixed-carriage
checks and independent actual-program admission. No physical collision freedom,
printability or machine readiness is established.

Search effort follows the job: descent runs to a local optimum, the relocated
segment length grows with the group and kicks spend at least the descent's own
work. Work is counted for cancellation and reporting, never capped.
Cancellation aborts. Contradictory barriers/prerequisites
refuse rather than dropping work. Construction/checking work and retained memory
must also be bounded by the caller's job supervision. No GPU scheduling kernel
is enabled; the contracts/compute dependency remains available to experiments.

Run `cargo test --locked -p spiling-deposition-routing --all-targets` and
`cargo run --locked -p spiling-deposition-routing --example route_matrix`.
The independent matrix judge checks task inventory, permissions, group/service
requirements, precedence and timing annotations without calling library checking
or pricing code. Captured slicer/Cathedral paths are historical routing inputs,
not current-producer native qualification. See the [lab](../../tools/route-lab/README.md)
for frozen source-edit cohorts, cached builds and sealed panels.

Local improvement's route caches are linear in the current tool group's size:
task positions, endpoints and incoming gap costs. A mutation refreshes only its
affected range and the following gap. Segment lengths evaluated against one
unchanged order share their insertion-entry prices; the scratch is cleared for
the next search visit. This reuses the frozen pricing without reducing candidate
coverage or effort, allocating an all-pairs cost table or retaining group state
across jobs.
