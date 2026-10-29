# PR #1435 contribution migration

This records how the work from [PR #1435](https://github.com/storytold/figcraft/pull/1435)
is reconstructed on the current FigCraft base. The maintainer's
[feedback](https://github.com/storytold/figcraft/pull/1435#issuecomment-6486025526)
requires small contributions, winit changes in its shared fork, native path imports without the
new byte-buffer ceiling, and no additional full image decode solely for validation. CodeRabbit
findings are diagnostic inputs; they do not override those architecture requirements.

## Preserved history and clean base

The original fork tip is `3ff7b55107081ca38e6e9fcbec4dbffc68718a90`. At synchronization it
was 8 commits ahead of and 309 commits behind the first fetched FigCraft upstream revision
`f702105ed0280a4552cd97e656097aecaa02e970`. Upstream advanced during validation; the original
tip is 8 ahead and 321 behind the refreshed base. Reconstruction starts from
`a82adf4657f73de3ddbec7dc3aa3b3385567fb96`; it does not rebase the entire vendored dependency
onto upstream.

Recoverable local and published backup refs preserve the starting points:

| Ref | Original revision |
|---|---|
| `backup/pr1435-original-20261029` | `3ff7b55107081ca38e6e9fcbec4dbffc68718a90` |
| `backup/origin-main-20261029` | `3ff7b55107081ca38e6e9fcbec4dbffc68718a90` |
| `backup/local-main-20261029` | `6ea44e936ff3da686ca69b9c14bdb6ff3804d2fc` |

The backups retain source and provenance even where an old commit is obsolete on the new base.
No merge commit or local runtime artifact cleanup needs to be replayed to preserve the feature.

| Original commit | Contribution | New disposition |
|---|---|---|
| `a39b0739114a00cede7181ac622b5d9ac2487698` — Add Drag-And-Drop | Application integration and vendored winit | Application behavior reconstructed separately; actual platform deltas extracted to winit. The unchanged upstream dependency files are not a contribution to migrate. |
| `9530c805b11b0d2d046637393c6fa2c5bd922874` — Delete .omx-state-locks directory | Local runtime artifact cleanup | Obsolete on the clean upstream base; original remains in backups. |
| `05c42f999753cec422b3721060b534688f68db52` — Delete .omx-state-locks.identity.json | Local runtime artifact cleanup | Obsolete on the clean upstream base; original remains in backups. |
| `ed82ad0c034d37928d62a97496e0b36a8ad7ff85` — Merge branch 'main' into hlnikniky/drag-and-drop | Upstream synchronization | Replaced by the current upstream base; relevant feature behavior is applied selectively. |
| `5e17d97003c5845a897fc5094548d734fd80ce2a` — Merge pull request #2 from Sesdear/hlnikniky/drag-and-drop | Feature merge container | No new merge is replayed; its code is represented by the extracted contributions. |
| `5b1ae96ac74948bae231d76cbc0aef9d48469fc6` — Merge branch 'main' into main | Upstream synchronization | Replaced by the current upstream base. |
| `5cb20e505d82d93ed9c43e4e4bbdbdacd5a8fe39` — fix(ui): restore web startup and bound Wayland file drops | Startup, URI-list bound and history repair | Valid startup retained with the web feature; upstream already has a valid startup without that feature. Wayland bound moved to `1cc46f43`. Existing upstream history retained; the real duplicate summary is fixed independently in `e5248896`. |
| `3ff7b55107081ca38e6e9fcbec4dbffc68718a90` — fix(dnd): bound import resources and clean up platform drag state | Resource admission and platform repairs | Byte-input admission retained with native path regression corrected. Wayland failure rejection is in `1cc46f43`; native coordinates, XDND state and macOS hotspot repair are in `b4472b1e`. Unreferenced X11/Windows modules are not migrated. |

## Winit baseline and patch series

The vendored source identifies baseline `a517b3846ee8457d84ee4975110bfda6797c44dc`.
The compatible shared-fork base is `ad81a32589115f621a1dbef9556e9a20edb69f6b`, on
`storytold/winit:craft/0.30.13`. It preserves the shared fork's FigCraft input/IME work.
`storytold/winit:master` has advanced to the 0.31.0-beta.3 workspace and is incompatible with
FigCraft's current egui/eframe 0.37 dependency on winit 0.30.13. Updating to that API is a separate
migration, not a prerequisite for these fixes.

| Revision | Scope | Dependency |
|---|---|---|
| `1cc46f4324dac807de3d2bd67f07a5fcc0dd9502` | Bound Wayland URI-list input to 1 MiB before extending the buffer; finish the failed read through existing empty-path cleanup; withdraw accepted MIME and flush when receive/registration fails. Focused boundary and cleanup tests. | Compatible base `ad81a325`. |
| `b4472b1e6c360e06f315a3db26697465a936cd21` | Preserve native drop positions on Wayland, X11, Windows and macOS; isolate XDND transfer state and late replies; fix macOS hoty lookup; shared coordinate and platform-state tests. | Includes the bounds commit above. |

Winit's `PATCHES.md` records the reasons and provenance. Platform changes are reviewed there;
FigCraft integration must use the immutable migrated commit, with no local `vendor/winit` patch.
A dependency pinned to a contributor fork is an integration dependency until the shared fork
accepts it; neither a local commit nor a successful build establishes upstream acceptance.

## Application behavior retained and corrected

Canvas-only feedback, receiving-view coordinates, multi-file partial success, gesture-specific
selection, undo/redo and save/reopen remain application-level integration. Native files with a
usable filesystem path pass the path to the existing engine commands. They do not consume the
byte-input budget or incur raw-to-base64 copies merely to import by path. Existing engine/format
limits still apply.

Browser and virtual byte inputs share 64 pending slots, two fallback native read threads and a
1 GiB charged-input budget. A reservation includes twelve times the raw length plus 64 KiB for
overlapping buffers, base64 and retained pending/repeat data; it follows the retained input until
its last owner is released. The repeat reservation belongs to the shared application, rather than
a window-local pane, because the engine retains its repeat parameters across window closure. The 512 MiB byte-file ceiling is an additional maximum, not a promise
that a file that size fits the aggregate budget. In particular, one byte-backed input can exhaust
the budget at roughly 85 MiB; path-backed native files no longer inherit that transport limit.
This is an input-transport bound, not an application RSS guarantee: format expansion, document
images, undo data and renderer allocations are governed separately.

Raster import validates headers, nonzero dimensions and advertised pixel allocation while keeping
the original encoded data. It does not perform another full pixel decode solely for validation.
The existing renderer detects pixel-data damage when it decodes; malformed headers can fail
before insertion, while a decodable header does not prove that all pixel data is valid.

## Focused contributions and documentation dates

- Web startup: no independent upstream fix is required. The current upstream closure already
  runs optional controls and returns one fully initialized `WebShell`; the drag bridge's valid
  construction belongs with the feature that introduces it.
- Wayland input bounds and reader failure rejection: the first winit commit is independently
  reviewable. The native coordinate/XDND series follows it.
- Documentation history: `fix/documentation-history`, commit
  `e5248896bbd6956894543e2a396df461bc76d50b`, removes only the repeated gap summary and
  records that correction. Upstream already preserves the 2026-10-26 save-protection and
  layer-name export entries, and the colour-picker and Android Vector Drawable summaries.
- Drag-and-Drop integration: reconstructed on the latest FigCraft base with the migrated winit
  dependency and the import regressions corrected.

The original branch's 2026-10-27 Canvas entries describe an unmerged implementation and serve as
provenance. This integration records the revised behavior on 2026-10-29, preserves all existing
upstream historical entries and leaves parity estimates unchanged. Prior-head screenshots and
smoke results are historical evidence, not qualification of the reconstructed revision.

## Validation and publication limits

Validation uses Rust 1.97.0 and the existing FigCraft target directory. Targeted tests cover
resource admission/refunds, metadata size changes, native paths above the byte-input ceiling,
Canvas zoom/pan and gesture isolation, undo/redo, image headers and import reporting. The
86 MiB regression imports a synthetic 12×6 PNG with a sparse trailing region through the real
engine and checks the stored source and undo/redo. A separate 513 MiB sparse file test proves
path dispatch without a byte read; it does not claim successful format parsing of that file.

The pinned upstream `craft-libs` revision has an unrelated `craft-menu` manifest resolution
failure: its edition-2024 workspace inherits `craft-keymap` with default features enabled while
the member tries to disable them. Native/WASM checks and the focused FigCraft tests therefore
use a temporary, external manifest override for that exact menu source, with explicit inherited
dependencies. No menu source, dependency upgrade or local path override is included here. The
committed lockfile retains the upstream menu Git source. A clean build without that validation
workaround remains blocked by the existing dependency manifest.

Strict Clippy with dependencies also encounters an unchanged upstream
`vectordrawable/src/paint.rs` range-check lint on Rust 1.97.0. Affected-crate Clippy with
`--no-deps --all-targets -- -D warnings` passes after correcting new test helpers. Windows and macOS target toolchains
are not installed; Linux checks and source guards do not substitute for those target builds or
native compositor/drop-gesture qualification. The independent web startup defect is absent
on the refreshed upstream base, so no empty startup-only change is proposed.

The compatible winit branches and FigCraft documentation branch are published to Sesdear's
forks. Creating upstream PRs through the connected GitHub integration returns HTTP 403
`Resource not accessible by integration`; publication of a branch does not mean its changes
have been accepted by the shared fork. Documentation coverage is scoped by CodeRabbit's
external check; the repository supplies no matching local measurement for the reported 80%
threshold, and no threshold attainment is claimed.
