//! Media stripping and processing for API requests.
//!
//! Handles removal of media attachments that exceed configured limits,
//! and provides utilities for image metadata and base64 image resizing.

use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration limits for media attachments in API requests.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MediaLimits {
    /// Maximum number of images allowed per request.
    pub max_images: usize,
    /// Maximum file size in megabytes per image.
    pub max_file_size_mb: u32,
}

impl Default for MediaLimits {
    fn default() -> Self {
        Self {
            max_images: 20,
            max_file_size_mb: 5,
        }
    }
}

impl MediaLimits {
    /// Create new media limits with custom values.
    #[must_use]
    pub fn new(max_images: usize, max_file_size_mb: u32) -> Self {
        Self {
            max_images,
            max_file_size_mb,
        }
    }

    /// Return the maximum file size in bytes.
    #[must_use]
    pub fn max_file_size_bytes(&self) -> u64 {
        u64::from(self.max_file_size_mb) * 1024 * 1024
    }
}

// ---------------------------------------------------------------------------
// Image metadata
// ---------------------------------------------------------------------------

/// Metadata about an image attachment.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImageMetadata {
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
    /// Image format (e.g. "png", "jpeg", "gif", "webp").
    pub format: String,
    /// Image file size in bytes.
    pub size: u64,
}

impl ImageMetadata {
    /// Create new image metadata.
    #[must_use]
    pub fn new(width: u32, height: u32, format: String, size: u64) -> Self {
        Self {
            width,
            height,
            format,
            size,
        }
    }

    /// Check if this image exceeds the given size limit.
    #[must_use]
    pub fn exceeds_size_limit(&self, max_bytes: u64) -> bool {
        self.size > max_bytes
    }

    /// Check if this image exceeds the given dimension limits.
    #[must_use]
    pub fn exceeds_dimension_limit(&self, max_width: u32, max_height: u32) -> bool {
        self.width > max_width || self.height > max_height
    }

    /// Calculate the aspect ratio (width / height) as a float string.
    #[must_use]
    pub fn aspect_ratio(&self) -> String {
        if self.height == 0 {
            return "unknown".to_string();
        }
        let ratio = f64::from(self.width) / f64::from(self.height);
        format!("{ratio:.2}")
    }
}

// ---------------------------------------------------------------------------
// Media stripping
// ---------------------------------------------------------------------------

/// Result of stripping media over the limit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StripResult {
    /// Number of images removed.
    pub removed_count: usize,
    /// Reason for removal, if any.
    pub reasons: Vec<String>,
}

/// Strip media attachments that exceed the configured limits.
///
/// Removes images from the content blocks that exceed the maximum count or
/// individual file size.
///
/// # Arguments
///
/// * `blocks` — The content blocks array from a message.
/// * `limits` — The media limits to enforce.
///
/// # Returns
///
/// A tuple of (filtered blocks, strip result).
pub fn strip_media_over_limit(blocks: &[Value], limits: &MediaLimits) -> (Vec<Value>, StripResult) {
    let mut image_count = 0usize;
    let mut filtered = Vec::new();
    let mut removed_count = 0usize;
    let mut reasons = Vec::new();

    for block in blocks {
        let block_type = block.get("type").and_then(Value::as_str).unwrap_or("");

        if block_type == "image" {
            image_count += 1;
            if image_count > limits.max_images {
                removed_count += 1;
                reasons.push(format!(
                    "Image #{image_count} exceeds max_images limit of {}",
                    limits.max_images
                ));
                continue;
            }

            // Check file size from source.data (base64 encoded).
            let size_bytes = estimate_base64_size(block);
            if size_bytes > limits.max_file_size_bytes() {
                removed_count += 1;
                reasons.push(format!(
                    "Image #{image_count} size ({size_bytes} bytes) exceeds limit of {} bytes",
                    limits.max_file_size_bytes()
                ));
                continue;
            }
        }

        filtered.push(block.clone());
    }

    (
        filtered,
        StripResult {
            removed_count,
            reasons,
        },
    )
}

/// Estimate the decoded size of a base64-encoded image in a content block.
///
/// Looks for `source.data` in the block and estimates the original byte size.
fn estimate_base64_size(block: &Value) -> u64 {
    let data = block
        .get("source")
        .and_then(|s| s.get("data"))
        .and_then(Value::as_str)
        .unwrap_or("");

    if data.is_empty() {
        return 0;
    }

    // Base64 encodes 3 bytes per 4 characters.
    // Remove padding characters for a more accurate estimate.
    let trimmed = data.trim_end_matches('=');
    let char_count = trimmed.len() as u64;
    (char_count * 3) / 4
}

// ---------------------------------------------------------------------------
// Image resizing
// ---------------------------------------------------------------------------

/// Result of an image resize operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResizeResult {
    /// The resized base64 data, if successful.
    pub data: Option<String>,
    /// Error message, if the operation failed.
    pub error: Option<String>,
    /// New dimensions after resize.
    pub new_dimensions: Option<(u32, u32)>,
    /// Estimated original size in bytes.
    pub estimated_size: Option<u64>,
}

/// Resize a base64-encoded image by decoding, scaling, and re-encoding.
///
/// Decodes the base64 data into a raster image, resizes it so that neither
/// dimension exceeds `max_dimension` while preserving aspect ratio, then
/// re-encodes the result as PNG back to base64.
///
/// # Arguments
///
/// * `base64_data` — The base64-encoded image data.
/// * `max_dimension` — The maximum dimension (width or height) to scale to.
///
/// # Returns
///
/// A `ResizeResult` indicating success or failure.
pub fn resize_image_base64(base64_data: &str, max_dimension: u32) -> ResizeResult {
    if base64_data.is_empty() {
        return ResizeResult {
            data: None,
            error: Some("Empty base64 data".to_string()),
            new_dimensions: None,
            estimated_size: None,
        };
    }

    // Decode base64.
    let raw_bytes = match base64::engine::general_purpose::STANDARD.decode(base64_data) {
        Ok(bytes) => bytes,
        Err(e) => {
            return ResizeResult {
                data: None,
                error: Some(format!("Invalid base64: {e}")),
                new_dimensions: None,
                estimated_size: None,
            };
        }
    };

    // Decode image.
    let mut img = match image::load_from_memory(&raw_bytes) {
        Ok(img) => img,
        Err(e) => {
            return ResizeResult {
                data: None,
                error: Some(format!("Failed to decode image: {e}")),
                new_dimensions: None,
                estimated_size: Some(raw_bytes.len() as u64),
            };
        }
    };

    let (orig_w, orig_h) = (img.width(), img.height());
    let max_dim = if max_dimension > 0 {
        max_dimension
    } else {
        1024
    };

    // Only resize if the image exceeds the max dimension.
    if orig_w > max_dim || orig_h > max_dim {
        img = img.resize(max_dim, max_dim, image::imageops::FilterType::Lanczos3);
    }

    let (new_w, new_h) = (img.width(), img.height());

    // Re-encode as PNG.
    let mut png_buf = Vec::new();
    if let Err(e) = img.write_to(
        &mut std::io::Cursor::new(&mut png_buf),
        image::ImageFormat::Png,
    ) {
        return ResizeResult {
            data: None,
            error: Some(format!("Failed to re-encode image: {e}")),
            new_dimensions: Some((new_w, new_h)),
            estimated_size: Some(raw_bytes.len() as u64),
        };
    }

    let encoded = base64::engine::general_purpose::STANDARD.encode(&png_buf);

    ResizeResult {
        data: Some(encoded),
        error: None,
        new_dimensions: Some((new_w, new_h)),
        estimated_size: Some(png_buf.len() as u64),
    }
}

/// Detect the image format from a base64 data URI prefix.
///
/// # Arguments
///
/// * `data_uri` — The data URI string (e.g. `"data:image/png;base64,..."`).
///
/// # Returns
///
/// The format string (e.g. `"png"`), or `None` if not detected.
#[must_use]
pub fn detect_format_from_data_uri(data_uri: &str) -> Option<String> {
    let lower = data_uri.to_ascii_lowercase();
    if lower.starts_with("data:image/png") {
        Some("png".to_string())
    } else if lower.starts_with("data:image/jpeg") || lower.starts_with("data:image/jpg") {
        Some("jpeg".to_string())
    } else if lower.starts_with("data:image/gif") {
        Some("gif".to_string())
    } else if lower.starts_with("data:image/webp") {
        Some("webp".to_string())
    } else if lower.starts_with("data:image/svg") {
        Some("svg".to_string())
    } else {
        None
    }
}

/// Count the number of image blocks in a content blocks array.
///
/// # Arguments
///
/// * `blocks` — The content blocks array.
///
/// # Returns
///
/// The number of image blocks.
#[must_use]
pub fn count_image_blocks(blocks: &[Value]) -> usize {
    blocks
        .iter()
        .filter(|b| {
            b.get("type")
                .and_then(Value::as_str)
                .is_some_and(|t| t == "image")
        })
        .count()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // --- MediaLimits ---

    #[test]
    fn media_limits_default() {
        let limits = MediaLimits::default();
        assert_eq!(limits.max_images, 20);
        assert_eq!(limits.max_file_size_mb, 5);
    }

    #[test]
    fn media_limits_custom() {
        let limits = MediaLimits::new(10, 2);
        assert_eq!(limits.max_images, 10);
        assert_eq!(limits.max_file_size_mb, 2);
    }

    #[test]
    fn media_limits_max_file_size_bytes() {
        let limits = MediaLimits::new(10, 5);
        assert_eq!(limits.max_file_size_bytes(), 5 * 1024 * 1024);
    }

    #[test]
    fn media_limits_serialization_roundtrip() {
        let limits = MediaLimits::new(15, 3);
        let json = serde_json::to_string(&limits).expect("serialize");
        let deserialized: MediaLimits = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(limits, deserialized);
    }

    // --- ImageMetadata ---

    #[test]
    fn image_metadata_new() {
        let meta = ImageMetadata::new(1920, 1080, "png".to_string(), 2_000_000);
        assert_eq!(meta.width, 1920);
        assert_eq!(meta.height, 1080);
        assert_eq!(meta.format, "png");
        assert_eq!(meta.size, 2_000_000);
    }

    #[test]
    fn image_metadata_exceeds_size_limit() {
        let meta = ImageMetadata::new(100, 100, "png".to_string(), 6_000_000);
        assert!(meta.exceeds_size_limit(5_000_000));
        assert!(!meta.exceeds_size_limit(7_000_000));
    }

    #[test]
    fn image_metadata_exceeds_dimension_limit() {
        let meta = ImageMetadata::new(2000, 1500, "png".to_string(), 1000);
        assert!(meta.exceeds_dimension_limit(1920, 1080));
        assert!(!meta.exceeds_dimension_limit(3840, 2160));
    }

    #[test]
    fn image_metadata_aspect_ratio() {
        let meta = ImageMetadata::new(1920, 1080, "png".to_string(), 1000);
        assert_eq!(meta.aspect_ratio(), "1.78");
    }

    #[test]
    fn image_metadata_aspect_ratio_zero_height() {
        let meta = ImageMetadata::new(100, 0, "png".to_string(), 1000);
        assert_eq!(meta.aspect_ratio(), "unknown");
    }

    // --- strip_media_over_limit ---

    #[test]
    fn strip_media_no_images() {
        let blocks = vec![
            json!({"type": "text", "text": "hello"}),
            json!({"type": "text", "text": "world"}),
        ];
        let limits = MediaLimits::default();
        let (filtered, result) = strip_media_over_limit(&blocks, &limits);
        assert_eq!(filtered.len(), 2);
        assert_eq!(result.removed_count, 0);
    }

    #[test]
    fn strip_media_within_limits() {
        let blocks = vec![
            json!({"type": "text", "text": "hello"}),
            json!({"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "abc123"}}),
        ];
        let limits = MediaLimits::new(5, 10);
        let (filtered, result) = strip_media_over_limit(&blocks, &limits);
        assert_eq!(filtered.len(), 2);
        assert_eq!(result.removed_count, 0);
    }

    #[test]
    fn strip_media_exceeds_count() {
        let mut blocks = vec![json!({"type": "text", "text": "hello"})];
        for i in 0..5 {
            blocks.push(json!({"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "abc"}, "index": i}));
        }
        let limits = MediaLimits::new(3, 100);
        let (filtered, result) = strip_media_over_limit(&blocks, &limits);
        assert_eq!(filtered.len(), 4); // 1 text + 3 images
        assert_eq!(result.removed_count, 2);
    }

    #[test]
    fn strip_media_exceeds_size() {
        // Create a large base64 string to simulate a large image.
        let large_data = "A".repeat(7_000_000);
        let blocks = vec![
            json!({"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": large_data}}),
        ];
        let limits = MediaLimits::new(10, 1); // 1 MB limit
        let (filtered, result) = strip_media_over_limit(&blocks, &limits);
        assert_eq!(filtered.len(), 0);
        assert_eq!(result.removed_count, 1);
        assert!(!result.reasons.is_empty());
    }

    #[test]
    fn strip_media_empty_blocks() {
        let blocks: Vec<Value> = vec![];
        let limits = MediaLimits::default();
        let (filtered, result) = strip_media_over_limit(&blocks, &limits);
        assert!(filtered.is_empty());
        assert_eq!(result.removed_count, 0);
    }

    // --- resize_image_base64 ---

    #[test]
    fn resize_image_empty_data() {
        let result = resize_image_base64("", 1024);
        assert!(result.data.is_none());
        assert!(result.error.is_some());
    }

    #[test]
    fn resize_image_invalid_base64() {
        let result = resize_image_base64("not!valid@base64#", 1024);
        assert!(result.data.is_none());
        assert!(result.error.is_some());
        assert!(
            result
                .error
                .as_ref()
                .is_some_and(|e| e.contains("Invalid base64"))
        );
    }

    #[test]
    fn resize_image_valid_png_no_resize_needed() {
        // Create a small 10x10 PNG image.
        let img = image::RgbImage::from_pixel(10, 10, image::Rgb([255, 0, 0]));
        let mut buf = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
            .expect("encode png");
        let b64 = base64::engine::general_purpose::STANDARD.encode(&buf);

        let result = resize_image_base64(&b64, 512);
        assert!(result.data.is_some());
        assert!(result.error.is_none());
        // Image is smaller than max_dimension, dimensions unchanged.
        assert_eq!(result.new_dimensions, Some((10, 10)));
    }

    #[test]
    fn resize_image_valid_png_downscaled() {
        // Create a 100x100 PNG and resize to max 50.
        let img = image::RgbImage::from_pixel(100, 100, image::Rgb([0, 255, 0]));
        let mut buf = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
            .expect("encode png");
        let b64 = base64::engine::general_purpose::STANDARD.encode(&buf);

        let result = resize_image_base64(&b64, 50);
        assert!(result.data.is_some());
        assert!(result.error.is_none());
        let (w, h) = result.new_dimensions.expect("dimensions");
        assert!(w <= 50);
        assert!(h <= 50);
    }

    #[test]
    fn resize_image_preserves_aspect_ratio() {
        // Create a 200x100 PNG (2:1 aspect ratio).
        let img = image::RgbImage::from_pixel(200, 100, image::Rgb([0, 0, 255]));
        let mut buf = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
            .expect("encode png");
        let b64 = base64::engine::general_purpose::STANDARD.encode(&buf);

        let result = resize_image_base64(&b64, 50);
        assert!(result.data.is_some());
        assert!(result.error.is_none());
        let (w, h) = result.new_dimensions.expect("dimensions");
        // Should preserve 2:1 aspect ratio within rounding.
        assert!(w <= 50);
        assert!(h <= 50);
        assert!(w > h);
    }

    #[test]
    fn resize_image_zero_dimension_defaults_to_1024() {
        // Create a 2048x2048 PNG and pass max_dimension=0 (should default to 1024).
        let img = image::RgbImage::from_pixel(2048, 2048, image::Rgb([128, 128, 128]));
        let mut buf = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
            .expect("encode png");
        let b64 = base64::engine::general_purpose::STANDARD.encode(&buf);

        let result = resize_image_base64(&b64, 0);
        assert!(result.data.is_some());
        assert!(result.error.is_none());
        let (w, h) = result.new_dimensions.expect("dimensions");
        assert_eq!((w, h), (1024, 1024));
    }

    #[test]
    fn resize_image_non_image_data() {
        // "SGVsbG8gV29ybGQ=" is "Hello World" — not a valid image.
        let result = resize_image_base64("SGVsbG8gV29ybGQ=", 512);
        assert!(result.data.is_none());
        assert!(
            result
                .error
                .as_ref()
                .is_some_and(|e| e.contains("Failed to decode image"))
        );
    }

    // --- detect_format_from_data_uri ---

    #[test]
    fn detect_format_png() {
        assert_eq!(
            detect_format_from_data_uri("data:image/png;base64,abc"),
            Some("png".to_string())
        );
    }

    #[test]
    fn detect_format_jpeg() {
        assert_eq!(
            detect_format_from_data_uri("data:image/jpeg;base64,abc"),
            Some("jpeg".to_string())
        );
    }

    #[test]
    fn detect_format_jpg() {
        assert_eq!(
            detect_format_from_data_uri("data:image/jpg;base64,abc"),
            Some("jpeg".to_string())
        );
    }

    #[test]
    fn detect_format_gif() {
        assert_eq!(
            detect_format_from_data_uri("data:image/gif;base64,abc"),
            Some("gif".to_string())
        );
    }

    #[test]
    fn detect_format_webp() {
        assert_eq!(
            detect_format_from_data_uri("data:image/webp;base64,abc"),
            Some("webp".to_string())
        );
    }

    #[test]
    fn detect_format_svg() {
        assert_eq!(
            detect_format_from_data_uri("data:image/svg+xml;base64,abc"),
            Some("svg".to_string())
        );
    }

    #[test]
    fn detect_format_unknown() {
        assert_eq!(
            detect_format_from_data_uri("data:application/pdf;base64,abc"),
            None
        );
    }

    #[test]
    fn detect_format_case_insensitive() {
        assert_eq!(
            detect_format_from_data_uri("DATA:IMAGE/PNG;BASE64,abc"),
            Some("png".to_string())
        );
    }

    // --- count_image_blocks ---

    #[test]
    fn count_image_blocks_mixed() {
        let blocks = vec![
            json!({"type": "text", "text": "a"}),
            json!({"type": "image", "source": {}}),
            json!({"type": "text", "text": "b"}),
            json!({"type": "image", "source": {}}),
        ];
        assert_eq!(count_image_blocks(&blocks), 2);
    }

    #[test]
    fn count_image_blocks_none() {
        let blocks = vec![
            json!({"type": "text", "text": "a"}),
            json!({"type": "tool_use", "id": "t1"}),
        ];
        assert_eq!(count_image_blocks(&blocks), 0);
    }

    #[test]
    fn count_image_blocks_empty() {
        assert_eq!(count_image_blocks(&[]), 0);
    }
}
