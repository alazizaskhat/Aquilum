use super::*;
use crate::ui::editor::embed::{Drawing, View};
use crate::ui::scroll::bar::Scrollbar;

impl TextEditor {
    pub(super) fn drawing_geometry(&self, start: usize) -> Option<(&Drawing, Vec2)> {
        let i = self.doc.at_offset(start);
        let p = self.doc.paragraph(i);
        let View::Drawing(drawing) = &p.embed?.view else { return None };
        drawing.viewport?;
        let y = if p.fold == Fold::Head {
            self.doc.top(i)
        } else {
            let last = self.doc.code_run(i).1;
            if self.doc.paragraph(last).fold != Fold::Below { return None };
            self.doc.bottom(last) - drawing.height
        };
        Some((drawing, Vec2::new(self.embed_x(i), y)))
    }

    fn drawing_under(&self, point: Point) -> Option<(usize, Point)> {
        let i = self.index_at(point)?;
        let p = self.doc.paragraph(i);
        let start = if p.fold == Fold::Head { p.start } else if p.block.is_code() {
            self.doc.paragraph(self.doc.code_run(i).0).start
        } else { return None };
        let (drawing, origin) = self.drawing_geometry(start)?;
        let local = point - origin;
        let width = drawing.viewport?.0;
        Rect::new(0.0, 0.0, width, drawing.height).contains(local).then_some((start, local))
    }

    pub(super) fn drawing_scroll(&mut self, ctx: &mut EventCtx<'_>, point: Point, dx: f64) -> bool {
        let Some((start, _)) = self.drawing_under(point) else { return false };
        let Some(span) = self.drawing_geometry(start).and_then(|(d, _)| d.scroll_span(self.table_scroll_x(start))) else { return false };
        self.table_scrolls.insert(start, (span.offset + dx).clamp(0.0, span.max()));
        ctx.request_render();
        true
    }

    pub(super) fn drawing_press(&mut self, ctx: &mut EventCtx<'_>, point: Point) -> bool {
        let Some((start, local)) = self.drawing_under(point) else { return false };
        let Some(span) = self.drawing_geometry(start).and_then(|(d, _)| d.scroll_span(self.table_scroll_x(start))) else { return false };
        let mut bar = Scrollbar::new(Axis::Horizontal);
        let Some(offset) = bar.press(local, &span) else { return false };
        self.table_scrolls.insert(start, offset);
        self.drawing_bar = Some((start, bar));
        ctx.capture_pointer();
        ctx.set_handled();
        ctx.request_render();
        true
    }

    pub(super) fn drawing_drag(&mut self, ctx: &mut EventCtx<'_>, point: Point) -> bool {
        let Some((start, bar)) = self.drawing_bar else { return false };
        // A track click also captures the pointer. Until release, its move
        // events must not turn into text selection and reveal the source.
        if !bar.dragging() { return true };
        let Some((drawing, origin)) = self.drawing_geometry(start) else { return false };
        if let Some(offset) = drawing.scroll_span(self.table_scroll_x(start)).and_then(|span| bar.drag(point - origin, &span)) {
            self.table_scrolls.insert(start, offset);
            ctx.request_render();
        }
        true
    }
}
