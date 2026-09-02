//! Bitmaps in the document.
//!
//! # A bitmap is a fill, not a kind of object
//!
//! Animate has two states for an imported image. Placed, it is a bitmap
//! instance; **broken apart** (Ctrl+B), it becomes a *shape filled with the
//! bitmap* — and from that moment every vector tool works on it. The Lasso
//! cuts it, the Magic Wand selects a region of it, the Eraser takes bites out
//! of it, and what is left is ordinary artwork that can be made into a symbol.
//!
//! That second state is the useful one, and it is the whole reason images are
//! modelled here as a [`crate::Paint`] beside a colour and a gradient rather
//! than as an `ObjectKind` of their own. A placed bitmap is a rectangle whose
//! fill happens to be an image; breaking it apart is then not a conversion at
//! all, and removing part of it is the boolean subtraction that CP-1.1b
//! already built. An `ObjectKind::Bitmap` would have needed its own hit test,
//! its own bounds, its own clipping, its own everything — and would still have
//! had to become a shape the moment anybody cut it.
//!
//! # What is stored
//!
//! The **file as imported**, byte for byte, and the decoded pixels beside it.
//! The same reasoning as [`crate::SoundAsset`]: keeping only the decode would
//! bake one decoder's idea of a JPEG into the document for ever and inflate a
//! 2 MB photograph into 32 MB in every saved file. Keeping only the source
//! would mean decoding on every frame. So both, shared behind an `Arc`, and
//! the container stores the source.

use std::sync::Arc;

use buzz_geom::{Affine, Point, Rect};
use serde::{Deserialize, Serialize};

/// Stable identity for an imported bitmap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ImageId(pub u64);

/// An imported bitmap: the file as it arrived, and its pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct ImageAsset {
    pub id: ImageId,
    /// Name in the Library.
    pub name: String,
    /// The file, byte for byte, for saving it back out unchanged.
    pub source: Arc<Vec<u8>>,
    /// Lower-case extension — what the file is.
    pub format: String,
    pub width: u32,
    pub height: u32,
    /// Decoded pixels, **straight-alpha RGBA8**, `width * height * 4` long.
    ///
    /// Straight rather than premultiplied because that is what the Magic Wand
    /// has to compare: a pixel's colour must not change with its own opacity,
    /// or a half-transparent red and an opaque dark red become the same
    /// number and the wand selects across an edge that is plainly there.
    pub pixels: Arc<Vec<u8>>,
    /// What the renderer's cache calls **these exact pixels**.
    ///
    /// # Why this exists
    ///
    /// The GPU keeps its own copy of every bitmap, and decides whether that
    /// copy is still good by comparing an identifier — not the pixels, which is
    /// the whole point of having a cache. Vello's identifier comes from
    /// `peniko::Blob`, and `Blob::new` takes a fresh number from a global
    /// counter on every call, so building the image brush once per frame made
    /// every frame a cache miss: a four-megapixel photograph re-uploaded sixty
    /// times a second for as long as it was on screen.
    ///
    /// # Why it is *issued* rather than worked out
    ///
    /// The obvious identity is `id` and a change counter. It is wrong, and the
    /// way it is wrong is nasty: two `ImageAsset`s can carry the same
    /// [`ImageId`] and hold **different pixels** — a document opened twice, an
    /// import run again, a symbol duplicated, or simply two tests building the
    /// same fixture. Vello's atlas keeps whichever arrived first under that
    /// identity and quietly serves it to everyone after, so the second picture
    /// draws as the first, or as nothing at all.
    ///
    /// So identity is taken from a counter, never derived from anything a
    /// caller can duplicate, and it is not reused. Copying an asset copies its
    /// identity — that is right, because the pixels are the same pixels — and
    /// [`ImageAsset::edit_pixels`] takes a new one, because they are not.
    identity: u64,
}

/// Why a bitmap could not be read.
#[derive(Debug, thiserror::Error)]
pub enum ImageError {
    #[error("this file could not be read as an image: {0}")]
    Decode(String),
    #[error(
        "that image is {width} x {height}, which is larger than BuzzAnimate will          import ({limit} x {limit}). Scale it down first."
    )]
    TooLarge { width: u32, height: u32, limit: u32 },
}

/// The largest bitmap that may be imported, per side.
///
/// A bound, not a judgement about what is useful. Decoding is `width x height
/// x 4` bytes and the result is held for the life of the document, so an
/// accidental drag of a 40 000-pixel scan would take sixteen gigabytes and the
/// process would die with no explanation. Sixteen thousand covers any
/// reasonable scan or photograph at four times 4K.
pub const MAX_IMAGE_SIDE: u32 = 16_384;

/// The next never-yet-used bitmap identity.
///
/// Monotonic and never reused. At one identity per painted stroke it would take
/// longer than the universe has existed to wrap.
fn next_identity() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

impl ImageAsset {
    /// Decode a file into an asset, keeping the original bytes.
    ///
    /// The format is taken from the *contents* rather than the extension: a
    /// PNG saved as `.jpg` is a thing that happens, and refusing it on the
    /// strength of its name would be wrong when the bytes say plainly what it
    /// is.
    pub fn decode(id: ImageId, name: impl Into<String>, bytes: &[u8]) -> Result<Self, ImageError> {
        let format = ::image::guess_format(bytes).map_err(|e| ImageError::Decode(e.to_string()))?;
        let decoded = ::image::load_from_memory_with_format(bytes, format)
            .map_err(|e| ImageError::Decode(e.to_string()))?;

        let (width, height) = (decoded.width(), decoded.height());
        if width > MAX_IMAGE_SIDE || height > MAX_IMAGE_SIDE {
            return Err(ImageError::TooLarge {
                width,
                height,
                limit: MAX_IMAGE_SIDE,
            });
        }

        // **Straight alpha, RGBA8.** `to_rgba8` gives exactly that, which is
        // what PNG stores, what the renderer wants and what the Magic Wand has
        // to compare — see the note on `pixels`.
        let rgba = decoded.to_rgba8();
        Ok(Self {
            id,
            name: name.into(),
            source: Arc::new(bytes.to_vec()),
            format: match format {
                ::image::ImageFormat::Png => "png",
                ::image::ImageFormat::Jpeg => "jpg",
                ::image::ImageFormat::Gif => "gif",
                ::image::ImageFormat::Bmp => "bmp",
                ::image::ImageFormat::WebP => "webp",
                _ => "png",
            }
            .to_string(),
            width,
            height,
            pixels: Arc::new(rgba.into_raw()),
            identity: next_identity(),
        })
    }

    /// A blank, fully transparent bitmap — a canvas to paint into.
    pub fn blank(id: ImageId, name: impl Into<String>, width: u32, height: u32) -> Self {
        let width = width.clamp(1, MAX_IMAGE_SIDE);
        let height = height.clamp(1, MAX_IMAGE_SIDE);
        Self {
            id,
            name: name.into(),
            source: Arc::new(Vec::new()),
            format: "png".to_string(),
            width,
            height,
            pixels: Arc::new(vec![0; (width as usize) * (height as usize) * 4]),
            identity: next_identity(),
        }
    }

    /// Encode the current pixels as a PNG.
    ///
    /// A painted canvas has no source file to write back — its pixels *are*
    /// the document — so saving one means encoding what is there now.
    pub fn encode_png(&self) -> Result<Vec<u8>, ImageError> {
        let buffer =
            ::image::RgbaImage::from_raw(self.width, self.height, self.pixels.as_ref().clone())
                .ok_or_else(|| ImageError::Decode("the pixel buffer is the wrong size".into()))?;
        let mut out = std::io::Cursor::new(Vec::new());
        buffer
            .write_to(&mut out, ::image::ImageFormat::Png)
            .map_err(|e| ImageError::Decode(e.to_string()))?;
        Ok(out.into_inner())
    }

    /// The bytes to store in the container.
    ///
    /// The imported file where there is one, so a photograph round-trips
    /// byte for byte; a fresh PNG encode for a canvas that was painted here
    /// and never came from a file.
    pub fn bytes_for_storage(&self) -> Result<Vec<u8>, ImageError> {
        if self.source.is_empty() {
            self.encode_png()
        } else {
            Ok(self.source.as_ref().clone())
        }
    }

    /// File name inside the `.buzz` container's `media/` directory.
    pub fn file_name(&self) -> String {
        format!("image-{}.{}", self.id.0, self.format)
    }

    pub fn size(&self) -> buzz_geom::Size {
        buzz_geom::Size::new(f64::from(self.width), f64::from(self.height))
    }

    /// Build an asset from pixels already in hand.
    ///
    /// **The only way to make one**, because it is the only way to be sure the
    /// identity is fresh — see the note on `identity`. A struct literal could
    /// give two different pictures the same one, and the renderer would then
    /// draw whichever it saw first for both.
    pub fn from_pixels(
        id: ImageId,
        name: impl Into<String>,
        width: u32,
        height: u32,
        pixels: Arc<Vec<u8>>,
    ) -> Self {
        Self {
            id,
            name: name.into(),
            source: Arc::new(Vec::new()),
            format: "png".to_string(),
            width,
            height,
            pixels,
            identity: next_identity(),
        }
    }

    /// The file this was decoded from, kept for saving it back unchanged.
    pub fn with_source(mut self, source: Arc<Vec<u8>>, format: &str) -> Self {
        self.source = source;
        self.format = format.to_ascii_lowercase();
        self
    }

    /// What the renderer's cache calls these pixels.
    pub fn blob_id(&self) -> u64 {
        self.identity
    }

    /// Edit the pixels, taking a new identity so the GPU reloads them.
    ///
    /// The only supported way to alter a decoded picture: writing to `pixels`
    /// directly leaves the renderer showing what was there before.
    pub fn edit_pixels(&mut self, f: impl FnOnce(&mut Vec<u8>)) {
        f(Arc::make_mut(&mut self.pixels));
        self.identity = next_identity();
    }

    /// The pixel at `(x, y)` as straight-alpha RGBA.
    ///
    /// Out of bounds reads as fully transparent rather than panicking or
    /// clamping: the Magic Wand walks off the edge by design, and an edge that
    /// repeated its border pixel would let a flood fill escape round the
    /// outside of the image.
    pub fn pixel(&self, x: i64, y: i64) -> [u8; 4] {
        if x < 0 || y < 0 || x >= i64::from(self.width) || y >= i64::from(self.height) {
            return [0, 0, 0, 0];
        }
        let i = ((y as usize) * self.width as usize + x as usize) * 4;
        match self.pixels.get(i..i + 4) {
            Some(p) => [p[0], p[1], p[2], p[3]],
            None => [0, 0, 0, 0],
        }
    }
}

/// A bitmap used as a fill, and where it sits.
///
/// The transform maps the **unit square** — `0..1` in both axes — onto the
/// object's own space, so the image is independent of its own pixel dimensions
/// and a shape can be cut, scaled or warped without the picture sliding about
/// inside it. The same reasoning as [`crate::Gradient`], and for the same
/// reason: the transform is what the file formats store and what a transform
/// tool edits.
#[derive(Debug, Clone, PartialEq)]
pub struct ImageFill {
    pub asset: Arc<ImageAsset>,
    /// Unit square to the object's own space.
    pub transform: Affine,
    /// Draw the image smoothed. Off is what pixel art wants.
    pub smooth: bool,
    /// Repeat the image across the shape instead of stretching one copy over it.
    /// What a seamless texture (paper, checker, a procedural tile) wants; off for
    /// an ordinary imported photo, which fills its shape once.
    pub tile: bool,
}

impl ImageFill {
    /// An image laid across `rect` at its natural proportions, filling it once.
    pub fn new(asset: Arc<ImageAsset>, rect: Rect) -> Self {
        Self {
            asset,
            transform: Affine::translate(rect.origin().to_vec2())
                * Affine::scale_non_uniform(
                    rect.width().max(f64::MIN_POSITIVE),
                    rect.height().max(f64::MIN_POSITIVE),
                ),
            smooth: true,
            tile: false,
        }
    }

    /// A seamless image tiled across the object, one copy spanning `cell` (a
    /// square, in object units). What procedural and bundled textures use.
    pub fn tiled(asset: Arc<ImageAsset>, cell: f64) -> Self {
        let cell = cell.max(f64::MIN_POSITIVE);
        Self {
            asset,
            transform: Affine::scale(cell),
            smooth: true,
            tile: true,
        }
    }

    /// The rectangle this image covers, at its own pixel size, placed at the
    /// origin — what a freshly imported bitmap should fill.
    pub fn natural_rect(asset: &ImageAsset) -> Rect {
        Rect::new(0.0, 0.0, f64::from(asset.width), f64::from(asset.height))
    }

    /// One colour standing in for the whole image.
    ///
    /// Every place that can only work in one colour asks for this: the
    /// lighting model, outline view, the colour wells. It is the mean of the
    /// pixels that are not transparent — averaging in the transparent ones
    /// would drag a cut-out towards black however bright the subject is.
    pub fn average_color(&self) -> peniko::Color {
        let (mut r, mut g, mut b, mut a, mut n) = (0u64, 0u64, 0u64, 0u64, 0u64);
        // Sampled rather than summed: a 4K photograph is thirty-three million
        // pixels and this is asked for while drawing. A stride that is not a
        // divisor of the width also avoids marching down one column of a
        // regular image and reporting the colour of a single stripe.
        for chunk in self.asset.pixels.chunks_exact(4).step_by(97) {
            if chunk[3] == 0 {
                continue;
            }
            r += u64::from(chunk[0]);
            g += u64::from(chunk[1]);
            b += u64::from(chunk[2]);
            a += u64::from(chunk[3]);
            n += 1;
        }
        if n == 0 {
            return peniko::Color::TRANSPARENT;
        }
        peniko::Color::from_rgba8((r / n) as u8, (g / n) as u8, (b / n) as u8, (a / n) as u8)
    }

    /// Carry the image through a transform, so it stays on its artwork.
    pub fn transformed(&self, t: Affine) -> Self {
        Self {
            asset: Arc::clone(&self.asset),
            transform: t * self.transform,
            smooth: self.smooth,
            tile: self.tile,
        }
    }

    /// Where a point in the object's own space lands in **pixel** coordinates.
    ///
    /// This is what the Magic Wand needs: a click on the stage, carried back
    /// through every transform, becomes a pixel to start a flood fill from.
    /// `None` when the transform has collapsed and there is no way back.
    pub fn to_pixel(&self, local: Point) -> Option<Point> {
        let c = self.transform.as_coeffs();
        if (c[0] * c[3] - c[1] * c[2]).abs() <= f64::MIN_POSITIVE {
            return None;
        }
        let unit = self.transform.inverse() * local;
        Some(Point::new(
            unit.x * f64::from(self.asset.width),
            unit.y * f64::from(self.asset.height),
        ))
    }

    /// The reverse: a pixel back into the object's own space.
    pub fn from_pixel(&self, pixel: Point) -> Point {
        self.transform
            * Point::new(
                pixel.x / f64::from(self.asset.width).max(1.0),
                pixel.y / f64::from(self.asset.height).max(1.0),
            )
    }
}

/// Every bitmap a document holds.
/// The map is behind an `Arc` so cloning the library is a pointer copy, not one
/// tree node per bitmap; see [`crate::Library`] for the rationale. Mutation
/// forks the map once via [`Arc::make_mut`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ImageLibrary {
    images: Arc<std::collections::BTreeMap<ImageId, Arc<ImageAsset>>>,
}

impl ImageLibrary {
    pub fn len(&self) -> usize {
        self.images.len()
    }

    pub fn is_empty(&self) -> bool {
        self.images.is_empty()
    }

    pub fn get(&self, id: ImageId) -> Option<&Arc<ImageAsset>> {
        self.images.get(&id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Arc<ImageAsset>> {
        self.images.values()
    }

    pub fn insert(&mut self, asset: ImageAsset) -> Arc<ImageAsset> {
        let asset = Arc::new(asset);
        Arc::make_mut(&mut self.images).insert(asset.id, Arc::clone(&asset));
        asset
    }

    pub fn remove(&mut self, id: ImageId) -> Option<Arc<ImageAsset>> {
        Arc::make_mut(&mut self.images).remove(&id)
    }

    /// A name no existing bitmap has, so the Library stays readable.
    pub fn unique_name(&self, wanted: &str) -> String {
        if !self.images.values().any(|i| i.name == wanted) {
            return wanted.to_string();
        }
        for n in 2..10_000 {
            let candidate = format!("{wanted} {n}");
            if !self.images.values().any(|i| i.name == candidate) {
                return candidate;
            }
        }
        wanted.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(width: u32, height: u32, fill: [u8; 4]) -> Arc<ImageAsset> {
        Arc::new(ImageAsset::from_pixels(
            ImageId(1),
            "Test",
            width,
            height,
            Arc::new(
                std::iter::repeat_n(fill, (width * height) as usize)
                    .flatten()
                    .collect(),
            ),
        ))
    }

    #[test]
    fn a_pixel_outside_the_image_reads_as_transparent() {
        let a = asset(4, 4, [10, 20, 30, 255]);
        assert_eq!(a.pixel(0, 0), [10, 20, 30, 255]);
        assert_eq!(a.pixel(-1, 0), [0, 0, 0, 0]);
        assert_eq!(a.pixel(0, -1), [0, 0, 0, 0]);
        assert_eq!(a.pixel(4, 0), [0, 0, 0, 0]);
        assert_eq!(a.pixel(0, 4), [0, 0, 0, 0]);
    }

    /// A click on the artwork has to become a pixel, and the same pixel back
    /// again. This is the whole basis of the Magic Wand.
    #[test]
    fn a_point_round_trips_between_artwork_and_pixels() {
        let a = asset(200, 100, [0, 0, 0, 255]);
        let fill = ImageFill::new(Arc::clone(&a), Rect::new(50.0, 20.0, 250.0, 120.0));

        // The top-left of the artwork is pixel (0, 0).
        let p = fill.to_pixel(Point::new(50.0, 20.0)).expect("a pixel");
        assert!(p.x.abs() < 1e-9 && p.y.abs() < 1e-9, "got {p:?}");

        // The middle of the artwork is the middle pixel.
        let p = fill.to_pixel(Point::new(150.0, 70.0)).expect("a pixel");
        assert!(
            (p.x - 100.0).abs() < 1e-9 && (p.y - 50.0).abs() < 1e-9,
            "got {p:?}"
        );

        // And back again.
        let back = fill.from_pixel(p);
        assert!((back.x - 150.0).abs() < 1e-9 && (back.y - 70.0).abs() < 1e-9);
    }

    /// A collapsed transform has no inverse; asking for a pixel must say so
    /// rather than produce a NaN that then indexes the pixel buffer.
    #[test]
    fn a_collapsed_image_has_no_pixel_under_the_pointer() {
        let a = asset(10, 10, [0, 0, 0, 255]);
        let mut fill = ImageFill::new(a, Rect::new(0.0, 0.0, 10.0, 10.0));
        fill.transform = Affine::scale(0.0);
        assert!(fill.to_pixel(Point::new(1.0, 1.0)).is_none());
    }

    /// **Transparent pixels are left out of the average.** A cut-out is mostly
    /// transparent, and averaging the holes in would report every asset as
    /// nearly black however bright the subject is.
    #[test]
    fn the_average_colour_ignores_transparent_pixels() {
        let mut pixels = Vec::new();
        for i in 0..400 {
            // A quarter opaque white, the rest fully transparent.
            if i % 4 == 0 {
                pixels.extend_from_slice(&[255, 255, 255, 255]);
            } else {
                pixels.extend_from_slice(&[0, 0, 0, 0]);
            }
        }
        let a = Arc::new(ImageAsset::from_pixels(
            ImageId(2),
            "Cutout",
            20,
            20,
            Arc::new(pixels),
        ));
        let fill = ImageFill::new(a, Rect::new(0.0, 0.0, 20.0, 20.0));
        let average = fill.average_color().to_rgba8().to_u8_array();
        assert!(
            average[0] > 200,
            "a mostly-transparent white cut-out averaged to {average:?}"
        );
    }

    #[test]
    fn a_fully_transparent_image_averages_to_nothing_rather_than_black() {
        let a = asset(8, 8, [0, 0, 0, 0]);
        let fill = ImageFill::new(a, Rect::new(0.0, 0.0, 8.0, 8.0));
        assert_eq!(fill.average_color().components[3], 0.0);
    }

    #[test]
    fn library_names_do_not_collide() {
        let mut library = ImageLibrary::default();
        library.insert(ImageAsset::from_pixels(
            ImageId(1),
            "Tree",
            1,
            1,
            Arc::new(vec![0, 0, 0, 255]),
        ));
        assert_eq!(library.unique_name("Tree"), "Tree 2");
        assert_eq!(library.unique_name("Hut"), "Hut");
    }
}
