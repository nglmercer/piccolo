//! A pure-Rust implementation of Lua's pattern matching language.
//!
//! This mirrors the semantics of PUC-Rio Lua's `lstrlib.c` pattern matcher: a backtracking,
//! recursive matcher supporting character classes, sets, quantifiers (`*`, `+`, `-`, `?`),
//! anchors (`^`, `$`), captures (including position captures), balanced matches (`%bxy`) and
//! frontier matches (`%f[set]`).
//!
//! All indexing is byte-based; Lua patterns operate on arbitrary bytes, not UTF-8.

/// The maximum number of nested captures supported, matching PUC-Rio's `MAXCAPTURES`.
pub const MAX_CAPTURES: usize = 32;

/// Special marker for a capture that is still open (its end is not yet known).
const CAP_UNFINISHED: isize = -1;
/// Special marker for a position capture (`()`).
const CAP_POSITION: isize = -2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum PatternError {
    /// The pattern ended in the middle of an escape or set.
    MalformedPattern,
    /// The pattern has mismatched capture parentheses.
    UnbalancedPattern,
    /// Too many captures.
    TooManyCaptures,
    /// A `%b` balance match was malformed.
    MalformedMatchBalance,
    /// A `%1`..`%9` back-reference was invalid.
    InvalidCaptureReference,
    /// A `%f` frontier was not followed by a set.
    MissingFrontierSet,
}

impl std::fmt::Display for PatternError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let msg = match self {
            PatternError::MalformedPattern => "malformed pattern (ends with '%')",
            PatternError::UnbalancedPattern => "unbalanced pattern",
            PatternError::TooManyCaptures => "too many captures",
            PatternError::MalformedMatchBalance => "missing arguments to '%b'",
            PatternError::InvalidCaptureReference => "invalid capture index",
            PatternError::MissingFrontierSet => "missing '[' after '%f' in pattern",
        };
        write!(f, "{msg}")
    }
}

impl std::error::Error for PatternError {}

/// A single capture: a byte range `[start, end)` into the subject, or a position capture.
#[derive(Clone, Copy, Debug)]
pub struct Capture {
    pub start: usize,
    /// `end` is `CAP_UNFINISHED` while the capture is open, `CAP_POSITION` for position captures,
    /// otherwise the exclusive end index.
    pub end: isize,
}

impl Capture {
    pub fn is_position(&self) -> bool {
        self.end == CAP_POSITION
    }

    pub fn is_open(&self) -> bool {
        self.end == CAP_UNFINISHED
    }

    /// The 1-based index of this capture as Lua reports it (position captures report a 1-based
    /// position, ranges report their byte range).
    pub fn position_value(&self) -> usize {
        self.start + 1
    }
}

/// The result of a successful match: the byte range `[start, end)` and the list of captures.
#[derive(Clone, Debug)]
pub struct Match {
    pub start: usize,
    pub end: usize,
    pub captures: Vec<Capture>,
}

struct Matcher<'a> {
    subject: &'a [u8],
    pattern: &'a [u8],
}

/// Returns true if `c` belongs to the single-letter class named by `cl` (case-insensitive class
/// letter; an uppercase letter means the complement).
fn match_class(c: u8, cl: u8) -> bool {
    let res = match cl.to_ascii_lowercase() {
        b'a' => c.is_ascii_alphabetic(),
        b'c' => c.is_ascii_control(),
        b'd' => c.is_ascii_digit(),
        b'g' => c > 0x20 && c < 0x7f, // printable except space
        b'l' => c.is_ascii_lowercase(),
        b'p' => (0x21..=0x2f).contains(&c)
            || (0x3a..=0x40).contains(&c)
            || (0x5b..=0x60).contains(&c)
            || (0x7b..=0x7e).contains(&c),
        b's' => matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c),
        b'u' => c.is_ascii_uppercase(),
        b'w' => c.is_ascii_alphanumeric(),
        b'x' => c.is_ascii_hexdigit(),
        _ => return cl == c, // not a real class: match the literal character
    };
    if cl.is_ascii_uppercase() {
        !res
    } else {
        res
    }
}

impl<'a> Matcher<'a> {
    fn new(subject: &'a [u8], pattern: &'a [u8]) -> Self {
        Matcher { subject, pattern }
    }

    /// Does the single subject byte at `s` match the class described at pattern index `p`?
    /// `p` must point at a class token (either a literal, a `%x` escape, or a `[...]` set).
    fn single_match(&self, s: usize, p: usize) -> bool {
        if s >= self.subject.len() {
            return false;
        }
        let sc = self.subject[s];
        match self.pattern[p] {
            b'.' => true,
            b'%' => {
                let cl = *self.pattern.get(p + 1).unwrap_or(&0);
                match_class(sc, cl)
            }
            b'[' => {
                let end = self.set_end(p);
                self.match_bracket_class(sc, p, end)
            }
            pc => pc == sc,
        }
    }

    /// Given `p` pointing at a `[`, return the index of the closing `]`.
    fn set_end(&self, p: usize) -> usize {
        let mut i = p + 1;
        if self.pattern.get(i) == Some(&b'^') {
            i += 1;
        }
        // A `]` immediately after `[` (or `[^`) is a literal member of the set.
        if self.pattern.get(i) == Some(&b']') {
            i += 1;
        }
        while let Some(&c) = self.pattern.get(i) {
            if c == b']' {
                return i;
            } else if c == b'%' {
                i += 1; // skip the escaped byte
            }
            i += 1;
        }
        // Unterminated set: treat end-of-pattern as the terminator.
        self.pattern.len()
    }

    /// Does byte `c` match the bracket set spanning `p..=set_end`?
    fn match_bracket_class(&self, c: u8, p: usize, set_end: usize) -> bool {
        let mut i = p + 1;
        let negate = if self.pattern.get(i) == Some(&b'^') {
            i += 1;
            true
        } else {
            false
        };

        let mut found = false;
        while i < set_end {
            let pc = self.pattern[i];
            if pc == b'%' {
                let cl = *self.pattern.get(i + 1).unwrap_or(&0);
                if match_class(c, cl) {
                    found = true;
                }
                i += 2;
            } else if let Some(&next) = self.pattern.get(i + 1) {
                if next == b'-' && i + 2 < set_end {
                    // Range `lo-hi`.
                    let lo = pc;
                    let hi = self.pattern[i + 2];
                    if lo <= hi {
                        if (lo..=hi).contains(&c) {
                            found = true;
                        }
                    } else if (hi..=lo).contains(&c) {
                        found = true;
                    }
                    i += 3;
                } else {
                    if pc == c {
                        found = true;
                    }
                    i += 1;
                }
            } else {
                if pc == c {
                    found = true;
                }
                i += 1;
            }
        }

        if negate {
            !found
        } else {
            found
        }
    }

    /// The index just past the class token starting at `p`.
    fn class_end(&self, p: usize) -> usize {
        match self.pattern[p] {
            b'%' => p + 2,
            b'[' => self.set_end(p) + 1,
            _ => p + 1,
        }
    }

    /// Greedy expansion for `*` and `+`: match as many as possible, then backtrack.
    fn max_expand(&self, s: usize, p: usize, ep: usize, captures: &mut Vec<Capture>) -> Option<usize> {
        let mut i = 0;
        while self.single_match(s + i, p) {
            i += 1;
        }
        // Try the longest match first, shrinking until the rest of the pattern matches.
        loop {
            if let Some(end) = self.match_here(s + i, ep, captures) {
                return Some(end);
            }
            if i == 0 {
                return None;
            }
            i -= 1;
        }
    }

    /// Lazy expansion for `-`: match as few as possible.
    fn min_expand(&self, s: usize, p: usize, ep: usize, captures: &mut Vec<Capture>) -> Option<usize> {
        let mut i = 0;
        loop {
            if let Some(end) = self.match_here(s + i, ep, captures) {
                return Some(end);
            }
            if !self.single_match(s + i, p) {
                return None;
            }
            i += 1;
        }
    }

    /// Match a balanced `%bxy` construct. `p` points at the `%`, returns the subject index just
    /// past the balanced region, or `None`.
    fn match_balance(&self, s: usize, p: usize) -> Option<usize> {
        let open = *self.pattern.get(p + 2)?;
        let close = *self.pattern.get(p + 3)?;
        if s >= self.subject.len() || self.subject[s] != open {
            return None;
        }
        let mut i = s + 1;
        let mut depth = 1usize;
        while i < self.subject.len() {
            let c = self.subject[i];
            if c == open {
                depth += 1;
            } else if c == close {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            i += 1;
        }
        None
    }

    /// The main recursive matcher. Attempts to match the pattern starting at pattern index `p`
    /// against the subject starting at subject index `s`. Returns the exclusive end subject index
    /// on success. `captures` is the live capture stack; it is restored on failure of a branch.
    fn match_here(&self, s: usize, p: usize, captures: &mut Vec<Capture>) -> Option<usize> {
        // End of pattern: success.
        if p >= self.pattern.len() {
            return Some(s);
        }

        let pc = self.pattern[p];
        match pc {
            b'(' => {
                // Start of a capture.
                if self.pattern.get(p + 1) == Some(&b')') {
                    // Position capture.
                    return self.start_capture(s, p + 2, CAP_POSITION, captures);
                }
                return self.start_capture(s, p + 1, CAP_UNFINISHED, captures);
            }
            b')' => {
                // End of a capture.
                return self.end_capture(s, p + 1, captures);
            }
            b'$' => {
                if p + 1 == self.pattern.len() {
                    // Anchor at end of pattern: only matches at end of subject.
                    return if s == self.subject.len() { Some(s) } else { None };
                }
                // Otherwise `$` is a literal, fall through to normal handling below.
            }
            b'%' => {
                match self.pattern.get(p + 1) {
                    Some(b'b') => {
                        let end = self.match_balance(s, p)?;
                        return self.match_here(end, p + 4, captures);
                    }
                    Some(b'f') => {
                        // Frontier match: `%f[set]`.
                        if self.pattern.get(p + 2) != Some(&b'[') {
                            return None;
                        }
                        let set_end = self.set_end(p + 2);
                        let prev = if s == 0 { 0 } else { self.subject[s - 1] };
                        let cur = if s < self.subject.len() {
                            self.subject[s]
                        } else {
                            0
                        };
                        let prev_in = if s == 0 {
                            false
                        } else {
                            self.match_bracket_class(prev, p + 2, set_end)
                        };
                        let cur_in = if s < self.subject.len() {
                            self.match_bracket_class(cur, p + 2, set_end)
                        } else {
                            false
                        };
                        if !prev_in && cur_in {
                            return self.match_here(s, set_end + 1, captures);
                        }
                        return None;
                    }
                    Some(&c) if c.is_ascii_digit() => {
                        // Back-reference to a capture.
                        let idx = (c - b'1') as usize;
                        let cap = captures.get(idx)?;
                        if cap.is_open() || cap.is_position() {
                            return None;
                        }
                        let cap_start = cap.start;
                        let cap_end = cap.end as usize;
                        let len = cap_end - cap_start;
                        if s + len > self.subject.len() {
                            return None;
                        }
                        if self.subject[s..s + len] == self.subject[cap_start..cap_end] {
                            return self.match_here(s + len, p + 2, captures);
                        }
                        return None;
                    }
                    _ => {}
                }
            }
            _ => {}
        }

        // Normal class handling with optional quantifier.
        let ep = self.class_end(p);
        let quant = self.pattern.get(ep);
        match quant {
            Some(b'*') => self.max_expand(s, p, ep + 1, captures),
            Some(b'+') => {
                // One or more: must match at least one.
                if self.single_match(s, p) {
                    self.max_expand(s + 1, p, ep + 1, captures)
                } else {
                    None
                }
            }
            Some(b'-') => self.min_expand(s, p, ep + 1, captures),
            Some(b'?') => {
                if self.single_match(s, p) {
                    if let Some(end) = self.match_here(s + 1, ep + 1, captures) {
                        return Some(end);
                    }
                }
                self.match_here(s, ep + 1, captures)
            }
            _ => {
                if self.single_match(s, p) {
                    self.match_here(s + 1, ep, captures)
                } else {
                    None
                }
            }
        }
    }

    fn start_capture(
        &self,
        s: usize,
        p: usize,
        what: isize,
        captures: &mut Vec<Capture>,
    ) -> Option<usize> {
        if captures.len() >= MAX_CAPTURES {
            return None;
        }
        captures.push(Capture { start: s, end: what });
        let res = self.match_here(s, p, captures);
        if res.is_none() {
            captures.pop();
        }
        res
    }

    fn end_capture(&self, s: usize, p: usize, captures: &mut Vec<Capture>) -> Option<usize> {
        // Find the innermost open capture.
        let open = captures.iter().rposition(|c| c.is_open());
        let Some(idx) = open else {
            return None;
        };
        captures[idx].end = s as isize;
        let res = self.match_here(s, p, captures);
        if res.is_none() {
            captures[idx].end = CAP_UNFINISHED;
        }
        res
    }
}

/// Attempt to match `pattern` against `subject` starting at subject byte offset `init`.
///
/// If the pattern begins with `^`, the match is anchored at `init`. Returns the match (end index
/// plus captures) on success.
pub fn match_at(
    subject: &[u8],
    pattern: &[u8],
    init: usize,
) -> Result<Option<Match>, PatternError> {
    validate(pattern)?;
    let matcher = Matcher::new(subject, pattern);

    let anchored = pattern.first() == Some(&b'^');
    let p_start = if anchored { 1 } else { 0 };

    let mut s = init;
    loop {
        let mut captures = Vec::new();
        if let Some(end) = matcher.match_here(s, p_start, &mut captures) {
            return Ok(Some(Match {
                start: s,
                end,
                captures,
            }));
        }
        if anchored || s >= subject.len() {
            return Ok(None);
        }
        s += 1;
    }
}

/// Validate a pattern, returning an error for structurally malformed patterns. This catches the
/// cases PUC-Rio reports as "malformed pattern" / "unbalanced pattern" before matching.
pub fn validate(pattern: &[u8]) -> Result<(), PatternError> {
    let mut depth = 0usize;
    let mut i = 0;
    while i < pattern.len() {
        match pattern[i] {
            b'%' => {
                let next = pattern.get(i + 1).copied();
                match next {
                    None => return Err(PatternError::MalformedPattern),
                    Some(b'b') => {
                        if pattern.get(i + 2).is_none() || pattern.get(i + 3).is_none() {
                            return Err(PatternError::MalformedMatchBalance);
                        }
                        i += 4;
                        continue;
                    }
                    Some(b'f') => {
                        if pattern.get(i + 2) != Some(&b'[') {
                            return Err(PatternError::MissingFrontierSet);
                        }
                        i += 2;
                        continue;
                    }
                    Some(_) => {
                        i += 2;
                        continue;
                    }
                }
            }
            b'[' => {
                let end = Matcher::new(&[], pattern).set_end(i);
                if end >= pattern.len() {
                    return Err(PatternError::MalformedPattern);
                }
                i = end + 1;
                continue;
            }
            b'(' => {
                depth += 1;
                if depth > MAX_CAPTURES {
                    return Err(PatternError::TooManyCaptures);
                }
            }
            b')' => {
                if depth == 0 {
                    return Err(PatternError::UnbalancedPattern);
                }
                depth -= 1;
            }
            _ => {}
        }
        i += 1;
    }
    if depth != 0 {
        return Err(PatternError::UnbalancedPattern);
    }
    Ok(())
}

/// Find the first match of `pattern` in `subject` at or after `init`.
pub fn find(subject: &[u8], pattern: &[u8], init: usize) -> Result<Option<Match>, PatternError> {
    match_at(subject, pattern, init)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m<'a>(subject: &'a str, pattern: &'a str) -> Option<Match> {
        match_at(subject.as_bytes(), pattern.as_bytes(), 0).unwrap()
    }

    #[test]
    fn literal_and_dot() {
        assert_eq!(m("hello", "ell").unwrap().end, 4);
        assert_eq!(m("hello", "h.llo").unwrap().end, 5);
        assert!(m("hello", "xyz").is_none());
    }

    #[test]
    fn anchors() {
        assert_eq!(m("hello", "^hello").unwrap().end, 5);
        assert!(m("hello", "^ello").is_none());
        assert_eq!(m("hello", "hello$").unwrap().end, 5);
        assert!(m("hello", "hell$").is_none());
    }

    #[test]
    fn quantifiers() {
        assert_eq!(m("aaab", "a*").unwrap().end, 3);
        assert_eq!(m("aaab", "a+").unwrap().end, 3);
        assert_eq!(m("aaab", "a-").unwrap().end, 0);
        // `a?b` matches the final `ab` (greedy `a?` takes the `a` just before `b`).
        let r = m("aaab", "a?b").unwrap();
        assert_eq!((r.start, r.end), (2, 4));
        assert_eq!(m("ab", "a?b").unwrap().end, 2);
    }

    #[test]
    fn classes_and_sets() {
        assert_eq!(m("123abc", "%d+").unwrap().end, 3);
        assert_eq!(m("abc123", "%a+").unwrap().end, 3);
        assert_eq!(m("hello", "[aeiou]+").unwrap().end, 2);
        assert_eq!(m("xyz", "[^aeiou]+").unwrap().end, 3);
        assert_eq!(m("a-c", "[a-z-]+").unwrap().end, 3);
    }

    #[test]
    fn captures() {
        let r = m("key=value", "(%w+)=(%w+)").unwrap();
        assert_eq!(r.captures.len(), 2);
        assert_eq!(&"key=value"[r.captures[0].start..r.captures[0].end as usize], "key");
        assert_eq!(&"key=value"[r.captures[1].start..r.captures[1].end as usize], "value");
    }

    #[test]
    fn position_capture() {
        let r = m("hello", "()o").unwrap();
        assert!(r.captures[0].is_position());
        assert_eq!(r.captures[0].position_value(), 5);
    }

    #[test]
    fn balance() {
        assert_eq!(m("(a(b)c)", "%b()").unwrap().end, 7);
    }

    #[test]
    fn frontier() {
        let r = m("hello world", "%f[%a]world").unwrap();
        assert_eq!(r.end, 11);
    }

    #[test]
    fn backreference() {
        assert!(m("ab", "(a)b%1").is_none());
        assert_eq!(m("aa", "(a)%1").unwrap().end, 2);
    }

    #[test]
    fn validation_errors() {
        assert_eq!(validate(b"abc%").unwrap_err(), PatternError::MalformedPattern);
        assert_eq!(validate(b"(abc").unwrap_err(), PatternError::UnbalancedPattern);
        assert_eq!(validate(b"abc)").unwrap_err(), PatternError::UnbalancedPattern);
        assert_eq!(validate(b"%b(").unwrap_err(), PatternError::MalformedMatchBalance);
    }
}
