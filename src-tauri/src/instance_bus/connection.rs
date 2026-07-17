//! 到另一个实例的单条 P2P 连接。
//!
//! 封装 tarpc client + 自动重连。调用方不需关心底层传输。

use std::io;

use tarpc::context;
use tokio::net::UnixStream;
use tracing::{debug, error};

use crate::mesh::types::instance::{InstanceMsg, InstanceServiceClient};

/// 到另一个 HoshinekoFM 实例的单条直连。
pub(crate) struct PeerConnection {
    instance_id: u64,
    client: InstanceServiceClient,
}

impl PeerConnection {
    /// 连接到指定实例。
    pub async fn connect(instance_id: u64, socket_path: &str) -> io::Result<Self> {
        let stream = UnixStream::connect(socket_path).await?;
        let client = make_client(stream);
        debug!("connected to instance {instance_id}");
        Ok(Self {
            instance_id,
            client,
        })
    }

    pub fn instance_id(&self) -> u64 {
        self.instance_id
    }

    /// 发送 InstanceMsg（按变体路由到对应 RPC）。
    pub async fn send(&mut self, msg: &InstanceMsg) -> io::Result<()> {
        match self.send_inner(msg).await {
            Ok(()) => Ok(()),
            Err(e) => {
                error!("send to instance {} failed: {e}", self.instance_id);
                Err(e)
            }
        }
    }

    async fn send_inner(&mut self, msg: &InstanceMsg) -> io::Result<()> {
        let ctx = context::current();
        match msg {
            InstanceMsg::OpenWindow { paths } => self.client.open_window(ctx, paths.clone()).await,
            InstanceMsg::TransferTab { tab } => self.client.transfer_tab(ctx, tab.clone()).await,
            InstanceMsg::ClipboardSync { state } => {
                self.client.clipboard_sync(ctx, state.clone()).await
            }
            InstanceMsg::ForwardWindowMsg { window_id, msg } => {
                self.client.forward(ctx, *window_id, msg.clone()).await
            }
        }
        .map_err(into_io)
    }

    /// 窗口注册广播（内部路由表同步）。
    pub async fn window_register(&mut self, window_id: u64, instance_id: u64) -> io::Result<()> {
        let ctx = context::current();
        self.client
            .window_register(ctx, window_id, instance_id)
            .await
            .map_err(into_io)
    }

    /// 窗口注销广播（内部路由表同步）。
    pub async fn window_unregister(&mut self, window_id: u64) -> io::Result<()> {
        let ctx = context::current();
        self.client
            .window_unregister(ctx, window_id)
            .await
            .map_err(into_io)
    }
}

fn make_client(stream: UnixStream) -> InstanceServiceClient {
    let transport = tarpc::serde_transport::new(
        crate::mesh::transport::frame_stream(stream),
        tarpc::tokio_serde::formats::Bincode::default(),
    );
    InstanceServiceClient::new(tarpc::client::Config::default(), transport).spawn()
}

fn into_io(e: tarpc::client::RpcError) -> io::Error {
    io::Error::other(e.to_string())
}
