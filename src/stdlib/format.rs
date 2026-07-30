//! A pure-Rust implementation of Lua's `string.format` conversion logic.
//!
//! This implements the subset of `printf`-style formatting that Lua supports: the conversion
//! specifiers `%c %d %i %o %u %x %X %e %E %f %g %G %s %q %%` together with the flags
//! `- + (space) # 0`, a field width, and a precision. (The exotic hex-float `%a`/`%A`
//! conversions are not supported.)

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum FormatError {
    /// A conversion specifier was not recognized.
    InvalidSpecifier(u8),
    /// The format string ended in a dangling `%`.
    DanglingPercent,
    /// A `%q` was given a value that cannot be quoted.
    BadQuotedValue,
    /// A numeric conversion was given a value that does not fit.
    ValueOutOfRange,
}

impl std::fmt::Display for FormatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FormatError::InvalidSpecifier(c) => {
                write!(f, "invalid conversion specifier '%{}'", *c as char)
            }
            FormatError::DanglingPercent => write!(f, "invalid conversion '%' at end of format"),
            FormatError::BadQuotedValue => write!(f, "value cannot be quoted with '%q'"),
            FormatError::ValueOutOfRange => write!(f, "value out of range for conversion"),
        }
    }
}

impl std::error::Error for FormatError {}

/// The value supplied for a single conversion.
#[derive(Clone, Copy)]
pub enum FormatArg {
    Integer(i64),
    Number(f64),
    String(*const [u8]),
}

impl FormatArg {
    pub fn string_bytes(self) -> Option<&'static [u8]> {
        match self {
            FormatArg::String(p) => Some(unsafe { &*p }),
            _ => None,
        }
    }
}

struct Spec {
    minus: bool,
    plus: bool,
    space: bool,
    hash: bool,
    zero: bool,
    width: Option<usize>,
    precision: Option<usize>,
    specifier: u8,
}

/// Parse a single conversion starting at `i` (just after the `%`). Returns the parsed spec and the
/// index just past the specifier character.
fn parse_spec(fmt: &[u8], mut i: usize) -> Result<(Spec, usize), FormatError> {
    let mut spec = Spec {
        minus: false,
        plus: false,
        space: false,
        hash: false,
        zero: false,
        width: None,
        precision: None,
        specifier: 0,
    };

    // Flags.
    loop {
        match fmt.get(i) {
            Some(b'-') => spec.minus = true,
            Some(b'+') => spec.plus = true,
            Some(b' ') => spec.space = true,
            Some(b'#') => spec.hash = true,
            Some(b'0') => spec.zero = true,
            _ => break,
        }
        i += 1;
    }

    // Width.
    let mut width = 0usize;
    let mut has_width = false;
    while let Some(c) = fmt.get(i) {
        if c.is_ascii_digit() {
            width = width.saturating_mul(10).saturating_add((c - b'0') as usize);
            has_width = true;
            i += 1;
        } else {
            break;
        }
    }
    if has_width {
        spec.width = Some(width);
    }

    // Precision.
    if fmt.get(i) == Some(&b'.') {
        i += 1;
        let mut prec = 0usize;
        while let Some(c) = fmt.get(i) {
            if c.is_ascii_digit() {
                prec = prec.saturating_mul(10).saturating_add((c - b'0') as usize);
                i += 1;
            } else {
                break;
            }
        }
        spec.precision = Some(prec);
    }

    let specifier = *fmt.get(i).ok_or(FormatError::DanglingPercent)?;
    spec.specifier = specifier;
    Ok((spec, i + 1))
}

fn is_int_specifier(c: u8) -> bool {
    matches!(c, b'c' | b'd' | b'i' | b'o' | b'u' | b'x' | b'X')
}

fn is_float_specifier(c: u8) -> bool {
    matches!(c, b'e' | b'E' | b'f' | b'g' | b'G')
}

/// Apply width, alignment, and zero/space padding to an already-produced body string.
fn apply_width(body: &str, spec: &Spec) -> String {
    let width = spec.width.unwrap_or(0);
    if body.len() >= width {
        return body.to_string();
    }
    let pad = width - body.len();
    if spec.minus {
        let mut s = body.to_string();
        s.extend(std::iter::repeat(' ').take(pad));
        s
    } else {
        let mut s = String::new();
        s.extend(std::iter::repeat(' ').take(pad));
        s.push_str(body);
        s
    }
}

fn format_integer(value: i64, spec: &Spec) -> Result<String, FormatError> {
    // Render the magnitude in the requested base.
    let negative = value < 0 && spec.specifier != b'u';
    let magnitude: u64 = if spec.specifier == b'u' {
        value as u64
    } else if negative {
        (value as i128).unsigned_abs() as u64
    } else {
        value as u64
    };

    let mut digits = match spec.specifier {
        b'd' | b'i' | b'u' => format!("{magnitude}"),
        b'o' => format!("{magnitude:o}"),
        b'x' => format!("{magnitude:x}"),
        b'X' => format!("{magnitude:X}"),
        b'c' => {
            // Character conversion: interpret as a byte.
            let c = (value as u32) as u8 as char;
            return Ok(apply_width(&c.to_string(), spec));
        }
        _ => return Err(FormatError::InvalidSpecifier(spec.specifier)),
    };

    // Precision for integer conversions sets a minimum number of digits.
    if let Some(prec) = spec.precision {
        if digits.len() < prec {
            let mut padded = "0".repeat(prec - digits.len());
            padded.push_str(&digits);
            digits = padded;
        }
        // An explicit precision disables zero-padding.
    }

    // Alternate form prefix.
    let prefix = if spec.hash && magnitude != 0 {
        match spec.specifier {
            b'o' => {
                if !digits.starts_with('0') {
                    "0"
                } else {
                    ""
                }
            }
            b'x' => "0x",
            b'X' => "0X",
            _ => "",
        }
    } else {
        ""
    };

    let sign = if negative {
        "-"
    } else if spec.plus {
        "+"
    } else if spec.space {
        " "
    } else {
        ""
    };

    let body_len = sign.len() + prefix.len() + digits.len();
    let width = spec.width.unwrap_or(0);

    // Zero padding: only when not left-aligned, no explicit precision, and width exceeds body.
    let use_zero_pad = spec.zero && spec.precision.is_none() && !spec.minus && body_len < width;

    let mut out = String::new();
    out.push_str(sign);
    out.push_str(prefix);
    if use_zero_pad {
        let pad = width - body_len;
        out.extend(std::iter::repeat('0').take(pad));
    }
    out.push_str(&digits);

    if use_zero_pad {
        Ok(out)
    } else {
        Ok(apply_width(&out, spec))
    }
}

fn format_float(value: f64, spec: &Spec) -> Result<String, FormatError> {
    let precision = spec.precision.unwrap_or(6);

    // Produce the core numeric text (without sign handling beyond what Rust gives us).
    let core = match spec.specifier {
        b'f' => format!("{value:.precision$}"),
        b'e' => format!("{value:.precision$e}"),
        b'E' => format!("{value:.precision$e}").to_uppercase(),
        b'g' | b'G' => format_g(value, spec, precision),
        _ => return Err(FormatError::InvalidSpecifier(spec.specifier)),
    };

    // Rust already includes a `-` for negatives; add an explicit sign for non-negatives.
    let body = if !core.starts_with('-') && !core.starts_with("nan") && !core.starts_with("inf") {
        if spec.plus {
            format!("+{core}")
        } else if spec.space {
            format!(" {core}")
        } else {
            core
        }
    } else {
        core
    };

    // Zero padding for floats (only when not left-aligned).
    let width = spec.width.unwrap_or(0);
    if spec.zero && !spec.minus && body.len() < width {
        let pad = width - body.len();
        let (sign, rest) = if body.starts_with('-')
            || body.starts_with('+')
            || body.starts_with(' ')
        {
            (&body[..1], &body[1..])
        } else {
            ("", body.as_str())
        };
        let mut out = String::new();
        out.push_str(sign);
        out.extend(std::iter::repeat('0').take(pad));
        out.push_str(rest);
        return Ok(out);
    }

    Ok(apply_width(&body, spec))
}

/// `%g`/`%G`: shortest of `%e` and `%f`, with trailing zeros removed.
fn format_g(value: f64, spec: &Spec, precision: usize) -> String {
    let prec = if precision == 0 { 1 } else { precision };
    // Determine the decimal exponent.
    let e_form = format!("{value:.prec$e}", prec = prec.saturating_sub(1));
    let exp = parse_exponent(&e_form);
    let upper = spec.specifier == b'G';

    let result = if exp < -4 || exp >= prec as i32 {
        let mut s = format!("{value:.prec$e}", prec = prec.saturating_sub(1));
        strip_trailing_zeros_g(&mut s);
        if upper {
            s.to_uppercase()
        } else {
            s
        }
    } else {
        let digits_after = prec as isize - 1 - exp as isize;
        let digits_after = digits_after.max(0) as usize;
        let mut s = format!("{value:.digits_after$}");
        strip_trailing_zeros_g(&mut s);
        s
    };

    if spec.hash {
        result
    } else {
        result
    }
}

fn parse_exponent(e_form: &str) -> i32 {
    // e_form looks like `1.2345e+03` or `-1.2e-05`.
    if let Some(pos) = e_form.find('e') {
        e_form[pos + 1..].parse::<i32>().unwrap_or(0)
    } else {
        0
    }
}

fn strip_trailing_zeros_g(s: &mut String) {
    // Remove trailing zeros in the fractional part, but keep the exponent intact.
    let exp_pos = s.find('e');
    let (mantissa, exponent) = match exp_pos {
        Some(p) => (&s[..p], Some(s[p..].to_string())),
        None => (s.as_str(), None),
    };
    let mut mantissa = mantissa.to_string();
    if mantissa.contains('.') {
        while mantissa.ends_with('0') {
            mantissa.pop();
        }
        if mantissa.ends_with('.') {
            mantissa.pop();
        }
    }
    *s = match exponent {
        Some(e) => format!("{mantissa}{e}"),
        None => mantissa,
    };
}

/// Quote a byte string the way Lua's `%q` does: escapes backslashes, quotes, newlines, and
/// non-printable bytes.
fn quote_string(bytes: &[u8]) -> String {
    let mut out = String::from("\"");
    for &b in bytes {
        match b {
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            0 => out.push_str("\\0"),
            0x1a => out.push_str("\\26"),
            0x20..=0x7e => out.push(b as char),
            _ => {
                out.push_str(&format!("\\{b:03}"));
            }
        }
    }
    out.push('"');
    out
}

/// Format a single conversion. `arg` is the value; `spec` is the parsed specifier.
fn format_one(arg: FormatArg, spec: &Spec) -> Result<String, FormatError> {
    match spec.specifier {
        b'%' => Ok("%".to_string()),
        b's' => {
            let bytes = arg.string_bytes().ok_or(FormatError::InvalidSpecifier(b's'))?;
            let text = String::from_utf8_lossy(bytes).into_owned();
            let text = if let Some(prec) = spec.precision {
                // Precision truncates by bytes.
                let end = prec.min(bytes.len());
                String::from_utf8_lossy(&bytes[..end]).into_owned()
            } else {
                text
            };
            Ok(apply_width(&text, spec))
        }
        b'q' => {
            let bytes = arg.string_bytes().ok_or(FormatError::BadQuotedValue)?;
            Ok(quote_string(bytes))
        }
        c if is_int_specifier(c) => {
            let value = match arg {
                FormatArg::Integer(i) => i,
                FormatArg::Number(n) => n as i64,
                FormatArg::String(_) => return Err(FormatError::InvalidSpecifier(c)),
            };
            format_integer(value, spec)
        }
        c if is_float_specifier(c) => {
            let value = match arg {
                FormatArg::Number(n) => n,
                FormatArg::Integer(i) => i as f64,
                FormatArg::String(_) => return Err(FormatError::InvalidSpecifier(c)),
            };
            format_float(value, spec)
        }
        c => Err(FormatError::InvalidSpecifier(c)),
    }
}

/// Format `fmt` with the given argument provider. `next_arg` is called for each conversion that
/// consumes an argument (all except `%%`).
pub fn format_with(
    fmt: &[u8],
    mut next_arg: impl FnMut() -> Result<FormatArg, FormatError>,
    out: &mut Vec<u8>,
) -> Result<(), FormatError> {
    let mut i = 0;
    while i < fmt.len() {
        let c = fmt[i];
        if c != b'%' {
            out.push(c);
            i += 1;
            continue;
        }
        // Found a `%`.
        let (spec, next) = parse_spec(fmt, i + 1)?;
        if spec.specifier == b'%' {
            out.push(b'%');
        } else {
            let arg = next_arg()?;
            let rendered = format_one(arg, &spec)?;
            out.extend_from_slice(rendered.as_bytes());
        }
        i = next;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fmt(fmt: &str, args: &[FormatArg]) -> String {
        let mut out = Vec::new();
        let mut iter = args.iter().copied();
        format_with(fmt.as_bytes(), || iter.next().ok_or(FormatError::ValueOutOfRange), &mut out)
            .unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn integers() {
        assert_eq!(fmt("%d", &[FormatArg::Integer(42)]), "42");
        assert_eq!(fmt("%d", &[FormatArg::Integer(-42)]), "-42");
        assert_eq!(fmt("%5d", &[FormatArg::Integer(42)]), "   42");
        assert_eq!(fmt("%-5d|", &[FormatArg::Integer(42)]), "42   |");
        assert_eq!(fmt("%05d", &[FormatArg::Integer(42)]), "00042");
        assert_eq!(fmt("%+d", &[FormatArg::Integer(42)]), "+42");
        assert_eq!(fmt("%x", &[FormatArg::Integer(255)]), "ff");
        assert_eq!(fmt("%X", &[FormatArg::Integer(255)]), "FF");
        assert_eq!(fmt("%#x", &[FormatArg::Integer(255)]), "0xff");
        assert_eq!(fmt("%o", &[FormatArg::Integer(8)]), "10");
        assert_eq!(fmt("%u", &[FormatArg::Integer(-1)]), "18446744073709551615");
    }

    #[test]
    fn chars() {
        assert_eq!(fmt("%c", &[FormatArg::Integer(65)]), "A");
    }

    #[test]
    fn floats() {
        assert_eq!(fmt("%f", &[FormatArg::Number(3.5)]), "3.500000");
        assert_eq!(fmt("%.2f", &[FormatArg::Number(3.14159)]), "3.14");
        assert_eq!(fmt("%e", &[FormatArg::Number(12345.0)]), "1.234500e4");
        assert_eq!(fmt("%10.2f", &[FormatArg::Number(3.5)]), "      3.50");
    }

    #[test]
    fn strings() {
        let s: &[u8] = b"hello";
        assert_eq!(
            fmt("%s", &[FormatArg::String(s as *const [u8])]),
            "hello"
        );
        assert_eq!(
            fmt("%.3s", &[FormatArg::String(s as *const [u8])]),
            "hel"
        );
        assert_eq!(
            fmt("%7s", &[FormatArg::String(s as *const [u8])]),
            "  hello"
        );
    }

    #[test]
    fn quoted() {
        let s: &[u8] = b"a\"b\nc";
        assert_eq!(
            fmt("%q", &[FormatArg::String(s as *const [u8])]),
            "\"a\\\"b\\nc\""
        );
    }

    #[test]
    fn percent_literal() {
        assert_eq!(fmt("100%%", &[]), "100%");
    }
}
