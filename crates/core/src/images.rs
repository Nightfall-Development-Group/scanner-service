//! Downloading and decoding room images.
//!
//! Deliberately produces raw pixels, not a GPU texture: this crate has no GUI
//! dependency (see the module docs on `lib.rs`), and a texture handle only
//! means something in the context of an `egui::Context`. The app crate turns a
//! [`DecodedImage`] into a texture; this module's job ends at decoded bytes.

use std::time::Duration;

/// Generous relative to db-api's own 10s timeout: images are larger than JSON
/// and the CDN is a different, unrelated host.
const IMAGE_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, PartialEq)]
pub struct DecodedImage {
    pub width: u32,
    pub height: u32,
    /// RGBA8, straight (non-premultiplied) alpha, row-major, top-to-bottom —
    /// exactly what `egui::ColorImage::from_rgba_unmultiplied` expects.
    pub rgba: Vec<u8>,
}

#[derive(Debug, thiserror::Error)]
pub enum ImageError {
    #[error("could not download image: {0}")]
    Transport(String),
    #[error("could not decode image: {0}")]
    Decode(String),
}

/// Download `url` and decode it. No auth header: room images are served from a
/// public CDN unrelated to db-api's bearer-key requirement.
pub async fn fetch_and_decode(url: &str) -> Result<DecodedImage, ImageError> {
    crate::tls::ensure_provider();

    let client = reqwest::Client::builder()
        .timeout(IMAGE_TIMEOUT)
        .user_agent(concat!("nfd-scanner/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| ImageError::Transport(e.to_string()))?;

    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| ImageError::Transport(e.to_string()))?;

    if !response.status().is_success() {
        return Err(ImageError::Transport(format!("HTTP {}", response.status())));
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|e| ImageError::Transport(e.to_string()))?;

    decode(&bytes)
}

/// Decode already-downloaded bytes. Split out from [`fetch_and_decode`] so
/// decoding is testable without a network round trip.
pub fn decode(bytes: &[u8]) -> Result<DecodedImage, ImageError> {
    let img = image::load_from_memory(bytes).map_err(|e| ImageError::Decode(e.to_string()))?;
    let (width, height) = (img.width(), img.height());
    let rgba = img.to_rgba8().into_raw();
    Ok(DecodedImage {
        width,
        height,
        rgba,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, ImageFormat, RgbaImage};

    /// Round-trips a small synthetic image through PNG rather than depending on
    /// a committed fixture file or network access — the point is exercising our
    /// thin wrapper, not re-testing the `image` crate's own codecs.
    fn png_bytes(width: u32, height: u32) -> (Vec<u8>, RgbaImage) {
        let img = RgbaImage::from_fn(width, height, |x, y| {
            image::Rgba([x as u8 * 10, y as u8 * 10, 5, 255])
        });
        let mut buf = Vec::new();
        DynamicImage::ImageRgba8(img.clone())
            .write_to(&mut std::io::Cursor::new(&mut buf), ImageFormat::Png)
            .expect("encoding a synthetic PNG cannot fail");
        (buf, img)
    }

    #[test]
    fn decodes_dimensions_and_pixels_correctly() {
        let (bytes, original) = png_bytes(4, 3);
        let decoded = decode(&bytes).expect("valid PNG decodes");

        assert_eq!(decoded.width, 4);
        assert_eq!(decoded.height, 3);
        assert_eq!(decoded.rgba, original.into_raw());
    }

    #[test]
    fn rejects_garbage_bytes() {
        let err = decode(b"this is not an image").unwrap_err();
        assert!(matches!(err, ImageError::Decode(_)));
    }

    #[test]
    fn rejects_an_empty_buffer() {
        assert!(decode(&[]).is_err());
    }

    #[test]
    fn a_single_pixel_round_trips() {
        // The degenerate case a fencepost error would miss.
        let (bytes, original) = png_bytes(1, 1);
        let decoded = decode(&bytes).unwrap();
        assert_eq!((decoded.width, decoded.height), (1, 1));
        assert_eq!(decoded.rgba, original.into_raw());
    }
}
