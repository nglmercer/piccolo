use piccolo::{Closure, Executor, ExternError, Lua, String};

/// A runtime error raised with `error` should carry a prepended source location and an appended
/// stack traceback that names each Lua frame with its defining line.
#[test]
fn error_has_source_location_and_traceback() -> Result<(), ExternError> {
    let mut lua = Lua::core();

    let executor = lua.try_enter(|ctx| {
        let closure = Closure::load(
            ctx,
            Some("traceback_test"),
            &br#"local function deep()
    error("boom")
end
local function middle()
    deep()
end
local ok, err = pcall(middle)
assert(not ok)
return err
"#[..],
        )?;
        Ok(ctx.stash(Executor::start(ctx, closure.into(), ())))
    })?;

    lua.finish(&executor).unwrap();
    lua.try_enter(|ctx| {
        let message = ctx
            .fetch(&executor)
            .take_result::<String>(ctx)??
            .to_str()
            .unwrap()
            .to_string();

        // The original message is preserved and prefixed with a `<chunk>:<line>:` source location
        // (the `error` call sits on line 2).
        assert!(message.contains("boom"), "missing message: {message}");
        assert!(
            message.contains("traceback_test:2:"),
            "missing source location: {message}"
        );

        // A stack traceback is appended, listing every Lua frame innermost-first with the
        // function name and its defining line.
        assert!(
            message.contains("stack traceback:"),
            "missing traceback header: {message}"
        );
        assert!(
            message.contains("<function 'deep' at line 1>"),
            "missing 'deep' frame: {message}"
        );
        assert!(
            message.contains("<function 'middle' at line 4>"),
            "missing 'middle' frame: {message}"
        );
        assert!(message.contains("<chunk>"), "missing chunk frame: {message}");

        Ok(())
    })
}

/// `assert` should augment its failure message with the caller's source location and a traceback,
/// while still returning its arguments unchanged on success.
#[test]
fn assert_failure_has_source_location() -> Result<(), ExternError> {
    let mut lua = Lua::core();

    let executor = lua.try_enter(|ctx| {
        let closure = Closure::load(
            ctx,
            Some("assert_test"),
            &br#"local ok, err = pcall(function()
    assert(false, "custom failure")
end)
assert(not ok)
return err
"#[..],
        )?;
        Ok(ctx.stash(Executor::start(ctx, closure.into(), ())))
    })?;

    lua.finish(&executor).unwrap();
    lua.try_enter(|ctx| {
        let message = ctx
            .fetch(&executor)
            .take_result::<String>(ctx)??
            .to_str()
            .unwrap()
            .to_string();

        assert!(message.contains("custom failure"), "missing message: {message}");
        assert!(
            message.contains("assert_test:2:"),
            "missing source location: {message}"
        );
        assert!(
            message.contains("stack traceback:"),
            "missing traceback header: {message}"
        );

        Ok(())
    })
}

/// Passing `level = 0` to `error` suppresses position information entirely.
#[test]
fn error_level_zero_has_no_location() -> Result<(), ExternError> {
    let mut lua = Lua::core();

    let executor = lua.try_enter(|ctx| {
        let closure = Closure::load(
            ctx,
            Some("level_zero_test"),
            &br#"local ok, err = pcall(function()
    error("bare", 0)
end)
assert(not ok)
return err
"#[..],
        )?;
        Ok(ctx.stash(Executor::start(ctx, closure.into(), ())))
    })?;

    lua.finish(&executor).unwrap();
    lua.try_enter(|ctx| {
        let message = ctx
            .fetch(&executor)
            .take_result::<String>(ctx)??
            .to_str()
            .unwrap()
            .to_string();

        assert_eq!(message, "bare", "level 0 should not add context: {message}");

        Ok(())
    })
}
