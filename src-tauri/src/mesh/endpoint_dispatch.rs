/// 为端点类型生成 `dispatch` 函数。
///
/// 用法：
/// ```ignore
/// endpoint_dispatch!(
///     /// 分发窗口消息
///     WindowMsg -> WindowHandler,
///     dispatch: dispatch_window_msg,
///     DndSessionActive { session_id, files, operation } => on_dnd_active,
///     DndSessionCompleted { session_id } => on_dnd_completed,
/// );
/// ```
///
/// 生成：
/// ```ignore
/// pub fn dispatch_window_msg(msg: &WindowMsg, handler: &dyn WindowHandler) {
///     match msg {
///         WindowMsg::DndSessionActive { session_id, files, operation } => {
///             handler.on_dnd_active(session_id.clone(), files.clone(), operation.clone())
///         }
///         WindowMsg::DndSessionCompleted { session_id } => {
///             handler.on_dnd_completed(session_id.clone())
///         }
///     }
/// }
/// ```
#[macro_export]
macro_rules! endpoint_dispatch {
    (
        $(#[$meta:meta])*
        $msg_ty:ident -> $trait_ty:ident,
        dispatch: $fn_name:ident,
        $(
            $variant:ident { $($field:ident),* $(,)? } => $method:ident,
        )*
    ) => {
        $(#[$meta])*
        pub fn $fn_name(msg: &$msg_ty, handler: &dyn $trait_ty) {
            match msg {
                $(
                    $msg_ty::$variant { $($field),* } => {
                        handler.$method($($field.clone()),*)
                    }
                )*
            }
        }
    };
}
