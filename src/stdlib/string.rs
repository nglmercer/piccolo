use gc_arena::lock::Lock;

use crate::{
    Callback, CallbackReturn, Context, Error, FromValue, IntoValue, String, Table, Value,
};

use super::format::{self, FormatArg, FormatError};
use super::pattern::{self, Capture, Match, PatternError};

pub fn load_string<'gc>(ctx: Context<'gc>) {
    let string = Table::new(&ctx);

    string.set_field(
        ctx,
        "len",
        Callback::from_fn(&ctx, |ctx, _, mut stack| {
            let string = stack.consume::<String>(ctx)?;
            let len = string.len();
            stack.replace(ctx, len);
            Ok(CallbackReturn::Return)
        }),
    );

    string.set_field(
        ctx,
        "byte",
        Callback::from_fn(&ctx, |ctx, _, mut stack| {
            let (string, i, j) = stack.consume::<(String, Option<i64>, Option<i64>)>(ctx)?;
            let i = i.unwrap_or(1);
            let substr = sub(string.as_bytes(), i, j.or(Some(i)))?;
            stack.extend(substr.iter().map(|b| Value::Integer(i64::from(*b))));
            Ok(CallbackReturn::Return)
        }),
    );

    string.set_field(
        ctx,
        "char",
        Callback::from_fn(&ctx, |ctx, _, mut stack| {
            let string = ctx.intern(
                &stack
                    .into_iter()
                    .map(|c| u8::from_value(ctx, c))
                    .collect::<Result<Vec<_>, _>>()?,
            );
            stack.replace(ctx, string);
            Ok(CallbackReturn::Return)
        }),
    );

    string.set_field(
        ctx,
        "sub",
        Callback::from_fn(&ctx, |ctx, _, mut stack| {
            let (string, i, j) = stack.consume::<(String, i64, Option<i64>)>(ctx)?;
            let substr = ctx.intern(sub(string.as_bytes(), i, j)?);
            stack.replace(ctx, substr);
            Ok(CallbackReturn::Return)
        }),
    );

    string.set_field(
        ctx,
        "lower",
        Callback::from_fn(&ctx, |ctx, _, mut stack| {
            let string = stack.consume::<String>(ctx)?;
            let lowered = ctx.intern(
                &string
                    .as_bytes()
                    .iter()
                    .map(u8::to_ascii_lowercase)
                    .collect::<Vec<_>>(),
            );
            stack.replace(ctx, lowered);
            Ok(CallbackReturn::Return)
        }),
    );

    string.set_field(
        ctx,
        "reverse",
        Callback::from_fn(&ctx, |ctx, _, mut stack| {
            let string = stack.consume::<String>(ctx)?;
            let reversed = ctx.intern(&string.as_bytes().iter().copied().rev().collect::<Vec<_>>());
            stack.replace(ctx, reversed);
            Ok(CallbackReturn::Return)
        }),
    );

    string.set_field(
        ctx,
        "upper",
        Callback::from_fn(&ctx, |ctx, _, mut stack| {
            let string = stack.consume::<String>(ctx)?;
            let uppered = ctx.intern(
                &string
                    .as_bytes()
                    .iter()
                    .map(u8::to_ascii_uppercase)
                    .collect::<Vec<_>>(),
            );
            stack.replace(ctx, uppered);
            Ok(CallbackReturn::Return)
        }),
    );

    string.set_field(
        ctx,
        "rep",
        Callback::from_fn(&ctx, |ctx, _, mut stack| {
            let (string, n, sep) = stack.consume::<(String, i64, Option<String>)>(ctx)?;
            let result = if n <= 0 {
                Vec::new()
            } else {
                let n = n as usize;
                let bytes = string.as_bytes();
                match sep {
                    Some(sep) => {
                        let sep = sep.as_bytes();
                        let mut out = Vec::with_capacity(bytes.len() * n + sep.len() * (n - 1));
                        for i in 0..n {
                            if i > 0 {
                                out.extend_from_slice(sep);
                            }
                            out.extend_from_slice(bytes);
                        }
                        out
                    }
                    None => bytes.repeat(n),
                }
            };
            stack.replace(ctx, ctx.intern(&result));
            Ok(CallbackReturn::Return)
        }),
    );

    string.set_field(
        ctx,
        "format",
        Callback::from_fn(&ctx, |ctx, _, mut stack| {
            let fmt = stack.remove(0).unwrap_or_default();
            let fmt = String::from_value(ctx, fmt)?;
            let fmt_bytes = fmt.as_bytes();

            // Remaining stack values are the arguments, consumed in order.
            let mut arg_index = 0usize;
            let args: Vec<Value> = stack.drain(..).collect();
            let mut out = Vec::new();

            let result = format::format_with(fmt_bytes, || {
                let value = args.get(arg_index).copied().unwrap_or(Value::Nil);
                arg_index += 1;
                Ok(value_to_format_arg(value))
            }, &mut out);

            match result {
                Ok(()) => {
                    stack.replace(ctx, ctx.intern(&out));
                    Ok(CallbackReturn::Return)
                }
                Err(e) => Err(format_error(ctx, e)),
            }
        }),
    );

    string.set_field(
        ctx,
        "find",
        Callback::from_fn(&ctx, |ctx, _, mut stack| {
            let (subject, pattern, init, plain) =
                stack.consume::<(String, String, Option<i64>, Option<bool>)>(ctx)?;
            let plain = plain.unwrap_or(false);
            let init = normalize_init(subject.as_bytes().len(), init);

            if plain {
                // Plain substring search, no pattern interpretation.
                let haystack = subject.as_bytes();
                let needle = pattern.as_bytes();
                let found = if needle.is_empty() {
                    Some(init)
                } else {
                    find_subslice(&haystack[init.min(haystack.len())..], needle)
                        .map(|i| i + init.min(haystack.len()))
                };
                stack.clear();
                match found {
                    Some(start) => {
                        stack.extend([
                            Value::Integer(start as i64 + 1),
                            Value::Integer((start + needle.len()) as i64),
                        ]);
                    }
                    None => stack.push_back(Value::Nil),
                }
                return Ok(CallbackReturn::Return);
            }

            match pattern::find(subject.as_bytes(), pattern.as_bytes(), init) {
                Ok(Some(m)) => {
                    stack.clear();
                    stack.extend([
                        Value::Integer(m.start as i64 + 1),
                        Value::Integer(m.end as i64),
                    ]);
                    push_captures(ctx, subject, &m, &mut stack);
                    Ok(CallbackReturn::Return)
                }
                Ok(None) => {
                    stack.replace(ctx, Value::Nil);
                    Ok(CallbackReturn::Return)
                }
                Err(e) => Err(pattern_error(ctx, e)),
            }
        }),
    );

    string.set_field(
        ctx,
        "match",
        Callback::from_fn(&ctx, |ctx, _, mut stack| {
            let (subject, pattern, init) =
                stack.consume::<(String, String, Option<i64>)>(ctx)?;
            let init = normalize_init(subject.as_bytes().len(), init);
            match pattern::find(subject.as_bytes(), pattern.as_bytes(), init) {
                Ok(Some(m)) => {
                    stack.clear();
                    push_match_values(ctx, subject, &m, &mut stack);
                    Ok(CallbackReturn::Return)
                }
                Ok(None) => {
                    stack.replace(ctx, Value::Nil);
                    Ok(CallbackReturn::Return)
                }
                Err(e) => Err(pattern_error(ctx, e)),
            }
        }),
    );

    string.set_field(
        ctx,
        "gmatch",
        Callback::from_fn(&ctx, |ctx, _, mut stack| {
            let (subject, pattern) = stack.consume::<(String, String)>(ctx)?;
            // Validate the pattern up front so errors surface at gmatch() call time.
            if let Err(e) = pattern::validate(pattern.as_bytes()) {
                return Err(pattern_error(ctx, e));
            }
            let state = GMatchState::new(&ctx, subject, pattern);
            let iter = Callback::from_fn_with(&ctx, state, gmatch_iter);
            stack.replace(ctx, iter);
            Ok(CallbackReturn::Return)
        }),
    );

    string.set_field(
        ctx,
        "gsub",
        Callback::from_fn(&ctx, |ctx, _, mut stack| {
            // gsub may need to call a function replacement, so we handle the simple
            // (string / table) replacements here and defer function replacement to a
            // sequence. For simplicity and correctness we implement string and table
            // replacements synchronously, and function replacements via a callback call
            // loop is complex; we support string and table replacements fully and
            // function replacement through a sequence.
            let subject = stack.get(0);
            let subject = String::from_value(ctx, subject)?;
            let pattern = stack.get(1);
            let pattern = String::from_value(ctx, pattern)?;
            let repl = stack.get(2);
            let max_arg = stack.get(3);
            let max = if max_arg.is_nil() {
                usize::MAX
            } else {
                let n = i64::from_value(ctx, max_arg)?;
                if n < 0 {
                    0
                } else {
                    n as usize
                }
            };

            if let Err(e) = pattern::validate(pattern.as_bytes()) {
                return Err(pattern_error(ctx, e));
            }

            // Function replacement requires async calls; handle via a sequence.
            if matches!(repl, Value::Function(_)) {
                let seq = GSub::new(subject, pattern, repl, max);
                return Ok(CallbackReturn::Sequence(crate::BoxSequence::new(&ctx, seq)));
            }

            let result = gsub_static(ctx, subject, pattern, repl, max)?;
            stack.replace(ctx, (ctx.intern(&result.0), result.1 as i64));
            Ok(CallbackReturn::Return)
        }),
    );

    ctx.set_global("string", string);
}

/// Convert a Lua value into a format argument, choosing integer vs number representation.
fn value_to_format_arg<'gc>(value: Value<'gc>) -> FormatArg {
    match value {
        Value::Integer(i) => FormatArg::Integer(i),
        Value::Number(n) => FormatArg::Number(n),
        Value::String(s) => FormatArg::String(s.as_bytes() as *const [u8]),
        // Booleans and others: coerce to integer 0/1 for numeric conversions.
        Value::Boolean(b) => FormatArg::Integer(if b { 1 } else { 0 }),
        _ => FormatArg::Integer(0),
    }
}

fn format_error<'gc>(ctx: Context<'gc>, e: FormatError) -> Error<'gc> {
    ctx.intern(format!("bad argument to 'format' ({e})").as_bytes())
        .into_value(ctx)
        .into()
}

fn pattern_error<'gc>(ctx: Context<'gc>, e: PatternError) -> Error<'gc> {
    ctx.intern(format!("bad argument to string function ({e})").as_bytes())
        .into_value(ctx)
        .into()
}

/// Normalize a 1-based (possibly negative) init argument into a 0-based byte offset clamped to the
/// subject length.
fn normalize_init(len: usize, init: Option<i64>) -> usize {
    let init = init.unwrap_or(1);
    if init > 0 {
        (init as usize).saturating_sub(1).min(len)
    } else if init < 0 {
        len.saturating_sub(init.unsigned_abs() as usize)
    } else {
        0
    }
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Push the capture values for a match onto the stack. If there are no captures, pushes the whole
/// match. Position captures push a 1-based integer; range captures push the substring.
fn push_captures<'gc>(
    ctx: Context<'gc>,
    subject: String<'gc>,
    m: &Match,
    stack: &mut crate::Stack<'gc, '_>,
) {
    if m.captures.is_empty() {
        stack.push_back(ctx.intern(&subject.as_bytes()[m.start..m.end]).into());
    } else {
        for cap in &m.captures {
            push_capture_value(ctx, subject, cap, stack);
        }
    }
}

/// Push the "match values" used by `string.match` and `gsub`/`gmatch` replacements: the captures
/// if any, otherwise the whole match.
fn push_match_values<'gc>(
    ctx: Context<'gc>,
    subject: String<'gc>,
    m: &Match,
    stack: &mut crate::Stack<'gc, '_>,
) {
    if m.captures.is_empty() {
        stack.push_back(ctx.intern(&subject.as_bytes()[m.start..m.end]).into());
    } else {
        for cap in &m.captures {
            push_capture_value(ctx, subject, cap, stack);
        }
    }
}

fn push_capture_value<'gc>(
    ctx: Context<'gc>,
    subject: String<'gc>,
    cap: &Capture,
    stack: &mut crate::Stack<'gc, '_>,
) {
    if cap.is_position() {
        stack.push_back(Value::Integer(cap.position_value() as i64));
    } else if cap.is_open() {
        // Unfinished capture: treat as empty (should not happen after a full match).
        stack.push_back(ctx.intern(b"").into());
    } else {
        let start = cap.start;
        let end = (cap.end as usize).min(subject.as_bytes().len());
        stack.push_back(ctx.intern(&subject.as_bytes()[start..end]).into());
    }
}

/// The captured state for a `gmatch` iterator.
#[derive(gc_arena::Collect)]
#[collect(no_drop)]
struct GMatchState<'gc> {
    subject: String<'gc>,
    pattern: String<'gc>,
    pos: gc_arena::Gc<'gc, Lock<usize>>,
}

impl<'gc> GMatchState<'gc> {
    fn new(ctx: &Context<'gc>, subject: String<'gc>, pattern: String<'gc>) -> Self {
        GMatchState {
            subject,
            pattern,
            pos: gc_arena::Gc::new(ctx, Lock::new(0)),
        }
    }
}

fn gmatch_iter<'gc>(
    state: &GMatchState<'gc>,
    ctx: Context<'gc>,
    _exec: crate::Execution<'gc, '_>,
    mut stack: crate::Stack<'gc, '_>,
) -> Result<CallbackReturn<'gc>, Error<'gc>> {
    let subject = state.subject;
    let pattern = state.pattern;
    let pos = state.pos.get();
    let len = subject.as_bytes().len();

    loop {
        if pos > len {
            stack.replace(ctx, Value::Nil);
            return Ok(CallbackReturn::Return);
        }
        match pattern::find(subject.as_bytes(), pattern.as_bytes(), pos) {
            Ok(Some(m)) => {
                // Advance position past this match (at least one byte to avoid infinite loops on
                // empty matches).
                let next = if m.end == m.start { m.end + 1 } else { m.end };
                state.pos.set(&ctx, next);
                stack.clear();
                push_match_values(ctx, subject, &m, &mut stack);
                return Ok(CallbackReturn::Return);
            }
            Ok(None) => {
                stack.replace(ctx, Value::Nil);
                return Ok(CallbackReturn::Return);
            }
            Err(e) => return Err(pattern_error(ctx, e)),
        }
    }
}

/// Perform a `gsub` with a string or table replacement (no function calls required).
fn gsub_static<'gc>(
    ctx: Context<'gc>,
    subject: String<'gc>,
    pattern: String<'gc>,
    repl: Value<'gc>,
    max: usize,
) -> Result<(Vec<u8>, usize), Error<'gc>> {
    let subject_bytes = subject.as_bytes();
    let pattern_bytes = pattern.as_bytes();
    let mut out = Vec::new();
    let mut pos = 0usize;
    let mut count = 0usize;
    let len = subject_bytes.len();

    while pos <= len && count < max {
        match pattern::find(subject_bytes, pattern_bytes, pos)? {
            Some(m) => {
                out.extend_from_slice(&subject_bytes[pos..m.start]);
                let replacement = gsub_replacement_static(ctx, subject, repl, &m)?;
                match replacement {
                    Some(bytes) => out.extend_from_slice(&bytes),
                    None => out.extend_from_slice(&subject_bytes[m.start..m.end]),
                }
                count += 1;
                pos = if m.end == m.start { m.end + 1 } else { m.end };
            }
            None => break,
        }
    }
    out.extend_from_slice(&subject_bytes[pos.min(len)..]);
    Ok((out, count))
}

/// Compute the replacement for a single match in the static (non-function) case. Returns `None` if
/// the replacement is "falsy" (nil/false), meaning keep the original match.
fn gsub_replacement_static<'gc>(
    ctx: Context<'gc>,
    subject: String<'gc>,
    repl: Value<'gc>,
    m: &Match,
) -> Result<Option<Vec<u8>>, Error<'gc>> {
    match repl {
        Value::String(s) => {
            // Handle `%0`..`%9` and `%%` escapes in the replacement string.
            Ok(Some(expand_replacement(s.as_bytes(), subject, m)))
        }
        Value::Table(t) => {
            let key = first_capture_value(ctx, subject, m);
            let v = t.get_value(ctx, key);
            Ok(replacement_from_value(ctx, v, subject, m))
        }
        Value::Nil | Value::Boolean(false) => Ok(None),
        Value::Boolean(true) => Ok(Some(expand_replacement(b"%0", subject, m))),
        other => {
            // Numbers and other values: use their string form.
            let v = replacement_from_value(ctx, other, subject, m);
            Ok(v)
        }
    }
}

fn replacement_from_value<'gc>(
    ctx: Context<'gc>,
    v: Value<'gc>,
    subject: String<'gc>,
    m: &Match,
) -> Option<Vec<u8>> {
    match v {
        Value::Nil | Value::Boolean(false) => None,
        Value::String(s) => Some(expand_replacement(s.as_bytes(), subject, m)),
        Value::Boolean(true) => Some(subject.as_bytes()[m.start..m.end].to_vec()),
        Value::Integer(i) => Some(i.to_string().into_bytes()),
        Value::Number(n) => Some(format_number(n).into_bytes()),
        other => Some(
            ctx.intern(format!("{}", other.display()).as_bytes())
                .as_bytes()
                .to_vec(),
        ),
    }
}

fn format_number(n: f64) -> std::string::String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

/// The value used as a table key / function argument for a match: the first capture if present,
/// otherwise the whole match.
fn first_capture_value<'gc>(ctx: Context<'gc>, subject: String<'gc>, m: &Match) -> Value<'gc> {
    if let Some(cap) = m.captures.first() {
        if cap.is_position() {
            Value::Integer(cap.position_value() as i64)
        } else {
            let start = cap.start;
            let end = (cap.end as usize).min(subject.as_bytes().len());
            ctx.intern(&subject.as_bytes()[start..end]).into()
        }
    } else {
        ctx.intern(&subject.as_bytes()[m.start..m.end]).into()
    }
}

/// Expand `%d` and `%%` escapes in a replacement string.
fn expand_replacement(repl: &[u8], subject: String, m: &Match) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < repl.len() {
        let c = repl[i];
        if c == b'%' && i + 1 < repl.len() {
            let next = repl[i + 1];
            if next == b'%' {
                out.push(b'%');
                i += 2;
                continue;
            } else if next.is_ascii_digit() {
                let idx = (next - b'0') as usize;
                append_capture(&mut out, subject, m, idx);
                i += 2;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

fn append_capture(out: &mut Vec<u8>, subject: String, m: &Match, idx: usize) {
    let bytes = subject.as_bytes();
    if idx == 0 {
        out.extend_from_slice(&bytes[m.start..m.end]);
    } else if let Some(cap) = m.captures.get(idx - 1) {
        if cap.is_position() {
            out.extend_from_slice(cap.position_value().to_string().as_bytes());
        } else if !cap.is_open() {
            let start = cap.start;
            let end = (cap.end as usize).min(bytes.len());
            out.extend_from_slice(&bytes[start..end]);
        }
    }
}

// ---- gsub with function replacement (async sequence) ----

use std::pin::Pin;

use crate::{Execution, Sequence, SequencePoll, Stack};

#[derive(gc_arena::Collect)]
#[collect(no_drop)]
struct GSub<'gc> {
    subject: String<'gc>,
    pattern: String<'gc>,
    repl: Value<'gc>,
    #[collect(require_static)]
    max: usize,
    #[collect(require_static)]
    phase: GSubPhase,
    #[collect(require_static)]
    out: Vec<u8>,
    #[collect(require_static)]
    pos: usize,
    #[collect(require_static)]
    count: usize,
    // The byte range of the match currently awaiting a function replacement.
    #[collect(require_static)]
    current: Option<(usize, usize)>,
}

#[derive(Clone, Copy, gc_arena::Collect)]
#[collect(require_static)]
enum GSubPhase {
    FindMatch,
    AwaitFunction,
    Done,
}

impl<'gc> GSub<'gc> {
    fn new(
        subject: String<'gc>,
        pattern: String<'gc>,
        repl: Value<'gc>,
        max: usize,
    ) -> Self {
        GSub {
            subject,
            pattern,
            repl,
            max,
            phase: GSubPhase::FindMatch,
            out: Vec::new(),
            pos: 0,
            count: 0,
            current: None,
        }
    }
}

impl<'gc> Sequence<'gc> for GSub<'gc> {
    fn poll(
        mut self: Pin<&mut Self>,
        ctx: Context<'gc>,
        _exec: Execution<'gc, '_>,
        mut stack: Stack<'gc, '_>,
    ) -> Result<SequencePoll<'gc>, Error<'gc>> {
        loop {
            match self.phase {
                GSubPhase::FindMatch => {
                    let pos = self.pos;
                    let count = self.count;
                    let len = self.subject.as_bytes().len();
                    if pos > len || count >= self.max {
                        let subject_bytes = self.subject.as_bytes();
                        self.out.extend_from_slice(&subject_bytes[pos.min(len)..]);
                        self.phase = GSubPhase::Done;
                        continue;
                    }

                    let found = pattern::find(
                        self.subject.as_bytes(),
                        self.pattern.as_bytes(),
                        pos,
                    )?;
                    match found {
                        Some(m) => {
                            let subject_bytes = self.subject.as_bytes();
                            self.out.extend_from_slice(&subject_bytes[pos..m.start]);
                            self.current = Some((m.start, m.end));

                            // Call the replacement function with the captures.
                            stack.clear();
                            push_match_values(ctx, self.subject, &m, &mut stack);
                            self.phase = GSubPhase::AwaitFunction;
                            return Ok(SequencePoll::Call {
                                bottom: 0,
                                function: crate::meta_ops::call(ctx, self.repl)?,
                            });
                        }
                        None => {
                            let subject_bytes = self.subject.as_bytes();
                            self.out.extend_from_slice(&subject_bytes[pos.min(len)..]);
                            self.phase = GSubPhase::Done;
                            continue;
                        }
                    }
                }
                GSubPhase::AwaitFunction => {
                    // The function's first return value is at the bottom of the stack.
                    let ret = stack.get(0);
                    let (m_start, m_end) = self.current.unwrap();
                    let m = Match {
                        start: m_start,
                        end: m_end,
                        captures: Vec::new(),
                    };
                    let replacement = replacement_from_value(ctx, ret, self.subject, &m);
                    let original = self.subject.as_bytes()[m_start..m_end].to_vec();
                    match replacement {
                        Some(bytes) => self.out.extend_from_slice(&bytes),
                        None => self.out.extend_from_slice(&original),
                    }

                    self.count += 1;
                    let next = if m_end == m_start { m_end + 1 } else { m_end };
                    self.pos = next;
                    self.phase = GSubPhase::FindMatch;
                    stack.clear();
                    continue;
                }
                GSubPhase::Done => {
                    let out = std::mem::take(&mut self.out);
                    let count = self.count;
                    stack.replace(ctx, (ctx.intern(&out), count as i64));
                    return Ok(SequencePoll::Return);
                }
            }
        }
    }
}

fn sub(string: &[u8], i: i64, j: Option<i64>) -> Result<&[u8], std::num::TryFromIntError> {
    let i = match i {
        i if i > 0 => i.saturating_sub(1).try_into()?,
        0 => 0,
        i => string.len().saturating_sub(i.unsigned_abs().try_into()?),
    };
    let j = if let Some(j) = j {
        if j >= 0 {
            j.try_into()?
        } else {
            let j: usize = j.unsigned_abs().try_into()?;
            string.len().saturating_sub(j.saturating_sub(1))
        }
    } else {
        string.len()
    }
    .clamp(0, string.len());

    Ok(if i >= j || i >= string.len() {
        &[]
    } else {
        &string[i..j]
    })
}
