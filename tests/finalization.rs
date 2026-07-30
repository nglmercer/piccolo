use piccolo::{Closure, Executor, ExternError, Lua};

fn run(lua: &mut Lua, code: &[u8]) -> Result<(), ExternError> {
    let executor = lua.try_enter(|ctx| {
        let closure = Closure::load(ctx, None, code)?;
        Ok(ctx.stash(Executor::start(ctx, closure.into(), ())))
    })?;
    lua.execute::<()>(&executor)
}

/// A `__gc` finalizer runs after the last reference to its object is dropped and a full collection
/// is forced.
#[test]
fn gc_finalizer_runs() -> Result<(), ExternError> {
    let mut lua = Lua::core();

    run(
        &mut lua,
        &br#"
            log = {}
            local obj = {}
            setmetatable(obj, { __gc = function(o)
                log[#log + 1] = "finalized"
            end })
            obj = nil
        "#[..],
    )?;

    // Nothing has been collected yet.
    run(&mut lua, b"assert(#log == 0)")?;

    lua.gc_collect();

    run(
        &mut lua,
        &br#"
            assert(#log == 1, "expected 1 finalization, got " .. #log)
            assert(log[1] == "finalized")
        "#[..],
    )?;

    Ok(())
}

/// A finalizer can resurrect its object by keeping a reference to it; the resurrected object stays
/// reachable and is not finalized a second time.
#[test]
fn gc_finalizer_resurrection() -> Result<(), ExternError> {
    let mut lua = Lua::core();

    run(
        &mut lua,
        &br#"
            count = 0
            saved = nil
            local obj = {}
            obj.name = "the-object"
            setmetatable(obj, { __gc = function(o)
                count = count + 1
                saved = o
            end })
            obj = nil
        "#[..],
    )?;

    lua.gc_collect();

    // The finalizer ran exactly once and stored the object in `saved`.
    run(
        &mut lua,
        &br#"
            assert(count == 1, "count=" .. count)
            assert(saved ~= nil, "object was not resurrected")
            assert(saved.name == "the-object")
        "#[..],
    )?;

    // The object is now reachable through `saved`, so a second collection must not finalize it.
    lua.gc_collect();
    run(&mut lua, b"assert(count == 1, 'finalized more than once: ' .. count)")?;

    Ok(())
}

/// An object without a reachable reference and without a `__gc` is simply collected; finalization
/// of other objects is unaffected.
#[test]
fn gc_finalizer_only_once_per_object() -> Result<(), ExternError> {
    let mut lua = Lua::core();

    run(
        &mut lua,
        &br#"
            count = 0
            do
                local a = setmetatable({}, { __gc = function() count = count + 1 end })
                local b = setmetatable({}, { __gc = function() count = count + 1 end })
            end
        "#[..],
    )?;

    lua.gc_collect();
    run(&mut lua, b"assert(count == 2, 'count=' .. count)")?;

    // Collecting again finalizes nothing new.
    lua.gc_collect();
    run(&mut lua, b"assert(count == 2, 'count=' .. count)")?;

    Ok(())
}
