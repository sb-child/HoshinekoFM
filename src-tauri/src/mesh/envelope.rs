//! Mesh 消息信封。

use serde::{Deserialize, Serialize};

use super::id::MeshId;

/// 带源和目标地址的消息信封。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope<M> {
    pub src: MeshId,
    pub dst: MeshId,
    pub msg: M,
}
