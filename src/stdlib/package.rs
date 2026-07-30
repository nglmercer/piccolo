//! The `package` library: Lua-module loading via `require`, `package.loaded`,
//! `package.preload`, `package.path`, `package.searchers`, and `package.searchpath`.
//!
//! Only pure-Lua modules are supported; loading C libraries (`package.loadlib`) is
//! intentionally out of scope.

use std::pin::Pin;

use gc_arena::Collect;

use crate::{
    meta_ops, BoxSequence, Callback, CallbackReturn, Closure, Context, Error, Execution, FromValue,
    Function, IntoValue, Sequence, SequencePoll, Stack, String, Table, Value,
};

pub fn load_package<'gc>(ctx: Context<'gc>) {
    let package = Table::new(&ctx);

    // package.loaded — cache of already-required modules.
    package.set_field(ctx, "loaded", Table::new(&ctx));

    // package.preload — table of module-name -> loader function.
    package.set_field(ctx, "preload", Table::new(&ctx));

    // package.path — search path for Lua modules. Honor $LUA_PATH when set.
    let path = std::env::var("LUA_PATH")
        .unwrap_or_else(|_| "./?.lua;./?/init.lua".to_owned());
    package.set_field(ctx, "path", ctx.intern(path.as_bytes()));

    // package.searchers — ordered array of searcher functions.
    let searchers = Table::new(&ctx);
    searchers
        .set(ctx, 1i64, Callback::from_fn(&ctx, searcher_preload))
        .unwrap();
    searchers
        .set(ctx, 2i64, Callback::from_fn(&ctx, searcher_file))
        .unwrap();
    package.set_field(ctx, "searchers", searchers);

    // package.searchpath(name, path [, sep [, rep]])
    package.set_field(ctx, "searchpath", Callback::from_fn(&ctx, searchpath));

    ctx.set_global("package", package);
    ctx.set_global("require", Callback::from_fn(&ctx, require));
}

/// `package.searchpath(name, path [, sep [, rep]])`
///
/// Searches for `name` in `path`, a `;`-separated list of templates each containing a single `?`.
/// Occurrences of `sep` (default ".") in `name` are replaced by `rep` (default "/") before
/// substitution. Returns the first template that names an existing file, or `nil` plus a message
/// listing every path that was tried.
fn searchpath<'gc>(
    ctx: Context<'gc>,
    _exec: Execution<'gc, '_>,
    mut stack: Stack<'gc, '_>,
) -> Result<CallbackReturn<'gc>, Error<'gc>> {
    let (name, path, sep, rep) =
        stack.consume::<(String, String, Option<String>, Option<String>)>(ctx)?;
    let sep = sep.unwrap_or_else(|| ctx.intern(b"."));
    let rep = rep.unwrap_or_else(|| ctx.intern(b"/"));

    match search_path(
        &name.display_lossy().to_string(),
        &path.display_lossy().to_string(),
        &sep.display_lossy().to_string(),
        &rep.display_lossy().to_string(),
    ) {
        Ok(found) => {
            stack.replace(ctx, ctx.intern(found.as_bytes()));
        }
        Err(tried) => {
            stack.replace(ctx, (Value::Nil, ctx.intern(tried.as_bytes())));
        }
    }
    Ok(CallbackReturn::Return)
}

/// The preload searcher: returns `package.preload[name]` if present, else a message fragment.
fn searcher_preload<'gc>(
    ctx: Context<'gc>,
    _exec: Execution<'gc, '_>,
    mut stack: Stack<'gc, '_>,
) -> Result<CallbackReturn<'gc>, Error<'gc>> {
    let modname = stack.consume::<String>(ctx)?;
    let package = Table::from_value(ctx, ctx.get_global_value("package"))?;
    let preload = Table::from_value(ctx, package.get_value(ctx, "preload"))?;
    let loader = preload.get_value(ctx, modname);
    if matches!(loader, Value::Nil) {
        let msg = format!(
            "\n\tno field package.preload['{}']",
            modname.display_lossy()
        );
        stack.replace(ctx, ctx.intern(msg.as_bytes()));
    } else {
        stack.replace(ctx, loader);
    }
    Ok(CallbackReturn::Return)
}

/// The file searcher: locates a module on `package.path`, compiles it, and returns the chunk as a
/// loader function plus the file path as an extra argument.
fn searcher_file<'gc>(
    ctx: Context<'gc>,
    _exec: Execution<'gc, '_>,
    mut stack: Stack<'gc, '_>,
) -> Result<CallbackReturn<'gc>, Error<'gc>> {
    let modname = stack.consume::<String>(ctx)?;
    let package = Table::from_value(ctx, ctx.get_global_value("package"))?;
    let path = String::from_value(ctx, package.get_value(ctx, "path"))?;

    match search_path(&modname.display_lossy().to_string(), &path.display_lossy().to_string(), ".", "/") {
        Ok(file_path) => {
            let source = std::fs::read(&file_path)
                .map_err(|e| Error::from(ctx.intern(e.to_string().as_bytes()).into_value(ctx)))?;
            let closure = Closure::load(ctx, Some(&file_path), &source)
                .map_err(|e| Error::from(ctx.intern(e.to_string().as_bytes()).into_value(ctx)))?;
            let loader: Function = closure.into();
            stack.replace(ctx, (loader, ctx.intern(file_path.as_bytes())));
        }
        Err(tried) => {
            stack.replace(ctx, ctx.intern(tried.as_bytes()));
        }
    }
    Ok(CallbackReturn::Return)
}

/// Resolve a module name against a search path, returning the first existing file or an error
/// message enumerating every candidate that was tried.
fn search_path(
    name: &str,
    path: &str,
    sep: &str,
    rep: &str,
) -> Result<std::string::String, std::string::String> {
    let converted = if sep.is_empty() {
        name.to_owned()
    } else {
        name.replace(sep, rep)
    };
    let mut tried = std::string::String::new();
    for template in path.split(';') {
        let template = template.trim();
        if template.is_empty() {
            continue;
        }
        let candidate = template.replace('?', &converted);
        if std::path::Path::new(&candidate).is_file() {
            return Ok(candidate);
        }
        tried.push_str("\n\tno file '");
        tried.push_str(&candidate);
        tried.push('\'');
    }
    Err(tried)
}

/// `require(modname)` — load a module, caching the result in `package.loaded`.
fn require<'gc>(
    ctx: Context<'gc>,
    _exec: Execution<'gc, '_>,
    mut stack: Stack<'gc, '_>,
) -> Result<CallbackReturn<'gc>, Error<'gc>> {
    let modname = stack.consume::<String>(ctx)?;
    Ok(CallbackReturn::Sequence(BoxSequence::new(
        &ctx,
        Require {
            modname,
            phase: RequirePhase::Start,
            searcher_index: 1,
            err: Vec::new(),
        },
    )))
}

#[derive(Collect)]
#[collect(no_drop)]
struct Require<'gc> {
    modname: String<'gc>,
    #[collect(require_static)]
    phase: RequirePhase,
    #[collect(require_static)]
    searcher_index: i64,
    #[collect(require_static)]
    err: Vec<u8>,
}

#[derive(Clone, Copy, Collect)]
#[collect(require_static)]
enum RequirePhase {
    /// Check `package.loaded` for a cached module, then begin searching.
    Start,
    /// Call `package.searchers[searcher_index]` with the module name.
    CallSearcher,
    /// Inspect a searcher's return value (loader function or message fragment).
    AfterSearch,
    /// Store the loader's return value in `package.loaded` and return it.
    AfterLoad,
}

impl<'gc> Sequence<'gc> for Require<'gc> {
    fn poll(
        mut self: Pin<&mut Self>,
        ctx: Context<'gc>,
        _exec: Execution<'gc, '_>,
        mut stack: Stack<'gc, '_>,
    ) -> Result<SequencePoll<'gc>, Error<'gc>> {
        loop {
            match self.phase {
                RequirePhase::Start => {
                    let package = Table::from_value(ctx, ctx.get_global_value("package"))?;
                    let loaded = Table::from_value(ctx, package.get_value(ctx, "loaded"))?;
                    let cached = loaded.get_value(ctx, self.modname);
                    if !matches!(cached, Value::Nil) {
                        stack.replace(ctx, cached);
                        return Ok(SequencePoll::Return);
                    }
                    self.searcher_index = 1;
                    self.phase = RequirePhase::CallSearcher;
                }
                RequirePhase::CallSearcher => {
                    let package = Table::from_value(ctx, ctx.get_global_value("package"))?;
                    let searchers = Table::from_value(ctx, package.get_value(ctx, "searchers"))?;
                    let searcher = searchers.get_value(ctx, self.searcher_index);
                    if matches!(searcher, Value::Nil) {
                        // No searcher produced a loader.
                        let mut msg = std::string::String::from("module '");
                        msg.push_str(&self.modname.display_lossy().to_string());
                        msg.push_str("' not found:");
                        msg.push_str(std::str::from_utf8(&self.err).unwrap_or(""));
                        return Err(Error::from(ctx.intern(msg.as_bytes()).into_value(ctx)));
                    }
                    let function = meta_ops::call(ctx, searcher)?;
                    stack.clear();
                    stack.push_back(self.modname.into());
                    self.phase = RequirePhase::AfterSearch;
                    return Ok(SequencePoll::Call {
                        bottom: 0,
                        function,
                    });
                }
                RequirePhase::AfterSearch => {
                    let result = stack.get(0);
                    match result {
                        Value::String(message) => {
                            // Searcher declined and left a message fragment.
                            self.err.extend_from_slice(message.as_bytes());
                            self.searcher_index += 1;
                            self.phase = RequirePhase::CallSearcher;
                            stack.clear();
                        }
                        Value::Nil | Value::Boolean(false) => {
                            // Searcher declined silently.
                            self.searcher_index += 1;
                            self.phase = RequirePhase::CallSearcher;
                            stack.clear();
                        }
                        loader => {
                            // Searcher returned a loader; call it with the module name and the
                            // optional extra value the searcher provided.
                            let extra = stack.get(1);
                            let function = meta_ops::call(ctx, loader)?;
                            stack.clear();
                            stack.push_back(self.modname.into());
                            if !matches!(extra, Value::Nil) {
                                stack.push_back(extra);
                            }
                            self.phase = RequirePhase::AfterLoad;
                            return Ok(SequencePoll::Call {
                                bottom: 0,
                                function,
                            });
                        }
                    }
                }
                RequirePhase::AfterLoad => {
                    let mut module = stack.get(0);
                    if matches!(module, Value::Nil) {
                        module = Value::Boolean(true);
                    }
                    let package = Table::from_value(ctx, ctx.get_global_value("package"))?;
                    let loaded = Table::from_value(ctx, package.get_value(ctx, "loaded"))?;
                    loaded
                        .set(ctx, self.modname, module)
                        .map_err(|_| Error::from(ctx.intern(b"invalid module name").into_value(ctx)))?;
                    stack.replace(ctx, module);
                    return Ok(SequencePoll::Return);
                }
            }
        }
    }
}
