//! The visible terminal has a Metal surface. Park it once a tab has stayed out of view.

use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

use objc2::DefinedClass;
use objc2::rc::Retained;
use objc2_quartz_core::CAMetalLayer;
use term_render::constant;
use term_render::thread::Renderer;
use term_session::Session;

use crate::view::TerminalView;

/// Keep the surface warm across quick tab switches.
const PARK_AFTER: Duration = Duration::from_secs(2);

pub(super) struct Metal {
    pub(super) supported: bool,
    surface: RefCell<Option<Surface>>,
    hidden_since: Cell<Option<Instant>>,
}

struct Surface {
    renderer: Renderer,
    layer: Retained<CAMetalLayer>,
}

impl Metal {
    pub(super) fn new() -> Self {
        Self {
            supported: term_render::gpu::shared().is_some(),
            surface: RefCell::new(None),
            hidden_since: Cell::new(None),
        }
    }
}

impl TerminalView {
    pub(super) fn with_renderer<R>(&self, f: impl FnOnce(&Renderer) -> R) -> Option<R> {
        self.ivars()
            .metal
            .surface
            .borrow()
            .as_ref()
            .map(|surface| f(&surface.renderer))
    }

    pub(super) fn has_renderer(&self) -> bool {
        self.ivars().metal.surface.borrow().is_some()
    }

    pub(super) fn in_front(&self) -> bool {
        !self.isHiddenOrHasHiddenAncestor() && self.window().is_some_and(|window| crate::kit::window_in_front(&window))
    }

    pub(super) fn update_surface_visibility(&self) {
        let metal = &self.ivars().metal;
        if !metal.supported {
            return;
        }
        if self.in_front() {
            if metal.hidden_since.take().is_some() {
                self.with_renderer(|renderer| renderer.set_visible(true));
            } else if !self.has_renderer() {
                self.start_surface();
            }
        } else if self.has_renderer() && metal.hidden_since.get().is_none() {
            let id = self.id();
            let since = Instant::now();
            metal.hidden_since.set(Some(since));
            self.with_renderer(|renderer| renderer.set_visible(false));
            term_base::main_after(PARK_AFTER, move || {
                if let Some(view) = crate::find(id)
                    && view.ivars().metal.hidden_since.get() == Some(since)
                {
                    view.ivars().metal.hidden_since.set(None);
                    if view.in_front() {
                        view.with_renderer(|renderer| renderer.set_visible(true));
                    } else {
                        view.park_surface();
                    }
                }
            });
        }
    }

    fn start_surface(&self) {
        let (Some(layer), Some(parent)) = (term_render::gpu::layer(), self.layer()) else {
            return;
        };
        let renderer = Renderer::new(&layer);
        let _ = self.fonts();
        renderer.set_face(self.render_face());
        renderer.set_theme(self.ivars().theme.borrow().clone());
        layer.setContentsGravity(constant!(objc2_quartz_core::kCAGravityTopLeft));
        parent.addSublayer(&layer);
        self.ivars().metal.surface.replace(Some(Surface { renderer, layer }));
        self.redraw_layer();
        self.connect_renderer();
        self.repaint();
        self.with_renderer(|renderer| renderer.attach(self));
    }

    pub(super) fn connect_renderer(&self) {
        self.with_renderer(|renderer| {
            if let Some(source) = self.with_session(|session| session.render_to_when_ready(renderer.sink())) {
                renderer.set_source(source);
            }
        });
    }

    fn park_surface(&self) {
        let surface = self.ivars().metal.surface.borrow_mut().take();
        if let Some(surface) = surface {
            self.with_session(Session::detach_renderer);
            surface.layer.removeFromSuperlayer();
            self.ivars().screen.borrow_mut().take();
            self.ivars().state.stale.set(true);
        }
    }
}
