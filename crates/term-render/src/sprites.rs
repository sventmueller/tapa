//! The pictures a GPU frame of the terminal is put together from (glyphs, line characters, shaped text and
//! underlines), each drawn once by Core Graphics into a texture, and the canvas that turns a frame into quads over
//! them.

use std::collections::HashMap;
use std::ptr::NonNull;
use std::time::{Duration, Instant};

use objc2::Message;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_core_foundation::{CFRetained, CGPoint, CGRect};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGBitmapContextGetBytesPerRow, CGBitmapContextGetData, CGColorSpace, CGContext,
    CGImageAlphaInfo, CGImageByteOrderInfo, kCGColorSpaceSRGB,
};
use objc2_core_text::{CTFontOrientation, CTLineBoundsOptions};
use objc2_metal::{
    MTLBlitCommandEncoder, MTLCommandBuffer, MTLCommandEncoder, MTLDevice, MTLOrigin, MTLPixelFormat,
    MTLResourceOptions, MTLSize, MTLTexture, MTLTextureDescriptor, MTLTextureUsage,
};
use term_session::Rgb;

use crate::boxdraw::{self, Pen};
use crate::render::{self, Canvas, Face, Fonts, Glyph, Stroke, rect};

/// The side of a view's first atlas in pixels; it doubles when a single frame does not fit, up to `LAST_SIDE`.
const FIRST_SIDE: usize = 1024;
const LAST_SIDE: usize = 8192;
/// An atlas emptied again within this many frames is smaller than what the output cycles through, so it doubles
/// instead; Ghostty's only ever doubles (`font/SharedGrid.zig`, `error.AtlasFull`).
const CHURN_FRAMES: u64 = 120;
/// How often an atlas that doubled for churn is checked for sprites no frame asked for since the last check.
const CALM: Duration = Duration::from_secs(30);
/// Glyphs are drawn at this many positions within a pixel.
const SUBPIXEL: f64 = 4.0;
/// Underlines are cut into sprites of at most this many cells.
const UNDERLINE_CELLS: usize = 32;
const WHITE: Rgb = Rgb::new(255, 255, 255);
const BLACK: Rgb = Rgb::new(0, 0, 0);

/// A rectangle of the frame filled with its colour.
pub const SOLID: u32 = 0;
/// A rectangle filled with its colour where the sprite under it covers it.
pub const MASK: u32 = 1;
/// A rectangle showing the sprite's own colours.
pub const PICTURE: u32 = 2;

/// One rectangle of a frame in pixels from the top left, as the shader reads it.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quad {
    /// Where the quad goes, x, y, width and height in pixels.
    pub rect: [f32; 4],
    /// The sprite's rectangle in the atlas, in pixels.
    pub uv: [f32; 4],
    /// Straight, not premultiplied.
    pub color: [f32; 4],
    /// `SOLID`, `MASK` or `PICTURE`, then padding.
    pub mode: [u32; 4],
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Key {
    Glyph {
        font: usize,
        glyph: u16,
        shift: u8,
        ink: Rgb,
    },
    Box {
        c: char,
        width: u32,
        height: u32,
    },
    Text {
        text: String,
        face: Face,
        shift: u8,
        ink: Rgb,
        color: Option<Rgb>,
    },
    Underline {
        kind: Stroke,
        cells: usize,
        phase: u32,
    },
}

/// A picture in the atlas.
#[derive(Debug, Clone, Copy)]
struct Sprite {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    /// Its top left relative to the pixel it is drawn at.
    left: i32,
    top: i32,
    /// Its pixels carry their own colours, as emoji do; otherwise they are coverage for the text colour.
    coloured: bool,
    /// The last frame that asked for it.
    seen: u64,
}

/// A sprite's pixels on their way into the texture.
struct Upload {
    origin: MTLOrigin,
    size: MTLSize,
    pixels: Vec<u8>,
}

/// A picture before it goes into the atlas: BGRA rows, premultiplied.
struct Picture {
    width: usize,
    height: usize,
    left: i32,
    top: i32,
    pixels: Vec<u8>,
    coloured: bool,
}

/// Rows of sprites of about one height, filled left to right.
#[derive(Debug, Default)]
struct Shelves {
    side: usize,
    rows: Vec<Shelf>,
    bottom: usize,
}

#[derive(Debug)]
struct Shelf {
    y: usize,
    height: usize,
    used: usize,
}

impl Shelves {
    fn new(side: usize) -> Shelves {
        Shelves {
            side,
            ..Shelves::default()
        }
    }

    /// Where a `width` × `height` sprite goes, leaving a pixel free to the right and below; none when it is full.
    fn place(&mut self, width: usize, height: usize) -> Option<(usize, usize)> {
        let side = self.side;
        if width >= side || height >= side {
            return None;
        }
        let fits = |shelf: &&mut Shelf| {
            shelf.height >= height && shelf.height <= height + height / 4 + 2 && shelf.used + width < side
        };
        if let Some(shelf) = self.rows.iter_mut().find(fits) {
            let at = (shelf.used, shelf.y);
            shelf.used += width + 1;
            return Some(at);
        }
        if self.bottom + height >= side {
            return None;
        }
        let y = self.bottom;
        self.bottom += height + 1;
        self.rows.push(Shelf {
            y,
            height,
            used: width + 1,
        });
        Some((0, y))
    }
}

/// A view's sprites in one texture, drawn with one set of fonts.
pub struct Sprites {
    /// Counts how often the atlas was emptied, so quads kept from earlier frames know when their sprites are gone.
    generation: u64,
    device: Retained<ProtocolObject<dyn MTLDevice>>,
    texture: Retained<ProtocolObject<dyn MTLTexture>>,
    /// Sprites placed but not yet in the texture: the next frame copies them in on the GPU, after the frames before
    /// it read the texture, so the processor never waits for one.
    uploads: Vec<Upload>,
    /// The texture the atlas grew out of, whose pixels the next frame copies over first.
    outgrown: Option<Retained<ProtocolObject<dyn MTLTexture>>>,
    shelves: Shelves,
    entries: HashMap<Key, Option<Sprite>>,
    widths: HashMap<(String, Face), f64>,
    fonts: Option<u64>,
    /// A sprite did not fit since the last `prepare`.
    full: bool,
    /// The atlas held sprites of earlier frames when the last `prepare` ran.
    inherited: bool,
    /// Frames started so far, and the one in which earlier frames' sprites last had to make room.
    frames: u64,
    made_room: Option<u64>,
    /// The side a single frame was seen to need.
    floor: usize,
    /// When and in which frame a grown atlas was last checked for unused sprites.
    checked: (Instant, u64),
}

impl Sprites {
    /// An empty atlas on `device`; none when the device has no memory for it.
    pub fn new(device: &ProtocolObject<dyn MTLDevice>) -> Option<Sprites> {
        Some(Sprites {
            generation: 0,
            device: device.retain(),
            texture: atlas(device, FIRST_SIDE)?,
            uploads: Vec::new(),
            outgrown: None,
            shelves: Shelves::new(FIRST_SIDE),
            entries: HashMap::new(),
            widths: HashMap::new(),
            fonts: None,
            full: false,
            inherited: false,
            frames: 0,
            made_room: None,
            floor: FIRST_SIDE,
            checked: (Instant::now(), 0),
        })
    }

    /// The texture the quads sample.
    pub fn texture(&self) -> &ProtocolObject<dyn MTLTexture> {
        &self.texture
    }

    /// Copies the sprites placed since the last committed frame into the texture, as the first pass of `frame`;
    /// whether it could. They stay pending until [`Sprites::uploaded`].
    pub fn upload(&self, frame: &ProtocolObject<dyn MTLCommandBuffer>) -> bool {
        if self.uploads.is_empty() && self.outgrown.is_none() {
            return true;
        }
        let Some(blit) = frame.blitCommandEncoder() else {
            return false;
        };
        if let Some(outgrown) = &self.outgrown {
            let origin = MTLOrigin { x: 0, y: 0, z: 0 };
            let size = MTLSize {
                width: outgrown.width(),
                height: outgrown.height(),
                depth: 1,
            };
            // SAFETY: The whole of the smaller texture goes into the corner of the larger one of the same format;
            // the command buffer retains both.
            unsafe {
                blit.copyFromTexture_sourceSlice_sourceLevel_sourceOrigin_sourceSize_toTexture_destinationSlice_destinationLevel_destinationOrigin(
                    outgrown,
                    0,
                    0,
                    origin,
                    size,
                    &self.texture,
                    0,
                    0,
                    origin,
                );
            }
        }
        if self.uploads.is_empty() {
            blit.endEncoding();
            return true;
        }
        let pixels: Vec<u8> = self.uploads.iter().flat_map(|u| u.pixels.iter().copied()).collect();
        let bytes = NonNull::from(pixels.as_slice()).cast();
        // SAFETY: The pointer covers the pixels' bytes, which Metal copies into the new buffer.
        let staging = unsafe {
            self.device
                .newBufferWithBytes_length_options(bytes, pixels.len(), MTLResourceOptions::StorageModeShared)
        };
        let Some(staging) = staging else {
            blit.endEncoding();
            return false;
        };
        let mut offset = 0;
        for upload in &self.uploads {
            let row = upload.size.width * 4;
            // SAFETY: Each copy reads one sprite's rows of 4 bytes per pixel from where they lie in the staging
            // buffer, which the command buffer retains, into a region inside the texture.
            unsafe {
                blit.copyFromBuffer_sourceOffset_sourceBytesPerRow_sourceBytesPerImage_sourceSize_toTexture_destinationSlice_destinationLevel_destinationOrigin(
                    &staging,
                    offset,
                    row,
                    row * upload.size.height,
                    upload.size,
                    &self.texture,
                    0,
                    0,
                    upload.origin,
                );
            }
            offset += upload.pixels.len();
        }
        blit.endEncoding();
        true
    }

    /// The frame that copied the pending sprites in was committed.
    pub fn uploaded(&mut self) {
        self.uploads.clear();
        self.outgrown = None;
    }

    /// Readies the sprites for a frame drawn with `fonts`, forgetting those of other fonts.
    pub fn prepare(&mut self, fonts: &Fonts) {
        if self.fonts != Some(fonts.id) {
            self.fonts = Some(fonts.id);
            self.generation += 1;
            self.entries.clear();
            self.widths.clear();
            self.uploads.clear();
            self.outgrown = None;
            self.shelves = Shelves::new(self.shelves.side);
        }
        self.start_frame(Instant::now());
    }

    fn start_frame(&mut self, now: Instant) {
        if now.saturating_duration_since(self.checked.0) >= CALM {
            self.shrink();
            self.checked = (now, self.frames);
        }
        self.frames += 1;
        self.full = false;
        self.inherited = !self.shelves.rows.is_empty();
    }

    /// How often the atlas was emptied; quads made before a change point at sprites that are gone.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// A sprite did not fit since `prepare`.
    pub fn full(&self) -> bool {
        self.full
    }

    /// Makes room after a frame did not fit, for drawing it again: an atlas that still holds sprites of earlier
    /// frames is emptied at its size, or doubles and keeps them when that keeps happening; one the frame alone
    /// filled doubles. False when it is as large as it gets.
    pub fn make_room(&mut self) -> bool {
        if self.inherited {
            let churning = self.made_room.is_some_and(|at| self.frames - at <= CHURN_FRAMES);
            if !(churning && self.grow()) {
                self.empty(self.shelves.side);
            }
            self.made_room = Some(self.frames);
            return true;
        }
        let grown = self.grow();
        self.floor = self.shelves.side;
        grown
    }

    /// Starts over in the smallest atlas, down to what one frame needs, that the sprites asked for since the last
    /// check fill at most by half.
    fn shrink(&mut self) {
        let used: usize = self
            .entries
            .values()
            .flatten()
            .filter(|sprite| sprite.seen > self.checked.1)
            .map(|sprite| (sprite.width as usize + 1) * (sprite.height as usize + 1))
            .sum();
        let mut side = self.shelves.side;
        while side / 2 >= self.floor && used * 8 <= side * side {
            side /= 2;
        }
        if side < self.shelves.side
            && let Some(texture) = atlas(&self.device, side)
        {
            self.texture = texture;
            self.empty(side);
        }
    }

    /// Moves the sprites into an atlas twice the size, where they keep their places; false when it is as large as
    /// it gets.
    pub(crate) fn grow(&mut self) -> bool {
        let side = self.shelves.side * 2;
        if side > LAST_SIDE {
            return false;
        }
        let Some(texture) = atlas(&self.device, side) else {
            return false;
        };
        let outgrown = std::mem::replace(&mut self.texture, texture);
        // One that grew twice before a frame was committed never got the pixels of the one before it.
        self.outgrown.get_or_insert(outgrown);
        self.shelves.side = side;
        true
    }

    /// Forgets every sprite and width; frames still on the GPU keep reading the old pixels, as Metal runs the next
    /// frame's copies over them only after those frames.
    fn empty(&mut self, side: usize) {
        self.generation += 1;
        self.uploads.clear();
        self.outgrown = None;
        self.shelves = Shelves::new(side);
        self.entries.clear();
        self.widths.clear();
    }

    fn get(&mut self, key: Key, make: impl FnOnce() -> Option<Picture>) -> Option<Sprite> {
        if let Some(found) = self.entries.get_mut(&key) {
            if let Some(sprite) = found {
                sprite.seen = self.frames;
            }
            return *found;
        }
        let Some(picture) = make() else {
            self.entries.insert(key, None);
            return None;
        };
        let Some((x, y)) = self.shelves.place(picture.width, picture.height) else {
            self.full = true;
            return None;
        };
        self.uploads.push(Upload {
            origin: MTLOrigin { x, y, z: 0 },
            size: MTLSize {
                width: picture.width,
                height: picture.height,
                depth: 1,
            },
            pixels: picture.pixels,
        });
        let narrow = |v: usize| u32::try_from(v).unwrap_or(u32::MAX);
        let sprite = Sprite {
            x: narrow(x),
            y: narrow(y),
            width: narrow(picture.width),
            height: narrow(picture.height),
            left: picture.left,
            top: picture.top,
            coloured: picture.coloured,
            seen: self.frames,
        };
        self.entries.insert(key, Some(sprite));
        Some(sprite)
    }
}

fn atlas(device: &ProtocolObject<dyn MTLDevice>, side: usize) -> Option<Retained<ProtocolObject<dyn MTLTexture>>> {
    // SAFETY: A plain two-dimensional texture without mipmaps.
    let descriptor = unsafe {
        MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
            MTLPixelFormat::BGRA8Unorm,
            side,
            side,
            false,
        )
    };
    descriptor.setUsage(MTLTextureUsage::ShaderRead);
    device.newTextureWithDescriptor(&descriptor)
}

/// `value` in whole pixels and the quarter pixels left over.
fn split(value: f64) -> (f64, u8) {
    let floor = value.floor();
    let shift = ((value - floor) * SUBPIXEL).round();
    if shift >= SUBPIXEL {
        (floor + 1.0, 0)
    } else {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a whole number from 0 to 3"
        )]
        (floor, shift as u8)
    }
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "pixel positions and sizes of a view are far inside f32's range"
)]
fn px(value: f64) -> f32 {
    value as f32
}

/// What `draw` paints within `frame`, in points around the origin, into a picture whose pixels line up with the
/// screen's: in `ink` on transparent, and where that brings colours of its own, as emoji do, in `color` instead.
fn picture(frame: CGRect, scale: f64, ink: Rgb, color: Rgb, draw: &dyn Fn(&CGContext, Rgb)) -> Option<Picture> {
    if frame.size.width <= 0.0 || frame.size.height <= 0.0 {
        return None;
    }
    let left = (frame.origin.x * scale).floor() - 1.0;
    let top = (frame.origin.y * scale).floor() - 1.0;
    let right = ((frame.origin.x + frame.size.width) * scale).ceil() + 1.0;
    let bottom = ((frame.origin.y + frame.size.height) * scale).ceil() + 1.0;
    let size = |v: f64| render::clamp_to(v, LAST_SIDE);
    let (width, height) = (size(right - left), size(bottom - top));
    let pixels = bitmap(width, height, scale, (left, top), &|ctx| draw(ctx, ink))?;
    let coloured = pixels
        .as_chunks::<4>()
        .0
        .iter()
        .any(|p| p[0].abs_diff(p[1]) > 2 || p[1].abs_diff(p[2]) > 2);
    let pixels = if coloured && color != ink {
        bitmap(width, height, scale, (left, top), &|ctx| draw(ctx, color))?
    } else {
        pixels
    };
    #[expect(clippy::cast_possible_truncation, reason = "whole pixels near a cell")]
    Some(Picture {
        width,
        height,
        left: left as i32,
        top: top as i32,
        pixels,
        coloured,
    })
}

/// The BGRA pixels `draw` paints into a `width` × `height` bitmap that starts at `origin` pixels from the point
/// sprites are drawn at, in points scaled by `scale` and running down like the view's.
pub fn bitmap(
    width: usize,
    height: usize,
    scale: f64,
    origin: (f64, f64),
    draw: &dyn Fn(&CGContext),
) -> Option<Vec<u8>> {
    let ctx = context(width, height)?;
    let c = Some(&*ctx);
    CGContext::translate_ctm(c, 0.0, height as f64);
    CGContext::scale_ctm(c, 1.0, -1.0);
    CGContext::translate_ctm(c, -origin.0, -origin.1);
    CGContext::scale_ctm(c, scale, scale);
    draw(&ctx);
    let data = CGBitmapContextGetData(c).cast::<u8>();
    let stride = CGBitmapContextGetBytesPerRow(c);
    if data.is_null() {
        return None;
    }
    let mut pixels = Vec::with_capacity(width * height * 4);
    for row in 0..height {
        // SAFETY: The context's buffer holds `height` rows of `stride` bytes, each starting with `width` pixels.
        let line = unsafe { std::slice::from_raw_parts(data.add(row * stride), width * 4) };
        pixels.extend_from_slice(line);
    }
    Some(pixels)
}

/// A transparent sRGB bitmap of `width` × `height` pixels, BGRA in memory like the atlas.
fn context(width: usize, height: usize) -> Option<CFRetained<CGContext>> {
    // SAFETY: The name is one of Core Graphics' own colour spaces.
    let space = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB }))?;
    let info = CGImageAlphaInfo::PremultipliedFirst.0 | CGImageByteOrderInfo::Order32Little.0;
    // SAFETY: A null buffer lets Core Graphics allocate one for the size given.
    unsafe { CGBitmapContextCreate(std::ptr::null_mut(), width, height, 8, 0, Some(&space), info) }
}

fn glyph_picture(fonts: &Fonts, glyph: Glyph, shift: u8, ink: Rgb) -> Option<Picture> {
    let font = fonts.font(glyph.font);
    let scale = fonts.metrics.scale;
    let mut id = glyph.glyph;
    // SAFETY: One glyph, and a null buffer for the rectangles of each asks only for the one around all.
    let bounds = unsafe {
        font.bounding_rects_for_glyphs(
            CTFontOrientation::Horizontal,
            NonNull::from(&mut id),
            std::ptr::null_mut(),
            1,
        )
    };
    let dx = f64::from(shift) / SUBPIXEL / scale;
    let frame = rect(
        dx + bounds.origin.x,
        -(bounds.origin.y + bounds.size.height),
        bounds.size.width,
        bounds.size.height,
    );
    picture(frame, scale, ink, ink, &|ctx, color| {
        render::set_color(ctx, color, 1.0);
        render::show_glyphs(ctx, &font, &[glyph.glyph], &[CGPoint::new(dx, 0.0)], 0.0);
    })
}

fn text_picture(fonts: &Fonts, text: &str, face: Face, shift: u8, ink: Rgb, color: Rgb) -> Option<Picture> {
    let scale = fonts.metrics.scale;
    let line = render::line_of(fonts, text, face);
    // SAFETY: A valid line; the option asks for the bounds of the glyphs' outlines.
    let bounds = unsafe { line.bounds_with_options(CTLineBoundsOptions::UseGlyphPathBounds) };
    let dx = f64::from(shift) / SUBPIXEL / scale;
    let frame = rect(
        dx + bounds.origin.x,
        -(bounds.origin.y + bounds.size.height),
        bounds.size.width,
        bounds.size.height,
    );
    picture(frame, scale, ink, color, &|ctx, color| {
        render::draw_line(ctx, &line, dx, 0.0, color);
    })
}

/// Collects a frame as quads over the sprites.
pub struct QuadCanvas<'a> {
    sprites: &'a mut Sprites,
    fonts: &'a Fonts,
    /// The quads so far.
    pub quads: Vec<Quad>,
}

impl<'a> QuadCanvas<'a> {
    /// A canvas filling `sprites` with glyphs of `fonts`.
    pub fn new(sprites: &'a mut Sprites, fonts: &'a Fonts) -> QuadCanvas<'a> {
        QuadCanvas {
            sprites,
            fonts,
            quads: Vec::new(),
        }
    }

    fn scale(&self) -> f64 {
        self.fonts.metrics.scale
    }

    /// `sprite` placed at the pixel `x`, `y`, tinted with `color` unless it has colours of its own.
    fn show(&mut self, sprite: Sprite, x: f64, y: f64, color: Rgb) {
        self.quads.push(Quad {
            rect: [
                px(x + f64::from(sprite.left)),
                px(y + f64::from(sprite.top)),
                px(f64::from(sprite.width)),
                px(f64::from(sprite.height)),
            ],
            uv: [
                px(f64::from(sprite.x)),
                px(f64::from(sprite.y)),
                px(f64::from(sprite.width)),
                px(f64::from(sprite.height)),
            ],
            color: rgba(color, 1.0),
            mode: [if sprite.coloured { PICTURE } else { MASK }, 0, 0, 0],
        });
    }
}

/// Black for dark text and white for light: Core Graphics draws thinner strokes for dark text on a light background
/// than for light text on a dark one, and a sprite keeps the strokes it was drawn with.
fn ink(color: Rgb) -> Rgb {
    let luminance = 0.299 * f64::from(color.r) + 0.587 * f64::from(color.g) + 0.114 * f64::from(color.b);
    if luminance < 128.0 { BLACK } else { WHITE }
}

fn rgba(color: Rgb, alpha: f64) -> [f32; 4] {
    let channel = |v: u8| f32::from(v) / 255.0;
    [channel(color.r), channel(color.g), channel(color.b), px(alpha)]
}

impl Canvas for QuadCanvas<'_> {
    fn fill(&mut self, color: Rgb, alpha: f64, area: CGRect) {
        let scale = self.scale();
        self.quads.push(Quad {
            rect: [
                px(area.origin.x * scale),
                px(area.origin.y * scale),
                px(area.size.width * scale),
                px(area.size.height * scale),
            ],
            uv: [0.0; 4],
            color: rgba(color, alpha),
            mode: [SOLID, 0, 0, 0],
        });
    }

    fn glyph(&mut self, glyph: Glyph, x: f64, baseline: f64, color: Rgb) {
        let scale = self.scale();
        let (x, shift) = split(x * scale);
        let ink = ink(color);
        let key = Key::Glyph {
            font: glyph.font,
            glyph: glyph.glyph,
            shift,
            ink,
        };
        let fonts = self.fonts;
        if let Some(sprite) = self.sprites.get(key, || glyph_picture(fonts, glyph, shift, ink)) {
            self.show(sprite, x, (baseline * scale).round(), color);
        }
    }

    fn boxdraw(&mut self, c: char, pen: &Pen, color: Rgb) -> bool {
        if !boxdraw::covers(c) {
            return false;
        }
        let scale = self.scale();
        let size = |v: f64| u32::try_from(render::clamp_to((v * scale).round(), LAST_SIDE)).unwrap_or(0);
        let key = Key::Box {
            c,
            width: size(pen.rect.size.width),
            height: size(pen.rect.size.height),
        };
        let cell = Pen {
            rect: rect(0.0, 0.0, pen.rect.size.width, pen.rect.size.height),
            ..*pen
        };
        let made = self.sprites.get(key, || {
            picture(cell.rect, scale, WHITE, WHITE, &|ctx, color| {
                render::set_color(ctx, color, 1.0);
                boxdraw::draw(ctx, c, &cell);
            })
        });
        if let Some(sprite) = made {
            let (x, y) = (pen.rect.origin.x * scale, pen.rect.origin.y * scale);
            self.show(sprite, x.round(), y.round(), color);
        }
        true
    }

    fn measure(&mut self, text: &str, face: Face) -> f64 {
        let key = (text.to_owned(), face);
        if let Some(width) = self.sprites.widths.get(&key) {
            return *width;
        }
        let width = render::line_width(&render::line_of(self.fonts, text, face));
        self.sprites.widths.insert(key, width);
        width
    }

    fn line(&mut self, text: &str, face: Face, x: f64, baseline: f64, color: Rgb) {
        let scale = self.scale();
        let (x, shift) = split(x * scale);
        let y = (baseline * scale).round();
        let fonts = self.fonts;
        let ink = ink(color);
        let key = |color| Key::Text {
            text: text.to_owned(),
            face,
            shift,
            ink,
            color,
        };
        let plain = self
            .sprites
            .get(key(None), || text_picture(fonts, text, face, shift, ink, ink));
        let sprite = match plain {
            Some(sprite) if sprite.coloured => self
                .sprites
                .get(key(Some(color)), || text_picture(fonts, text, face, shift, ink, color)),
            other => other,
        };
        if let Some(sprite) = sprite {
            self.show(sprite, x, y, color);
        }
    }

    fn underline(&mut self, kind: Stroke, area: CGRect, y: f64, color: Rgb) {
        let m = self.fonts.metrics;
        let scale = m.scale;
        let cells = render::clamp_to((area.size.width / m.cell_width).round(), usize::MAX);
        let pad = m.cell_height / 2.0;
        let mut start = 0;
        while start < cells {
            let count = (cells - start).min(UNDERLINE_CELLS);
            let offset = start as f64 * m.cell_width;
            let phase = kind.period(m.stroke).map_or(0.0, |period| offset % period);
            let key = Key::Underline {
                kind,
                cells: count,
                phase: u32::try_from(render::clamp_to((phase * scale).round(), LAST_SIDE)).unwrap_or(0),
            };
            let piece = rect(0.0, 0.0, count as f64 * m.cell_width, m.cell_height);
            let frame = rect(0.0, -pad, piece.size.width, piece.size.height + 2.0 * pad);
            let below = y - area.origin.y;
            let made = self.sprites.get(key, || {
                picture(frame, scale, WHITE, WHITE, &|ctx, color| {
                    render::set_color(ctx, color, 1.0);
                    render::draw_underline(ctx, &m, kind, piece, below, phase);
                })
            });
            if let Some(sprite) = made {
                let x = ((area.origin.x + offset) * scale).round();
                self.show(sprite, x, (area.origin.y * scale).round(), color);
            }
            start += count;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shelves_keep_sprites_apart_and_say_when_they_are_full() {
        let mut shelves = Shelves::new(64);
        let a = shelves.place(20, 10).unwrap();
        let b = shelves.place(20, 10).unwrap();
        let c = shelves.place(20, 30).unwrap();
        assert_eq!(a, (0, 0));
        assert_eq!(b, (21, 0));
        assert_eq!(c, (0, 11));
        assert!(shelves.place(64, 1).is_none(), "wider than the atlas");
        assert!(shelves.place(10, 40).is_none(), "no room below");
        assert_eq!(
            shelves.place(20, 9),
            Some((42, 0)),
            "a slightly lower sprite shares a shelf"
        );
    }

    #[test]
    fn a_full_atlas_doubles_until_it_is_as_large_as_it_gets_and_remembers_pictures_that_are_none() {
        let Some(device) = objc2_metal::MTLCreateSystemDefaultDevice() else {
            return;
        };
        let mut sprites = Sprites::new(&device).unwrap();
        let tall = |side: usize| Picture {
            width: 8,
            height: side,
            left: 0,
            top: 0,
            pixels: vec![255; 8 * side * 4],
            coloured: false,
        };
        let key = |glyph| Key::Glyph {
            font: 0,
            glyph,
            shift: 0,
            ink: WHITE,
        };
        assert!(sprites.get(key(1), || Some(tall(FIRST_SIDE + 1))).is_none());
        assert!(sprites.full(), "the picture did not fit");
        assert!(sprites.grow());
        assert_eq!(sprites.texture().width(), FIRST_SIDE * 2);
        let placed = sprites.get(key(1), || Some(tall(FIRST_SIDE + 1))).unwrap();
        assert_eq!(
            (placed.x, placed.y, placed.height),
            (0, 0, u32::try_from(FIRST_SIDE + 1).unwrap())
        );
        while sprites.grow() {}
        assert_eq!(sprites.texture().width(), LAST_SIDE);
        let mut made = 0;
        for _ in 0..2 {
            assert!(
                sprites
                    .get(key(2), || {
                        made += 1;
                        None
                    })
                    .is_none()
            );
        }
        assert_eq!(made, 1, "a glyph without a picture is asked for once");
    }

    #[test]
    fn a_full_atlas_is_emptied_for_the_frame_and_grows_only_when_one_frame_does_not_fit() {
        let Some(device) = objc2_metal::MTLCreateSystemDefaultDevice() else {
            return;
        };
        let mut sprites = Sprites::new(&device).unwrap();
        let tall = |height: usize| {
            move || {
                Some(Picture {
                    width: 8,
                    height,
                    left: 0,
                    top: 0,
                    pixels: vec![255; 8 * height * 4],
                    coloured: false,
                })
            }
        };
        let key = |glyph| Key::Glyph {
            font: 0,
            glyph,
            shift: 0,
            ink: WHITE,
        };
        let three_quarters = FIRST_SIDE * 3 / 4;
        sprites.start_frame(Instant::now());
        assert!(sprites.get(key(1), tall(three_quarters)).is_some());
        assert!(!sprites.full());

        sprites.start_frame(Instant::now());
        assert!(sprites.get(key(2), tall(FIRST_SIDE / 2)).is_none());
        assert!(sprites.full(), "the earlier frame's sprite leaves no room");
        assert!(sprites.make_room());
        assert_eq!(sprites.texture().width(), FIRST_SIDE, "emptied at its size");
        assert!(sprites.entries.is_empty() && sprites.uploads.is_empty());

        sprites.start_frame(Instant::now());
        assert!(sprites.get(key(2), tall(FIRST_SIDE / 2)).is_some());
        assert!(sprites.get(key(1), tall(three_quarters)).is_none());
        assert!(sprites.full(), "this frame alone does not fit");
        assert!(sprites.make_room());
        assert_eq!(sprites.texture().width(), FIRST_SIDE * 2, "only now the atlas grows");

        sprites.start_frame(Instant::now());
        assert!(sprites.get(key(2), tall(FIRST_SIDE / 2)).is_some());
        assert!(sprites.get(key(1), tall(three_quarters)).is_some());
        assert!(!sprites.full());
    }

    #[test]
    fn an_atlas_emptied_again_and_again_doubles_and_goes_back_once_it_is_calm() {
        let Some(device) = objc2_metal::MTLCreateSystemDefaultDevice() else {
            return;
        };
        let mut sprites = Sprites::new(&device).unwrap();
        let side = FIRST_SIDE * 3 / 4;
        let tall = || {
            Some(Picture {
                width: side,
                height: side,
                left: 0,
                top: 0,
                pixels: vec![255; side * side * 4],
                coloured: false,
            })
        };
        let key = |glyph| Key::Glyph {
            font: 0,
            glyph,
            shift: 0,
            ink: WHITE,
        };
        let now = Instant::now();
        let frame = |sprites: &mut Sprites, glyph| {
            sprites.start_frame(now);
            if sprites.get(key(glyph), tall).is_none() {
                assert!(sprites.make_room());
                sprites.start_frame(now);
                assert!(sprites.get(key(glyph), tall).is_some());
            }
        };
        frame(&mut sprites, 1);
        frame(&mut sprites, 2);
        assert_eq!(sprites.texture().width(), FIRST_SIDE, "emptied the first time");
        frame(&mut sprites, 3);
        assert_eq!(sprites.texture().width(), FIRST_SIDE * 2, "doubled the second time");
        assert!(sprites.entries.contains_key(&key(3)) && sprites.entries.contains_key(&key(2)));

        sprites.start_frame(now + CALM * 2);
        assert!(sprites.get(key(3), tall).is_some());
        assert_eq!(sprites.texture().width(), FIRST_SIDE * 2, "its sprites were in use");
        sprites.start_frame(now + CALM * 4);
        assert!(sprites.get(key(3), tall).is_some());
        assert_eq!(sprites.texture().width(), FIRST_SIDE * 2, "and still are");
        sprites.start_frame(now + CALM * 6);
        sprites.start_frame(now + CALM * 8);
        assert_eq!(sprites.texture().width(), FIRST_SIDE, "unused since the last check");
        assert!(sprites.entries.is_empty());
    }

    #[test]
    fn positions_split_into_pixels_and_quarters() {
        assert_eq!(split(10.0), (10.0, 0));
        assert_eq!(split(10.26), (10.0, 1));
        assert_eq!(split(10.5), (10.0, 2));
        assert_eq!(split(10.9), (11.0, 0));
    }
}
