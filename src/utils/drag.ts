/**
 * Native file drag utility for HoshinekoFM.
 *
 * Provides a replacement for the Electron-era {@link window.electron.startDrag}.
 * Calls the Tauri `start_drag` command in the embedded `drag` module.
 *
 * API adapted from:
 *   https://www.npmjs.com/package/@crabnebula/tauri-plugin-drag (CrabNebula Ltd., Apache-2.0 OR MIT)
 */

import { invoke, Channel } from "@tauri-apps/api/core";

/** Drag payload: either a list of absolute file paths, or arbitrary data. */
export type DragItem =
  | string[]
  | { data: string | Record<string, string>; types: string[] };

/** Result returned after the drop session finishes. */
export type DragResult = "Dropped" | "Cancelled";

/** Logical cursor position at drop. */
export interface CursorPosition {
  x: number;
  y: number;
}

/** Payload passed to the `onEvent` callback. */
export interface CallbackPayload {
  result: DragResult;
  cursorPos: CursorPosition;
}

/** Options for {@link startDrag}. */
export interface StartDragOptions {
  /** Files (absolute paths) or data to drag. */
  item: DragItem;
  /** Preview icon. Either an absolute file path OR a base64-encoded PNG string (with optional `data:image/png;base64,` prefix). */
  icon?: string;
  /** Drag operation mode. Defaults to `"copy"`. */
  mode?: "copy" | "move";
}

/**
 * Starts a native file drag operation out of the Tauri window.
 *
 * @example
 * ```ts
 * import { startDrag } from "../utils/drag";
 *
 * // Drag files to another app:
 * startDrag({ item: ["/home/user/photo.png"] }, (payload) => {
 *   console.log("Drag result:", payload.result);
 * });
 * ```
 *
 * @param options  The drag options (files/data, preview icon, mode).
 * @param onEvent  Optional callback receiving the final drag result and cursor position.
 */
export async function startDrag(
  options: StartDragOptions,
  onEvent?: (payload: CallbackPayload) => void,
): Promise<void> {
  const onEventChannel = new Channel<CallbackPayload>();
  if (onEvent) {
    onEventChannel.onmessage = onEvent;
  }
  await invoke("start_drag", {
    item: options.item,
    image: options.icon,
    options: {
      mode: options.mode,
    },
    onEvent: onEventChannel,
  });
}
