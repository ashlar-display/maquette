//! The SCDC hook: a test's quirk on top of the built-in SCDC sink.

use crate::target::Env;

/// A test's change to how the built-in SCDC sink takes a byte the source writes, or answers
/// a read: a vendor quirk on top of the standard sink, which keeps its offset, bursts, lock,
/// HPD resets and records. Set with
/// [`BenchBuilder::scdc_hook`](crate::BenchBuilder::scdc_hook); one per bench, kept across a
/// [`plug`](crate::Bench::plug). Presets do not pass through it: they are the test's own. Like
/// a target, it runs with the bench locked and sees it through [`Env`] only.
///
/// ```
/// use maquette::{Bench, DdcBus, Env, HookWrite, ScdcHook, ScdcRegisters};
///
/// /// A sink that ignores writes to Config_0.
/// struct NoConfig0;
///
/// impl ScdcHook for NoConfig0 {
///     fn write(&mut self, reg: u8, value: u8, _: &ScdcRegisters<'_>, _: &mut Env<'_>) -> HookWrite {
///         if reg == 0x30 { HookWrite::Drop } else { HookWrite::Store(value) }
///     }
/// }
///
/// let mut hook = NoConfig0;
/// let bench = Bench::builder(&[]).scdc_hook(&mut hook).build().unwrap();
/// bench.ddc().write(0x54, &[0x30, 0x02, 0x35]).unwrap();
/// assert_eq!(bench.sink().config_0(), Some(0));
/// assert_eq!(bench.sink().config_1(), Some(0x35));
/// ```
pub trait ScdcHook {
    /// A byte the source writes to `reg`, before the register's role applies; by default
    /// it goes on as written.
    fn write(
        &mut self,
        reg: u8,
        value: u8,
        regs: &ScdcRegisters<'_>,
        env: &mut Env<'_>,
    ) -> HookWrite {
        let _ = (reg, regs, env);
        HookWrite::Store(value)
    }

    /// What a read of `reg`, holding `value`, returns on the wire; by default `value`. The
    /// register keeps its value: the lock, the bench's `Sink` view and later reads see it.
    fn read(&mut self, reg: u8, value: u8, regs: &ScdcRegisters<'_>, env: &mut Env<'_>) -> u8 {
        let _ = (reg, regs, env);
        value
    }
}

/// What the sink does with a byte the source writes, as an [`ScdcHook`] decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum HookWrite {
    /// The byte goes on with this value: the register's role applies as without a hook.
    Store(u8),
    /// The sink ignores the byte; recorded as an `Event::IgnoredWrite` with
    /// [`Ignored::Hook`](crate::Ignored::Hook).
    Drop,
    /// The sink does not acknowledge the byte: `NoAcknowledge(Data)`, and the burst ends;
    /// recorded as `Drop` is.
    Nack,
    /// Stored as [`Store`](Self::Store), and the sink loses its scrambling lock, as at its
    /// own reset: the status clears, and the lock is gained again once its trigger has
    /// passed. A sink that re-syncs when rewritten.
    StoreRelocking(u8),
}

/// A read-only view of the SCDC registers, as the sink holds them.
#[derive(Debug, Clone, Copy)]
pub struct ScdcRegisters<'r> {
    regs: &'r [u8; 256],
}

impl<'r> ScdcRegisters<'r> {
    pub(crate) const fn new(regs: &'r [u8; 256]) -> Self {
        Self { regs }
    }

    /// The value of `reg`.
    pub fn get(&self, reg: u8) -> u8 {
        self.regs[usize::from(reg)]
    }
}
