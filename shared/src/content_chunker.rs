use crate::models::DocumentChunk;

pub struct ContentChunker;

impl ContentChunker {
    /// Chunk document content into smaller pieces for embedding generation.
    ///
    /// Positions and `max_chunk_size` are measured in characters, consistent
    /// with the Python chunker, so cuts never land inside a multi-byte UTF-8
    /// character.
    pub fn chunk_content(content: &str, max_chunk_size: usize) -> Vec<DocumentChunk> {
        let chars: Vec<char> = content.chars().collect();
        let char_count = chars.len();

        if char_count == 0 {
            return vec![];
        }
        if max_chunk_size == 0 || char_count <= max_chunk_size {
            return vec![DocumentChunk {
                text: content.to_string(),
                index: 0,
            }];
        }

        let mut chunks = Vec::new();
        let mut current_pos = 0;
        let mut chunk_index = 0;

        while current_pos < char_count {
            let end_pos = std::cmp::min(current_pos + max_chunk_size, char_count);

            // Try to break at sentence boundaries for better semantic coherence
            let chunk_end = if end_pos < char_count {
                // Look for sentence endings within the last 100 characters
                let search_start = std::cmp::max(current_pos, end_pos.saturating_sub(100));
                let search_window = &chars[search_start..end_pos];

                if let Some(sentence_end) = search_window.iter().rposition(|&c| c == '.') {
                    search_start + sentence_end + 1
                } else if let Some(paragraph_end) = search_window.iter().rposition(|&c| c == '\n') {
                    search_start + paragraph_end + 1
                } else {
                    end_pos
                }
            } else {
                char_count
            };

            let chunk_text: String = chars[current_pos..chunk_end].iter().collect();
            chunks.push(DocumentChunk {
                text: chunk_text,
                index: chunk_index,
            });

            if chunk_end <= current_pos {
                // Prevent infinite loops on degenerate inputs
                break;
            }
            current_pos = chunk_end;
            chunk_index += 1;
        }

        chunks
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chunk_empty_content() {
        let chunks = ContentChunker::chunk_content("", 1000);
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_chunk_small_content() {
        let content = "This is a small piece of content.";
        let chunks = ContentChunker::chunk_content(content, 1000);

        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].text, content);
        assert_eq!(chunks[0].index, 0);
    }

    #[test]
    fn test_chunk_large_content_preserves_all_text() {
        let content =
            "This is the first sentence. This is the second sentence. This is the third sentence.";
        let chunks = ContentChunker::chunk_content(content, 50);

        assert!(chunks.len() > 1);

        // Joining all chunks must reproduce the original content exactly
        let reconstructed: String = chunks.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(reconstructed, content);

        // Every chunk except possibly the last must break at a sentence boundary
        for chunk in &chunks[..chunks.len() - 1] {
            assert!(
                chunk.text.ends_with('.') || chunk.text.ends_with('\n'),
                "Intermediate chunk should end at sentence/paragraph boundary: {:?}",
                chunk.text
            );
        }

        // Verify indices are sequential starting from 0
        for (i, chunk) in chunks.iter().enumerate() {
            assert_eq!(chunk.index, i as i32);
        }
    }

    #[test]
    fn test_chunk_breaks_at_paragraph_boundary() {
        // Content with newlines but no periods in the search window
        let content = "Line one without period\nLine two without period\nLine three without period\nLine four without period";
        let chunks = ContentChunker::chunk_content(content, 50);

        assert!(chunks.len() > 1);

        // Reconstructed text must match original
        let reconstructed: String = chunks.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(reconstructed, content);

        // Intermediate chunks should break at newline since there are no periods
        for chunk in &chunks[..chunks.len() - 1] {
            assert!(
                chunk.text.ends_with('\n'),
                "Should break at paragraph boundary: {:?}",
                chunk.text
            );
        }
    }

    #[test]
    fn test_chunk_hard_break_when_no_boundaries() {
        // Long string with no sentence or paragraph boundaries in the search window
        let content = "a".repeat(200);
        let chunks = ContentChunker::chunk_content(&content, 50);

        assert_eq!(chunks.len(), 4); // 200 / 50 = 4 chunks

        let reconstructed: String = chunks.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(reconstructed, content);

        // Each chunk must be exactly max_chunk_size (except possibly the last)
        for chunk in &chunks[..chunks.len() - 1] {
            assert_eq!(chunk.text.len(), 50);
        }
    }

    #[test]
    fn test_chunk_multibyte_unicode() {
        // Mix of ASCII, multi-byte (emoji), and CJK characters
        let content = "Hello \u{1F600} world! \u{4F60}\u{597D}\u{4E16}\u{754C}. More text here to ensure we get multiple chunks out of this content.";
        let chunks = ContentChunker::chunk_content(content, 30);

        assert!(!chunks.is_empty());

        // Must not panic and must reconstruct correctly
        let reconstructed: String = chunks.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(reconstructed, content);

        for (i, chunk) in chunks.iter().enumerate() {
            assert_eq!(chunk.index, i as i32);
            // Verify each chunk is valid UTF-8 (would panic on access if not)
            assert!(!chunk.text.is_empty() || i == chunks.len() - 1);
        }
    }

    #[test]
    fn test_chunk_indices_are_sequential() {
        let content = "Sentence one. Sentence two. Sentence three. Sentence four. Sentence five. Sentence six. Sentence seven. Sentence eight.";
        let chunks = ContentChunker::chunk_content(content, 40);

        let indices: Vec<i32> = chunks.iter().map(|c| c.index).collect();
        let expected: Vec<i32> = (0..chunks.len() as i32).collect();
        assert_eq!(indices, expected);
    }

    #[test]
    fn test_chunk_multibyte_cut_never_panics() {
        // Cut positions are byte-based, so a chunk boundary can land inside a
        // multi-byte UTF-8 character (accented text, emoji, CJK). Slicing must
        // not panic and must still reproduce the source text exactly.
        let content = "H\u{e9}llo world! ".repeat(10);
        let chunks = ContentChunker::chunk_content(&content, 30);

        let reconstructed: String = chunks.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(reconstructed, content);
    }

    #[test]
    fn test_chunk_size_limit_is_character_based() {
        // The Python chunker sizes chunks in characters. The Rust chunker must
        // use the same unit, so that a max_chunk_size chunk holds up to
        // max_chunk_size characters rather than bytes.
        let content = "H\u{e9}llo world! ".repeat(10);
        let chunks = ContentChunker::chunk_content(&content, 30);

        assert!(chunks.len() > 1);
        for chunk in &chunks[..chunks.len() - 1] {
            assert_eq!(chunk.text.chars().count(), 30);
        }
    }
}
