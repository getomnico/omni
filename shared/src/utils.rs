use std::sync::{LazyLock, Mutex};
use ulid::Generator;

static ULID_GENERATOR: LazyLock<Mutex<Generator>> = LazyLock::new(|| Mutex::new(Generator::new()));

pub fn generate_ulid() -> String {
    ULID_GENERATOR
        .lock()
        .expect("ULID generator mutex poisoned")
        .generate()
        .expect("monotonic ULID generation overflowed")
        .to_string()
}

/// Safely slices a string at the given CHARACTER positions, clamping to the
/// string's bounds.
///
/// Chunk start/end offsets are character offsets (the Python chunker measures
/// them with `len()`, i.e. characters). Interpreting them as byte positions
/// mis-slices any text containing multi-byte UTF-8 characters, so this helper
/// converts the character range to byte positions before slicing.
///
/// # Arguments
/// * `content` - The string to slice
/// * `start` - Start character position
/// * `end` - End character position
///
/// # Returns
/// A string slice from the character at `start` to the character before `end`
pub fn safe_str_slice(content: &str, start: usize, end: usize) -> &str {
    if start > end {
        panic!("safe_str_slice: start ({}) > end ({})", start, end);
    }
    if start == end {
        return "";
    }

    let char_count = content.chars().count();
    if start >= char_count {
        return "";
    }
    let end = end.min(char_count);

    let start_byte = content
        .char_indices()
        .nth(start)
        .map(|(byte, _)| byte)
        .unwrap_or(content.len());
    let end_byte = content
        .char_indices()
        .nth(end)
        .map(|(byte, _)| byte)
        .unwrap_or(content.len());

    &content[start_byte..end_byte]
}

/// Normalizes whitespace in text content for clean storage and indexing.
///
/// - CRLF / CR → LF
/// - Removes control characters (except \n, \t) and zero-width Unicode characters
/// - Collapses runs of horizontal whitespace per line to a single space, trims each line
/// - Collapses 3+ consecutive newlines to 2 (preserves paragraph breaks)
/// - Trims leading/trailing whitespace
pub fn normalize_whitespace(text: &str) -> String {
    let mut result = String::with_capacity(text.len());

    // First pass: normalize line endings, remove control/zero-width chars,
    // collapse horizontal whitespace per line
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        // Normalize CRLF and CR to LF
        if ch == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            result.push('\n');
            continue;
        }

        // Remove zero-width characters
        if matches!(
            ch,
            '\u{200B}' | '\u{FEFF}' | '\u{200C}' | '\u{200D}' | '\u{00AD}'
        ) {
            continue;
        }

        // Remove control characters except \n and \t
        if ch.is_control() && ch != '\n' && ch != '\t' {
            continue;
        }

        // Collapse horizontal whitespace (space, tab, non-breaking space) to single space
        if ch == ' ' || ch == '\t' || ch == '\u{00A0}' {
            // Skip additional horizontal whitespace
            while chars
                .peek()
                .is_some_and(|&c| c == ' ' || c == '\t' || c == '\u{00A0}')
            {
                chars.next();
            }
            result.push(' ');
            continue;
        }

        result.push(ch);
    }

    // Second pass: trim each line and collapse 3+ newlines to 2
    let lines: Vec<&str> = result.split('\n').map(|line| line.trim()).collect();
    let mut final_result = String::with_capacity(result.len());
    let mut consecutive_empty = 0;

    for (i, line) in lines.iter().enumerate() {
        if line.is_empty() {
            consecutive_empty += 1;
        } else {
            consecutive_empty = 0;
        }

        // Allow at most 2 consecutive newlines (1 empty line between paragraphs)
        if consecutive_empty <= 1 {
            if i > 0 {
                final_result.push('\n');
            }
            final_result.push_str(line);
        }
    }

    final_result.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_safe_str_slice_ascii() {
        let content = "hello world";
        assert_eq!(safe_str_slice(content, 0, 5), "hello");
        assert_eq!(safe_str_slice(content, 6, 11), "world");
    }

    #[test]
    fn test_safe_str_slice_multibyte_char_offsets() {
        // \u{1F600} is 4 bytes, so byte-based slicing would shift the window.
        // Offsets are character positions and must ignore the byte width.
        let content = "\u{1F600}abc";
        assert_eq!(safe_str_slice(content, 0, 1), "\u{1F600}");
        assert_eq!(safe_str_slice(content, 1, 3), "ab");
        assert_eq!(safe_str_slice(content, 1, 4), "abc");
    }

    #[test]
    fn test_safe_str_slice_accented_text() {
        // \u{00E9} (é) is 2 bytes; offsets are characters so [0,4) is "café".
        let content = "caf\u{00E9} au lait";
        assert_eq!(safe_str_slice(content, 0, 4), "caf\u{00E9}");
        assert_eq!(safe_str_slice(content, 4, 7), " au");
    }

    #[test]
    fn test_safe_str_slice_start_equals_end() {
        let content = "hello";
        assert_eq!(safe_str_slice(content, 2, 2), "");
    }

    #[test]
    fn test_safe_str_slice_end_clamped_to_length() {
        let content = "short";
        assert_eq!(safe_str_slice(content, 0, 100), "short");
    }

    #[test]
    fn test_safe_str_slice_offsets_are_character_positions() {
        // The embedding pipeline stores chunk start/end offsets as CHARACTER
        // offsets (Python len()), and retrieval slices chunk content back out of
        // the source text with them. Slicing at the same numeric positions as
        // BYTES must not shift the window: for "😀😀hello world" chars [2,7)
        // is "hello".
        let content = "\u{1F600}\u{1F600}hello world";
        let start = 2;
        let end = 7;
        let expected: String = content.chars().skip(start).take(end - start).collect();
        assert_eq!(expected, "hello");
        assert_eq!(safe_str_slice(content, start, end), expected);
    }

    #[test]
    fn test_safe_str_slice_start_beyond_length_returns_empty() {
        let content = "hello";
        assert_eq!(safe_str_slice(content, 10, 15), "");
    }

    #[test]
    #[should_panic(expected = "start")]
    fn test_safe_str_slice_start_greater_than_end_panics() {
        safe_str_slice("hello", 3, 1);
    }

    #[test]
    fn test_generate_ulid_is_monotonic() {
        let mut previous = generate_ulid();
        for _ in 0..1000 {
            let current = generate_ulid();
            assert!(previous < current);
            previous = current;
        }
    }

    #[test]
    fn test_normalize_normal_text_unchanged() {
        let input = "Hello world.\n\nThis is a paragraph.";
        assert_eq!(normalize_whitespace(input), input);
    }

    #[test]
    fn test_normalize_multiple_spaces_collapsed() {
        assert_eq!(normalize_whitespace("hello    world"), "hello world");
    }

    #[test]
    fn test_normalize_tabs_and_nbsp_collapsed() {
        assert_eq!(
            normalize_whitespace("hello\t\t\u{00A0}world"),
            "hello world"
        );
    }

    #[test]
    fn test_normalize_excessive_newlines_collapsed() {
        assert_eq!(
            normalize_whitespace("hello\n\n\n\n\nworld"),
            "hello\n\nworld"
        );
    }

    #[test]
    fn test_normalize_preserves_paragraph_break() {
        let input = "paragraph one\n\nparagraph two";
        assert_eq!(normalize_whitespace(input), input);
    }

    #[test]
    fn test_normalize_crlf() {
        assert_eq!(
            normalize_whitespace("hello\r\nworld\rfoo"),
            "hello\nworld\nfoo"
        );
    }

    #[test]
    fn test_normalize_control_chars_removed() {
        assert_eq!(normalize_whitespace("hello\x01\x02world"), "helloworld");
    }

    #[test]
    fn test_normalize_zero_width_chars_removed() {
        assert_eq!(
            normalize_whitespace("hello\u{200B}\u{FEFF}\u{200C}\u{200D}\u{00AD}world"),
            "helloworld"
        );
    }

    #[test]
    fn test_normalize_empty_input() {
        assert_eq!(normalize_whitespace(""), "");
    }

    #[test]
    fn test_normalize_whitespace_only() {
        assert_eq!(normalize_whitespace("   \n\n\n  \t  "), "");
    }

    #[test]
    fn test_normalize_trims_lines() {
        assert_eq!(normalize_whitespace("  hello  \n  world  "), "hello\nworld");
    }

    #[test]
    fn test_normalize_mixed_scenario() {
        let input = "  Hello\r\n\r\n\r\n\r\n  world  \t foo\x00\u{200B}  \n\nbar  ";
        let expected = "Hello\n\nworld foo\n\nbar";
        assert_eq!(normalize_whitespace(input), expected);
    }
}
