//! Draw-only popup transform; layout stays stable while its presentation animates.
use cosmic::iced::advanced::Renderer as _;
use cosmic::iced::advanced::{Clipboard, Layout, Shell, Widget, layout, mouse, renderer, widget};
use cosmic::iced::{Event, Length, Rectangle, Size, Transformation, Vector};
use cosmic::{Element, Theme};

pub fn animated<'a, M: 'a>(content: Element<'a, M>, progress: f32) -> Element<'a, M> {
    Element::new(Motion { content, progress })
}
struct Motion<'a, M> {
    content: Element<'a, M>,
    progress: f32,
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
