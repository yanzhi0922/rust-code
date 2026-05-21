//! Image format detection and processing utilities.
//!
//! Provides functions for detecting image formats from base64 data,
//! estimating image dimensions, and classifying image errors.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// ImageFormat
// ---------------------------------------------------------------------------

/// Supported image formats.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum ImageFormat {
    /// PNG format.
    Png,
    /// JPEG format.
    Jpeg,
    /// GIF format.
    Gif,
    /// WebP format.
    WebP,
}

impl ImageFormat {
    /// Return the file extension for this format.
    #[must_use]
    pub fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Gif => "gif",
            Self::WebP => "webp",
        }
    }

    /// Return the MIME type for this format.
    #[must_use]
    pub fn mime_type(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Gif => "image/gif",
            Self::WebP => "image/webp",
        }
    }

    /// Parse from string.
    pub fn from_str_opt(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "png" => Some(Self::Png),
            "jpg" | "jpeg" => Some(Self::Jpeg),
            "gif" => Some(Self::Gif),
            "webp" => Some(Self::WebP),
            _ => None,
        }
    }

    /// All known formats.
    #[must_use]
    pub fn all_values() -> &'static [ImageFormat] {
        &[
            ImageFormat::Png,
            ImageFormat::Jpeg,
            ImageFormat::Gif,
            ImageFormat::WebP,
        ]
    }
}

impl std::fmt::Display for ImageFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.extension())
    }
}

// ---------------------------------------------------------------------------
// ImageError
// ---------------------------------------------------------------------------

/// Errors that can occur during image processing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageError {
    /// The image data is empty.
    EmptyData,
    /// The base64 encoding is invalid.
    InvalidBase64(String),
    /// The image format is unsupported.
    UnsupportedFormat(String),
    /// The image exceeds size limits.
    SizeExceeded {
        /// Actual size in bytes.
        actual: u64,
        /// Maximum allowed size in bytes.
        max: u64,
    },
    /// The image dimensions exceed limits.
    DimensionExceeded {
        /// Actual dimensions.
        actual: (u32, u32),
        /// Maximum dimensions.
        max: (u32, u32),
    },
    /// Generic processing error.
    ProcessingError(String),
}

impl std::fmt::Display for ImageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyData => write!(f, "Image data is empty"),
            Self::InvalidBase64(msg) => write!(f, "Invalid base64: {msg}"),
            Self::UnsupportedFormat(fmt) => write!(f, "Unsupported format: {fmt}"),
            Self::SizeExceeded { actual, max } => {
                write!(f, "Size {actual} exceeds maximum {max}")
            }
            Self::DimensionExceeded { actual, max } => {
                write!(
                    f,
                    "Dimensions {}x{} exceeds maximum {}x{}",
                    actual.0, actual.1, max.0, max.1
                )
            }
            Self::ProcessingError(msg) => write!(f, "Processing error: {msg}"),
        }
    }
}

impl std::error::Error for ImageError {}

// ---------------------------------------------------------------------------
// Format detection
// ---------------------------------------------------------------------------

/// Detect the image format from base64-encoded data.
///
/// Uses magic bytes (file signatures) to determine the format.
///
/// # Arguments
///
/// * `base64_data` — The base64-encoded image data.
///
/// # Returns
///
/// The detected format, or `None` if unrecognized.
#[must_use]
pub fn detect_image_format(base64_data: &str) -> Option<ImageFormat> {
    if base64_data.is_empty() {
        return None;
    }

    // Decode the first few bytes to check magic numbers.
    let data = base64_data.trim_end_matches('=');
    if data.len() < 8 {
        return None;
    }

    // Decode first 12 base64 chars = 9 bytes.
    let prefix = &data[..data.len().min(16)];

    // Check for known magic bytes in base64-encoded form.
    // PNG: starts with \x89PNG -> base64: iVBOR
    if prefix.starts_with("iVBOR") {
        return Some(ImageFormat::Png);
    }

    // JPEG: starts with \xFF\xD8\xFF -> base64: /9j/
    if prefix.starts_with("/9j/") {
        return Some(ImageFormat::Jpeg);
    }

    // GIF: starts with "GIF87a" or "GIF89a" -> base64: R0lGOD
    if prefix.starts_with("R0lGOD") {
        return Some(ImageFormat::Gif);
    }

    // WebP: starts with "RIFF....WEBP" -> base64: UklGR
    if prefix.starts_with("UklGR") {
        return Some(ImageFormat::WebP);
    }

    None
}

/// Detect image format from a data URI.
///
/// # Arguments
///
/// * `data_uri` — The data URI string.
///
/// # Returns
///
/// The detected format, or `None`.
#[must_use]
pub fn detect_format_from_data_uri(data_uri: &str) -> Option<ImageFormat> {
    let lower = data_uri.to_ascii_lowercase();
    if lower.contains("image/png") {
        Some(ImageFormat::Png)
    } else if lower.contains("image/jpeg") || lower.contains("image/jpg") {
        Some(ImageFormat::Jpeg)
    } else if lower.contains("image/gif") {
        Some(ImageFormat::Gif)
    } else if lower.contains("image/webp") {
        Some(ImageFormat::WebP)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Dimension estimation
// ---------------------------------------------------------------------------

/// Estimate image dimensions from base64 data size.
///
/// This is a rough estimate based on typical compression ratios.
/// For accurate dimensions, a proper image decoder would be needed.
///
/// # Arguments
///
/// * `base64_data` — The base64-encoded image data.
/// * `format` — The image format.
///
/// # Returns
///
/// Estimated `(width, height)` dimensions.
#[must_use]
pub fn estimate_dimensions(base64_data: &str, format: ImageFormat) -> (u32, u32) {
    let data = base64_data.trim_end_matches('=');
    let byte_size = (data.len() as u64 * 3) / 4;

    // Rough estimation based on typical compression ratios.
    let pixels = match format {
        ImageFormat::Png => byte_size / 3, // PNG is lossless but compressed.
        ImageFormat::Jpeg => byte_size * 10, // JPEG has ~10:1 compression.
        ImageFormat::Gif => byte_size * 5, // GIF has moderate compression.
        ImageFormat::WebP => byte_size * 8, // WebP is efficient.
    };

    // Assume square image.
    let side = (pixels as f64).sqrt() as u32;
    (side.max(1), side.max(1))
}

/// Validate base64 data and return the decoded size.
///
/// # Arguments
///
/// * `base64_data` — The base64-encoded data.
///
/// # Returns
///
/// `Ok(decoded_size)` if valid, or an `ImageError`.
pub fn validate_base64(base64_data: &str) -> Result<u64, ImageError> {
    if base64_data.is_empty() {
        return Err(ImageError::EmptyData);
    }

    let trimmed = base64_data.trim_end_matches('=');
    for c in trimmed.chars() {
        if !c.is_ascii_alphanumeric() && c != '+' && c != '/' {
            return Err(ImageError::InvalidBase64(format!(
                "Invalid character: '{c}'"
            )));
        }
    }

    Ok((trimmed.len() as u64 * 3) / 4)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- ImageFormat ---

    #[test]
    fn image_format_extensions() {
        assert_eq!(ImageFormat::Png.extension(), "png");
        assert_eq!(ImageFormat::Jpeg.extension(), "jpg");
        assert_eq!(ImageFormat::Gif.extension(), "gif");
        assert_eq!(ImageFormat::WebP.extension(), "webp");
    }

    #[test]
    fn image_format_mime_types() {
        assert_eq!(ImageFormat::Png.mime_type(), "image/png");
        assert_eq!(ImageFormat::Jpeg.mime_type(), "image/jpeg");
        assert_eq!(ImageFormat::Gif.mime_type(), "image/gif");
        assert_eq!(ImageFormat::WebP.mime_type(), "image/webp");
    }

    #[test]
    fn image_format_from_str_opt() {
        assert_eq!(ImageFormat::from_str_opt("png"), Some(ImageFormat::Png));
        assert_eq!(ImageFormat::from_str_opt("jpg"), Some(ImageFormat::Jpeg));
        assert_eq!(ImageFormat::from_str_opt("jpeg"), Some(ImageFormat::Jpeg));
        assert_eq!(ImageFormat::from_str_opt("gif"), Some(ImageFormat::Gif));
        assert_eq!(ImageFormat::from_str_opt("webp"), Some(ImageFormat::WebP));
        assert_eq!(ImageFormat::from_str_opt("bmp"), None);
    }

    #[test]
    fn image_format_display() {
        assert_eq!(ImageFormat::Png.to_string(), "png");
    }

    #[test]
    fn image_format_all_values() {
        assert_eq!(ImageFormat::all_values().len(), 4);
    }

    #[test]
    fn image_format_serialization_roundtrip() {
        let fmt = ImageFormat::WebP;
        let json = serde_json::to_string(&fmt).expect("serialize");
        let deserialized: ImageFormat = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(fmt, deserialized);
    }

    // --- ImageError ---

    #[test]
    fn image_error_display() {
        assert_eq!(ImageError::EmptyData.to_string(), "Image data is empty");
        assert!(
            ImageError::InvalidBase64("bad".to_string())
                .to_string()
                .contains("bad")
        );
        assert!(
            ImageError::UnsupportedFormat("bmp".to_string())
                .to_string()
                .contains("bmp")
        );
        assert!(
            ImageError::SizeExceeded {
                actual: 100,
                max: 50
            }
            .to_string()
            .contains("100")
        );
    }

    // --- detect_image_format ---

    #[test]
    fn detect_png_magic_bytes() {
        // PNG magic bytes in base64: \x89PNG\r\n\x1a\n -> iVBORw0KGgo
        assert_eq!(
            detect_image_format("iVBORw0KGgoAAAANS"),
            Some(ImageFormat::Png)
        );
    }

    #[test]
    fn detect_jpeg_magic_bytes() {
        // JPEG magic bytes: \xFF\xD8\xFF -> /9j/
        assert_eq!(
            detect_image_format("/9j/4AAQSkZJRg"),
            Some(ImageFormat::Jpeg)
        );
    }

    #[test]
    fn detect_gif_magic_bytes() {
        // GIF magic bytes: GIF89a -> R0lGODlh
        assert_eq!(
            detect_image_format("R0lGODlhAQABAIAAAP"),
            Some(ImageFormat::Gif)
        );
    }

    #[test]
    fn detect_webp_magic_bytes() {
        // WebP magic bytes: RIFF....WEBP -> UklGR
        assert_eq!(
            detect_image_format("UklGRjoAAABXRUJQ"),
            Some(ImageFormat::WebP)
        );
    }

    #[test]
    fn detect_unknown_format() {
        assert_eq!(detect_image_format("SGVsbG8gV29ybGQ="), None);
    }

    #[test]
    fn detect_empty_data() {
        assert_eq!(detect_image_format(""), None);
    }

    // --- detect_format_from_data_uri ---

    #[test]
    fn detect_data_uri_png() {
        assert_eq!(
            detect_format_from_data_uri("data:image/png;base64,abc"),
            Some(ImageFormat::Png)
        );
    }

    #[test]
    fn detect_data_uri_jpeg() {
        assert_eq!(
            detect_format_from_data_uri("data:image/jpeg;base64,abc"),
            Some(ImageFormat::Jpeg)
        );
    }

    #[test]
    fn detect_data_uri_unknown() {
        assert_eq!(
            detect_format_from_data_uri("data:application/pdf;base64,abc"),
            None
        );
    }

    // --- estimate_dimensions ---

    #[test]
    fn estimate_dimensions_png() {
        // 1000 base64 chars ≈ 750 bytes.
        let data = "A".repeat(1000);
        let (w, h) = estimate_dimensions(&data, ImageFormat::Png);
        assert!(w > 0);
        assert!(h > 0);
    }

    #[test]
    fn estimate_dimensions_jpeg() {
        let data = "A".repeat(1000);
        let (w, h) = estimate_dimensions(&data, ImageFormat::Jpeg);
        assert!(w > 0);
        assert!(h > 0);
    }

    #[test]
    fn estimate_dimensions_small() {
        let (w, h) = estimate_dimensions("AA", ImageFormat::Png);
        assert_eq!((w, h), (1, 1));
    }

    // --- validate_base64 ---

    #[test]
    fn validate_base64_valid() {
        let size = validate_base64("SGVsbG8=").expect("valid");
        assert!(size > 0);
    }

    #[test]
    fn validate_base64_empty() {
        assert!(matches!(validate_base64(""), Err(ImageError::EmptyData)));
    }

    #[test]
    fn validate_base64_invalid_chars() {
        assert!(matches!(
            validate_base64("not!valid"),
            Err(ImageError::InvalidBase64(_))
        ));
    }

    #[test]
    fn validate_base64_size_calculation() {
        // "SGVsbG8=" decodes to "Hello" = 5 bytes.
        // Trimmed: "SGVsbG8" = 7 chars -> (7*3)/4 = 5.
        let size = validate_base64("SGVsbG8=").expect("valid");
        assert_eq!(size, 5);
    }
}
