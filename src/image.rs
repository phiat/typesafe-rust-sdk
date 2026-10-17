//! Images to ask about.

use std::fmt;
use std::path::Path;
use std::sync::Arc;

use serde::{Serialize, Serializer};
use serde_json::Value;

use crate::error::Error;

/// What every image reference starts with, so a reference that was never
/// attached can be told from ordinary text.
pub(crate) const REFERENCE_PREFIX: &str = "typesafe-image-";
const REFERENCE_DIGITS: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ImageFormat {
    Png,
    Jpeg,
}

impl ImageFormat {
    pub(crate) fn content_type(self) -> &'static str {
        match self {
            ImageFormat::Png => "image/png",
            ImageFormat::Jpeg => "image/jpeg",
        }
    }

    pub(crate) fn extension(self) -> &'static str {
        match self {
            ImageFormat::Png => "png",
            ImageFormat::Jpeg => "jpg",
        }
    }

    fn detect(bytes: &[u8]) -> Option<ImageFormat> {
        if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            Some(ImageFormat::Png)
        } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
            Some(ImageFormat::Jpeg)
        } else {
            None
        }
    }
}

/// A PNG or JPEG to send with a call.
///
/// An image goes into a request by its *reference*, a string that stands for it.
/// Write the reference where the image belongs, in the state or in a question's
/// instructions or descriptions, and attach the image to the call with
/// [`CallOptions::image`](crate::CallOptions::image). An `Image` serializes as
/// its reference and displays as it, so it can sit inside a `json!` value or a
/// `format!` string.
///
/// Every use of a reference sends another copy of the image to the model, and
/// images are billed as input tokens.
///
/// Images work with `jev-1.13.0` and `jev-1.14.0`. The API rejects what it can't
/// use: a side shorter than 280 or longer than 1568 pixels, a file over 16 MiB,
/// or anything other than PNG and JPEG. It doesn't resize. Rotate the pixels
/// upright first, because EXIF orientation is not applied.
///
/// ```no_run
/// use serde_json::json;
/// use typesafe::{BoxQuestion, CallOptions, Image, Noul, Questions};
///
/// # fn run(client: typesafe::blocking::Client) -> Result<(), typesafe::Error> {
/// let photo = Image::from_path("kitchen.jpg")?;
/// let questions = Questions::new()
///     .ask("mug_present", Noul::new("Is there a coffee mug in `photo`?"))
///     .ask("mug", BoxQuestion::new("The coffee mug in `photo`"));
///
/// let response = client.system_one_with(
///     &json!({ "photo": photo }),
///     &questions,
///     &CallOptions::new().model("jev-1.14.0").image(&photo),
/// )?;
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
pub struct Image {
    reference: String,
    format: ImageFormat,
    bytes: Arc<[u8]>,
}

impl Image {
    /// An image from the bytes of a PNG or JPEG file.
    ///
    /// Fails with [`ErrorKind::InvalidRequest`](crate::ErrorKind::InvalidRequest)
    /// for anything else, such as GIF, WebP or HEIC, which the API doesn't take.
    pub fn from_bytes(bytes: impl Into<Vec<u8>>) -> Result<Image, Error> {
        let bytes = bytes.into();
        let format = ImageFormat::detect(&bytes).ok_or_else(|| {
            Error::invalid_request(
                "an image must be a PNG or JPEG file; convert GIF, WebP and HEIC first",
            )
        })?;
        Ok(Image {
            reference: new_reference(),
            format,
            bytes: bytes.into(),
        })
    }

    /// An image read from a PNG or JPEG file.
    pub fn from_path(path: impl AsRef<Path>) -> Result<Image, Error> {
        let path = path.as_ref();
        let bytes = std::fs::read(path).map_err(|e| {
            Error::invalid_request(format!("could not read image {}: {e}", path.display()))
        })?;
        Image::from_bytes(bytes).map_err(|e| {
            Error::invalid_request(format!("image {}: {}", path.display(), e.message()))
        })
    }

    /// The string that stands for this image in a request.
    pub fn reference(&self) -> &str {
        &self.reference
    }

    pub(crate) fn format(&self) -> ImageFormat {
        self.format
    }

    pub(crate) fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

fn new_reference() -> String {
    format!(
        "{REFERENCE_PREFIX}{:0width$x}",
        fastrand::u128(..),
        width = REFERENCE_DIGITS
    )
}

/// The first image reference in `text` that is not one of `attached`.
pub(crate) fn find_unattached<'a>(text: &'a str, attached: &[&Image]) -> Option<&'a str> {
    text.match_indices(REFERENCE_PREFIX).find_map(|(at, _)| {
        let candidate = text.get(at..at + REFERENCE_PREFIX.len() + REFERENCE_DIGITS)?;
        let digits = &candidate[REFERENCE_PREFIX.len()..];
        let is_reference = digits.bytes().all(|b| b.is_ascii_hexdigit());
        (is_reference && !attached.iter().any(|image| image.reference == candidate))
            .then_some(candidate)
    })
}

impl fmt::Display for Image {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.reference)
    }
}

impl fmt::Debug for Image {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Image")
            .field("reference", &self.reference)
            .field("format", &self.format)
            .field("bytes", &self.bytes.len())
            .finish()
    }
}

impl Serialize for Image {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.reference)
    }
}

impl From<&Image> for Value {
    fn from(image: &Image) -> Value {
        Value::String(image.reference.clone())
    }
}

impl From<Image> for Value {
    fn from(image: Image) -> Value {
        Value::String(image.reference)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\nrest";
    const JPEG: &[u8] = &[0xFF, 0xD8, 0xFF, 0xE0, 0x00];

    #[test]
    fn detects_png_and_jpeg() {
        assert_eq!(Image::from_bytes(PNG).unwrap().format(), ImageFormat::Png);
        assert_eq!(Image::from_bytes(JPEG).unwrap().format(), ImageFormat::Jpeg);
        for not_an_image in [&b"GIF89a"[..], b"RIFF....WEBP", b"", b"hello"] {
            let error = Image::from_bytes(not_an_image).unwrap_err();
            assert_eq!(error.kind(), crate::ErrorKind::InvalidRequest);
            assert!(error.message().contains("PNG or JPEG"));
        }
    }

    #[test]
    fn references_are_unique_and_recognisable() {
        let (a, b) = (
            Image::from_bytes(PNG).unwrap(),
            Image::from_bytes(PNG).unwrap(),
        );
        assert_ne!(a.reference(), b.reference());
        assert!(a.reference().starts_with(REFERENCE_PREFIX));
        assert_eq!(
            a.reference().len(),
            REFERENCE_PREFIX.len() + REFERENCE_DIGITS
        );
        assert_eq!(a.to_string(), a.reference());
        assert_eq!(serde_json::to_value(&a).unwrap(), Value::from(&a));
        assert_eq!(Value::from(a.clone()), serde_json::json!(a.reference()));
    }

    #[test]
    fn finds_references_that_were_not_attached() {
        let (a, b) = (
            Image::from_bytes(PNG).unwrap(),
            Image::from_bytes(PNG).unwrap(),
        );
        let text = format!("look at {a} and then at {b}.");
        assert_eq!(find_unattached(&text, &[&a, &b]), None);
        assert_eq!(find_unattached(&text, &[&a]), Some(b.reference()));
        assert_eq!(
            find_unattached("typesafe-image-notahexreference", &[]),
            None
        );
        assert_eq!(find_unattached("typesafe-image-abc", &[]), None);
    }

    #[test]
    fn debug_leaves_the_bytes_out() {
        let shown = format!("{:?}", Image::from_bytes(PNG).unwrap());
        assert!(shown.contains("bytes: 12"), "{shown}");
    }

    #[test]
    fn reads_a_file_and_names_it_in_errors() {
        let missing = Image::from_path("/no/such/photo.png").unwrap_err();
        assert!(missing.message().contains("/no/such/photo.png"));
        let text =
            Image::from_path(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml")).unwrap_err();
        assert!(text.message().contains("Cargo.toml") && text.message().contains("PNG or JPEG"));
    }
}
