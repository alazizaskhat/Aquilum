use masonry::app::RenderRoot;
use masonry::core::keyboard::{Key, NamedKey};
use masonry::core::{NewWidget, PropertySet};
use masonry::properties::ContentColor;

use super::support::{PAGE, TAG, click, drag, editor_id, press, scrolled, text, wheel};
use crate::test_dir::command;
use crate::ui::editor::TextEditor;

#[test]
fn mermaid_reading_and_editing_previews_paint_in_light_and_dark_themes() {
    let _lock = crate::ui::theme::test_lock();
    let before = crate::ui::theme::is_dark();
    let source = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/mermaid-test.md"));
    for theme in ["light", "dark"] {
        crate::ui::theme::apply_setting(theme, false, "#1471eb");
        let editor = NewWidget::new(TextEditor::new(source).markdown().styled(crate::ui::style::editor_text))
            .with_tag(TAG)
            .with_props(PropertySet::new().with(ContentColor::new(crate::ui::theme::current().editor_text)));
        let mut root = RenderRoot::new(editor.erased(), |_| {}, crate::test_dir::options(1050, 1600, 1.0));
        let _ = root.redraw();
        root.focus_on(Some(editor_id(&root)));
        press(&mut root, Key::Named(NamedKey::End), command());
        let (layers, _) = root.redraw();
        let mut renderer = crate::render::Renderer::new(1);
        renderer.render(&layers, 1050, 1600, 1.0, crate::ui::theme::background()).unwrap();
        assert_eq!(text(&root), source, "rendering must preserve Markdown");
        let unique: std::collections::HashSet<_> = renderer.frame_pixels().iter().copied().collect();
        assert!(unique.len() > 100, "diagram and text should be visible");
        if let Ok(out) = std::env::var("AQUILUM_TEST_PNGS") {
            std::fs::write(std::path::Path::new(&out).join(format!("mermaid-{theme}.png")), renderer.png()).unwrap();
        }
        let reading = renderer.frame_pixels().to_vec();
        click(&mut root, 450.0, 180.0);
        let (layers, _) = root.redraw();
        renderer.render(&layers, 1050, 1600, 1.0, crate::ui::theme::background()).unwrap();
        assert_ne!(reading, renderer.frame_pixels(), "clicking the diagram should reveal editable code");
        assert_eq!(text(&root), source);
        if let Ok(out) = std::env::var("AQUILUM_TEST_PNGS") {
            std::fs::write(std::path::Path::new(&out).join(format!("mermaid-{theme}-editing.png")), renderer.png()).unwrap();
        }
    }
    crate::ui::theme::apply_setting(if before { "dark" } else { "light" }, before, "#1471eb");
}

#[test]
fn mermaid_wide_landscape_scrolls_at_readable_size_without_entering_source() {
    let _lock = crate::ui::theme::test_lock();
    let before_theme = crate::ui::theme::is_dark();
    crate::ui::theme::apply_setting("dark", false, "#1471eb");
    let source = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/mermaid-wide.md"));
    let editor = NewWidget::new(TextEditor::new(source).markdown().styled(crate::ui::style::editor_text))
        .with_tag(TAG)
        .with_props(PropertySet::new().with(ContentColor::new(crate::ui::theme::current().editor_text)));
    let mut root = RenderRoot::new(editor.erased(), |_| {}, crate::test_dir::options(520, 1400, 1.0));
    let _ = root.redraw();
    root.focus_on(Some(editor_id(&root)));
    press(&mut root, Key::Named(NamedKey::End), command());
    let state = |root: &RenderRoot| root.get_widget_with_tag(TAG).unwrap().inner().drawing_scroll_state().expect("wide diagram scrollbar");
    let before = state(&root);
    let caret_before = root.get_widget_with_tag(TAG).unwrap().inner().caret_offset();
    assert!(before.0.y0 > 300.0, "diagram must retain readable height at a 520px column");
    let mut renderer = crate::render::Renderer::new(1);
    let (layers, _) = root.redraw();
    renderer.render(&layers, 520, 1400, 1.0, crate::ui::theme::background()).unwrap();
    let reading = renderer.frame_pixels().to_vec();
    if let Ok(out) = std::env::var("AQUILUM_TEST_PNGS") {
        std::fs::write(std::path::Path::new(&out).join("mermaid-wide-readable.png"), renderer.png()).unwrap();
    }
    wheel(&mut root, (260.0, 200.0), (-240.0, 0.0));
    let after = state(&root);
    assert_eq!(root.get_widget_with_tag(TAG).unwrap().inner().caret_offset(), caret_before, "wheel must preserve the caret");
    assert!(after.1 > 0.0, "horizontal wheel must pan the diagram");
    let (layers, _) = root.redraw();
    renderer.render(&layers, 520, 1400, 1.0, crate::ui::theme::background()).unwrap();
    assert_ne!(reading, renderer.frame_pixels(), "panning must reveal another part of the diagram");
    if let Ok(out) = std::env::var("AQUILUM_TEST_PNGS") {
        std::fs::write(std::path::Path::new(&out).join("mermaid-wide-panned.png"), renderer.png()).unwrap();
    }
    let track = after.0;
    click(&mut root, track.x1 - 6.0, track.center().y);
    assert_eq!(root.get_widget_with_tag(TAG).unwrap().inner().caret_offset(), caret_before, "track click must preserve the caret");
    assert!(state(&root).1 > after.1, "clicking the track must page the diagram");
    // Return to the start, then drag the thumb across the track.
    wheel(&mut root, (260.0, 200.0), (100_000.0, 0.0));
    assert_eq!(state(&root).1, 0.0);
    drag(&mut root, (track.x0 + 12.0, track.center().y), (track.x1 - 12.0, track.center().y));
    assert!(state(&root).1 > after.1, "the horizontal thumb must be draggable");
    assert_eq!(root.get_widget_with_tag(TAG).unwrap().inner().caret_offset(), caret_before, "thumb dragging must preserve the caret");
    assert_eq!(text(&root), source);
    crate::ui::theme::apply_setting(if before_theme { "dark" } else { "light" }, before_theme, "#1471eb");
}

#[test]
fn mermaid_vertical_gesture_keeps_scrolling_the_page() {
    let source = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/mermaid-wide.md"));
    let body = format!("{source}\n{}", "paragraph\n".repeat(200));
    let mut root = scrolled(TextEditor::new(&body).markdown(), 520, 300);
    for _ in 0..5 {
        wheel(&mut root, (260.0, 100.0), (-1.5, -12.0));
    }
    assert!(root.get_widget_with_tag(PAGE).unwrap().inner().offset() >= 50.0);
}
