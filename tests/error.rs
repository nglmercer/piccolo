use piccolo::{error::LuaError, Callback, Closure, Error, Executor, ExternError, Lua, Value};
use thiserror::Error;

#[test]
fn error_unwind() -> Result<(), ExternError> {
    let mut lua = Lua::core();

    let executor = lua.try_enter(|ctx| {
        let closure = Closure::load(
            ctx,
            None,
            &br#"
                function do_error()
                    error('test error')
                end

                do_error()
            "#[..],
        )?;
        Ok(ctx.stash(Executor::start(ctx, closure.into(), ())))
    })?;

    lua.finish(&executor).unwrap();
    lua.try_enter(|ctx| {
        match ctx.fetch(&executor).take_result::<()>(ctx)? {
            Err(Error::Lua(LuaError(Value::String(s)))) => {
                let s = s.to_str().unwrap();
                // The original message is preserved.
                assert!(s.contains("test error"), "missing message: {s}");
                // A source location is prepended (`error` is called on line 3).
                assert!(s.contains("<anonymous>:3:"), "missing source location: {s}");
                // A stack traceback with function/line information is appended.
                assert!(s.contains("stack traceback:"), "missing traceback: {s}");
                assert!(
                    s.contains("<function 'do_error' at line 2>"),
                    "missing function frame: {s}"
                );
            }
            _ => panic!("wrong error returned"),
        }
        Ok(())
    })
}

#[test]
fn error_tostring() -> Result<(), ExternError> {
    let mut lua = Lua::core();

    #[derive(Debug, Error)]
    #[error("test error")]
    struct TestError;

    let executor = lua.try_enter(|ctx| {
        let callback = Callback::from_fn(&ctx, |_, _, _| Err(TestError.into()));
        ctx.set_global("callback", callback);

        let closure = Closure::load(
            ctx,
            None,
            &br#"
                local r, e = pcall(callback)
                assert(not r)
                assert(tostring(e) == "test error")
            "#[..],
        )?;

        Ok(ctx.stash(Executor::start(ctx, closure.into(), ())))
    })?;

    lua.execute(&executor)
}
