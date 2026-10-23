# Local Mermaid preview build

This change adds offline rendering for fenced `mermaid` and `mmd` blocks to
Aquilum's native editor. Notes remain ordinary Markdown.

- Reading preserves the diagram's natural label size. Wide diagrams have a
  horizontal scrollbar instead of shrinking into the text column. Drag the
  scrollbar, scroll sideways on a trackpad, or use Shift + mouse wheel to pan.
- Entering a diagram reveals its editable source and keeps the preview below it.
- Changes to source refresh the preview; resizing reuses its cached pixels.
- Light and dark themes produce separate previews.
- Invalid, unsupported, unfinished, or oversized diagrams remain visible as code.
- The existing editor drawing pipeline also carries the rendered image into PDF
  export.

Rendering uses `mermaid-rs-renderer` 0.3.1 and `resvg` 0.47 in process. This is a
native implementation of Mermaid syntax, with broad diagram support, rather
than the official Mermaid JavaScript engine. Advanced syntax and layout can
differ from the official engine. The source is preserved when rendering fails.

Limits: 64 KiB of source, 512 lines, 200 graph nodes, 1000 graph edges and eight
million raster pixels. The recent-preview cache holds up to 16 entries / 32 MiB.
SVG image references cannot load local or remote resources.

## Build and check

With Rust stable and Visual Studio C++ Build Tools:

```powershell
cd aquilum-app
cargo test --workspace --locked
cargo build --release -p aquilum-native --locked
```

The executable is `.artifacts/cargo-native/release/aquilum-native.exe` beneath
the repository root. The standard `installed` feature is intentionally omitted
from a local build so an upstream automatic update cannot replace the custom
renderer. Upstream source updates must be rebuilt with this patch.

The fixture `aquilum-app/native/testdata/mermaid-test.md` contains the request
execution flowchart used to verify the change. Set `AQUILUM_TEST_PNGS` to an
existing output folder to save light/dark editor screenshots during tests.
`mermaid-wide.md` reproduces the wide System landscape diagram and verifies
readable labels at a 520-pixel column, horizontal panning and scrollbar dragging.

Suggested commit message: `feat: render Mermaid diagrams in the native editor`
