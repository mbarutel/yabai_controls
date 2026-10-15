//! SIGINT / SIGTERM → graceful shutdown.
//!
//! The first termination signal is forwarded to the dispatch loop as
//! [`Event::Shutdown`], so Ctrl-C and `kill` tear down exactly like
//! `yabai_controls shutdown` (rules removed, yabai signals removed, socket
//! unlinked). A second signal exits immediately with `128 + signo`, the
//! shell convention, in case teardown is stuck on an unresponsive yabai.
//!
//! Not to be confused with [`crate::yabai::signal`], which is yabai's own
//! event-notification mechanism.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Sender;
use std::thread;

use anyhow::{Context, Result};
use signal_hook::consts::{SIGINT, SIGTERM};
use signal_hook::flag;
use signal_hook::iterator::Signals;
use tracing::info;

use crate::events::Event;

const TERMINATION: [i32; 2] = [SIGINT, SIGTERM];

/// Installs the handlers and spawns the thread that forwards the first
/// signal to `tx`. Returns once the handlers are registered.
pub fn forward_termination(tx: Sender<Event>) -> Result<()> {
    // signal-hook runs actions in registration order, so on the first signal
    // the conditional shutdown sees `armed == false` and does nothing, then
    // `register` arms it; the second signal exits inside the handler.
    let armed = Arc::new(AtomicBool::new(false));
    for sig in TERMINATION {
        flag::register_conditional_shutdown(sig, 128 + sig, Arc::clone(&armed))
            .with_context(|| format!("registering forced exit for {}", name(sig)))?;
        flag::register(sig, Arc::clone(&armed))
            .with_context(|| format!("arming forced exit for {}", name(sig)))?;
    }
    let mut signals = Signals::new(TERMINATION).context("registering termination signals")?;

    thread::Builder::new()
        .name("os-signal".into())
        .spawn(move || {
            if let Some(sig) = signals.forever().next() {
                info!(signal = name(sig), "termination signal; shutting down");
                let _ = tx.send(Event::Shutdown);
            }
            // Dropping `signals` unregisters the forwarder; the flag actions
            // stay so a second signal still force-exits during teardown.
        })
        .context("spawning os-signal thread")?;
    Ok(())
}

fn name(sig: i32) -> &'static str {
    match sig {
        SIGINT => "SIGINT",
        SIGTERM => "SIGTERM",
        _ => "signal",
    }
}
