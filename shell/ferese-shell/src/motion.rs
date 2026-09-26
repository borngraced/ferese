//! Draw-only popup transform; layout stays stable while its presentation animates.
use cosmic::iced::advanced::Renderer as _;
use cosmic::iced::advanced::{Clipboard, Layout, Shell, Widget, layout, mouse, renderer, widget};
use cosmic::iced::{Event, Length, Rectangle, Size, Transformation, Vector};
use cosmic::{Element, Theme};

pub type Regions = std::sync::Arc<std::sync::Mutex<Vec<[i32; 5]>>>;

pub fn animated<'a, M: 'a>(
    content: Element<'a, M>,
    progress: f32,
    regions: Regions,
) -> Element<'a, M> {
    Element::new(Motion {
        content,
        progress,
        regions,
    })
}
struct Motion<'a, M> {
    content: Element<'a, M>,
    progress: f32,
    regions: Regions,
}
impl<M> Motion<'_, M> {
    fn translation(&self) -> Vector {
        Vector::new(0.0, -4.0 * (1.0 - self.progress))
    }
    fn cursor(&self, cursor: mouse::Cursor) -> mouse::Cursor {
        cursor.position().map_or(mouse::Cursor::Unavailable, |p| {
            mouse::Cursor::Available(p - self.translation())
        })
    }
}
impl<M> Widget<M, Theme, cosmic::Renderer> for Motion<'_, M> {
    fn tag(&self) -> widget::tree::Tag {
        self.content.as_widget().tag()
    }
    fn state(&self) -> widget::tree::State {
        self.content.as_widget().state()
    }
    fn children(&self) -> Vec<widget::Tree> {
        self.content.as_widget().children()
    }
    fn diff(&mut self, tree: &mut widget::Tree) {
        self.content.as_widget_mut().diff(tree);
    }
    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }
    fn layout(
        &mut self,
        tree: &mut widget::Tree,
        renderer: &cosmic::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.content.as_widget_mut().layout(tree, renderer, limits)
    }
    fn update(
        &mut self,
        tree: &mut widget::Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &cosmic::Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, M>,
        viewport: &Rectangle,
    ) {
        let mut collector = CollectRegions(Vec::new());
        self.content
            .as_widget_mut()
            .operate(tree, layout, renderer, &mut collector);
        if collector.0.is_empty() {
            // A single-content popover (e.g. battery) has one inner card, not
            // a full-surface rectangle including the outer transparent padding.
            if let Some(content) = layout.children().next() {
                collector.0.push(content.bounds());
            }
        }
        let origin = layout.bounds().position();
        let translation = self.translation();
        let regions = collector
            .0
            .into_iter()
            .take(32)
            .filter(|r| r.width > 0.0 && r.height > 0.0)
            .map(|r| {
                [
                    (r.x - origin.x + translation.x).round() as i32,
                    (r.y - origin.y + translation.y).round() as i32,
                    r.width.round() as i32,
                    r.height.round() as i32,
                    11,
                ]
            })
            .collect();
        *self.regions.lock().unwrap() = regions;
        let cursor = self.cursor(cursor);
        self.content.as_widget_mut().update(
            tree, event, layout, cursor, renderer, clipboard, shell, viewport,
        );
    }
    fn draw(
        &self,
        tree: &widget::Tree,
        renderer: &mut cosmic::Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let v = self.translation();
        let transform = Transformation::translate(v.x, v.y);
        renderer.with_transformation(transform, |renderer| {
            self.content.as_widget().draw(
                tree,
                renderer,
                theme,
                style,
                layout,
                self.cursor(cursor),
                viewport,
            )
        });
    }
    fn mouse_interaction(
        &self,
        tree: &widget::Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &cosmic::Renderer,
    ) -> mouse::Interaction {
        self.content.as_widget().mouse_interaction(
            tree,
            layout,
            self.cursor(cursor),
            viewport,
            renderer,
        )
    }
    fn operate(
        &mut self,
        tree: &mut widget::Tree,
        layout: Layout<'_>,
        renderer: &cosmic::Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        self.content
            .as_widget_mut()
            .operate(tree, layout, renderer, operation);
    }
}

struct CollectRegions(Vec<Rectangle>);
impl widget::Operation for CollectRegions {
    fn traverse(&mut self, children: &mut dyn FnMut(&mut dyn widget::Operation)) {
        children(self);
    }
    fn container(&mut self, id: Option<&widget::Id>, bounds: Rectangle) {
        if id == Some(&widget::Id::new("ferese-blur-card")) {
            let contains = |outer: &Rectangle, inner: &Rectangle| {
                outer.x <= inner.x
                    && outer.y <= inner.y
                    && outer.x + outer.width >= inner.x + inner.width
                    && outer.y + outer.height >= inner.y + inner.height
            };
            // Nested controls belong to their enclosing card's single blur pass.
            if self.0.iter().any(|outer| contains(outer, &bounds)) {
                return;
            }
            self.0.retain(|inner| !contains(&bounds, inner));
            self.0.push(bounds);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn material_regions_include_cards_but_not_the_parent_or_gaps() {
        let mut collector = CollectRegions(Vec::new());
        let id = widget::Id::new("ferese-blur-card");
        widget::Operation::container(
            &mut collector,
            None,
            Rectangle::new((0.0, 0.0).into(), (320.0, 200.0).into()),
        );
        let first = Rectangle::new((12.0, 12.0).into(), (140.0, 80.0).into());
        let second = Rectangle::new((160.0, 12.0).into(), (140.0, 80.0).into());
        widget::Operation::container(&mut collector, Some(&id), first);
        widget::Operation::container(&mut collector, Some(&id), second);
        assert_eq!(collector.0, vec![first, second]);
        widget::Operation::container(
            &mut collector,
            Some(&id),
            Rectangle::new((20.0, 20.0).into(), (40.0, 40.0).into()),
        );
        assert_eq!(collector.0, vec![first, second]);
    }
}
