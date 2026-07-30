use std::cell::RefCell;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::pin::Pin;

use gc_arena::{Collect, Rootable};

use crate::{
    meta_ops::{self, MetaResult},
    BoxSequence, Callback, CallbackReturn, Context, Error, Execution, FromValue, IntoValue,
    Sequence, SequencePoll, Singleton, Stack, String, Table, UserData, Value,
};

pub fn load_io<'gc>(ctx: Context<'gc>) {
    load_print(ctx);

    let io_table = Table::new(&ctx);
    let file_methods = Table::new(&ctx);

    // ---- file methods ----
    file_methods.set_field(ctx, "read", Callback::from_fn(&ctx, file_read));
    file_methods.set_field(ctx, "write", Callback::from_fn(&ctx, file_write));
    file_methods.set_field(ctx, "close", Callback::from_fn(&ctx, file_close));
    file_methods.set_field(ctx, "flush", Callback::from_fn(&ctx, file_flush));
    file_methods.set_field(ctx, "seek", Callback::from_fn(&ctx, file_seek));
    file_methods.set_field(ctx, "lines", Callback::from_fn(&ctx, file_lines));

    // The file metatable: __index = file_methods, __name = "FILE*".
    let meta = ctx.singleton::<Rootable![FileMeta<'_>]>().0;
    meta.set_field(ctx, "__index", file_methods);
    meta.set_field(ctx, "__name", ctx.intern(b"FILE*"));
    meta.set_field(
        ctx,
        "__tostring",
        Callback::from_fn(&ctx, |ctx, _, mut stack| {
            let ud = stack.consume::<UserData>(ctx)?;
            let handle = ud.downcast_static::<FileHandle>().ok();
            let closed = handle
                .map(|h| matches!(*h.kind.borrow(), FileKind::Closed))
                .unwrap_or(true);
            let text = if closed {
                "file (closed)".to_string()
            } else {
                format!("file ({:p})", gc_arena::Gc::as_ptr(ud.into_inner()))
            };
            stack.replace(ctx, ctx.intern(text.as_bytes()));
            Ok(CallbackReturn::Return)
        }),
    );

    // ---- io functions ----
    io_table.set_field(
        ctx,
        "open",
        Callback::from_fn_with(&ctx, meta, |meta, ctx, _, mut stack| {
            let (path, mode) = stack.consume::<(String, Option<String>)>(ctx)?;
            let mode = mode.unwrap_or_else(|| ctx.intern(b"r"));
            let path_str = path.display_lossy().to_string();
            let mode_str = mode.display_lossy().to_string();
            match open_file(&path_str, &mode_str) {
                Ok(file) => {
                    let handle = make_handle(ctx, *meta, FileKind::File(file));
                    stack.replace(ctx, handle);
                }
                Err(e) => {
                    stack.replace(
                        ctx,
                        (Value::Nil, ctx.intern(e.to_string().as_bytes())),
                    );
                }
            }
            Ok(CallbackReturn::Return)
        }),
    );

    io_table.set_field(
        ctx,
        "type",
        Callback::from_fn(&ctx, |ctx, _, mut stack| {
            let value = stack.get(0);
            let result = match value {
                Value::UserData(ud) => match ud.downcast_static::<FileHandle>() {
                    Ok(handle) => {
                        if matches!(*handle.kind.borrow(), FileKind::Closed) {
                            "closed file"
                        } else {
                            "file"
                        }
                    }
                    Err(_) => {
                        stack.replace(ctx, Value::Nil);
                        return Ok(CallbackReturn::Return);
                    }
                },
                _ => {
                    stack.replace(ctx, Value::Nil);
                    return Ok(CallbackReturn::Return);
                }
            };
            stack.replace(ctx, ctx.intern(result.as_bytes()));
            Ok(CallbackReturn::Return)
        }),
    );

    io_table.set_field(
        ctx,
        "write",
        Callback::from_fn(&ctx, |ctx, _, mut stack| {
            let args: Vec<Value> = stack.drain(..).collect();
            let mut stdout = io::stdout();
            for arg in args {
                write_value(&mut stdout, ctx, arg)?;
            }
            stdout.flush()?;
            // io.write returns stdout as a "file"; we return true as a simple success marker.
            stack.replace(ctx, true);
            Ok(CallbackReturn::Return)
        }),
    );

    io_table.set_field(
        ctx,
        "read",
        Callback::from_fn(&ctx, |ctx, _, mut stack| {
            let formats: Vec<Value> = stack.drain(..).collect();
            let formats = if formats.is_empty() {
                vec![ctx.intern(b"*l").into()]
            } else {
                formats
            };
            let mut stdin = io::stdin();
            let mut results = Vec::new();
            for fmt in formats {
                let fmt_str = String::from_value(ctx, fmt)?;
                match read_stdin(ctx, &mut stdin, fmt_str.as_bytes())? {
                    Some(v) => results.push(v),
                    None => {
                        results.push(Value::Nil);
                    }
                }
            }
            stack.replace(ctx, crate::Variadic(results));
            Ok(CallbackReturn::Return)
        }),
    );

    io_table.set_field(
        ctx,
        "tmpfile",
        Callback::from_fn_with(&ctx, meta, |meta, ctx, _, mut stack| {
            let mut dir = std::env::temp_dir();
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            dir.push(format!("piccolo_tmpfile_{nanos:x}"));
            match OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(true)
                .open(&dir)
            {
                Ok(file) => {
                    let handle = make_handle(ctx, *meta, FileKind::File(file));
                    stack.replace(ctx, handle);
                }
                Err(e) => {
                    stack.replace(
                        ctx,
                        (Value::Nil, ctx.intern(e.to_string().as_bytes())),
                    );
                }
            }
            Ok(CallbackReturn::Return)
        }),
    );

    io_table.set_field(
        ctx,
        "lines",
        Callback::from_fn_with(&ctx, meta, |meta, ctx, _, mut stack| {
            // io.lines(filename) opens the file and returns an iterator that closes it at EOF.
            let path = stack.get(0);
            if path.is_nil() {
                return Err(ctx
                    .intern(b"io.lines requires a filename")
                    .into_value(ctx)
                    .into());
            }
            let path = String::from_value(ctx, path)?;
            let path_str = path.display_lossy().to_string();
            match File::open(&path_str) {
                Ok(file) => {
                    let handle = make_handle(ctx, *meta, FileKind::File(file));
                    let iter = Callback::from_fn_with(&ctx, handle, lines_iter);
                    stack.replace(ctx, iter);
                }
                Err(e) => {
                    return Err(ctx
                        .intern(format!("io.lines: {e}").as_bytes())
                        .into_value(ctx)
                        .into());
                }
            }
            Ok(CallbackReturn::Return)
        }),
    );

    io_table.set_field(
        ctx,
        "close",
        Callback::from_fn(&ctx, |ctx, _, mut stack| {
            let ud = stack.consume::<UserData>(ctx)?;
            if let Ok(handle) = ud.downcast_static::<FileHandle>() {
                *handle.kind.borrow_mut() = FileKind::Closed;
                stack.replace(ctx, true);
            } else {
                stack.replace(ctx, Value::Nil);
            }
            Ok(CallbackReturn::Return)
        }),
    );

    ctx.set_global("io", io_table);
}

fn load_print<'gc>(ctx: Context<'gc>) {
    ctx.set_global(
        "print",
        Callback::from_fn(&ctx, |ctx, _, mut stack| {
            #[derive(Collect)]
            #[collect(require_static)]
            struct PrintSeq {
                first: bool,
            }

            impl<'gc> Sequence<'gc> for PrintSeq {
                fn poll(
                    mut self: Pin<&mut Self>,
                    ctx: Context<'gc>,
                    _exec: Execution<'gc, '_>,
                    mut stack: Stack<'gc, '_>,
                ) -> Result<SequencePoll<'gc>, Error<'gc>> {
                    let mut stdout = io::stdout();

                    while let Some(value) = stack.pop_back() {
                        match meta_ops::tostring(ctx, value)? {
                            MetaResult::Value(v) => {
                                if self.first {
                                    self.first = false;
                                } else {
                                    stdout.write_all(b"\t")?;
                                }
                                if let Value::String(s) = v {
                                    stdout.write_all(s.as_bytes())?;
                                } else {
                                    write!(stdout, "{}", v.display())?;
                                }
                            }
                            MetaResult::Call(call) => {
                                let bottom = stack.len();
                                stack.extend(call.args);
                                return Ok(SequencePoll::Call {
                                    function: call.function,
                                    bottom,
                                });
                            }
                        }
                    }

                    stdout.write_all(b"\n")?;
                    stdout.flush()?;
                    Ok(SequencePoll::Return)
                }
            }

            stack[..].reverse();

            Ok(CallbackReturn::Sequence(BoxSequence::new(
                &ctx,
                PrintSeq { first: true },
            )))
        }),
    );
}

// ---- file handle types ----

enum FileKind {
    File(File),
    Closed,
}

struct FileHandle {
    kind: RefCell<FileKind>,
}

#[derive(Copy, Clone, Collect)]
#[collect(no_drop)]
struct FileMeta<'gc>(Table<'gc>);

impl<'gc> Singleton<'gc> for FileMeta<'gc> {
    fn create(ctx: Context<'gc>) -> Self {
        FileMeta(Table::new(&ctx))
    }
}

fn make_handle<'gc>(
    ctx: Context<'gc>,
    meta: Table<'gc>,
    kind: FileKind,
) -> UserData<'gc> {
    let handle = FileHandle {
        kind: RefCell::new(kind),
    };
    let ud = UserData::new_static(&ctx, handle);
    ud.set_metatable(&ctx, Some(meta));
    ud
}

fn open_file(path: &str, mode: &str) -> io::Result<File> {
    let mut opts = OpenOptions::new();
    match mode.trim_end_matches('b') {
        "r" => {
            opts.read(true);
        }
        "w" => {
            opts.write(true).create(true).truncate(true);
        }
        "a" => {
            opts.append(true).create(true);
        }
        "r+" => {
            opts.read(true).write(true);
        }
        "w+" => {
            opts.read(true).write(true).create(true).truncate(true);
        }
        "a+" => {
            opts.read(true).append(true).create(true);
        }
        other => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("invalid mode '{other}'"),
            ));
        }
    }
    opts.open(path)
}

fn write_value<'gc, W: Write>(
    w: &mut W,
    ctx: Context<'gc>,
    value: Value<'gc>,
) -> Result<(), Error<'gc>> {
    match value {
        Value::String(s) => {
            w.write_all(s.as_bytes())?;
        }
        Value::Integer(i) => {
            write!(w, "{i}")?;
        }
        Value::Number(n) => {
            write!(w, "{}", format_number(n))?;
        }
        other => {
            // Use tostring metamethod for other types.
            match meta_ops::tostring(ctx, other)? {
                MetaResult::Value(Value::String(s)) => w.write_all(s.as_bytes())?,
                MetaResult::Value(v) => write!(w, "{}", v.display())?,
                MetaResult::Call(_) => {
                    return Err(ctx
                        .intern(b"cannot write a value with a __tostring metamethod here")
                        .into_value(ctx)
                        .into());
                }
            }
        }
    }
    Ok(())
}

fn format_number(n: f64) -> std::string::String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

fn with_file<'gc, R>(
    ud: UserData<'gc>,
    f: impl FnOnce(&mut File) -> Result<R, Error<'gc>>,
    ctx: Context<'gc>,
) -> Result<R, Error<'gc>> {
    let handle = ud.downcast_static::<FileHandle>().map_err(|_| {
        Error::from(ctx.intern(b"expected a file handle").into_value(ctx))
    })?;
    let mut borrow = handle.kind.borrow_mut();
    match &mut *borrow {
        FileKind::File(file) => f(file),
        FileKind::Closed => Err(ctx
            .intern(b"attempt to use a closed file")
            .into_value(ctx)
            .into()),
    }
}

fn file_read<'gc>(
    ctx: Context<'gc>,
    _exec: Execution<'gc, '_>,
    mut stack: Stack<'gc, '_>,
) -> Result<CallbackReturn<'gc>, Error<'gc>> {
    let ud = stack.get(0);
    let ud = UserData::from_value(ctx, ud)?;
    let formats: Vec<Value> = stack.drain(1..).collect();
    let formats = if formats.is_empty() {
        vec![ctx.intern(b"*l").into()]
    } else {
        formats
    };

    let mut results = Vec::new();
    for fmt in formats {
        let fmt_str = String::from_value(ctx, fmt)?;
        let bytes = fmt_str.as_bytes().to_vec();
        let value = with_file(
            ud,
            |file| read_from_file(ctx, file, &bytes),
            ctx,
        )?;
        results.push(value);
    }
    stack.replace(ctx, crate::Variadic(results));
    Ok(CallbackReturn::Return)
}

fn read_from_file<'gc>(
    ctx: Context<'gc>,
    file: &mut File,
    fmt: &[u8],
) -> Result<Value<'gc>, Error<'gc>> {
    // A numeric format reads that many bytes.
    if let Some(n) = parse_format_number(fmt) {
        let mut buf = vec![0u8; n as usize];
        let mut total = 0;
        while total < buf.len() {
            match file.read(&mut buf[total..]) {
                Ok(0) => break,
                Ok(k) => total += k,
                Err(e) => return Err(io_err(ctx, e)),
            }
        }
        if total == 0 {
            return Ok(Value::Nil);
        }
        buf.truncate(total);
        return Ok(ctx.intern(&buf).into());
    }

    let spec = if fmt.starts_with(b"*") { &fmt[1..] } else { fmt };
    match spec {
        b"l" => match read_line(file, false) {
            Ok(Some(line)) => Ok(ctx.intern(&line).into()),
            Ok(None) => Ok(Value::Nil),
            Err(e) => Err(io_err(ctx, e)),
        },
        b"L" => match read_line(file, true) {
            Ok(Some(line)) => Ok(ctx.intern(&line).into()),
            Ok(None) => Ok(Value::Nil),
            Err(e) => Err(io_err(ctx, e)),
        },
        b"a" => {
            let mut buf = Vec::new();
            file.read_to_end(&mut buf).map_err(|e| io_err(ctx, e))?;
            Ok(ctx.intern(&buf).into())
        }
        b"n" => match read_number(file) {
            Ok(Some(v)) => Ok(v),
            Ok(None) => Ok(Value::Nil),
            Err(e) => Err(io_err(ctx, e)),
        },
        _ => Err(ctx
            .intern(b"invalid read format")
            .into_value(ctx)
            .into()),
    }
}

fn read_stdin<'gc>(
    ctx: Context<'gc>,
    stdin: &mut io::Stdin,
    fmt: &[u8],
) -> Result<Option<Value<'gc>>, Error<'gc>> {
    // A minimal line/whole-input reader for interactive use. Not exercised by the automated tests.
    use std::io::BufRead;
    let spec = if fmt.starts_with(b"*") { &fmt[1..] } else { fmt };
    let mut lock = stdin.lock();
    match spec {
        b"l" | b"L" => {
            let mut line = std::string::String::new();
            let n = lock.read_line(&mut line).map_err(|e| io_err(ctx, e))?;
            if n == 0 {
                return Ok(None);
            }
            let out = if spec == b"l" {
                line.trim_end_matches('\n').to_string()
            } else {
                line
            };
            Ok(Some(ctx.intern(out.as_bytes()).into()))
        }
        b"a" => {
            let mut buf = Vec::new();
            lock.read_to_end(&mut buf).map_err(|e| io_err(ctx, e))?;
            Ok(Some(ctx.intern(&buf).into()))
        }
        _ => Err(ctx
            .intern(b"invalid read format")
            .into_value(ctx)
            .into()),
    }
}

fn parse_format_number(fmt: &[u8]) -> Option<usize> {
    if fmt.is_empty() || !fmt[0].is_ascii_digit() {
        return None;
    }
    let s = std::str::from_utf8(fmt).ok()?;
    s.parse::<usize>().ok()
}

fn read_line(file: &mut File, keep_newline: bool) -> io::Result<Option<Vec<u8>>> {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        match file.read(&mut byte)? {
            0 => {
                if buf.is_empty() {
                    return Ok(None);
                }
                return Ok(Some(buf));
            }
            _ => {
                if byte[0] == b'\n' {
                    if keep_newline {
                        buf.push(b'\n');
                    }
                    return Ok(Some(buf));
                }
                buf.push(byte[0]);
            }
        }
    }
}

fn read_number<'gc>(file: &mut File) -> io::Result<Option<Value<'gc>>> {
    // Skip whitespace.
    let mut byte = [0u8; 1];
    let mut text = Vec::new();
    loop {
        match file.read(&mut byte)? {
            0 => break,
            _ => {
                if byte[0].is_ascii_whitespace() && text.is_empty() {
                    continue;
                }
                if is_number_byte(byte[0], &text) {
                    text.push(byte[0]);
                } else {
                    // Seek back one byte so the next read sees this delimiter.
                    file.seek(SeekFrom::Current(-1))?;
                    break;
                }
            }
        }
    }
    if text.is_empty() {
        return Ok(None);
    }
    let s = std::str::from_utf8(&text).unwrap_or("");
    if let Ok(i) = s.parse::<i64>() {
        return Ok(Some(Value::Integer(i)));
    }
    if let Ok(f) = s.parse::<f64>() {
        return Ok(Some(Value::Number(f)));
    }
    Ok(None)
}

fn is_number_byte(b: u8, so_far: &[u8]) -> bool {
    b.is_ascii_digit()
        || b == b'-' && so_far.is_empty()
        || b == b'+' && so_far.is_empty()
        || b == b'.' && !so_far.contains(&b'.')
        || (b == b'e' || b == b'E')
            && !so_far.is_empty()
            && !so_far.contains(&b'e')
            && !so_far.contains(&b'E')
}

fn file_write<'gc>(
    ctx: Context<'gc>,
    _exec: Execution<'gc, '_>,
    mut stack: Stack<'gc, '_>,
) -> Result<CallbackReturn<'gc>, Error<'gc>> {
    let ud = stack.get(0);
    let ud = UserData::from_value(ctx, ud)?;
    let args: Vec<Value> = stack.drain(1..).collect();

    with_file(
        ud,
        |file| {
            for arg in &args {
                write_value(file, ctx, *arg)?;
            }
            Ok::<(), Error>(())
        },
        ctx,
    )?;

    // file:write returns the file handle on success.
    stack.replace(ctx, ud);
    Ok(CallbackReturn::Return)
}

fn file_close<'gc>(
    ctx: Context<'gc>,
    _exec: Execution<'gc, '_>,
    mut stack: Stack<'gc, '_>,
) -> Result<CallbackReturn<'gc>, Error<'gc>> {
    let ud = stack.consume::<UserData>(ctx)?;
    let handle = ud.downcast_static::<FileHandle>().map_err(|_| {
        Error::from(ctx.intern(b"expected a file handle").into_value(ctx))
    })?;
    *handle.kind.borrow_mut() = FileKind::Closed;
    stack.replace(ctx, true);
    Ok(CallbackReturn::Return)
}

fn file_flush<'gc>(
    ctx: Context<'gc>,
    _exec: Execution<'gc, '_>,
    mut stack: Stack<'gc, '_>,
) -> Result<CallbackReturn<'gc>, Error<'gc>> {
    let ud = stack.get(0);
    let ud = UserData::from_value(ctx, ud)?;
    with_file(
        ud,
        |file| {
            file.flush()?;
            Ok::<(), Error>(())
        },
        ctx,
    )?;
    stack.replace(ctx, true);
    Ok(CallbackReturn::Return)
}

fn file_seek<'gc>(
    ctx: Context<'gc>,
    _exec: Execution<'gc, '_>,
    mut stack: Stack<'gc, '_>,
) -> Result<CallbackReturn<'gc>, Error<'gc>> {
    let (ud, whence, offset) =
        stack.consume::<(UserData, Option<String>, Option<i64>)>(ctx)?;
    let whence = whence.unwrap_or_else(|| ctx.intern(b"cur"));
    let offset = offset.unwrap_or(0);
    let whence_str = whence.display_lossy().to_string();
    let from = match whence_str.as_str() {
        "set" => SeekFrom::Start(offset as u64),
        "cur" => SeekFrom::Current(offset),
        "end" => SeekFrom::End(offset),
        other => {
            return Err(ctx
                .intern(format!("invalid seek whence '{other}'").as_bytes())
                .into_value(ctx)
                .into());
        }
    };
    let pos = with_file(ud, |file| Ok(file.seek(from)?), ctx)?;
    stack.replace(ctx, pos as i64);
    Ok(CallbackReturn::Return)
}

fn file_lines<'gc>(
    ctx: Context<'gc>,
    _exec: Execution<'gc, '_>,
    mut stack: Stack<'gc, '_>,
) -> Result<CallbackReturn<'gc>, Error<'gc>> {
    let ud = stack.consume::<UserData>(ctx)?;
    // Verify it's a file handle.
    ud.downcast_static::<FileHandle>().map_err(|_| {
        Error::from(ctx.intern(b"expected a file handle").into_value(ctx))
    })?;
    let iter = Callback::from_fn_with(&ctx, ud, lines_iter);
    stack.replace(ctx, iter);
    Ok(CallbackReturn::Return)
}

fn lines_iter<'gc>(
    ud: &UserData<'gc>,
    ctx: Context<'gc>,
    _exec: Execution<'gc, '_>,
    mut stack: Stack<'gc, '_>,
) -> Result<CallbackReturn<'gc>, Error<'gc>> {
    let ud = *ud;
    let result = with_file(
        ud,
        |file| match read_line(file, false) {
            Ok(Some(line)) => Ok(Some(ctx.intern(&line))),
            Ok(None) => Ok(None),
            Err(e) => Err(io_err(ctx, e)),
        },
        ctx,
    )?;
    match result {
        Some(s) => stack.replace(ctx, s),
        None => {
            // Close the file at EOF (matches io.lines semantics).
            if let Ok(handle) = ud.downcast_static::<FileHandle>() {
                *handle.kind.borrow_mut() = FileKind::Closed;
            }
            stack.replace(ctx, Value::Nil);
        }
    }
    Ok(CallbackReturn::Return)
}

fn io_err<'gc>(ctx: Context<'gc>, e: io::Error) -> Error<'gc> {
    ctx.intern(e.to_string().as_bytes()).into_value(ctx).into()
}
