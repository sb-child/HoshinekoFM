import React from "react";
import { toast, type Id as ToastId } from "react-toastify";
import { t } from "../i18n";
import "./toast.css";

type ToastType = "success" | "error" | "info" | "warning";

/**
 * Truncate a filesystem path for display in toast messages.
 * Keeps the last 2 segments; prepends `.../` if truncated.
 */
export function shortPath(p: string, maxLen = 30): string {
  if (p.length <= maxLen) return p;
  const parts = p.split("/").filter(Boolean);
  if (parts.length <= 2) return ".../" + parts[parts.length - 1];
  return ".../" + parts.slice(-2).join("/");
}

/** Renders a Material Web md-icon element */
// eslint-disable-next-line react-refresh/only-export-components
const MdIcon = ({ name }: { name: string }) =>
  React.createElement("md-icon", null, name);

/** Renders a Material Web md-linear-progress element */
// eslint-disable-next-line react-refresh/only-export-components
const MdLinearProgress = ({ indeterminate }: { indeterminate?: boolean }) =>
  React.createElement(
    "md-linear-progress",
    indeterminate ? { indeterminate: "" } : undefined,
  );

/** Default autoClose durations per toast type (ms). */
const AUTO_CLOSE: Record<ToastType, number | false> = {
  success: 3000,
  error: 5000,
  info: 3000,
  warning: 4000,
};

/**
 * Progress toast state stored alongside each progress toast.
 * Used by {@link updateProgress} and {@link finishToast} to re-render.
 */
interface ProgressState {
  message: string;
  total?: number;
  current: number;
  onCancel?: () => void;
}

const progressStates = new Map<ToastId, ProgressState>();

/**
 * Internal React component rendered inside a progress toast.
 * Shows a Material linear progress bar with optional cancel button and count.
 */
// eslint-disable-next-line react-refresh/only-export-components
const ProgressToastContent: React.FC<{
  message: string;
  total?: number;
  current: number;
  onCancel?: () => void;
}> = ({ message, total, current, onCancel }) => (
  <div className="toast-progress">
    <div className="toast-progress-header">
      <span className="toast-message">{message}</span>
      {onCancel && (
        <button
          className="toast-action-btn"
          onClick={(e) => {
            e.stopPropagation();
            onCancel();
          }}
          title={t("toast.cancel_action")}
        >
          <MdIcon name="close" />
        </button>
      )}
    </div>
    {total !== undefined ? (
      <div className="toast-progress-bar">
        <div className="toast-progress-track">
          <div
            className="toast-progress-fill"
            style={{
              width: total > 0
                ? `${Math.min((current / total) * 100, 100)}%`
                : "0%",
            }}
          />
        </div>
        <span className="toast-progress-count">
          {t("toast.progress_count", current, total)}
        </span>
      </div>
    ) : (
      <MdLinearProgress indeterminate />
    )}
  </div>
);

/**
 * Show a simple toast notification.
 *
 * @param message The message text.
 * @param type Toast type — controls icon, color, and autoClose duration.
 * @returns Toast ID usable with {@link dismissToast}.
 */
export function showToast(
  message: string,
  type: ToastType = "info",
): ToastId {
  return toast(
    <span className="toast-message">{message}</span>,
    {
      type,
      autoClose: AUTO_CLOSE[type],
      closeButton: ({
        closeToast,
      }: {
        closeToast: (e: React.MouseEvent) => void;
      }) => (
        <div className="toast-actions">
          <button
            className="toast-action-btn"
            onClick={(e) => {
              e.stopPropagation();
              navigator.clipboard.writeText(message);
            }}
            title={t("toast.copy_action")}
          >
            <MdIcon name="content_copy" />
          </button>
          <button
            className="toast-action-btn"
            onClick={closeToast}
            title={t("toast.close_action")}
          >
            <MdIcon name="close" />
          </button>
        </div>
      ),
    },
  );
}

/** Options for {@link showProgressToast}. */
export interface ProgressToastOptions {
  /**
   * Total item count for determininate progress.
   * Omit (or pass `undefined`) for an indeterminate spinner —
   * suitable for "Loading directory..." etc.
   */
  total?: number;
  /**
   * Called when the user clicks the cancel button.
   * The button is only shown when this callback is provided.
   */
  onCancel?: () => void;
}

/**
 * Show a toast with a progress bar.
 *
 * - If `total` is provided, the progress bar is determininate and a
 *   count text (`current / total`) is displayed below it.
 * - Otherwise an indeterminate `<md-linear-progress>` spinner is shown.
 *
 * Call {@link updateProgress} to advance the count; call
 * {@link finishToast} to transition the toast into its final state
 * (success / error / warning).
 *
 * @param message Initial message text.
 * @param options Progress configuration.
 * @returns Toast ID for use with the update/dismiss functions.
 */
export function showProgressToast(
  message: string,
  options: ProgressToastOptions = {},
): ToastId {
  const data: ProgressState = {
    message,
    total: options.total,
    current: 0,
    onCancel: options.onCancel,
  };

  const id = toast(
    <ProgressToastContent
      message={data.message}
      total={data.total}
      current={data.current}
      onCancel={data.onCancel}
    />,
    {
      autoClose: false,
      draggable: false,
      isLoading: true,
    },
  );

  progressStates.set(id, data);
  return id;
}

/**
 * Update the progress count on a determininate progress toast.
 *
 * The toast must have been created with `total` set.
 *
 * @param id Toast ID returned by {@link showProgressToast}.
 * @param current Number of items completed so far.
 * @param message Optional new message text (updates the heading).
 */
export function updateProgress(
  id: ToastId,
  current: number,
  message?: string,
): void {
  const data = progressStates.get(id);
  if (!data) return;

  data.current = current;
  if (message !== undefined) data.message = message;

  toast.update(id, {
    render: (
      <ProgressToastContent
        message={data.message}
        total={data.total}
        current={data.current}
        onCancel={data.onCancel}
      />
    ),
  });
}

/**
 * Transition a progress toast into its final state.
 *
 * Cleans up internal state, switches to a plain toast with the
 * given type and message.  The toast auto-closes according to the
 * type's default duration.
 *
 * @param id Toast ID returned by {@link showProgressToast}.
 * @param message Final message text.
 * @param type Final toast type.
 */
export function finishToast(
  id: ToastId,
  message: string,
  type: ToastType,
): void {
  progressStates.delete(id);

  toast.update(id, {
    render: <span className="toast-message">{message}</span>,
    type,
    autoClose: AUTO_CLOSE[type],
    isLoading: false,
    closeButton: ({
      closeToast,
    }: {
      closeToast: (e: React.MouseEvent) => void;
    }) => (
      <div className="toast-actions">
        <button
          className="toast-action-btn"
          onClick={(e) => {
            e.stopPropagation();
            navigator.clipboard.writeText(message);
          }}
          title={t("toast.copy_action")}
        >
          <MdIcon name="content_copy" />
        </button>
        <button
          className="toast-action-btn"
          onClick={closeToast}
          title={t("toast.close_action")}
        >
          <MdIcon name="close" />
        </button>
      </div>
    ),
  });
}

/**
 * Dismiss a toast immediately (either a regular toast or a progress
 * toast).  Safe to call with any toast ID.
 *
 * @param id Toast ID returned by {@link showToast} or {@link showProgressToast}.
 */
export function dismissToast(id: ToastId): void {
  progressStates.delete(id);
  toast.dismiss(id);
}
