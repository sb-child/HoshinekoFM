//! 操作归组 + 进度泵。
//!
//! 将 Progress/op 绑定到 ContextId（tab/window），
//! 在后台消费 ProgressEvent 并在操作完成后自动清理。

use std::sync::Arc;

use crate::app::fs_service::{Canceller, Progress};
use crate::fsworker::protocol::ProgressEvent;
use crate::mesh::types::ui::ContextId;

use super::UIService;

impl UIService {
    pub fn track_op(self: &Arc<Self>, context_ids: &[ContextId], progress: Progress) {
        let op_id = progress.op_id();
        {
            let mut ctxs = self.contexts.lock().unwrap();
            for cid in context_ids {
                ctxs.entry(*cid).or_default().insert(op_id);
            }
        }
        self.cancels
            .lock()
            .unwrap()
            .insert(op_id, progress.canceller());

        let ctx_ids: Vec<ContextId> = context_ids.to_vec();
        let this = self.clone();
        let progress = progress;
        tokio::spawn(async move {
            while let Ok(ev) = progress.events.recv().await {
                match ev {
                    ProgressEvent::Conflict { conflict_id, .. } => {
                        progress.resolve(
                            conflict_id,
                            crate::fsworker::protocol::ConflictResolution::AutoRename,
                        );
                    }
                    ProgressEvent::Done { .. }
                    | ProgressEvent::ConnectionLost {
                        reconnecting: false,
                        ..
                    } => {
                        break;
                    }
                    _ => {}
                }
            }
            this.forget_op(&ctx_ids, op_id);
        });
    }

    fn forget_op(&self, context_ids: &[ContextId], op_id: u64) {
        let mut ctxs = self.contexts.lock().unwrap();
        for cid in context_ids {
            if let Some(set) = ctxs.get_mut(cid) {
                set.remove(&op_id);
            }
        }
        ctxs.retain(|_, v| !v.is_empty());
        self.cancels.lock().unwrap().remove(&op_id);
    }

    pub(super) fn context_busy(&self, ids: &[ContextId]) -> Vec<u64> {
        let ctxs = self.contexts.lock().unwrap();
        let mut ops = Vec::new();
        for id in ids {
            if let Some(set) = ctxs.get(id) {
                ops.extend(set.iter().copied());
            }
        }
        ops
    }

    pub(super) async fn cancel_contexts(&self, ids: &[ContextId]) {
        let cancellers: Vec<Canceller> = {
            let ctxs = self.contexts.lock().unwrap();
            let cancels = self.cancels.lock().unwrap();
            let mut v = Vec::new();
            for id in ids {
                if let Some(set) = ctxs.get(id) {
                    for op in set {
                        if let Some(c) = cancels.get(op) {
                            v.push(c.clone());
                        }
                    }
                }
            }
            v
        };
        for c in cancellers {
            c.cancel().await;
        }
    }
}
