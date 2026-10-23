//! Byte-cap helpers for prompt/suffix slicing. Exist purely so we can take
//! `PREFIX_CAP`/`SUFFIX_CAP` bytes off a `&str` without ever splitting a
//! multi-byte UTF-8 character — stdlib's str slicing would panic.

/// Take the first `max` bytes of `s`, backing off to a UTF-8 char boundary.
pub fn cap_suffix(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Take the last `max` bytes of `s`, walking forward to a UTF-8 char boundary.
pub fn cap_prefix(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut start = s.len() - max;
    while start < s.len() && !s.is_char_boundary(start) {
        start += 1;
    }
    &s[start..]
}
