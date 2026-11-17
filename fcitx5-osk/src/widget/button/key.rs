use std::{
    collections::HashSet,
    time::{Duration, Instant},
};

use iced::{
    Background, Color, Element, Event, Font, Length, Padding, Pixels, Point, Rectangle, Shadow,
    Size, Vector, alignment,
    advanced::{
        Clipboard, Layout, Shell, Widget, layout, overlay, renderer,
        text::{self, LineHeight, Shaping, Wrapping},
        widget::{Operation, Tree, tree},
    },
    mouse::{
        Button as MouseButton, Cursor as MouseCursor, Event as MouseEvent,
        Interaction as MouseInteraction,
    },
    touch::{Event as TouchEvent, Finger as TouchFinger},
    widget::button::{Status as ButtonStatus, Style as ButtonStyle, StyleFn as ButtonStyleFn},
    window::Event as WindowEvent,
};

use crate::widget::button::{ExtButtonCatalog, animation_enabled, border_radius, preview_enabled};

const PRESS_SECS: f32 = 0.07;
const RELEASE_SECS: f32 = 0.14;
/// How much a key shrinks, in proportion of its size, when it is fully pressed.
const PRESS_SHRINK: f32 = 0.08;

/// Local state of the [`Key`].
#[derive(Default)]
struct KeyState {
    fingers: HashSet<Option<TouchFinger>>,
    hovered: bool,
    /// The animation goes from `anim_from` to `anim_target` (1: pressed, 0: released).
    anim_from: f32,
    anim_target: f32,
    anim_since: Option<Instant>,
    pressed_at: Option<Instant>,
}

impl KeyState {
    fn anim_secs(&self) -> f32 {
        if self.anim_target > self.anim_from {
            PRESS_SECS
        } else {
            RELEASE_SECS
        }
    }

    /// 0.0 (released) ..= 1.0 (pressed)
    fn level(&self, now: Instant) -> f32 {
        let Some(since) = self.anim_since else {
            return self.anim_target;
        };
        let t = (now.saturating_duration_since(since).as_secs_f32() / self.anim_secs()).clamp(0., 1.);
        let eased = 1. - (1. - t).powi(3);
        self.anim_from + (self.anim_target - self.anim_from) * eased
    }

    fn is_animating(&self, now: Instant) -> bool {
        self.anim_since
            .is_some_and(|s| now.saturating_duration_since(s).as_secs_f32() < self.anim_secs())
    }

    fn animate_to(&mut self, target: f32, now: Instant) {
        self.anim_from = self.level(now);
        self.anim_target = target;
        self.anim_since = Some(now);
    }

    fn is_hovered(&self) -> bool {
        self.hovered
    }

    fn has_finger_pressed(&self) -> bool {
        !self.fingers.is_empty()
    }

    fn is_pressed(&self, finger: &Option<TouchFinger>) -> bool {
        self.fingers.contains(finger)
    }

    fn finger_pressed(&mut self, finger: Option<TouchFinger>) {
        self.fingers.insert(finger);
    }

    fn finger_released(&mut self, finger: &Option<TouchFinger>) {
        self.fingers.remove(finger);
    }
}

#[derive(Clone, Debug)]
pub struct KeyEvent {
    pub pressed: bool,
    pub cancelled: bool,
    pub finger: Option<TouchFinger>,
    pub bounds: Rectangle,
}

/// An enlarged copy of the key symbol shown above the key while it is pressed.
#[derive(Clone)]
pub struct KeyPreview {
    pub text: String,
    pub font: Font,
    /// Hide the preview after this duration, it is used by keys showing a popup when held.
    pub max_duration: Option<Duration>,
}

/// A widget works like MouseArea, Emit messages on mouse press/release events and finger press/lift/lost events.
pub struct Key<'a, Message, PressCb, ReleaseCb, Theme = iced::Theme, Renderer = iced::Renderer>
where
    Theme: ExtButtonCatalog,
{
    content: Element<'a, Message, Theme, Renderer>,
    width: Length,
    height: Length,
    padding: Padding,
    on_press_with: Option<PressCb>,
    on_release_with: Option<ReleaseCb>,
    preview: Option<KeyPreview>,
    class: Theme::Class<'a>,
}

impl<'a, Message, PressCb, ReleaseCb, Theme, Renderer>
    Key<'a, Message, PressCb, ReleaseCb, Theme, Renderer>
where
    Theme: ExtButtonCatalog,
{
    /// The callback for getting a message on a press event.
    pub fn on_press_with<NewPressCb>(
        self,
        cb: Option<NewPressCb>,
    ) -> Key<'a, Message, NewPressCb, ReleaseCb, Theme, Renderer> {
        let Key {
            content,
            width,
            height,
            padding,
            on_press_with: _on_press_with,
            on_release_with,
            preview,
            class,
        } = self;
        Key {
            content,
            width,
            height,
            padding,
            on_press_with: cb,
            on_release_with,
            preview,
            class,
        }
    }

    /// The callback for getting a message on a release event.
    pub fn on_release_with<NewReleaseCb>(
        self,
        cb: Option<NewReleaseCb>,
    ) -> Key<'a, Message, PressCb, NewReleaseCb, Theme, Renderer> {
        let Key {
            content,
            width,
            height,
            padding,
            on_press_with,
            on_release_with: _on_release_with,
            preview,
            class,
        } = self;
        Key {
            content,
            width,
            height,
            padding,
            on_press_with,
            on_release_with: cb,
            preview,
            class,
        }
    }

    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    pub fn height(mut self, height: impl Into<Length>) -> Self {
        self.height = height.into();
        self
    }

    pub fn padding(mut self, padding: impl Into<Padding>) -> Self {
        self.padding = padding.into();
        self
    }

    /// Show an enlarged preview above the key while it is pressed.
    pub fn preview(mut self, preview: Option<KeyPreview>) -> Self {
        self.preview = preview;
        self
    }

    pub fn style(mut self, style: impl Fn(&Theme, ButtonStatus) -> ButtonStyle + 'a) -> Self
    where
        Theme::Class<'a>: From<ButtonStyleFn<'a, Theme>>,
    {
        self.class = (Box::new(style) as ButtonStyleFn<'a, Theme>).into();
        self
    }
}

pub type DummyCb<Message> = fn(KeyEvent) -> Message;

impl<'a, Message, Theme, Renderer>
    Key<'a, Message, DummyCb<Message>, DummyCb<Message>, Theme, Renderer>
where
    Theme: ExtButtonCatalog,
    Renderer: renderer::Renderer,
{
    /// Creates a [`Key`] with the given content.
    pub fn new(content: impl Into<Element<'a, Message, Theme, Renderer>>) -> Self {
        let content = content.into();
        let size = content.as_widget().size_hint();
        Self {
            content,
            width: size.width.fluid(),
            height: size.height.fluid(),
            padding: Default::default(),
            on_press_with: Default::default(),
            on_release_with: Default::default(),
            preview: None,
            class: Theme::custom_default(),
        }
    }
}

impl<Message, PressCb, ReleaseCb, Theme, Renderer> Widget<Message, Theme, Renderer>
    for Key<'_, Message, PressCb, ReleaseCb, Theme, Renderer>
where
    Message: Clone,
    PressCb: 'static + Fn(KeyEvent) -> Message,
    ReleaseCb: 'static + Fn(KeyEvent) -> Message,
    Theme: ExtButtonCatalog,
    Renderer: renderer::Renderer + text::Renderer<Font = Font>,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<KeyState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(KeyState::default())
    }

    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_ref(&self.content));
    }

    fn size(&self) -> Size<Length> {
        Size {
            width: self.width,
            height: self.height,
        }
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        layout::padded(limits, self.width, self.height, self.padding, |limits| {
            self.content
                .as_widget_mut()
                .layout(&mut tree.children[0], renderer, limits)
        })
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.content
            .as_widget_mut()
            .operate(&mut tree.children[0], layout, renderer, operation);
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: MouseCursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );

        update(self, tree, event, layout, cursor, shell);
    }

    fn mouse_interaction(
        &self,
        _tree: &Tree,
        layout: Layout<'_>,
        cursor: MouseCursor,
        _viewport: &Rectangle,
        _renderer: &Renderer,
    ) -> MouseInteraction {
        let is_mouse_over = cursor.is_over(layout.bounds());

        if is_mouse_over && (self.on_press_with.is_some() || self.on_release_with.is_some()) {
            MouseInteraction::Pointer
        } else {
            MouseInteraction::default()
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        renderer_style: &renderer::Style,
        layout: Layout<'_>,
        cursor: MouseCursor,
        viewport: &Rectangle,
    ) {
        let state: &KeyState = tree.state.downcast_ref();
        let enabled = self.on_press_with.is_some() || self.on_release_with.is_some();
        let (style, bounds) = if animation_enabled() && enabled {
            let level = state.level(Instant::now());
            let base_status = if state.is_hovered() {
                ButtonStatus::Hovered
            } else {
                ButtonStatus::Active
            };
            let base = theme.style(&self.class, base_status);
            if level <= 0.001 {
                (base, layout.bounds())
            } else {
                let pressed = theme.style(&self.class, ButtonStatus::Pressed);
                let background = mix_background(base.background, pressed.background, level);
                let shadow = if level > 0.5 { pressed.shadow } else { base.shadow };
                let bounds = layout.bounds();
                let (dx, dy) = (
                    bounds.width * PRESS_SHRINK * level / 2.,
                    bounds.height * PRESS_SHRINK * level / 2.,
                );
                (
                    ButtonStyle {
                        background,
                        shadow,
                        ..base
                    },
                    Rectangle {
                        x: bounds.x + dx,
                        y: bounds.y + dy,
                        width: bounds.width - dx * 2.,
                        height: bounds.height - dy * 2.,
                    },
                )
            }
        } else {
            let status = if state.has_finger_pressed() {
                ButtonStatus::Pressed
            } else if state.is_hovered() {
                ButtonStatus::Hovered
            } else if enabled {
                ButtonStatus::Active
            } else {
                ButtonStatus::Disabled
            };
            (theme.style(&self.class, status), layout.bounds())
        };
        renderer.fill_quad(
            renderer::Quad {
                bounds,
                border: style.border,
                shadow: style.shadow,
                snap: style.snap,
            },
            style
                .background
                .unwrap_or(Background::Color(Color::TRANSPARENT)),
        );
        // after padding we should use layout.children[0] instead of layout to draw content
        self.content.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            renderer_style,
            layout
                .children()
                .next()
                .expect("it should have content layout"),
            cursor,
            viewport,
        );
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        let state: &KeyState = tree.state.downcast_ref();
        if preview_enabled()
            && state.has_finger_pressed()
            && let Some(preview) = self.preview.as_ref()
            && preview
                .max_duration
                .zip(state.pressed_at)
                .is_none_or(|(max, at)| at.elapsed() < max)
        {
            let key_bounds = Rectangle::new(
                layout.position() + translation,
                layout.bounds().size(),
            );
            return Some(overlay::Element::new(Box::new(PreviewOverlay {
                key_bounds,
                preview,
                class: &self.class,
            })));
        }
        self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
        )
    }
}

fn mix_color(a: Color, b: Color, t: f32) -> Color {
    Color {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a + (b.a - a.a) * t,
    }
}

fn mix_background(a: Option<Background>, b: Option<Background>, t: f32) -> Option<Background> {
    match (a, b) {
        (Some(Background::Color(a)), Some(Background::Color(b))) => {
            Some(Background::Color(mix_color(a, b, t)))
        }
        (None, Some(Background::Color(b))) => {
            Some(Background::Color(mix_color(Color::TRANSPARENT, b, t)))
        }
        (a, b) => {
            if t < 0.5 {
                a
            } else {
                b
            }
        }
    }
}

/// The enlarged symbol shown above a pressed key.
struct PreviewOverlay<'b, 'a, Theme: ExtButtonCatalog> {
    key_bounds: Rectangle,
    preview: &'b KeyPreview,
    class: &'b Theme::Class<'a>,
}

impl<Message, Theme, Renderer> overlay::Overlay<Message, Theme, Renderer>
    for PreviewOverlay<'_, '_, Theme>
where
    Theme: ExtButtonCatalog,
    Renderer: renderer::Renderer + text::Renderer<Font = Font>,
{
    fn layout(&mut self, _renderer: &Renderer, bounds: Size) -> layout::Node {
        let key = self.key_bounds;
        let width = (key.width * 1.5).min(bounds.width);
        let height = key.height * 1.6;
        let x = (key.center_x() - width / 2.).clamp(0., (bounds.width - width).max(0.));
        let y = (key.y - height - key.height * 0.1).max(0.);
        layout::Node::new(Size::new(width, height)).move_to(Point::new(x, y))
    }

    fn draw(
        &self,
        renderer: &mut Renderer,
        theme: &Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: MouseCursor,
    ) {
        let bounds = layout.bounds();
        let style = theme.style(self.class, ButtonStatus::Active);
        let mut border = style.border;
        border.radius = (border_radius() * 1.5).into();
        renderer.fill_quad(
            renderer::Quad {
                bounds,
                border,
                shadow: Shadow {
                    color: Color::from_rgba(0., 0., 0., 0.4),
                    offset: Vector::new(0., 2.),
                    blur_radius: 8.,
                },
                snap: style.snap,
            },
            style
                .background
                .unwrap_or(Background::Color(Color::BLACK)),
        );
        renderer.fill_text(
            text::Text {
                content: self.preview.text.clone(),
                bounds: bounds.size(),
                size: Pixels(bounds.height * 0.55),
                line_height: LineHeight::default(),
                font: self.preview.font,
                align_x: text::Alignment::Center,
                align_y: alignment::Vertical::Center,
                shaping: Shaping::Advanced,
                wrapping: Wrapping::None,
            },
            Point::new(bounds.center_x(), bounds.center_y()),
            style.text_color,
            bounds,
        );
    }
}

impl<'a, Message, PressCb, ReleaseCb, Theme, Renderer>
    From<Key<'a, Message, PressCb, ReleaseCb, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: 'a + Clone,
    PressCb: 'static + Fn(KeyEvent) -> Message,
    ReleaseCb: 'static + Fn(KeyEvent) -> Message,
    Theme: 'a + ExtButtonCatalog,
    Renderer: 'a + renderer::Renderer + text::Renderer<Font = Font>,
{
    fn from(
        widget: Key<'a, Message, PressCb, ReleaseCb, Theme, Renderer>,
    ) -> Element<'a, Message, Theme, Renderer> {
        Element::new(widget)
    }
}

/// Processes the given [`Event`] and updates the [`KeyState`] of an [`Key`]
/// accordingly.
fn update<Message, PressCb, ReleaseCb, Theme, Renderer>(
    widget: &mut Key<'_, Message, PressCb, ReleaseCb, Theme, Renderer>,
    tree: &mut Tree,
    event: &Event,
    layout: Layout<'_>,
    cursor: MouseCursor,
    shell: &mut Shell<'_, Message>,
) where
    Message: Clone,
    PressCb: 'static + Fn(KeyEvent) -> Message,
    ReleaseCb: 'static + Fn(KeyEvent) -> Message,
    Theme: ExtButtonCatalog,
{
    let state: &mut KeyState = tree.state.downcast_mut();

    if widget.on_press_with.is_none() && widget.on_release_with.is_none() {
        if state.hovered {
            state.hovered = false;
            shell.request_redraw();
        }
        if state.has_finger_pressed() {
            // the key went away while pressed, its release will never be seen
            state.fingers.clear();
            state.pressed_at = None;
            state.animate_to(0., Instant::now());
            shell.request_redraw();
        }
        return;
    }

    let bounds = layout.bounds();

    // NOTE the position won't be in the bounds if the button is inside a scrollable, use the
    // position from `cursor`
    let position = cursor.position();
    let (pressed, cancelled, finger) = match *event {
        Event::Mouse(MouseEvent::ButtonPressed(MouseButton::Left)) => (true, false, None),
        Event::Mouse(MouseEvent::ButtonReleased(MouseButton::Left)) => (false, false, None),
        Event::Touch(TouchEvent::FingerPressed { id, .. }) => (true, false, Some(id)),
        Event::Touch(TouchEvent::FingerLifted { id, .. }) => (false, false, Some(id)),
        Event::Touch(TouchEvent::FingerLost { id, .. }) => (false, true, Some(id)),
        Event::Mouse(MouseEvent::CursorMoved { .. }) => {
            if !shell.is_event_captured() {
                if cursor.is_over(bounds) && !state.hovered {
                    state.hovered = true;
                    shell.request_redraw();
                } else if !cursor.is_over(bounds) && state.hovered {
                    state.hovered = false;
                    shell.request_redraw();
                }
            }
            return;
        }
        Event::Window(WindowEvent::RedrawRequested(_)) => {
            if state.hovered && !cursor.is_over(bounds) {
                state.hovered = false;
                shell.request_redraw();
            }
            if state.is_animating(Instant::now()) {
                shell.request_redraw();
            }
            return;
        }
        _ => return,
    };

    // Check after clearing stale hovered state
    if shell.is_event_captured() {
        return;
    }

    if pressed {
        if let (false, Some(position)) = (state.is_pressed(&finger), position)
            && bounds.contains(position)
        {
            tracing::trace!(
                "key[{:?}] is pressed at {:?} by finger {:?}",
                bounds,
                position,
                finger
            );
            if !state.has_finger_pressed()
                && let Some(cb) = &widget.on_press_with
            {
                shell.publish(cb(KeyEvent {
                    pressed,
                    cancelled,
                    finger,
                    bounds,
                }));
            }
            if !state.has_finger_pressed() {
                let now = Instant::now();
                state.animate_to(1., now);
                state.pressed_at = Some(now);
                if let Some(max) = widget.preview.as_ref().and_then(|p| p.max_duration) {
                    // redraw to hide the preview when the popup of secondary keys takes over
                    shell.request_redraw_at(now + max);
                }
            }
            state.finger_pressed(finger);
            shell.request_redraw();
            shell.capture_event();
        }
    } else if state.is_pressed(&finger) {
        state.finger_released(&finger);
        if !state.has_finger_pressed() {
            state.animate_to(0., Instant::now());
            state.pressed_at = None;
        }
        shell.request_redraw();
        tracing::trace!(
            "key[{:?}] is released by finger {:?}, pressed: {}",
            bounds,
            finger,
            state.fingers.len(),
        );
        if !state.has_finger_pressed()
            && let Some(cb) = widget.on_release_with.as_ref()
        {
            shell.publish(cb(KeyEvent {
                pressed,
                cancelled,
                finger,
                bounds,
            }));
        }
        shell.capture_event();
    }
}

/// Local state of the [`PopupKey`].
#[derive(Default)]
struct PopupKeyState {
    is_active: bool,
}

/// A widget works like MouseArea, Emit messages on mouse enter/leave events and finger move event.
pub struct PopupKey<'a, Message, Theme = iced::Theme, Renderer = iced::Renderer>
where
    Theme: ExtButtonCatalog,
{
    content: Element<'a, Message, Theme, Renderer>,
    finger: Option<TouchFinger>,
    width: Length,
    height: Length,
    padding: Padding,
    on_enter: Option<Message>,
    on_exit: Option<Message>,
    class: Theme::Class<'a>,
}

impl<'a, Message, Theme, Renderer> PopupKey<'a, Message, Theme, Renderer>
where
    Theme: ExtButtonCatalog,
{
    /// The message to emit on a enter event.
    #[must_use]
    pub fn on_enter(mut self, message: Message) -> Self {
        self.on_enter = Some(message);
        self
    }

    /// The message to emit on a exit event.
    #[must_use]
    pub fn on_exit(mut self, message: Message) -> Self {
        self.on_exit = Some(message);
        self
    }

    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    pub fn height(mut self, height: impl Into<Length>) -> Self {
        self.height = height.into();
        self
    }

    pub fn padding(mut self, padding: impl Into<Padding>) -> Self {
        self.padding = padding.into();
        self
    }

    pub fn style(mut self, style: impl Fn(&Theme, ButtonStatus) -> ButtonStyle + 'a) -> Self
    where
        Theme::Class<'a>: From<ButtonStyleFn<'a, Theme>>,
    {
        self.class = (Box::new(style) as ButtonStyleFn<'a, Theme>).into();
        self
    }
}

impl<'a, Message, Theme, Renderer> PopupKey<'a, Message, Theme, Renderer>
where
    Theme: ExtButtonCatalog,
    Renderer: renderer::Renderer,
{
    /// Creates a [`PopupKey`] with the given content.
    pub fn new(
        content: impl Into<Element<'a, Message, Theme, Renderer>>,
        finger: Option<TouchFinger>,
    ) -> Self {
        let content = content.into();
        let size = content.as_widget().size_hint();
        Self {
            content,
            finger,
            width: size.width.fluid(),
            height: size.height.fluid(),
            padding: Default::default(),
            on_enter: Default::default(),
            on_exit: Default::default(),
            class: Theme::custom_default(),
        }
    }
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for PopupKey<'_, Message, Theme, Renderer>
where
    Message: Clone,
    Theme: ExtButtonCatalog,
    Renderer: renderer::Renderer,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<PopupKeyState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(PopupKeyState::default())
    }

    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_ref(&self.content));
    }

    fn size(&self) -> Size<Length> {
        Size {
            width: self.width,
            height: self.height,
        }
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        layout::padded(limits, self.width, self.height, self.padding, |limits| {
            self.content
                .as_widget_mut()
                .layout(&mut tree.children[0], renderer, limits)
        })
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.content
            .as_widget_mut()
            .operate(&mut tree.children[0], layout, renderer, operation);
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: MouseCursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        let captured = shell.is_event_captured();
        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );
        let child_captured = captured != shell.is_event_captured();

        let state: &mut PopupKeyState = tree.state.downcast_mut();

        if self.on_enter.is_none() && self.on_exit.is_none() {
            if state.is_active {
                state.is_active = false;
                shell.request_redraw();
            }
            return;
        }

        // NOTE the position won't be in the bounds if the button is inside a scrollable, use the
        // position from `cursor`
        let finger = match *event {
            Event::Mouse(MouseEvent::CursorMoved { .. }) => None,
            Event::Touch(TouchEvent::FingerMoved { id, .. }) => Some(id),
            _ => return,
        };

        // Clear active state even if the event is captured
        let cur_is_active = state.is_active;
        if finger == self.finger {
            let is_hovered = cursor.is_over(layout.bounds());
            if state.is_active && !is_hovered {
                state.is_active = false;
                shell.request_redraw();
            }
        }

        // only return if event is captured by its child, so that the on_exit event can be published
        if child_captured {
            return;
        }

        if finger == self.finger {
            let is_hovered = cursor.is_over(layout.bounds());
            if is_hovered {
                shell.capture_event();
            }
            match (is_hovered, cur_is_active, &self.on_enter, &self.on_exit) {
                (true, false, Some(on_enter), _) => {
                    state.is_active = true;
                    shell.request_redraw();
                    shell.publish(on_enter.clone());
                }
                (false, true, _, Some(on_exit)) => {
                    shell.publish(on_exit.clone());
                }
                _ => {}
            }
        }
    }

    fn mouse_interaction(
        &self,
        _tree: &Tree,
        layout: Layout<'_>,
        cursor: MouseCursor,
        _viewport: &Rectangle,
        _renderer: &Renderer,
    ) -> MouseInteraction {
        let is_mouse_over = cursor.is_over(layout.bounds());

        if is_mouse_over && (self.on_enter.is_some() || self.on_exit.is_some()) {
            MouseInteraction::Pointer
        } else {
            MouseInteraction::default()
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        renderer_style: &renderer::Style,
        layout: Layout<'_>,
        cursor: MouseCursor,
        viewport: &Rectangle,
    ) {
        let state: &PopupKeyState = tree.state.downcast_ref();
        let mut style = theme.style(&self.class, ButtonStatus::Pressed);
        if !state.is_active {
            // use the background of outside container, theme.as_ref().extended_palette().primary.weak.color
            style.background = None;
        }
        renderer.fill_quad(
            renderer::Quad {
                bounds: layout.bounds(),
                border: style.border,
                shadow: style.shadow,
                snap: style.snap,
            },
            style
                .background
                .unwrap_or(Background::Color(Color::TRANSPARENT)),
        );
        self.content.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            renderer_style,
            layout
                .children()
                .next()
                .expect("it should have content layout"),
            cursor,
            viewport,
        );
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
        )
    }
}

impl<'a, Message, Theme, Renderer> From<PopupKey<'a, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: 'a + Clone,
    Theme: 'a + ExtButtonCatalog,
    Renderer: 'a + renderer::Renderer,
{
    fn from(
        widget: PopupKey<'a, Message, Theme, Renderer>,
    ) -> Element<'a, Message, Theme, Renderer> {
        Element::new(widget)
    }
}
