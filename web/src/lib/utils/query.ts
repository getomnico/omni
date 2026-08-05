/**
 * Query normalization and minimum-length check.
 *
 * Mirrors the backend searcher's ``has_minimum_query_length`` / ``normalize``
 * in ``services/searcher/src/typeahead.rs`` so frontend and backend agree on
 * what counts as a valid query.
 *
 * Normalization steps:
 *  1. Lowercase
 *  2. Replace non-alphanumeric (Unicode Letter / Number) with spaces
 *  3. Collapse consecutive whitespace
 *  4. Trim
 *
 * Minimum length is 3 normalized Unicode code points.
 */

/**
 * Normalize a query string the same way the backend searcher does.
 *
 * Non-alphanumeric characters become spaces, then runs of whitespace are
 * collapsed into a single space and leading/trailing space is trimmed.
 */
export function normalizeQuery(query: string): string {
    const lowered = query.toLowerCase()
    const mapped: string[] = []
    for (const ch of lowered) {
        // Rust's char::is_alphanumeric() combines the Unicode Alphabetic and
        // Number properties. Alphabetic includes marks used by scripts such as
        // Devanagari, not only characters in the Letter category.
        if (/[\p{Alphabetic}\p{Number}]/u.test(ch)) {
            mapped.push(ch)
        } else {
            mapped.push(' ')
        }
    }
    return mapped.join('').split(/\s+/).filter(Boolean).join(' ')
}

/**
 * Count the number of Unicode code points in the normalized form of *query*.
 *
 * Uses spread (``[...str].length``) to count code points, not UTF-16 code
 * units, which matches Rust's ``.chars().count()``.
 */
export function normalizedQueryLength(query: string): number {
    return [...normalizeQuery(query)].length
}

/**
 * Return true when the normalized form of *query* has at least 3 Unicode
 * code points — the same minimum the backend enforces for typeahead/search.
 */
export function hasMinimumQueryLength(query: string): boolean {
    return normalizedQueryLength(query) >= 3
}
