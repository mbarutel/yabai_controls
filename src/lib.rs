//! yabai_controls: a hotkey daemon that drives yabai.
//!
//! Threads at runtime:
//!
//! ```text
//! main: CGEventTap + CFRunLoop ──Event::Hotkey──▶ dispatch loop ──Action──▶ action worker ──▶ yabai / sh
//! control-socket thread ─────────Event::{Yabai,Reload,Shutdown}──▲
//!   ◀── `yabai_controls notify …` spawned by yabai signals          │
//! os-signal thread (SIGINT/SIGTERM) ──Event::Shutdown──────────────┘
//! ```
//!
//! Module map:
//! - [`yabai`]: socket client, JSON models, typed queries, signal registration
//! - [`keys`]: hotkey type, chord parsing, keycode table, CGEventTap
//! - [`config`]: TOML config → validated bindings per mode
//! - [`spaces`]: applies `[spaces]` assignments (create, label, app rules)
//! - [`state`]: in-memory displays/spaces/windows snapshot
//! - [`events`]: the `Event` enum and the daemon's control socket
//! - [`os_signal`]: SIGINT/SIGTERM → `Event::Shutdown`
//! - [`actions`]: `Action` enum and the worker thread that runs them
//! - [`dispatch`]: the event loop that ties the above together

pub mod actions;
pub mod config;
pub mod dispatch;
pub mod events;
pub mod keys;
pub mod os_signal;
pub mod spaces;
pub mod state;
pub mod yabai;
