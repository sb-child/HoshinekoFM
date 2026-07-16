//! 剪贴板 + DnD 状态管理。

use std::sync::Mutex;

use tauri::Emitter;
use tracing::debug;

use crate::mesh::types::ui::{ClipOp, ClipboardState};

use super::UIService;

impl UIService {
    fn clipboard_inner(&self) -> &Mutex<ClipboardState> {
        static CLIPBOARD: std::sync::LazyLock<Mutex<ClipboardState>> =
            std::sync::LazyLock::new(|| {
                Mutex::new(ClipboardState {
                    operation: None,
                    files: Vec::new(),
                })
            });
        &CLIPBOARD
    }

    pub fn clip_copy(&self, window: &tauri::Window, paths: &[String]) {
        {
            let mut cb = self.clipboard_inner().lock().unwrap();
            cb.operation = Some(ClipOp::Copy);
            cb.files = paths.to_vec();
        }
        let _ = window.emit("hf:clipboard", &self.clipboard_state());
        debug!("clip_copy: {} files", paths.len());
    }

    pub fn clip_cut(&self, window: &tauri::Window, paths: &[String]) {
        {
            let mut cb = self.clipboard_inner().lock().unwrap();
            cb.operation = Some(ClipOp::Cut);
            cb.files = paths.to_vec();
        }
        let _ = window.emit("hf:clipboard", &self.clipboard_state());
        debug!("clip_cut: {} files", paths.len());
    }

    pub fn clipboard_state(&self) -> ClipboardState {
        self.clipboard_inner().lock().unwrap().clone()
    }

    pub fn clipboard_sync(&self, state: ClipboardState) {
        {
            let mut cb = self.clipboard_inner().lock().unwrap();
            *cb = state;
        }
        // broadcast clipboard to all local windows
        let reg = self.mgr.window_registry.lock().unwrap();
        let cb_state = self.clipboard_state();
        for window in reg.values() {
            let _ = window.emit("hf:clipboard", &cb_state);
        }
        debug!("clipboard_sync: updated from remote");
    }
}
