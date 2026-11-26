//! The render thread (E5): one thread with its own run loop draws every terminal's frames, each paced by a
//! `CADisplayLink` of its view, which beats at the rate of the display the view is on. A surface that drew nothing in
//! the last display frame draws a new frame at once, so an echo is committed within a millisecond and shows with the
//! next display frame (B1); while frames keep coming, the link paces them. With nothing new for a few display frames it
//! pauses its link, and a new frame wakes it. `CAMetalDisplayLink`, which E5 names, refuses `nextDrawable` and so
//! cannot draw between its beats. Atlas, fonts and GPU buffers belong to this thread; the main thread only hands over the layer's
//! geometry, the colours and the overlay, and the session's reader thread only says that a frame waits.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{AnyThread, DefinedClass, Message, define_class, msg_send};
use objc2_core_foundation::{CFRetained, CFRunLoop, CGRect, CGSize};
use objc2_foundation::{NSObject, NSObjectProtocol, NSRunLoop};
use objc2_metal::{MTLCommandBuffer, MTLDrawable};
use objc2_quartz_core::{CADisplayLink, CAMetalDrawable, CAMetalLayer};
use term_base::LockIgnoringPoison;
use term_session::{FrameSink, FrameSource, Snapshot};

use crate::palette::Theme;
use crate::render::{Fonts, Overlay};
use crate::shape::Typography;
use crate::sprites::{QuadCanvas, Sprites};

/// How many display frames a surface waits with nothing new before it pauses its link.
const IDLE_FRAMES: u32 = 3;

/// How long a display link may stay silent before a woken surface draws by itself, as where a display's link never fires
/// (a virtual display, the self-tests' among them).
const LINK_SILENT: std::time::Duration = std::time::Duration::from_millis(100);

/// How many latencies a surface keeps for the bench.
const KEPT_LATENCIES: usize = 4096;

/// An object both threads hold, of a class Apple documents as safe to use from any thread: a run loop, or a Metal
/// layer, whose drawables any thread may take.
struct Shareable<T>(T);

// SAFETY: Only used for `CFRunLoop`, whose functions are thread-safe, and `CAMetalLayer`, whose `nextDrawable` and
// display link the render thread uses while the main thread sets its geometry, as Metal's threading guide allows.
unsafe impl<T> Send for Shareable<T> {}
// SAFETY: As for `Send`.
unsafe impl<T> Sync for Shareable<T> {}

/// What lies over a frame, as the view decides it on the main thread.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OverlayState {
    /// The view has the keyboard focus and its window is the key window.
    pub focused: bool,
    /// The cursor is in the visible phase of its blinking.
    pub cursor_on: bool,
    /// Something of the view stands in for the cursor, such as copy mode's, so the program's is not drawn.
    pub hide_cursor: bool,
    /// Text an input method is composing, shown at the cursor.
    pub marked: String,
    /// A suggestion for the rest of the line being typed and the cursor position it was made for.
    pub ghost: Option<(term_session::Point, String)>,
    /// A line drawn after the program ended, and where.
    pub final_line: Option<(usize, String)>,
    /// The bell's flash.
    pub flash: bool,
}

/// The face the frames are drawn in.
#[derive(Debug, Clone, PartialEq)]
pub struct Face {
    /// The family, as the settings name it.
    pub family: String,
    /// The size in points.
    pub size: f64,
    /// Ligatures, features and axes (F13).
    pub typography: Typography,
}

/// Where the layer is and how many pixels a point has.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Geometry {
    /// The layer's frame in the view, in points.
    pub frame: CGRect,
    /// Pixels per point.
    pub scale: f64,
}

/// What the main thread changed since the render thread last looked.
#[derive(Default)]
struct Inputs {
    source: Option<FrameSource>,
    theme: Option<Theme>,
    face: Option<Face>,
    geometry: Option<Geometry>,
    overlay: OverlayState,
    /// Something above changed, so the next display frame draws even without a new frame of the session.
    dirty: bool,
    hidden: bool,
    closed: bool,
}

/// A surface as both threads share it.
struct Shared {
    id: u64,
    layer: Shareable<Retained<CAMetalLayer>>,
    inputs: Mutex<Inputs>,
    /// When the last key was pressed, in nanoseconds since the thread started; 0 once its frame showed.
    key_at: AtomicU64,
    latencies: Mutex<Vec<u32>>,
    awake: AtomicBool,
    /// The view's display link, made on the main thread and handed to the render thread's run loop.
    link: Mutex<Option<Shareable<Retained<CADisplayLink>>>>,
}

impl FrameSink for Shared {
    fn frame_ready(&self) {
        wake(self.id);
    }
}

/// The main thread's handle of a surface the render thread draws.
pub struct Renderer {
    shared: Arc<Shared>,
}

impl std::fmt::Debug for Renderer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Renderer")
            .field("id", &self.shared.id)
            .finish_non_exhaustive()
    }
}

impl Renderer {
    /// A surface that draws into `layer` on the render thread.
    pub fn new(layer: &CAMetalLayer) -> Renderer {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let shared = Arc::new(Shared {
            id: NEXT.fetch_add(1, Ordering::Relaxed),
            layer: Shareable(layer.retain()),
            inputs: Mutex::new(Inputs::default()),
            key_at: AtomicU64::new(0),
            latencies: Mutex::new(Vec::new()),
            awake: AtomicBool::new(false),
            link: Mutex::new(None),
        });
        Renderer { shared }
    }

    /// Paces the surface with the display link of `view`, which follows the view from display to display; on the
    /// main thread, once the view exists.
    pub fn attach(&self, view: &objc2_app_kit::NSView) {
        let target = LinkTarget::new(self.shared.id);
        // SAFETY: `termRenderTick:` takes the link as its one argument; the target holds only the surface's id, and the
        // render thread invalidates the link when the surface goes.
        let link = unsafe { view.displayLinkWithTarget_selector(&target, objc2::sel!(termRenderTick:)) };
        *self.shared.link.lock_ignoring_poison() = Some(Shareable(link));
        thread().added.lock_ignoring_poison().push(self.shared.clone());
        wake(self.shared.id);
    }

    /// Where the session's frames come from; the session tells this surface when one waits.
    pub fn sink(&self) -> Arc<dyn FrameSink> {
        self.shared.clone()
    }

    /// Draws the frames of `source` from now on.
    pub fn set_source(&self, source: FrameSource) {
        self.change(|inputs| inputs.source = Some(source));
    }

    /// Draws in `theme` from now on.
    pub fn set_theme(&self, theme: Theme) {
        self.change(|inputs| inputs.theme = Some(theme));
    }

    /// Draws with `face` from now on.
    pub fn set_face(&self, face: Face) {
        self.change(|inputs| inputs.face = Some(face));
    }

    /// Lays out the layer at `geometry`; on the main thread, which owns the layer's place in the view.
    pub fn set_geometry(&self, geometry: Geometry) {
        let layer = &self.shared.layer.0;
        let pixels = CGSize::new(
            (geometry.frame.size.width * geometry.scale).round(),
            (geometry.frame.size.height * geometry.scale).round(),
        );
        objc2_quartz_core::CATransaction::begin();
        objc2_quartz_core::CATransaction::setDisableActions(true);
        layer.setFrame(geometry.frame);
        layer.setContentsScale(geometry.scale);
        if pixels.width >= 1.0 && pixels.height >= 1.0 && layer.drawableSize() != pixels {
            layer.setDrawableSize(pixels);
        }
        objc2_quartz_core::CATransaction::commit();
        self.change(|inputs| inputs.geometry = Some(geometry));
    }

    /// Draws `overlay` over the frames from now on.
    pub fn set_overlay(&self, overlay: OverlayState) {
        let same = self.shared.inputs.lock_ignoring_poison().overlay == overlay;
        if !same {
            self.change(|inputs| inputs.overlay = overlay);
        }
    }

    /// Draws the latest frame again at the next display frame.
    pub fn redraw(&self) {
        self.change(|_| {});
    }

    /// Stops taking drawables for a hidden view while keeping its surface warm.
    pub fn set_visible(&self, visible: bool) {
        self.change(|inputs| inputs.hidden = !visible);
    }

    /// A key was pressed: the frame that next shows counts the time since for the bench (section 3 of the plan).
    pub fn key_pressed(&self) {
        let at = u64::try_from(epoch().elapsed().as_nanos()).unwrap_or(u64::MAX).max(1);
        self.shared.key_at.store(at, Ordering::Release);
    }

    /// The times from a key to the frame that showed after it, in microseconds, since this was asked last.
    pub fn take_latencies(&self) -> Vec<u32> {
        std::mem::take(&mut *self.shared.latencies.lock_ignoring_poison())
    }

    fn change(&self, f: impl FnOnce(&mut Inputs)) {
        {
            let mut inputs = self.shared.inputs.lock_ignoring_poison();
            f(&mut inputs);
            inputs.dirty = true;
        }
        wake(self.shared.id);
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        self.shared.inputs.lock_ignoring_poison().closed = true;
        wake(self.shared.id);
    }
}

fn epoch() -> Instant {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    *EPOCH.get_or_init(Instant::now)
}

/// The render thread as other threads reach it.
struct Thread {
    run_loop: Shareable<CFRetained<CFRunLoop>>,
    added: Mutex<Vec<Arc<Shared>>>,
    woken: Mutex<Vec<u64>>,
}

/// The render thread, started on first use.
fn thread() -> &'static Thread {
    static THREAD: OnceLock<Thread> = OnceLock::new();
    THREAD.get_or_init(|| {
        let (sender, receiver) = std::sync::mpsc::channel();
        let started = std::thread::Builder::new().name("term-render".into()).spawn(move || {
            term_base::priority::set_qos(term_base::priority::Qos::UserInitiated);
            let run_loop = CFRunLoop::current().expect("every thread has a run loop");
            let _ = sender.send(Shareable(run_loop));
            // A run loop without a source returns at once; the port keeps it waiting for display links and blocks.
            let port = objc2_foundation::NSMachPort::new();
            let current = NSRunLoop::currentRunLoop();
            // SAFETY: A port of this thread in the run loop's default mode, which keeps the loop alive.
            unsafe { current.addPort_forMode(&port, objc2_foundation::NSDefaultRunLoopMode) };
            loop {
                CFRunLoop::run();
            }
        });
        if let Err(error) = started {
            term_base::diag::report("starting the render thread", &error);
        }
        let run_loop = receiver
            .recv()
            .unwrap_or_else(|_| Shareable(CFRunLoop::main().expect("the main thread has a run loop")));
        Thread {
            run_loop,
            added: Mutex::new(Vec::new()),
            woken: Mutex::new(Vec::new()),
        }
    })
}

/// Has the render thread look at surface `id` at once.
fn wake(id: u64) {
    let thread = thread();
    {
        let mut woken = thread.woken.lock_ignoring_poison();
        if !woken.contains(&id) {
            woken.push(id);
        }
    }
    let block = RcBlock::new(|| term_base::guarded((), serve_wakes));
    // SAFETY: The common modes are a constant of CoreFoundation; the block only touches the render thread's own state
    // and the shared queues, which are behind locks.
    unsafe {
        let modes = objc2_core_foundation::kCFRunLoopCommonModes;
        thread.run_loop.0.perform_block(modes.map(|m| m.as_ref()), Some(&block));
    }
    thread.run_loop.0.wake_up();
}

/// A surface on the render thread.
struct Surface {
    shared: Arc<Shared>,
    link: Retained<CADisplayLink>,
    sprites: Option<Sprites>,
    fonts: Option<Rc<Fonts>>,
    face: Option<Face>,
    theme: Option<Theme>,
    geometry: Option<Geometry>,
    overlay: OverlayState,
    source: Option<FrameSource>,
    frame: Option<Arc<Snapshot>>,
    /// The frame of the picture committed last.
    shown: Option<Arc<Snapshot>>,
    needs_draw: bool,
    idle: u32,
    /// When the display link last fired.
    last_beat: Option<Instant>,
    lines: crate::lines::Lines,
    ring: crate::gpu::Ring,
}

thread_local! {
    static SURFACES: RefCell<HashMap<u64, Surface>> = RefCell::new(HashMap::new());
}

/// On the render thread: takes in new surfaces and starts the links of those that were woken.
fn serve_wakes() {
    let thread = thread();
    let added: Vec<Arc<Shared>> = std::mem::take(&mut *thread.added.lock_ignoring_poison());
    for shared in added {
        let id = shared.id;
        let Some(link) = shared.link.lock_ignoring_poison().take().map(|link| link.0) else {
            continue;
        };
        // SAFETY: The render thread's own run loop in its common modes; the surface invalidates the link when it goes.
        unsafe { link.addToRunLoop_forMode(&NSRunLoop::currentRunLoop(), objc2_foundation::NSRunLoopCommonModes) };
        SURFACES.with(|surfaces| {
            surfaces.borrow_mut().insert(
                id,
                Surface {
                    shared,
                    link,
                    sprites: None,
                    fonts: None,
                    face: None,
                    theme: None,
                    geometry: None,
                    overlay: OverlayState::default(),
                    source: None,
                    frame: None,
                    shown: None,
                    needs_draw: false,
                    idle: 0,
                    last_beat: None,
                    lines: crate::lines::Lines::default(),
                    ring: crate::gpu::Ring::default(),
                },
            );
        });
    }
    let woken: Vec<u64> = std::mem::take(&mut *thread.woken.lock_ignoring_poison());
    let mut closed = Vec::new();
    SURFACES.with(|surfaces| {
        let mut surfaces = surfaces.borrow_mut();
        for id in woken {
            if let Some(surface) = surfaces.get_mut(&id)
                && !surface.wake_now()
            {
                closed.push(id);
            }
        }
        for id in closed {
            if let Some(surface) = surfaces.remove(&id) {
                surface.link.invalidate();
            }
        }
    });
}

impl Surface {
    /// Something changed: a surface that drew nothing in the last display frame draws now, so an echo that comes
    /// between two frames is committed at once and shows with the next (B1); a busy one waits for its link. Whether
    /// the surface lives on.
    fn wake_now(&mut self) -> bool {
        {
            let inputs = self.shared.inputs.lock_ignoring_poison();
            if inputs.closed {
                return false;
            }
            if inputs.hidden {
                self.link.setPaused(true);
                return true;
            }
        }
        self.shared.awake.store(true, Ordering::Release);
        let silent = self.last_beat.is_none_or(|at| at.elapsed() > LINK_SILENT);
        let resting = self.link.isPaused() || self.idle > 0 || silent;
        if self.link.isPaused() {
            self.link.setPaused(false);
        }
        if !resting {
            return true;
        }
        self.tick()
    }

    /// A display frame, or a frame drawn between two: draws if anything changed; whether the surface lives on.
    fn tick(&mut self) -> bool {
        let layer = self.shared.layer.0.clone();
        self.update(&|| layer.nextDrawable())
    }

    /// Takes what changed and draws into a drawable `next` hands out if anything did; whether the surface lives on.
    fn update(&mut self, next: &dyn Fn() -> Option<Retained<ProtocolObject<dyn CAMetalDrawable>>>) -> bool {
        let dirty = {
            let mut inputs = self.shared.inputs.lock_ignoring_poison();
            if inputs.closed {
                return false;
            }
            if inputs.hidden {
                self.link.setPaused(true);
                return true;
            }
            if let Some(source) = inputs.source.take() {
                self.frame = source.latest();
                self.source = Some(source);
            }
            if let Some(theme) = inputs.theme.take() {
                self.theme = Some(theme);
            }
            if let Some(face) = inputs.face.take() {
                self.face = Some(face);
                self.fonts = None;
            }
            if let Some(geometry) = inputs.geometry.take() {
                if self
                    .geometry
                    .is_none_or(|old| (old.scale - geometry.scale).abs() > f64::EPSILON)
                {
                    self.fonts = None;
                }
                self.geometry = Some(geometry);
            }
            self.overlay.clone_from(&inputs.overlay);
            std::mem::take(&mut inputs.dirty)
        };
        let fresh = self.source.as_ref().and_then(FrameSource::take);
        let arrived = fresh.is_some();
        if let Some(frame) = fresh {
            self.frame = Some(frame);
        }
        // Output that changes nothing, such as moving the cursor to where it is, makes a frame equal to the picture.
        let new_frame = (arrived || dirty || self.needs_draw) && self.shown != self.frame;
        self.needs_draw |= dirty || new_frame;
        if arrived && !self.needs_draw {
            // The link keeps its beat, at which the next frame is taken.
            self.idle = 0;
            return true;
        }
        if !self.needs_draw {
            self.idle += 1;
            if self.idle >= IDLE_FRAMES {
                self.link.setPaused(true);
            }
            return true;
        }
        if self.geometry.is_none() || self.theme.is_none() {
            self.link.setPaused(true);
            return true;
        }
        self.idle = 0;
        if let Some(drawable) = next()
            && self.draw(&drawable)
        {
            self.needs_draw = false;
            if new_frame {
                self.note_latency();
            }
        }
        true
    }

    fn note_latency(&self) {
        let pressed = self.shared.key_at.swap(0, Ordering::AcqRel);
        if pressed == 0 {
            return;
        }
        let now = u64::try_from(epoch().elapsed().as_nanos()).unwrap_or(u64::MAX);
        let micros = u32::try_from(now.saturating_sub(pressed) / 1000).unwrap_or(u32::MAX);
        let mut kept = self.shared.latencies.lock_ignoring_poison();
        if kept.len() < KEPT_LATENCIES {
            kept.push(micros);
        }
    }

    fn draw(&mut self, drawable: &ProtocolObject<dyn CAMetalDrawable>) -> bool {
        let _interval = term_base::signpost::begin(term_base::signpost::name!("terminal frame"));
        let (Some(theme), Some(geometry)) = (self.theme.clone(), self.geometry) else {
            return false;
        };
        let Some(gpu) = crate::gpu::shared() else { return false };
        let face = self.face.clone().unwrap_or(Face {
            family: String::new(),
            size: 13.0,
            typography: Typography::default(),
        });
        let fonts = self
            .fonts
            .get_or_insert_with(|| Rc::new(Fonts::styled(&face.family, face.size, geometry.scale, &face.typography)))
            .clone();
        let sprites = match &mut self.sprites {
            Some(sprites) => sprites,
            None => match Sprites::new(gpu.device()) {
                Some(sprites) => self.sprites.insert(sprites),
                None => return false,
            },
        };
        let bounds = CGRect::new(objc2_core_foundation::CGPoint::ZERO, geometry.frame.size);
        let overlay = &self.overlay;
        let final_line = overlay.final_line.as_ref().map(|(line, text)| (*line, text.as_str()));
        let drawn = Overlay {
            focused: overlay.focused,
            cursor_on: overlay.cursor_on,
            hide_cursor: overlay.hide_cursor,
            marked: &overlay.marked,
            ghost: overlay.ghost.as_ref().map(|(point, text)| (*point, text.as_str())),
            final_line,
            flash: overlay.flash,
        };
        let quads = match self.frame.as_deref() {
            Some(screen) => self.lines.quads(sprites, &fonts, &theme, screen, &drawn, bounds),
            None => {
                let paint = |canvas: &mut QuadCanvas| {
                    crate::render::Canvas::fill(canvas, theme.palette.background, 1.0, bounds);
                };
                crate::gpu::quads(sprites, &fonts, &paint)
            }
        };
        let target = drawable.texture();
        let pixels = CGSize::new(
            (geometry.frame.size.width * geometry.scale).round(),
            (geometry.frame.size.height * geometry.scale).round(),
        );
        let Some(buffer) = gpu.encode(
            &target,
            pixels,
            &quads,
            sprites,
            theme.palette.background,
            &mut self.ring,
        ) else {
            return false;
        };
        let drawable: &ProtocolObject<dyn MTLDrawable> = ProtocolObject::from_ref(drawable);
        buffer.presentDrawable(drawable);
        buffer.commit();
        sprites.uploaded();
        self.shown.clone_from(&self.frame);
        true
    }
}

/// Called on the render thread at each beat of the display link of surface `id`.
fn beat(id: u64) {
    let keep = SURFACES.with(|surfaces| {
        let mut surfaces = surfaces.borrow_mut();
        surfaces.get_mut(&id).map(|surface| {
            surface.last_beat = Some(Instant::now());
            surface.tick()
        })
    });
    if keep == Some(false) {
        SURFACES.with(|surfaces| {
            if let Some(surface) = surfaces.borrow_mut().remove(&id) {
                surface.link.invalidate();
            }
        });
    }
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and `LinkTarget` does not implement Drop.
    #[unsafe(super(NSObject))]
    #[name = "TermRenderLinkTarget"]
    #[ivars = u64]
    struct LinkTarget;

    unsafe impl NSObjectProtocol for LinkTarget {}

    impl LinkTarget {
        #[unsafe(method(termRenderTick:))]
        fn tick(&self, _link: &CADisplayLink) {
            term_base::guarded((), || beat(*self.ivars()));
        }
    }
);

impl LinkTarget {
    fn new(id: u64) -> Retained<LinkTarget> {
        let this = Self::alloc().set_ivars(id);
        // SAFETY: NSObject's initializer, called once on the newly allocated object.
        unsafe { msg_send![super(this), init] }
    }
}
