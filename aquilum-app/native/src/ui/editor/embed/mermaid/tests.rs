use masonry::core::StyleSet;
use masonry::parley::{FontContext, LayoutContext};

use super::{Item, complete_flowchart, mermaid};
use crate::ui::editor::document::{Document, Fold};
use crate::ui::editor::highlight::Lang;
use crate::ui::editor::markdown::Fonts;

pub(crate) const EXAMPLE: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/mermaid-test.md"));

fn image(source: &str, width: f64, dark: bool) -> (u64, Vec<u8>) {
    let drawing = mermaid(source, width, dark).expect("rendered diagram");
    let [Item::Image(frame, image, _)] = drawing.items.as_slice() else { panic!("diagram image") };
    assert!(frame.rect().x0 >= 0.0);
    if frame.rect().x1 > width {
        let span = drawing.scroll_span(0.0).expect("a wide diagram has a scrollbar");
        assert_eq!(span.viewport, width);
        assert!(span.content >= frame.rect().x1);
    }
    assert!(drawing.height > 24.0);
    let pixels = image.image.data.as_ref().to_vec();
    assert!(pixels.chunks_exact(4).any(|p| p[3] > 0));
    (image.image.data.id(), pixels)
}

#[test]
fn mermaid_wide_landscape_keeps_readable_geometry_in_a_narrow_column() {
    let sample = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/mermaid-wide.md"));
    let source = sample.split("```mermaid\n").nth(1).unwrap().split("```").next().unwrap();
    let narrow = mermaid(source, 500.0, true).unwrap();
    let wide = mermaid(source, 1500.0, true).unwrap();
    let [Item::Image(narrow_frame, narrow_image, _)] = narrow.items.as_slice() else { panic!("diagram") };
    let [Item::Image(wide_frame, wide_image, _)] = wide.items.as_slice() else { panic!("diagram") };
    assert_eq!(narrow_frame.rect().size(), wide_frame.rect().size(), "narrow columns must not shrink the labels");
    assert!(narrow_frame.rect().width() > 1500.0);
    assert!(narrow_frame.rect().height() > 300.0, "the screenshot regression must not collapse to a tiny strip");
    assert_eq!(narrow_image.image.data.id(), wide_image.image.data.id());
    let span = narrow.scroll_span(f64::MAX).unwrap();
    assert_eq!(span.offset, span.max());
}

#[test]
fn mermaid_renders_common_diagrams_and_reuses_pixels_when_resized() {
    assert_eq!(Lang::from_info("Mermaid"), Some(Lang::Mermaid));
    assert_eq!(Lang::from_info("mmd"), Some(Lang::Mermaid));
    for source in [
        "flowchart TD\n A[Start] --> B{Ready?}\n B -->|Yes| C[Finish]",
        "sequenceDiagram\n Alice->>Bob: Hello\n Bob-->>Alice: Hi",
        "classDiagram\n Animal <|-- Duck\n Duck: +swim()",
        "stateDiagram-v2\n [*] --> Active\n Active --> [*]",
        "erDiagram\n CUSTOMER ||--o{ ORDER : places",
        "flowchart LR\n A[\"Привет\"] --> B[\"Мир\"]",
        "flowchart LR\n A>Input] --> B[Output]",
    ] {
        let light = image(source, 600.0, false);
        assert_eq!(light.0, image(source, 180.0, false).0, "resize should reuse the raster");
        assert_ne!(light.1, image(source, 600.0, true).1, "dark theme should change the diagram");
    }
    let source = EXAMPLE.split("```mermaid\n").nth(1).unwrap().split("```").next().unwrap();
    image(source, 800.0, true);
}

#[test]
fn mermaid_invalid_or_oversized_input_does_not_replace_source() {
    for source in ["", "not a diagram", "flowchart TD\n A[broken"] {
        assert!(mermaid(source, 600.0, false).is_none(), "{source:?}");
    }
    assert!(mermaid(&"x".repeat(65 * 1024), 600.0, false).is_none());
    assert!(mermaid("flowchart TD\n A-->B", f64::NAN, false).is_none());
    assert!(mermaid("flowchart TD\n A-->B", 0.0, false).is_none());
    assert!(complete_flowchart("flowchart TD\n%% [comment\n A[\"a ) and [\"] --> B[(DB)]"));
    assert!(!complete_flowchart("graph LR\n A[\"unfinished"));
}

fn layout(doc: &mut Document, caret: Option<usize>) {
    doc.layout(&mut FontContext::default(), &mut LayoutContext::new(), &StyleSet::new(16.0), Some(600.0), None, caret.map(|at| at..at));
}

fn document(text: &str) -> Document {
    let mut doc = Document::new(text);
    doc.set_markdown(Some(Fonts::new(16.0, true, true)));
    doc.keep = (0.0, 10_000.0);
    doc
}

#[test]
fn mermaid_preview_reveals_source_updates_and_recovers_from_invalid_edits() {
    let source = "```mermaid\nflowchart TD\n A-->B\n```\nafter";
    let mut doc = document(source);
    layout(&mut doc, None);
    assert_eq!(doc.paragraph(0).fold, Fold::Head);
    assert_eq!(doc.paragraph(2).fold, Fold::Member);
    assert!(doc.paragraph(0).embed.is_some());
    layout(&mut doc, Some(source.find("A-->").unwrap()));
    assert_eq!(doc.paragraph(2).fold, Fold::None);
    assert_eq!(doc.paragraph(3).fold, Fold::Below);
    doc.replace(source.find('B').unwrap()..source.find('B').unwrap() + 1, "C");
    layout(&mut doc, None);
    assert_eq!(doc.paragraph(0).fold, Fold::Head);
    // An invalid edit restores the code block and discards the stale preview.
    doc.replace(11..22, "invalid");
    layout(&mut doc, None);
    assert!(doc.paragraph(0).embed.is_none());
    assert_eq!(doc.paragraph(0).fold, Fold::None);
    doc.replace(0..doc.end(), source);
    layout(&mut doc, None);
    assert_eq!(doc.paragraph(0).fold, Fold::Head, "fixing the source restores the preview");
    let mut invalid = document("```mermaid\nnot a diagram\n```\nafter");
    layout(&mut invalid, None);
    assert!(invalid.paragraph(0).embed.is_none());
    for source in ["```mermaid\nflowchart TD\n A-->B", "```rust\nlet x = 1;\n```", "> ```mermaid\n> flowchart LR\n> A-->B\n> ```\nafter"] {
        let mut doc = document(source);
        layout(&mut doc, None);
        if source.starts_with('>') {
            assert_eq!(doc.paragraph(0).fold, Fold::Head);
        } else {
            assert!(doc.paragraph(0).embed.is_none());
            assert_eq!(doc.paragraph(0).fold, Fold::None);
        }
    }
}
