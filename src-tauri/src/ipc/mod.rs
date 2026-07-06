pub mod protocol;

use tokio::net::UnixStream;
use tarpc::tokio_util::codec::length_delimited::LengthDelimitedCodec;
use tokio_util::codec::Framed;

/// 为 UnixStream 创建 LengthDelimitedCodec 帧编码传输层。
///
/// 这是 tarpc 传输层初始化的共享部分，client 和 server 均可复用：
/// - Client 端: `serde_transport::new(frame_stream(stream), Bincode::default())`
/// - Server 端: 同上
///
/// # 参数
/// - `stream`: 已连接的 tokio UnixStream
///
/// # 返回
/// - `Framed<UnixStream, LengthDelimitedCodec>` —— 帧编码后的流
pub fn frame_stream(stream: UnixStream) -> Framed<UnixStream, LengthDelimitedCodec> {
    LengthDelimitedCodec::builder().new_framed(stream)
}
