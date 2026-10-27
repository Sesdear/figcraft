# FigCraft's vendored winit

## Provenance and licence

This is winit 0.30.13 from [storytold/winit](https://github.com/storytold/winit),
commit `a517b3846ee8457d84ee4975110bfda6797c44dc`. It retains that fork's existing
changes, documented in [`PATCHES.md`](PATCHES.md), and its original Apache-2.0
[`LICENSE`](LICENSE). The FigCraft-specific changes below are Apache-2.0.

The copy contains the library, build script, workspace metadata and `dpi` member,
tests and Markdown documentation. Git metadata, CI configuration, examples and
documentation images are intentionally omitted. The manifest's example target and
example packaging entry are removed accordingly; no runtime library source is omitted.

## Local native file-drop patch

[`figcraft-file-drop.patch`](figcraft-file-drop.patch) records the delta against
that exact revision, including the manifest adjustment and regression tests. It
does not add a public API or enqueue files a second time: it emits the existing
`CursorMoved` event for accepted file gestures, before their `DroppedFile` events.

- **Wayland:** retain the surface-local logical drag point through asynchronous URI
  reading, convert using the window's current scale, and replay it before delivery.
  URI lists are limited to 1 MiB; oversized lists follow the read-error path,
  releasing the buffer, completing with no files and removing the pipe reader.
- **X11:** unpack signed root pixels and translate into client pixels. Keep one URI
  conversion in flight. Give each transfer a temporary unmapped requestor window,
  so canceled replies cannot overwrite a later transfer's property. Validate the
  requestor, timestamp and selection owner; defer a fast drop until its reply
  arrives. Freeze the last accepted point at release; consume the transfer before replaying that point and
  delivering files once. Leave cancels a matching hover, not a released transfer.
- **Windows:** correct the private COM `POINTL` parameters to their by-value ABI;
  subtract the existing client-area screen origin without another DPI conversion.
- **macOS:** read `NSDraggingInfo.draggingLocation` (window logical coordinates),
  convert into the flipped content view, then apply the backing scale once. Enter
  and update callbacks return an `NSUInteger` operation mask, not a Boolean.

Existing cancellation and file-reading mechanisms are retained. No unsafe code is
added to FigCraft crates. The Cocoa location message is confined to the existing
upstream Objective-C callback boundary; the Windows conversion uses existing safe
wrappers and does not dereference the event point.

## Verification and maintenance

From the FigCraft workspace, run:

```sh
CARGO_TARGET_DIR="$PWD/target" cargo test --manifest-path vendor/winit/Cargo.toml --test file_drop_patches
CARGO_TARGET_DIR="$PWD/target" cargo test --manifest-path vendor/winit/Cargo.toml --lib file_drop_position
CARGO_TARGET_DIR="$PWD/target" cargo test --manifest-path vendor/winit/Cargo.toml --lib file_drop_state
CARGO_TARGET_DIR="$PWD/target" cargo test --manifest-path vendor/winit/Cargo.toml --lib dnd::tests
```

The tests execute shared coordinate conversions and the pure X11 transfer state,
including both URI/drop orderings, frozen coordinates, duplicate delivery and
canceled/unrelated replies. Source guards cover callback ordering/ABI, following
the fork's existing patch-test convention, and Wayland read-error cleanup. Wayland
unit tests cover URI parsing, exact size boundaries, oversized streams and read
errors. They do not replace native OS drag tests or cross-platform compilation.
A FigCraft X11 XDND smoke run on 2026-10-27
verified hover/cancel, multi-file delivery, fast Drop before the URI reply and
Zoom/Pan placement, with screenshots inspected. Wayland, Windows and macOS gestures
have not been run for this patch. Temporary X11 requestor windows are destroyed
on reset, source replacement, completion and backend teardown; no per-drag atoms
are allocated. Even legacy `CurrentTime` or equal-timestamp replies remain isolated
by requestor identity and property storage.

When updating the fork, rebase or remove the local patch only after equivalent
coordinate delivery, ordering, cancellation and native drag behavior are verified
on Wayland, X11, Windows and macOS. Keep provenance, licence and the patch in sync.
