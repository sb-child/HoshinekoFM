// Native drag module for HoshinekoFM.
//
// Sources (both Apache-2.0 OR MIT, CrabNebula Ltd.):
//   https://crates.io/crates/drag               — cross-platform native drag
//   https://crates.io/crates/tauri-plugin-drag   — Tauri integration layer

#[cfg(target_os = "linux")]
mod linux;
pub mod commands;

#[cfg(target_os = "linux")]
#[allow(unused_imports)]
pub(crate) use linux::start_drag_native;
