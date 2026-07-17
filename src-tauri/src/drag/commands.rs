// Tauri command layer for native drag.
//
// Source: https://crates.io/crates/tauri-plugin-drag (CrabNebula Ltd., Apache-2.0 OR MIT)
//   - src/commands.rs

use std::{collections::HashMap, path::PathBuf};

use serde::{Deserialize, Deserializer, Serialize, ser::Serializer};
use tauri::{AppHandle, Runtime, Window, command, ipc::Channel};
use tracing::{Instrument, warn};

#[cfg(target_os = "linux")]
use super::linux::{self, DragMode as NativeDragMode};

type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Tauri(#[from] tauri::Error),
    #[error(transparent)]
    Base64(#[from] base64::DecodeError),
    #[cfg(target_os = "linux")]
    #[error("native drag error: {0}")]
    NativeDrag(#[from] linux::Error),
    #[cfg(not(target_os = "linux"))]
    #[error("drag is not supported on this platform")]
    UnsupportedPlatform,
    #[error("failed to start drag operation")]
    FailedToStartDrag,
}

impl Serialize for Error {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.to_string().as_ref())
    }
}

/// Base64-encoded PNG image (with optional `data:image/png;base64,` prefix).
pub struct Base64Image(String);

impl<'de> Deserialize<'de> for Base64Image {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        let stripped = value
            .strip_prefix("data:image/png;base64,")
            .unwrap_or(&value)
            .to_string();
        Ok(Self(stripped))
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
pub enum Image {
    Base64(Base64Image),
    Raw(String),
}

#[derive(Deserialize)]
#[serde(untagged)]
pub enum DragItem {
    /// A list of absolute file paths to drag.
    Files(Vec<PathBuf>),
    /// Data to share with another app.
    Data {
        data: SharedData,
        types: Vec<String>,
    },
}

#[derive(Deserialize)]
#[serde(untagged)]
pub enum SharedData {
    Fixed(String),
    Map(HashMap<String, String>),
}

#[derive(Serialize, Clone)]
pub struct CallbackResult {
    result: &'static str,
    #[serde(rename = "cursorPos")]
    cursor_pos: CursorPosition,
}

#[derive(Serialize, Clone)]
pub struct CursorPosition {
    x: i32,
    y: i32,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum DragMode {
    #[default]
    Copy,
    Move,
}

#[derive(Default, Deserialize)]
pub struct DragOptions {
    #[serde(default)]
    mode: DragMode,
}

impl From<DragMode> for NativeDragMode {
    fn from(value: DragMode) -> Self {
        match value {
            DragMode::Copy => Self::Copy,
            DragMode::Move => Self::Move,
        }
    }
}

#[command]
pub async fn start_drag<R: Runtime>(
    app: AppHandle<R>,
    window: Window<R>,
    item: DragItem,
    image: Option<Image>,
    options: Option<DragOptions>,
    on_event: Channel<CallbackResult>,
) -> Result<()> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (app, window, item, image, options, on_event);
        return Err(Error::UnsupportedPlatform);
    }

    #[cfg(target_os = "linux")]
    {
        let (tx, rx) = crate::channel::oneshot::oneshot();

        let image = image.map(|img| match img {
            Image::Raw(path) => linux::Image::File(PathBuf::from(path)),
            Image::Base64(b) => {
                use base64::Engine;
                linux::Image::Raw(
                    base64::engine::general_purpose::STANDARD
                        .decode(&b.0)
                        .unwrap_or_default(),
                )
            }
        });

        let options = options.unwrap_or_default();

        // ---- 在 /tmp 下创建临时 symlink，确保 Wayland compositor 可访问 ----
        let temp_dir = {
            let pid = std::process::id();
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_micros();
            let dir = std::env::temp_dir().join(format!("hnfm-dnd-{pid}-{ts:x}"));
            if let Err(e) = std::fs::create_dir_all(&dir) {
                tracing::warn!("failed to create drag temp dir {dir:?}: {e}");
            }
            dir
        };

        // 从源文件创建临时副本（空文件占位，后续移除此 mock）
        let (drag_paths, is_data) = match item {
            DragItem::Files(paths) => {
                let links: Vec<PathBuf> = paths
                    .iter()
                    .filter_map(|p| {
                        let name = p.file_name()?;
                        let link = temp_dir.join(name);
                        std::fs::File::create(&link).ok()?;
                        Some(link)
                    })
                    .collect();
                (links, false)
            }
            DragItem::Data { .. } => (vec![], true),
        };

        app.run_on_main_thread(move || {
            let raw_window = match window.gtk_window() {
                Ok(w) => w,
                Err(e) => {
                    if let Err(e) = std::fs::remove_dir_all(&temp_dir) {
                        warn!("failed to remove temp_dir {temp_dir:?}: {e}");
                    }
                    let _ = tx.send(Err(Error::Tauri(e)));
                    return;
                }
            };

            if is_data {
                if let Err(e) = std::fs::remove_dir_all(&temp_dir) {
                    warn!("failed to remove temp_dir {temp_dir:?}: {e}");
                }
                let _ = tx.send(Ok(()));
                return;
            }

            let drag_item = linux::DragItem::Files(drag_paths);

            let r = linux::start_drag_native(
                &raw_window,
                drag_item,
                image.unwrap_or(linux::Image::Raw(vec![])),
                move |result, cursor_pos| {
                    let callback_result = CallbackResult {
                        result: match result {
                            linux::DragResult::Dropped => "Dropped",
                            linux::DragResult::Cancel => "Cancelled",
                        },
                        cursor_pos: CursorPosition {
                            x: cursor_pos.x,
                            y: cursor_pos.y,
                        },
                    };
                    let _ = on_event.send(callback_result);

                    // 延迟清理临时 symlink（给外部 app 时间读取文件）
                    // FIXME: 应使用 CancellationToken 使清理可取消
                    let td = temp_dir.clone();
                    tokio::spawn(
                        async move {
                            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                            if let Err(e) = std::fs::remove_dir_all(&td) {
                                tracing::warn!("failed to cleanup drag temp dir {td:?}: {e}");
                            }
                        }
                        .instrument(tracing::info_span!("drag::delayed_cleanup")),
                    );
                },
                linux::Options {
                    skip_animation_on_cancel_or_failure: false,
                    mode: options.mode.into(),
                },
            )
            .map_err(Error::from);
            let _ = tx.send(r);
        })?;

        rx.recv().map_err(|_| Error::FailedToStartDrag)?
    }
}
