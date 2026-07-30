use crate::{
    Callback, CallbackReturn, Context, Error, IntoValue, String, Table, Value, Variadic,
};

/// Load the `utf8` library: `char`, `codepoint`, `len`, `offset`, and `charpattern`.
pub fn load_utf8<'gc>(ctx: Context<'gc>) {
    let utf8 = Table::new(&ctx);

    // utf8.charpattern: a pattern matching exactly one UTF-8 codepoint.
    utf8.set_field(
        ctx,
        "charpattern",
        ctx.intern(b"[\0-\x7F\xC2-\xF4][\x80-\xBF]*"),
    );

    utf8.set_field(
        ctx,
        "char",
        Callback::from_fn(&ctx, |ctx, _, mut stack| {
            let codes = stack.consume::<Variadic<Vec<i64>>>(ctx)?;
            let mut buf = Vec::new();
            for code in codes.0 {
                let c = u32::try_from(code)
                    .ok()
                    .and_then(char::from_u32)
                    .ok_or_else(|| -> Error {
                        ctx.intern(b"bad argument to 'char' (value out of range)")
                            .into_value(ctx)
                            .into()
                    })?;
                let mut tmp = [0u8; 4];
                buf.extend_from_slice(c.encode_utf8(&mut tmp).as_bytes());
            }
            stack.replace(ctx, ctx.intern(&buf));
            Ok(CallbackReturn::Return)
        }),
    );

    utf8.set_field(
        ctx,
        "codepoint",
        Callback::from_fn(&ctx, |ctx, _, mut stack| {
            let (s, i, j) = stack.consume::<(String, Option<i64>, Option<i64>)>(ctx)?;
            let bytes = s.as_bytes();
            let len = bytes.len();
            let i = utf8_pos(len, i.unwrap_or(1));
            let j = utf8_pos(len, j.unwrap_or(i as i64));
            let mut idx = i;
            let mut out: Vec<Value> = Vec::new();
            while idx <= j && idx < len {
                let (cp, size) = match decode_utf8_at(bytes, idx) {
                    Ok(v) => v,
                    Err(_) => {
                        return Err(ctx
                            .intern(b"invalid UTF-8 code")
                            .into_value(ctx)
                            .into());
                    }
                };
                out.push(Value::Integer(cp as i64));
                idx += size;
            }
            stack.replace(ctx, Variadic(out));
            Ok(CallbackReturn::Return)
        }),
    );

    utf8.set_field(
        ctx,
        "len",
        Callback::from_fn(&ctx, |ctx, _, mut stack| {
            let (s, i, j) = stack.consume::<(String, Option<i64>, Option<i64>)>(ctx)?;
            let bytes = s.as_bytes();
            let len = bytes.len();
            let i = utf8_pos(len, i.unwrap_or(1));
            let j = utf8_pos(len, j.unwrap_or(-1));
            let mut idx = i;
            let mut count = 0i64;
            while idx <= j && idx < len {
                match decode_utf8_at(bytes, idx) {
                    Ok((_, size)) => {
                        count += 1;
                        idx += size;
                    }
                    Err(_) => {
                        stack.replace(ctx, (Value::Nil, (idx + 1) as i64));
                        return Ok(CallbackReturn::Return);
                    }
                }
            }
            stack.replace(ctx, count);
            Ok(CallbackReturn::Return)
        }),
    );

    utf8.set_field(
        ctx,
        "offset",
        Callback::from_fn(&ctx, |ctx, _, mut stack| {
            let (s, n, i) = stack.consume::<(String, i64, Option<i64>)>(ctx)?;
            let bytes = s.as_bytes();
            let len = bytes.len();
            let default_i = if n >= 0 { 1 } else { len as i64 + 1 };
            let i = i.unwrap_or(default_i);
            // Convert to a 0-based position, PUC-Rio style (1 <= posi-1 <= len allowed).
            let mut posi: i64 = if i > 0 { i - 1 } else { len as i64 + i };
            if !(0..=len as i64).contains(&posi) {
                return Err(ctx
                    .intern(b"bad argument #3 to 'offset' (position out of bounds)")
                    .into_value(ctx)
                    .into());
            }

            if n == 0 {
                // Find the beginning of the current byte sequence.
                while posi > 0 && is_continuation(bytes[posi as usize]) {
                    posi -= 1;
                }
                stack.replace(ctx, posi + 1);
                return Ok(CallbackReturn::Return);
            }

            if posi < len as i64 && is_continuation(bytes[posi as usize]) {
                return Err(ctx
                    .intern(b"bad argument #3 to 'offset' (initial position is a continuation byte)")
                    .into_value(ctx)
                    .into());
            }

            if n < 0 {
                let mut n = n;
                while n < 0 && posi > 0 {
                    posi -= 1;
                    while posi > 0 && is_continuation(bytes[posi as usize]) {
                        posi -= 1;
                    }
                    n += 1;
                }
                if n == 0 {
                    stack.replace(ctx, posi + 1);
                } else {
                    stack.replace(ctx, Value::Nil);
                }
            } else {
                let mut n = n - 1; // do not move for the 1st character
                while n > 0 && posi < len as i64 {
                    posi += 1;
                    while posi < len as i64 && is_continuation(bytes[posi as usize]) {
                        posi += 1;
                    }
                    n -= 1;
                }
                if n == 0 {
                    stack.replace(ctx, posi + 1);
                } else {
                    stack.replace(ctx, Value::Nil);
                }
            }
            Ok(CallbackReturn::Return)
        }),
    );

    ctx.set_global("utf8", utf8);
}

fn is_continuation(b: u8) -> bool {
    (b & 0b1100_0000) == 0b1000_0000
}

fn decode_utf8_at(bytes: &[u8], idx: usize) -> Result<(u32, usize), ()> {
    let b0 = *bytes.get(idx).ok_or(())?;
    if b0 < 0x80 {
        return Ok((b0 as u32, 1));
    }
    let (len, mut cp) = if b0 >> 5 == 0b110 {
        (2, (b0 & 0x1f) as u32)
    } else if b0 >> 4 == 0b1110 {
        (3, (b0 & 0x0f) as u32)
    } else if b0 >> 3 == 0b11110 {
        (4, (b0 & 0x07) as u32)
    } else {
        return Err(());
    };
    if idx + len > bytes.len() {
        return Err(());
    }
    for k in 1..len {
        let b = bytes[idx + k];
        if !is_continuation(b) {
            return Err(());
        }
        cp = (cp << 6) | (b & 0x3f) as u32;
    }
    let min = match len {
        2 => 0x80,
        3 => 0x800,
        4 => 0x10000,
        _ => 0,
    };
    if cp < min || (0xD800..=0xDFFF).contains(&cp) || cp > 0x10FFFF {
        return Err(());
    }
    Ok((cp, len))
}

fn utf8_pos(len: usize, pos: i64) -> usize {
    if pos > 0 {
        (pos as usize).saturating_sub(1).min(len)
    } else if pos < 0 {
        len.saturating_sub(pos.unsigned_abs() as usize).min(len)
    } else {
        0
    }
}
