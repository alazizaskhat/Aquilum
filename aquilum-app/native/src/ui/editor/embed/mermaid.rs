use std::sync::{Arc, Mutex, OnceLock};

use aquilum_core::recent::Recent;
use masonry::kurbo::Rect;
use masonry::peniko::{ImageAlphaType, ImageBrush};
use mermaid_rs_renderer::{LayoutConfig, Theme, compute_layout, parse_mermaid_strict, render_svg};
use resvg::{tiny_skia, usvg};

use super::drawing::{Drawing, Item};

const PAD: f64 = 12.0;
const MAX_SOURCE: usize = 64 * 1024;
const MAX_PIXELS: f32 = 8_000_000.0;
const MAX_SIDE: f32 = 16384.0;

#[derive(Clone)]
struct Diagram {
    image: ImageBrush,
    width: f64,
    height: f64,
}

type Cache = Recent<(String, bool), Option<Diagram>>;

fn cache() -> std::sync::MutexGuard<'static, Cache> {
    static CACHE: Mutex<Cache> = Mutex::new(Recent::new(16, 32 << 20));
    CACHE.lock().unwrap_or_else(|e| e.into_inner())
}

fn fonts() -> Arc<usvg::fontdb::Database> {
    static FONTS: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    FONTS.get_or_init(|| {
        let mut fonts = usvg::fontdb::Database::new();
        for font in crate::ui::theme::bundled_fonts() {
            fonts.load_font_data(font.data.as_ref().to_vec());
        }
        fonts.set_sans_serif_family("Inter");
        Arc::new(fonts)
    }).clone()
}

fn raster(source: &str, dark: bool) -> Option<Diagram> {
    let parsed = parse_mermaid_strict(source).ok()?;
    if parsed.graph.nodes.len() > 200 || parsed.graph.edges.len() > 1000 || !complete_flowchart(source) {
        return None;
    }
    let mut theme = if dark { Theme::dark() } else { Theme::modern() };
    theme.font_family = "Inter, Inter Cyrillic, sans-serif".into();
    let config = LayoutConfig::default();
    let layout = compute_layout(&parsed.graph, &theme, &config);
    let svg = render_svg(&layout, &theme, &config);
    let options = usvg::Options {
        font_family: "Inter".into(),
        fontdb: fonts(),
        // A note's diagram must not read files or fetch external images.
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_data: Box::new(|_, _, _| None),
            resolve_string: Box::new(|_, _| None),
        },
        ..usvg::Options::default()
    };
    let tree = usvg::Tree::from_str(&svg, &options).ok()?;
    let (width, height) = (tree.size().width(), tree.size().height());
    let scale = 2.0_f32.min(MAX_SIDE / width.max(height)).min((MAX_PIXELS / (width * height)).sqrt());
    if !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    let (w, h) = ((width * scale).floor().max(1.0) as u32, (height * scale).floor().max(1.0) as u32);
    let mut pixels = tiny_skia::Pixmap::new(w, h)?;
    resvg::render(&tree, tiny_skia::Transform::from_scale(scale, scale), &mut pixels.as_mut());
    let image = crate::render::from_rgba(pixels.take(), w, h, ImageAlphaType::AlphaPremultiplied)?;
    Some(Diagram { image, width: width.into(), height: height.into() })
}

// The native parser tolerates unfinished flowchart shapes. Do not hide their
// source while the user is still entering a label or a closing delimiter.
fn complete_flowchart(source: &str) -> bool {
    let kind = source.lines().map(str::trim).find(|line| !line.is_empty() && !line.starts_with("%%")).unwrap_or_default();
    if !matches!(kind.split_whitespace().next(), Some("flowchart" | "graph")) {
        return true;
    }
    let mut stack = Vec::new();
    let (mut quoted, mut escaped) = (false, false);
    for line in source.lines() {
        let mut chars = line.chars().peekable();
        while let Some(ch) = chars.next() {
            if escaped {
                escaped = false;
                continue;
            }
            if quoted && ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                quoted = !quoted;
            } else if !quoted {
                match ch {
                    '%' if chars.peek() == Some(&'%') => break,
                    '[' | '(' | '{' => stack.push(ch),
                    ']' | ')' | '}' => {
                        let open = match ch { ']' => '[', ')' => '(', _ => '{' };
                        // Asymmetric shapes (A>label]) have no matching opener.
                        if let Some(at) = stack.iter().rposition(|&c| c == open) {
                            stack.remove(at);
                        }
                    }
                    _ => {},
                }
            }
        }
    }
    !quoted && stack.is_empty()
}

pub fn mermaid(source: &str, available: f64, dark: bool) -> Option<Drawing> {
    if source.trim().is_empty() || source.len() > MAX_SOURCE || source.lines().count() > 512 || !available.is_finite() || available <= PAD * 2.0 {
        return None;
    }
    let key = (source.to_owned(), dark);
    let cached = cache().get(&key).cloned();
    let diagram = match cached {
        Some(result) => result?,
        None => {
            // A parser failure leaves the editable fenced block visible.
            let result = std::panic::catch_unwind(|| raster(source, dark)).ok().flatten();
            let bytes = source.len() + result.as_ref().map_or(0, |d| d.image.image.data.len());
            cache().put(key, result.clone(), bytes);
            result?
        }
    };
    // Preserve the renderer's text size. Wide diagrams scroll instead of
    // shrinking every label to a few unreadable pixels.
    let (width, height) = (diagram.width, diagram.height);
    let content = width + PAD * 2.0;
    let overflow = content > available;
    let x = if overflow { PAD } else { (available - width) / 2.0 };
    let frame = Rect::new(x, PAD, x + width, PAD + height).to_rounded_rect(4.0);
    let bar = if overflow { crate::ui::tokens::size::SCROLLBAR_WIDTH } else { 0.0 };
    let mut drawing = Drawing::new(height + PAD * 2.0 + bar, vec![Item::Image(frame, diagram.image, [0.0, 0.0, 100.0, 100.0])]);
    drawing.viewport = overflow.then_some((available, content));
    Some(drawing)
}

#[cfg(test)]
mod tests;
