import React from "react";
import { toast, type ToastOptions } from "react-toastify";
import { t } from "../i18n";
import "./toast.css";

export function showToast(
  message: string,
  type: "success" | "error" | "info" | "warning" = "info",
) {
  toast(<span className="toast-message">{message}</span>, {
    type: type as ToastOptions["type"],
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
          <span className="material-symbols-rounded">content_copy</span>
        </button>
        <button
          className="toast-action-btn"
          onClick={closeToast}
          title={t("toast.close_action")}
        >
          <span className="material-symbols-rounded">close</span>
        </button>
      </div>
    ),
  });
}
