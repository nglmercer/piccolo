use gc_arena::{lock::RefLock, Collect, Finalization, Gc, GcWeak, Mutation};

use crate::{
    meta_ops::{self, MetaMethod},
    table::TableInner,
    thread::ThreadInner,
    userdata::UserDataInner,
    Context, Function, Table, Thread, UserData, Value,
};

#[derive(Copy, Clone, Collect)]
#[collect(no_drop)]
pub struct Finalizers<'gc>(Gc<'gc, RefLock<FinalizersState<'gc>>>);

impl<'gc> Finalizers<'gc> {
    const THREAD_ERR: &'static str = "thread finalization was missed";

    pub(crate) fn new(mc: &Mutation<'gc>) -> Self {
        Finalizers(Gc::new(mc, RefLock::default()))
    }

    pub(crate) fn register_thread(&self, mc: &Mutation<'gc>, ptr: Gc<'gc, ThreadInner<'gc>>) {
        self.0.borrow_mut(mc).threads.push(Gc::downgrade(ptr));
    }

    /// Register a table as a candidate for `__gc` finalization.
    ///
    /// The table is held weakly; whether a finalizer actually runs is decided at collection time
    /// based on whether the table is unreachable and has a callable `__gc` metamethod.
    pub(crate) fn register_table(&self, mc: &Mutation<'gc>, ptr: Gc<'gc, TableInner<'gc>>) {
        self.0
            .borrow_mut(mc)
            .finalizable
            .push(Finalizable::Table(Gc::downgrade(ptr)));
    }

    /// Register a userdata as a candidate for `__gc` finalization.
    ///
    /// This is an internal capability; the Lua-facing `setmetatable` currently only accepts tables,
    /// so userdata registration is reserved for Rust-side use and is not yet exercised by the
    /// stdlib.
    #[allow(dead_code)]
    pub(crate) fn register_userdata(&self, mc: &Mutation<'gc>, ptr: Gc<'gc, UserDataInner<'gc>>) {
        self.0
            .borrow_mut(mc)
            .finalizable
            .push(Finalizable::UserData(Gc::downgrade(ptr)));
    }

    /// Pop the next pending finalizer (object, `__gc` function) to run, if any.
    pub(crate) fn pop_pending(&self, mc: &Mutation<'gc>) -> Option<(Value<'gc>, Function<'gc>)> {
        self.0.borrow_mut(mc).pending.pop()
    }

    /// First stage of two-phase finalization.
    ///
    /// This stage can cause resurrection, so the arena must be *fully re-marked* before stage two
    /// (`Finalizers::finalize`).
    pub(crate) fn prepare(&self, ctx: Context<'gc>, fc: &Finalization<'gc>) {
        // Resurrect live upvalues held by threads that are about to be collected.
        {
            let state = self.0.borrow();
            for &ptr in &state.threads {
                let thread = Thread::from_inner(ptr.upgrade(fc).expect(Self::THREAD_ERR));
                thread.resurrect_live_upvalues(fc).unwrap();
            }
        }

        // Detect finalizable objects that became unreachable, resurrect them so their finalizer
        // can observe them, and queue their `__gc` to run after collection completes.
        //
        // We partition the tracking list *before* resurrecting: an object that is dead at the
        // start of this callback is finalized (or collected) exactly once and removed from
        // tracking, regardless of the fact that resurrecting it makes it reachable again.
        let mut queued: Vec<(Value<'gc>, Function<'gc>)> = Vec::new();
        {
            let mut state = self.0.borrow_mut(fc);
            let tracked = std::mem::take(&mut state.finalizable);
            for fin in tracked {
                if fin.is_dead(fc) {
                    if let Some(pair) = fin.resurrect_and_queue(ctx, fc) {
                        queued.push(pair);
                    }
                    // Dead objects are dropped from tracking so they are never finalized twice.
                } else {
                    state.finalizable.push(fin);
                }
            }
            state.pending.extend(queued);
        }
    }

    /// Second stage of two-phase finalization.
    ///
    /// Assuming stage one was called (`Finalizers::prepare`) and the arena fully re-marked, this
    /// method will *not* cause any resurrection.
    ///
    /// The arena must *immediately* transition to `CollectionPhase::Collecting` afterwards to not
    /// miss any finalizers.
    pub(crate) fn finalize(&self, fc: &Finalization<'gc>) {
        let mut state = self.0.borrow_mut(fc);
        state.threads.retain(|&ptr| {
            let ptr = ptr.upgrade(fc).expect(Self::THREAD_ERR);
            if Gc::is_dead(fc, ptr) {
                Thread::from_inner(ptr).reset(fc).unwrap();
                false
            } else {
                true
            }
        });
    }
}

/// A weakly-held object that may need to run a `__gc` finalizer when it becomes unreachable.
#[derive(Collect)]
#[collect(no_drop)]
#[allow(dead_code)] // The UserData variant is reserved for Rust-side registration (see above).
enum Finalizable<'gc> {
    Table(GcWeak<'gc, TableInner<'gc>>),
    UserData(GcWeak<'gc, UserDataInner<'gc>>),
}

impl<'gc> Finalizable<'gc> {
    fn is_dead(&self, fc: &Finalization<'gc>) -> bool {
        match self {
            Finalizable::Table(w) => w.is_dead(fc),
            Finalizable::UserData(w) => w.is_dead(fc),
        }
    }

    /// Resurrect the dead object and, if it has a callable `__gc`, return the object together with
    /// the finalizer to run. Returns `None` if the object was already dropped or has no callable
    /// `__gc`.
    fn resurrect_and_queue(
        &self,
        ctx: Context<'gc>,
        fc: &Finalization<'gc>,
    ) -> Option<(Value<'gc>, Function<'gc>)> {
        let (value, metatable) = match self {
            Finalizable::Table(w) => {
                let table = Table::from_inner(w.resurrect(fc)?);
                (Value::Table(table), table.metatable())
            }
            Finalizable::UserData(w) => {
                let userdata = UserData::from_inner(w.resurrect(fc)?);
                (Value::UserData(userdata), userdata.metatable())
            }
        };
        let gc = metatable?.get_value(ctx, MetaMethod::Gc);
        let function = meta_ops::call(ctx, gc).ok()?;
        Some((value, function))
    }
}

#[derive(Default, Collect)]
#[collect(no_drop)]
struct FinalizersState<'gc> {
    threads: Vec<GcWeak<'gc, ThreadInner<'gc>>>,
    finalizable: Vec<Finalizable<'gc>>,
    pending: Vec<(Value<'gc>, Function<'gc>)>,
}
