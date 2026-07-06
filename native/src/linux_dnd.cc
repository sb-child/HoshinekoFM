/**
 * linux_dnd.cc — GDK4-based native drag-and-drop addon for Electron on Linux.
 *
 * Replaces Electron's broken `startDrag()` on Wayland by using GDK4's own
 * drag-and-drop implementation, which works correctly on both X11 and Wayland.
 *
 * Architecture:
 *   - Initializes GTK/GDK in a headless fashion (hidden 1×1 window).
 *   - After realizing the widget, pumps the GLib event loop to ensure the
 *     Wayland surface receives its configure ack from the compositor.
 *   - On startDrag(), builds a text/uri-list string and wraps it in a
 *     GdkContentProvider.
 *   - Runs a GLib main loop synchronously until the drag completes.
 *   - Returns the drop action (copy / move / none).
 *
 * Only supports Linux (X11 + Wayland).  Does nothing on other platforms.
 */

#include <napi.h>
#include <gtk/gtk.h>
#include <gio/gio.h>

#include <string>
#include <cstdio>

// ═══════════════════════════════════════════════════════════════════
//  Debug logging helper
// ═══════════════════════════════════════════════════════════════════

#define DLOG(fmt, ...)  ::fprintf(stderr, "[linux_dnd] " fmt "\n", ##__VA_ARGS__)

// ═══════════════════════════════════════════════════════════════════
//  Module-level state
// ═══════════════════════════════════════════════════════════════════

static GtkWidget  *drag_window  = nullptr;
static GdkSurface *drag_surface = nullptr;
static GMainLoop  *main_loop    = nullptr;
static bool        gdk_ready    = false;

/** Stores the result action from the last drag session. */
static int drag_result_action = 0;

// ═══════════════════════════════════════════════════════════════════
//  Signal callbacks
// ═══════════════════════════════════════════════════════════════════

/** Fired when the drag operation finishes (drop or cancel).
 *  Captures the selected action and quits the GLib main loop. */
static void on_drag_dnd_finished(GdkDrag *drag, gpointer /*user_data*/) {
  drag_result_action = static_cast<int>(gdk_drag_get_selected_action(drag));
  DLOG("dnd-finished: action=%d", drag_result_action);
  if (main_loop && g_main_loop_is_running(main_loop)) {
    g_main_loop_quit(main_loop);
  }
}

/** Fired when the drag is explicitly cancelled (e.g. Escape key). */
static void on_drag_cancel(GdkDrag * /*drag*/, GdkDragCancelReason reason,
                           gpointer /*user_data*/) {
  DLOG("drag-cancel: reason=%d", static_cast<int>(reason));
  drag_result_action = 0;
  if (main_loop && g_main_loop_is_running(main_loop)) {
    g_main_loop_quit(main_loop);
  }
}

// ═══════════════════════════════════════════════════════════════════
//  Exported JS functions
// ═══════════════════════════════════════════════════════════════════

/**
 * init() — Initialize GTK/GDK.
 *
 * Creates a hidden 1×1 toplevel window that serves as the drag-source
 * surface.  After realizing, we pump the GLib event loop to let the
 * Wayland compositor configure the surface (xdg_surface.configure →
 * ack), which is required before the surface can be used as a drag
 * source.
 *
 * Must be called once before startDrag().
 *
 * @returns {boolean} true on success, throws on failure.
 */
static Napi::Value Init(const Napi::CallbackInfo &info) {
  Napi::Env env = info.Env();

  if (gdk_ready) {
    return Napi::Boolean::New(env, true);
  }

  // Initialize GTK (which also initializes GDK)
  if (!gtk_init_check()) {
    Napi::Error::New(env, "linux_dnd: gtk_init_check() failed")
      .ThrowAsJavaScriptException();
    return env.Undefined();
  }

  DLOG("gtk_init_check OK — backend: %s",
       gdk_display_get_name(gdk_display_get_default()));

  // ── Create a tiny, invisible toplevel window ──────────────────
  drag_window = gtk_window_new();
  gtk_window_set_default_size(GTK_WINDOW(drag_window), 1, 1);
  gtk_window_set_decorated(GTK_WINDOW(drag_window), FALSE);
  gtk_widget_set_opacity(drag_window, 0.0);

  // ── Realize → create Wayland wl_surface ───────────────────────
  gtk_widget_realize(drag_window);
  drag_surface = gtk_native_get_surface(GTK_NATIVE(drag_window));
  DLOG("widget realized, surface=%p", static_cast<void *>(drag_surface));

  // ── Map → send xdg_toplevel configure ─────────────────────────
  gtk_widget_set_visible(drag_window, TRUE);

  // ── Pump events until the compositor has ack'd the configure ──
  //    Without this the Wayland surface has no valid role and
  //    wl_data_device.start_drag() may be rejected by the compositor.
  GMainContext *ctx = g_main_context_default();
  bool mapped = false;
  for (int i = 0; i < 100; i++) {
    // Process any pending events (non-blocking)
    while (g_main_context_pending(ctx)) {
      g_main_context_iteration(ctx, FALSE);  // may_block=false
    }

    // Block briefly if still not mapped, otherwise we're done
    if (gtk_widget_get_mapped(drag_window)) {
      mapped = true;
      break;
    }
    // Block for at most 50ms waiting for the next Wayland event
    g_main_context_iteration(ctx, TRUE);  // may_block=true
  }

  DLOG("surface mapped=%d after pump", mapped ? 1 : 0);

  // ── Create the GLib main loop (run only during startDrag) ─────
  main_loop = g_main_loop_new(nullptr, FALSE);

  gdk_ready = true;
  return Napi::Boolean::New(env, true);
}

/**
 * startDrag(files: string[], iconPath?: string) → { action: string }
 *
 * Initiates a native file-drag operation via GDK4.
 *
 * This call **blocks synchronously** until the drag completes (drop,
 * cancel, or Escape).  The Electron main process event loop is paused
 * during the drag, but the renderer (frontend) continues rendering
 * normally — it is a separate Chromium process.
 *
 * @param {string[]} files  — Absolute file paths to include in the drag.
 * @param {string}  [iconPath] — Optional path to a PNG icon (not yet used).
 *
 * @returns {{ action: 'copy' | 'move' | 'none' }}
 *
 * @throws {Error} if GDK is not initialized or no valid files provided.
 */
static Napi::Value StartDrag(const Napi::CallbackInfo &info) {
  Napi::Env env = info.Env();

  if (!gdk_ready || !drag_surface || !main_loop) {
    Napi::Error::New(env, "linux_dnd: not initialized — call init() first")
      .ThrowAsJavaScriptException();
    return env.Undefined();
  }

  // ── Parse arguments ──────────────────────────────────────────
  if (info.Length() < 1 || !info[0].IsArray()) {
    Napi::TypeError::New(env, "Expected files (string[]) as first argument")
      .ThrowAsJavaScriptException();
    return env.Undefined();
  }

  Napi::Array files_arr = info[0].As<Napi::Array>();
  const uint32_t count = files_arr.Length();
  if (count == 0) {
    Napi::Error::New(env, "No file paths provided")
      .ThrowAsJavaScriptException();
    return env.Undefined();
  }

  // ── Build text/uri-list string (RFC 2483) ────────────────────
  GString *uri_list = g_string_new(nullptr);
  guint valid_count = 0;
  for (uint32_t i = 0; i < count; i++) {
    Napi::Value elem = files_arr.Get(i);
    if (!elem.IsString()) continue;
    std::string path_str = elem.As<Napi::String>().Utf8Value();
    GFile *gfile = g_file_new_for_path(path_str.c_str());
    char *uri = g_file_get_uri(gfile);        // "file:///home/..."
    g_string_append(uri_list, uri);
    g_string_append(uri_list, "\r\n");         // RFC 2483 CRLF
    g_free(uri);
    g_object_unref(gfile);
    valid_count++;
  }

  if (valid_count == 0) {
    g_string_free(uri_list, TRUE);
    Napi::Error::New(env, "No valid file paths provided")
      .ThrowAsJavaScriptException();
    return env.Undefined();
  }

  DLOG("startDrag: %u files, uri_list=%.200s", valid_count, uri_list->str);

  // ── Create content provider ──────────────────────────────────
  GBytes *file_bytes = g_bytes_new(uri_list->str, uri_list->len);
  GdkContentProvider *content = gdk_content_provider_new_for_bytes(
    "text/uri-list", file_bytes);
  g_bytes_unref(file_bytes);
  g_string_free(uri_list, TRUE);

  // Print the MIME types that our content provider exposes
  {
    GdkContentFormats *fmts = gdk_content_provider_ref_formats(content);
    gsize n_mime = 0;
    const char * const *mime_types = gdk_content_formats_get_mime_types(fmts, &n_mime);
    DLOG("content provider MIME types (%zu):", n_mime);
    for (gsize j = 0; j < n_mime; j++) {
      DLOG("  [%zu] %s", j, mime_types[j]);
    }
    gdk_content_formats_unref(fmts);
  }

  // ── Get pointer device ────────────────────────────────────────
  GdkDisplay *display = gdk_surface_get_display(drag_surface);
  GdkSeat *seat = gdk_display_get_default_seat(display);
  GdkDevice *device = gdk_seat_get_pointer(seat);

  if (!device) {
    g_object_unref(content);
    Napi::Error::New(env, "linux_dnd: no pointer device found")
      .ThrowAsJavaScriptException();
    return env.Undefined();
  }

  // ── Start the drag ────────────────────────────────────────────
  DLOG("calling gdk_drag_begin(surface=%p, device=%p, content=%p, actions=COPY|MOVE)",
       static_cast<void *>(drag_surface),
       static_cast<void *>(device),
       static_cast<void *>(content));

  GdkDrag *drag = gdk_drag_begin(
    drag_surface,
    device,
    content,
    static_cast<GdkDragAction>(GDK_ACTION_COPY | GDK_ACTION_MOVE),
    0, 0);

  g_object_unref(content);

  DLOG("gdk_drag_begin returned drag=%p", static_cast<void *>(drag));

  if (!drag) {
    Napi::Error::New(env, "linux_dnd: gdk_drag_begin() returned null")
      .ThrowAsJavaScriptException();
    return env.Undefined();
  }

  // ── Wire up signals ───────────────────────────────────────────
  drag_result_action = 0;
  g_signal_connect(drag, "dnd-finished", G_CALLBACK(on_drag_dnd_finished), nullptr);
  g_signal_connect(drag, "cancel",       G_CALLBACK(on_drag_cancel),       nullptr);

  // ── Block until drag completes ─────────────────────────────────
  DLOG("entering g_main_loop_run...");
  g_main_loop_run(main_loop);
  DLOG("g_main_loop_run returned, drag_result_action=%d", drag_result_action);

  g_object_unref(drag);

  // ── Return result ─────────────────────────────────────────────
  Napi::Object result = Napi::Object::New(env);
  if (drag_result_action == static_cast<int>(GDK_ACTION_COPY)) {
    result.Set("action", Napi::String::New(env, "copy"));
  } else if (drag_result_action == static_cast<int>(GDK_ACTION_MOVE)) {
    result.Set("action", Napi::String::New(env, "move"));
  } else {
    result.Set("action", Napi::String::New(env, "none"));
  }
  return result;
}

/**
 * destroy() — Clean up GTK resources.
 *
 * Should be called when the app exits (typically not needed for a
 * long-running Electron app, but provided for completeness).
 */
static Napi::Value Destroy(const Napi::CallbackInfo &info) {
  if (main_loop) {
    g_main_loop_unref(main_loop);
    main_loop = nullptr;
  }
  if (drag_window) {
    gtk_window_destroy(GTK_WINDOW(drag_window));
    drag_window = nullptr;
    drag_surface = nullptr;
  }
  gdk_ready = false;
  return info.Env().Undefined();
}

// ═══════════════════════════════════════════════════════════════════
//  Module registration
// ═══════════════════════════════════════════════════════════════════

static Napi::Object RegisterModule(Napi::Env env, Napi::Object exports) {
  exports.Set("init",      Napi::Function::New(env, Init));
  exports.Set("startDrag", Napi::Function::New(env, StartDrag));
  exports.Set("destroy",   Napi::Function::New(env, Destroy));
  return exports;
}

NODE_API_MODULE(linux_dnd, RegisterModule)
