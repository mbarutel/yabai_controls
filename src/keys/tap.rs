//! CGEventTap-based keyboard capture.
//!
//! This is the only module that touches Core Graphics and the only code that
//! must run on the main thread. It does one thing: translate key-down events
//! into [`Hotkey`]s, swallow the ones in the grab set, and forward them over a
//! channel. All decisions about what a hotkey *does* live in `dispatch`.
//!
//! Requires the Accessibility permission (System Settings → Privacy &
//! Security → Accessibility) for the running binary.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use anyhow::{Result, anyhow};
use core_foundation::runloop::{CFRunLoop, kCFRunLoopCommonModes, kCFRunLoopDefaultMode};
use core_graphics::event::{
    CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventType,
    CallbackResult, EventField,
};
use tracing::{debug, info, warn};

use super::{Hotkey, Modifiers};

/// The set of hotkeys the tap should swallow and forward. Shared with
/// `dispatch`, which rewrites it on mode changes and config reloads.
#[derive(Debug, Clone, Default)]
pub struct GrabSet(Arc<RwLock<HashSet<Hotkey>>>);

impl GrabSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn replace(&self, hotkeys: impl IntoIterator<Item = Hotkey>) {
        let mut set = self.0.write().expect("grab set poisoned");
        set.clear();
        set.extend(hotkeys);
    }

    pub fn contains(&self, hotkey: &Hotkey) -> bool {
        self.0.read().expect("grab set poisoned").contains(hotkey)
    }
}

/// Flag another thread can set to make [`run`] return.
#[derive(Debug, Clone, Default)]
pub struct Shutdown(Arc<AtomicBool>);

impl Shutdown {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn request(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn requested(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// An installed, enabled event tap. Lives on the main thread; call
/// [`Tap::run`] to pump events.
pub struct Tap {
    tap: CGEventTap<'static>,
    needs_reenable: Arc<AtomicBool>,
}

impl Tap {
    /// Creates the tap and attaches it to the current thread's run loop.
    /// Fails without the Accessibility permission, so call this before doing
    /// anything with side effects (registering yabai signals, binding
    /// sockets). Must be called from the main thread.
    ///
    /// `on_hotkey` is called from the tap callback for every grabbed key and
    /// must return quickly; return `false` to pass the key through to the app
    /// anyway (for example when the receiver has gone away).
    pub fn install<F>(grab: GrabSet, on_hotkey: F) -> Result<Tap>
    where
        F: Fn(Hotkey) -> bool + Send + 'static,
    {
        let needs_reenable = Arc::new(AtomicBool::new(false));

        let tap = {
            let needs_reenable = Arc::clone(&needs_reenable);
            // Only KeyDown goes in the mask. TapDisabledByTimeout /
            // TapDisabledByUserInput are delivered to every tap regardless,
            // and their discriminants overflow the crate's `1 << type` mask.
            CGEventTap::new(
                CGEventTapLocation::Session,
                CGEventTapPlacement::HeadInsertEventTap,
                CGEventTapOptions::Default,
                vec![CGEventType::KeyDown],
                move |_proxy, event_type, event| match event_type {
                    CGEventType::KeyDown => {
                        let keycode = event
                            .get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE)
                            as u16;
                        let modifiers = Modifiers::from_flags(event.get_flags(), keycode);
                        let hotkey = Hotkey::new(modifiers, keycode);

                        if grab.contains(&hotkey) {
                            debug!(%hotkey, "grabbed");
                            if on_hotkey(hotkey) {
                                CallbackResult::Drop
                            } else {
                                warn!(%hotkey, "hotkey not consumed; passing key through");
                                CallbackResult::Keep
                            }
                        } else {
                            CallbackResult::Keep
                        }
                    }
                    CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput => {
                        warn!(?event_type, "event tap disabled by system; will re-enable");
                        needs_reenable.store(true, Ordering::SeqCst);
                        CallbackResult::Keep
                    }
                    _ => CallbackResult::Keep,
                },
            )
            .map_err(|()| {
                anyhow!(
                    "failed to create event tap: grant this binary Accessibility permission \
                     in System Settings → Privacy & Security → Accessibility"
                )
            })?
        };

        let source = tap
            .mach_port()
            .create_runloop_source(0)
            .map_err(|()| anyhow!("failed to create run loop source for event tap"))?;
        // SAFETY: kCFRunLoopCommonModes is a valid static CFStringRef provided
        // by CoreFoundation for the lifetime of the process.
        CFRunLoop::get_current().add_source(&source, unsafe { kCFRunLoopCommonModes });
        tap.enable();
        info!("event tap installed");

        Ok(Tap {
            tap,
            needs_reenable,
        })
    }

    /// Pumps the run loop until `shutdown` is requested. Blocks the calling
    /// (main) thread.
    pub fn run(self, shutdown: Shutdown) {
        // Short slices so we can service the re-enable flag and the shutdown
        // request without needing a callback that owns the tap.
        while !shutdown.requested() {
            // SAFETY: kCFRunLoopDefaultMode is a valid static CFStringRef.
            CFRunLoop::run_in_mode(
                unsafe { kCFRunLoopDefaultMode },
                Duration::from_millis(500),
                false,
            );
            if self.needs_reenable.swap(false, Ordering::SeqCst) {
                self.tap.enable();
                info!("event tap re-enabled");
            }
        }
        info!("event tap shutting down");
    }
}
