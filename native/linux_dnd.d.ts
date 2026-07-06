/**
 * Type declarations for the linux_dnd native addon.
 * Only meaningful on Linux; `null` on other platforms.
 */
declare module '../../native/build/Release/linux_dnd.node' {
  export interface DragResult {
    /** The drop action: "copy", "move", or "none" */
    action: 'copy' | 'move' | 'none';
  }

  /** Initialize GTK/GDK. Must be called once before startDrag(). */
  export function init(): boolean;

  /**
   * Start a native file-drag operation via GDK4.
   * Blocks synchronously until the drag completes.
   * @param files - Absolute file paths to include in the drag.
   * @param iconPath - Optional PNG icon path for drag cursor (not yet used).
   */
  export function startDrag(files: string[], iconPath?: string): DragResult;

  /** Clean up GTK resources. */
  export function destroy(): void;
}
