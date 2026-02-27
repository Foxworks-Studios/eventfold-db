//! View modules for each TUI tab.
//!
//! Each sub-module renders one tab of the console interface.
//! The [`format_bytes`] helper is shared across views for displaying
//! payload and metadata fields.

pub mod global_log;
pub mod live_tail;
pub mod stream_detail;
pub mod streams;

/// Format raw bytes for display in the TUI.
///
/// Attempts three strategies in order:
/// 1. If `pretty_json` is true and the bytes are valid JSON, pretty-print them.
/// 2. If the bytes are valid UTF-8, return as a string.
/// 3. Otherwise, return a hex dump (first 128 bytes, with "..." if truncated).
///
/// # Arguments
///
/// * `bytes` - The raw bytes to format.
/// * `pretty_json` - Whether to attempt JSON pretty-printing.
///
/// # Returns
///
/// A formatted string representation of the bytes.
pub fn format_bytes(bytes: &[u8], pretty_json: bool) -> String {
    if pretty_json && let Ok(value) = serde_json::from_slice::<serde_json::Value>(bytes) {
        // serde_json::to_string_pretty never fails on a valid Value.
        return serde_json::to_string_pretty(&value)
            .expect("serializing valid JSON value should not fail");
    }

    if let Ok(s) = std::str::from_utf8(bytes) {
        return s.to_string();
    }

    // Hex dump fallback.
    let limit = 128;
    let hex: String = bytes
        .iter()
        .take(limit)
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ");
    if bytes.len() > limit {
        format!("{hex} ...")
    } else {
        hex
    }
}

/// Truncate a string to a maximum width, appending "..." if truncated.
///
/// Uses character count (not byte length) for correct handling of multi-byte
/// characters.
///
/// # Arguments
///
/// * `s` - The string to truncate.
/// * `max_width` - Maximum number of characters to keep.
///
/// # Returns
///
/// The truncated string, or the original if within bounds.
pub fn truncate(s: &str, max_width: usize) -> String {
    if s.chars().count() <= max_width {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_width.saturating_sub(3)).collect();
        format!("{truncated}...")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- format_bytes: JSON pretty-print --

    #[test]
    fn format_bytes_pretty_prints_json() {
        let json = br#"{"key":"value","num":42}"#;
        let result = format_bytes(json, true);
        assert!(result.contains("\"key\""), "got: {result}");
        assert!(
            result.contains('\n'),
            "expected newlines for pretty JSON: {result}"
        );
    }

    #[test]
    fn format_bytes_json_disabled_returns_raw_utf8() {
        let json = br#"{"key":"value"}"#;
        let result = format_bytes(json, false);
        assert_eq!(result, "{\"key\":\"value\"}");
    }

    // -- format_bytes: UTF-8 fallback --

    #[test]
    fn format_bytes_plain_utf8() {
        let text = b"hello world";
        let result = format_bytes(text, true);
        assert_eq!(result, "hello world");
    }

    #[test]
    fn format_bytes_utf8_non_json() {
        let text = b"not json at all";
        let result = format_bytes(text, true);
        assert_eq!(result, "not json at all");
    }

    // -- format_bytes: hex fallback --

    #[test]
    fn format_bytes_invalid_utf8_returns_hex() {
        let bytes = vec![0xff, 0xfe, 0x00, 0x01];
        let result = format_bytes(&bytes, true);
        assert_eq!(result, "ff fe 00 01");
    }

    #[test]
    fn format_bytes_hex_truncates_at_128() {
        let bytes: Vec<u8> = (0..200).map(|i| (i & 0xff) as u8).collect();
        // Prepend invalid UTF-8 to force hex path.
        let mut data = vec![0xff];
        data.extend_from_slice(&bytes);
        let result = format_bytes(&data, true);
        assert!(result.ends_with("..."), "expected truncation: {result}");
    }

    // -- format_bytes: empty input --

    #[test]
    fn format_bytes_empty_returns_empty() {
        let result = format_bytes(&[], true);
        assert_eq!(result, "");
    }

    // -- truncate --

    #[test]
    fn truncate_short_string_unchanged() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn truncate_long_string_adds_ellipsis() {
        let result = truncate("abcdefghij", 7);
        assert!(result.ends_with("..."), "got: {result}");
        assert!(result.chars().count() <= 7, "got: {result}");
    }

    #[test]
    fn truncate_exact_length_unchanged() {
        assert_eq!(truncate("abc", 3), "abc");
    }
}
