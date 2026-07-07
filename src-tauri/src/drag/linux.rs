// Linux (GTK) native drag implementation.
//
// Source: https://crates.io/crates/drag (CrabNebula Ltd., Apache-2.0 OR MIT)
//   - src/platform_impl/gtk/mod.rs
//   - src/lib.rs (public types)

use std::{
    rc::Rc,
    sync::{Arc, Mutex},
};

use gtk::{
    gdk, gdk_pixbuf,
    glib::{ObjectExt, Propagation, SignalHandlerId},
    prelude::{
        DeviceExt, DragContextExtManual, PixbufLoaderExt, SeatExt, WidgetExt, WidgetExtManual,
    },
};

/// Result of a drag operation.
#[derive(Debug, Clone, Copy)]
pub enum DragResult {
    Dropped,
    Cancel,
}

/// Item to be dragged.
pub enum DragItem {
    Files(Vec<std::path::PathBuf>),
}

/// An image definition for the drag preview.
pub enum Image {
    File(std::path::PathBuf),
    Raw(Vec<u8>),
}

/// Drag operation mode.
#[derive(Debug, Clone, Copy, Default)]
pub enum DragMode {
    #[default]
    Copy,
    Move,
}

/// Drag options.
#[derive(Default)]
pub struct Options {
    pub skip_animation_on_cancel_or_failure: bool,
    pub mode: DragMode,
}

/// Logical cursor position.
#[derive(Debug, Clone)]
pub struct CursorPosition {
    pub x: i32,
    pub y: i32,
}

/// Errors that can occur during a drag operation.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("failed to start drag")]
    FailedToStartDrag,
    #[error("empty drag target list")]
    EmptyTargetList,
    #[error("failed to get cursor position")]
    FailedToGetCursorPosition,
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

/// Starts a native drag operation on the given GTK window.
pub fn start_drag_native<F: Fn(DragResult, CursorPosition) + Send + 'static>(
    window: &gtk::ApplicationWindow,
    item: DragItem,
    image: Image,
    on_drop_callback: F,
    options: Options,
) -> Result<(), Error> {
    tracing::debug!("Starting native drag with mode: {:?}", options.mode);

    let handler_ids: Arc<Mutex<Vec<SignalHandlerId>>> = Arc::new(Mutex::new(vec![]));

    let drag_action = match options.mode {
        DragMode::Copy => gdk::DragAction::COPY,
        DragMode::Move => gdk::DragAction::MOVE,
    };

    tracing::debug!("Setting drag source with action: {:?}", drag_action);
    window.drag_source_set(gdk::ModifierType::BUTTON1_MASK, &[], drag_action);

    match item {
        DragItem::Files(paths) => {
            tracing::debug!("Setting up file drag with {} paths", paths.len());
            window.drag_source_add_uri_targets();
            handler_ids
                .lock()
                .unwrap()
                .push(window.connect_drag_data_get(move |_, _, data, _, _| {
                    tracing::debug!("Preparing URIs for drag data");
                    let uris: Vec<String> = paths
                        .iter()
                        .map(|path| format!("file://{}", path.display()))
                        .collect();
                    let uris: Vec<&str> = uris.iter().map(|s| s.as_str()).collect();
                    tracing::debug!("Setting URIs: {:?}", uris);
                    data.set_uris(&uris);
                }));
        }
    }

    let target_list = window
        .drag_source_get_target_list()
        .ok_or(Error::EmptyTargetList)?;

    tracing::debug!("Got target list, initiating drag");

    let drag_context = window
        .drag_begin_with_coordinates(
            &target_list,
            drag_action,
            gdk::ffi::GDK_BUTTON1_MASK as i32,
            None,
            -1,
            -1,
        )
        .ok_or(Error::FailedToStartDrag)?;

    tracing::debug!("Drag context created successfully");

    let callback = Rc::new(on_drop_callback);
    on_drop_failed(callback.clone(), window, &handler_ids, &options);
    on_drop_performed(callback.clone(), window, &handler_ids, &drag_context);

    tracing::debug!("Setting up drag icon");
    let icon_pixbuf: Option<gdk_pixbuf::Pixbuf> = match &image {
        Image::Raw(data) => image_binary_to_pixbuf(data),
        Image::File(path) => std::fs::read(path)
            .ok()
            .and_then(|bytes| image_binary_to_pixbuf(&bytes)),
    };
    if let Some(icon) = icon_pixbuf {
        drag_context.drag_set_icon_pixbuf(&icon, 0, 0);
    }

    Ok(())
}

fn image_binary_to_pixbuf(data: &[u8]) -> Option<gdk_pixbuf::Pixbuf> {
    let loader = gdk_pixbuf::PixbufLoader::new();
    loader.write(data).and_then(|_| loader.close()).ok()?;
    loader.pixbuf()
}

fn clear_signal_handlers(window: &gtk::ApplicationWindow, handler_ids: &mut Vec<SignalHandlerId>) {
    for handler_id in handler_ids.drain(..) {
        window.disconnect(handler_id);
    }
}

fn on_drop_failed<F: Fn(DragResult, CursorPosition) + Send + 'static>(
    callback: Rc<F>,
    window: &gtk::ApplicationWindow,
    handler_ids: &Arc<Mutex<Vec<SignalHandlerId>>>,
    options: &Options,
) {
    tracing::debug!("Setting up drop failed handler");
    let window_clone = window.clone();
    let handler_ids_clone = handler_ids.clone();
    let skip_animation = options.skip_animation_on_cancel_or_failure;

    handler_ids
        .lock()
        .unwrap()
        .push(window.connect_drag_failed(move |_, _, _| {
            tracing::debug!("Drag failed or cancelled");
            callback(
                DragResult::Cancel,
                get_cursor_position(&window_clone).unwrap_or(CursorPosition { x: 0, y: 0 }),
            );
            cleanup_signal_handlers(&handler_ids_clone, &window_clone);
            if skip_animation {
                Propagation::Stop
            } else {
                Propagation::Proceed
            }
        }));
}

fn cleanup_signal_handlers(
    handler_ids: &Arc<Mutex<Vec<SignalHandlerId>>>,
    window: &gtk::ApplicationWindow,
) {
    tracing::debug!("Cleaning up signal handlers");
    let handler_ids = &mut handler_ids.lock().unwrap();
    clear_signal_handlers(window, handler_ids);
    window.drag_source_unset();
    tracing::debug!("Signal handlers cleaned up");
}

fn on_drop_performed<F: Fn(DragResult, CursorPosition) + Send + 'static>(
    callback: Rc<F>,
    window: &gtk::ApplicationWindow,
    handler_ids: &Arc<Mutex<Vec<SignalHandlerId>>>,
    drag_context: &gdk::DragContext,
) {
    tracing::debug!("Setting up drop performed handler");
    let window = window.clone();
    let handler_ids = handler_ids.clone();

    drag_context.connect_drop_performed(move |_context, _| {
        tracing::debug!("Drop performed successfully");
        cleanup_signal_handlers(&handler_ids, &window);
        callback(
            DragResult::Dropped,
            get_cursor_position(&window).unwrap_or(CursorPosition { x: 0, y: 0 }),
        );
    });
}

fn get_cursor_position(window: &gtk::ApplicationWindow) -> Option<CursorPosition> {
    let (_, x, y) = window.display().default_seat()?.pointer()?.position();
    Some(CursorPosition { x, y })
}
