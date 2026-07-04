import React from "react";
import { toast, type ToastOptions } from "react-toastify";
import { t } from "../i18n";
import "./toast.css";

/** Renders a Material Web md-icon element */
const MdIcon = ({ name }: { name: string }) =>
  React.createElement("md-icon", null, name);

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
