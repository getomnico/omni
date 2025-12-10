use ulid::Ulid;

pub fn generate_ulid() -> String {
    Ulid::new().to_string()
}

/// Safely slices a string at the given byte positions, adjusting to char boundaries.
///
/// # Arguments
/// * `content` - The string to slice
/// * `start` - Start byte position (will be adjusted to char boundary if needed)
/// * `end` - End byte position (will be adjusted to char boundary if needed)
///
/// # Returns
/// A string slice from the adjusted start to adjusted end
pub fn safe_str_slice(content: &str, start: usize, end: usize) -> &str {
    if start >= content.len() {
        panic!(
            "safe_str_slice: start ({}) >= content length ({})",
            start,
            content.len()
        );
    }
    if start == end {
        return "";
    }

    if start > end {
        panic!("safe_str_slice: start ({}) >= end ({})", start, end);
    }

    let mut adjusted_start = start.min(content.len());
    let mut adjusted_end = end.min(content.len());

    while adjusted_start > 0 && !content.is_char_boundary(adjusted_start) {
        adjusted_start -= 1;
    }

    while adjusted_end < content.len() && !content.is_char_boundary(adjusted_end) {
        adjusted_end += 1;
    }

    if adjusted_start >= adjusted_end {
        panic!(
            "safe_str_slice: adjusted bounds invalid - start ({}) >= end ({})",
            adjusted_start, adjusted_end
        );
    }

    &content[adjusted_start..adjusted_end]
}
