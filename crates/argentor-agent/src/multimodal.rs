//! Multimodal message support — text + image inputs for vision-capable LLMs.
//!
//! This module provides a non-invasive, additive layer over the existing
//! [`argentor_core::Message`] type. Instead of modifying `Message::content` to
//! support multiple content blocks (which would cascade across the whole
//! codebase), we introduce a dedicated [`MultimodalMessage`] type plus a
//! [`VisionBackend`] trait for backends that accept image inputs.
//!
//! # Quick start
//!
//! ```ignore
//! use argentor_agent::multimodal::{MultimodalMessage, ImageInput};
//!
//! let msg = MultimodalMessage::new("What's in this image?")
//!     .with_image_url("https://example.com/cat.png");
//!
//! assert_eq!(msg.image_count(), 1);
//! ```
//!
//! # Providers
//!
//! See [`crate::vision_backends`] for Claude, OpenAI (GPT-4o) and Gemini
//! implementations of [`VisionBackend`].

use serde::{Deserialize, Serialize};
use std::path::Path;

/// A message that can contain text plus zero or more images.
///
/// This is the multimodal counterpart to [`argentor_core::Message`], used
/// exclusively with vision-capable backends. Regular chat flows continue to
/// use `Message` unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MultimodalMessage {
    /// The textual portion of the message.
    pub text: String,
    /// Zero or more images attached to the message.
    #[serde(default)]
    pub images: Vec<ImageInput>,
}

/// An image input — either a public URL the model can fetch, or base64-encoded
/// inline data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ImageInput {
    /// A public URL the LLM can fetch.
    Url(String),
    /// Base64-encoded image data with an explicit media type.
    Base64 {
        /// IANA media type (e.g. `"image/png"`, `"image/jpeg"`, `"image/webp"`, `"image/gif"`).
        media_type: String,
        /// Base64-encoded bytes (without any `data:` URL prefix).
        data: String,
    },
}

impl ImageInput {
    /// Returns `true` when this input is a [`ImageInput::Url`] variant.
    pub fn is_url(&self) -> bool {
        matches!(self, ImageInput::Url(_))
    }

    /// Returns `true` when this input is a [`ImageInput::Base64`] variant.
    pub fn is_base64(&self) -> bool {
        matches!(self, ImageInput::Base64 { .. })
    }

    /// Returns the media type for base64 inputs, or `None` for URLs.
    pub fn media_type(&self) -> Option<&str> {
        match self {
            ImageInput::Base64 { media_type, .. } => Some(media_type),
            ImageInput::Url(_) => None,
        }
    }
}

impl MultimodalMessage {
    /// Create a new multimodal message with the given text and no images.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            images: Vec::new(),
        }
    }

    /// Attach an image by public URL (builder-style).
    pub fn with_image_url(mut self, url: impl Into<String>) -> Self {
        self.images.push(ImageInput::Url(url.into()));
        self
    }

    /// Attach a base64-encoded image with an explicit media type (builder-style).
    pub fn with_image_base64(
        mut self,
        media_type: impl Into<String>,
        data: impl Into<String>,
    ) -> Self {
        self.images.push(ImageInput::Base64 {
            media_type: media_type.into(),
            data: data.into(),
        });
        self
    }

    /// Read an image from a local file, base64-encode it, and attach it.
    ///
    /// The media type is inferred from the file extension. Unrecognized
    /// extensions default to `"image/png"`.
    pub fn with_image_file(mut self, path: impl AsRef<Path>) -> Result<Self, std::io::Error> {
        let path = path.as_ref();
        let bytes = std::fs::read(path)?;

        let media_type = match path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("png") => "image/png",
            Some("jpg") | Some("jpeg") => "image/jpeg",
            Some("webp") => "image/webp",
            Some("gif") => "image/gif",
            _ => "image/png",
        }
        .to_string();

        use base64::Engine;
        let data = base64::engine::general_purpose::STANDARD.encode(&bytes);

        self.images.push(ImageInput::Base64 { media_type, data });
        Ok(self)
    }

    /// Total number of attached images.
    pub fn image_count(&self) -> usize {
        self.images.len()
    }

    /// Returns `true` when the message has at least one image.
    pub fn has_images(&self) -> bool {
        !self.images.is_empty()
    }
}

/// Granularity of vision support a backend advertises.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VisionCapability {
    /// No vision support.
    None,
    /// Basic image understanding (captioning, simple Q&A).
    Limited,
    /// Full analysis: OCR, charts, diagrams, fine-grained reasoning.
    Full,
}

impl VisionCapability {
    /// Returns `true` for [`VisionCapability::Limited`] or [`VisionCapability::Full`].
    pub fn supports_vision(&self) -> bool {
        !matches!(self, VisionCapability::None)
    }
}

/// Trait for LLM backends that accept image inputs in addition to text.
///
/// Implementations wrap an underlying provider client (Claude, OpenAI, Gemini,
/// …) and translate a [`MultimodalMessage`] into the provider-specific wire
/// format.
#[async_trait::async_trait]
pub trait VisionBackend: Send + Sync {
    /// The granularity of vision this backend supports.
    fn vision_capability(&self) -> VisionCapability;

    /// Send a multimodal message (text + images) and return the model's text reply.
    async fn ask_with_image(
        &self,
        message: &MultimodalMessage,
    ) -> argentor_core::ArgentorResult<String>;

    /// Short provider identifier used for routing (e.g. `"claude"`, `"openai"`, `"gemini"`).
    fn provider_name(&self) -> &str;
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- ImageInput ---

    #[test]
    fn image_input_url_is_url() {
        let i = ImageInput::Url("https://x".into());
        assert!(i.is_url());
        assert!(!i.is_base64());
    }

    #[test]
    fn image_input_base64_is_base64() {
        let i = ImageInput::Base64 {
            media_type: "image/png".into(),
            data: "AAAA".into(),
        };
        assert!(i.is_base64());
        assert!(!i.is_url());
    }

    #[test]
    fn image_input_media_type_base64() {
        let i = ImageInput::Base64 {
            media_type: "image/jpeg".into(),
            data: "x".into(),
        };
        assert_eq!(i.media_type(), Some("image/jpeg"));
    }

    #[test]
    fn image_input_media_type_url_none() {
        let i = ImageInput::Url("https://x".into());
        assert_eq!(i.media_type(), None);
    }

    #[test]
    fn image_input_serde_url_roundtrip() {
        let i = ImageInput::Url("https://example.com/a.png".into());
        let s = serde_json::to_string(&i).unwrap();
        let back: ImageInput = serde_json::from_str(&s).unwrap();
        assert_eq!(i, back);
    }

    #[test]
    fn image_input_serde_base64_roundtrip() {
        let i = ImageInput::Base64 {
            media_type: "image/png".into(),
            data: "AQID".into(),
        };
        let s = serde_json::to_string(&i).unwrap();
        let back: ImageInput = serde_json::from_str(&s).unwrap();
        assert_eq!(i, back);
    }

    // --- MultimodalMessage construction ---

    #[test]
    fn new_has_no_images() {
        let m = MultimodalMessage::new("hello");
        assert_eq!(m.text, "hello");
        assert_eq!(m.image_count(), 0);
        assert!(!m.has_images());
    }

    #[test]
    fn new_accepts_string_and_str() {
        let m1 = MultimodalMessage::new("str");
        let m2 = MultimodalMessage::new(String::from("String"));
        assert_eq!(m1.text, "str");
        assert_eq!(m2.text, "String");
    }

    #[test]
    fn with_image_url_appends() {
        let m = MultimodalMessage::new("q").with_image_url("https://x");
        assert_eq!(m.image_count(), 1);
        assert!(m.images[0].is_url());
    }

    #[test]
    fn with_image_url_multiple() {
        let m = MultimodalMessage::new("q")
            .with_image_url("https://a")
            .with_image_url("https://b")
            .with_image_url("https://c");
        assert_eq!(m.image_count(), 3);
    }

    #[test]
    fn with_image_base64_appends() {
        let m = MultimodalMessage::new("q").with_image_base64("image/png", "AAAA");
        assert_eq!(m.image_count(), 1);
        assert!(m.images[0].is_base64());
        assert_eq!(m.images[0].media_type(), Some("image/png"));
    }

    #[test]
    fn with_image_base64_and_url_mixed() {
        let m = MultimodalMessage::new("q")
            .with_image_url("https://x")
            .with_image_base64("image/jpeg", "AAAA");
        assert_eq!(m.image_count(), 2);
        assert!(m.images[0].is_url());
        assert!(m.images[1].is_base64());
    }

    #[test]
    fn has_images_true_when_any() {
        let m = MultimodalMessage::new("q").with_image_url("https://x");
        assert!(m.has_images());
    }

    #[test]
    fn has_images_false_when_empty() {
        assert!(!MultimodalMessage::new("q").has_images());
    }

    // --- with_image_file ---

    /// Minimal valid 1x1 transparent PNG (67 bytes).
    const TINY_PNG: [u8; 67] = [
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    fn write_tmp(ext: &str, bytes: &[u8]) -> tempfile::TempPath {
        use std::io::Write;
        let f = tempfile::Builder::new()
            .suffix(&format!(".{ext}"))
            .tempfile()
            .unwrap();
        f.as_file().write_all(bytes).unwrap();
        f.into_temp_path()
    }

    #[test]
    fn with_image_file_png() {
        let path = write_tmp("png", &TINY_PNG);
        let m = MultimodalMessage::new("q").with_image_file(&path).unwrap();
        assert_eq!(m.image_count(), 1);
        assert_eq!(m.images[0].media_type(), Some("image/png"));
    }

    #[test]
    fn with_image_file_jpg_extension() {
        let path = write_tmp("jpg", &[0xFF, 0xD8, 0xFF]);
        let m = MultimodalMessage::new("q").with_image_file(&path).unwrap();
        assert_eq!(m.images[0].media_type(), Some("image/jpeg"));
    }

    #[test]
    fn with_image_file_jpeg_extension() {
        let path = write_tmp("jpeg", &[0xFF, 0xD8, 0xFF]);
        let m = MultimodalMessage::new("q").with_image_file(&path).unwrap();
        assert_eq!(m.images[0].media_type(), Some("image/jpeg"));
    }

    #[test]
    fn with_image_file_webp_extension() {
        let path = write_tmp("webp", &[0x52, 0x49, 0x46, 0x46]);
        let m = MultimodalMessage::new("q").with_image_file(&path).unwrap();
        assert_eq!(m.images[0].media_type(), Some("image/webp"));
    }

    #[test]
    fn with_image_file_gif_extension() {
        let path = write_tmp("gif", &[0x47, 0x49, 0x46]);
        let m = MultimodalMessage::new("q").with_image_file(&path).unwrap();
        assert_eq!(m.images[0].media_type(), Some("image/gif"));
    }

    #[test]
    fn with_image_file_unknown_extension_defaults_png() {
        let path = write_tmp("xyz", &[0xAA, 0xBB]);
        let m = MultimodalMessage::new("q").with_image_file(&path).unwrap();
        assert_eq!(m.images[0].media_type(), Some("image/png"));
    }

    #[test]
    fn with_image_file_encodes_base64() {
        let path = write_tmp("png", &TINY_PNG);
        let m = MultimodalMessage::new("q").with_image_file(&path).unwrap();
        match &m.images[0] {
            ImageInput::Base64 { data, .. } => {
                use base64::Engine;
                let decoded = base64::engine::general_purpose::STANDARD
                    .decode(data)
                    .unwrap();
                assert_eq!(decoded, TINY_PNG);
            }
            _ => panic!("expected base64 variant"),
        }
    }

    #[test]
    fn with_image_file_missing_is_error() {
        let res = MultimodalMessage::new("q").with_image_file("/no/such/file.png");
        assert!(res.is_err());
    }

    // --- image_count ---

    #[test]
    fn image_count_zero() {
        assert_eq!(MultimodalMessage::new("q").image_count(), 0);
    }

    #[test]
    fn image_count_matches_added() {
        let m = MultimodalMessage::new("q")
            .with_image_url("https://a")
            .with_image_base64("image/png", "A")
            .with_image_url("https://c");
        assert_eq!(m.image_count(), 3);
    }

    // --- Serialization ---

    #[test]
    fn multimodal_message_serde_roundtrip() {
        let m = MultimodalMessage::new("describe")
            .with_image_url("https://x.png")
            .with_image_base64("image/png", "AAA=");
        let s = serde_json::to_string(&m).unwrap();
        let back: MultimodalMessage = serde_json::from_str(&s).unwrap();
        assert_eq!(m, back);
    }

    #[test]
    fn multimodal_message_serde_empty_images() {
        let m = MultimodalMessage::new("only text");
        let s = serde_json::to_string(&m).unwrap();
        let back: MultimodalMessage = serde_json::from_str(&s).unwrap();
        assert_eq!(m, back);
        assert_eq!(back.image_count(), 0);
    }

    // --- VisionCapability ---

    #[test]
    fn capability_none_does_not_support() {
        assert!(!VisionCapability::None.supports_vision());
    }

    #[test]
    fn capability_limited_supports() {
        assert!(VisionCapability::Limited.supports_vision());
    }

    #[test]
    fn capability_full_supports() {
        assert!(VisionCapability::Full.supports_vision());
    }

    #[test]
    fn capability_serde_roundtrip() {
        for cap in [
            VisionCapability::None,
            VisionCapability::Limited,
            VisionCapability::Full,
        ] {
            let s = serde_json::to_string(&cap).unwrap();
            let back: VisionCapability = serde_json::from_str(&s).unwrap();
            assert_eq!(cap, back);
        }
    }

    #[test]
    fn capability_equality() {
        assert_eq!(VisionCapability::Full, VisionCapability::Full);
        assert_ne!(VisionCapability::Full, VisionCapability::None);
    }
}
