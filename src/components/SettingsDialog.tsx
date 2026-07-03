import React from "react";
import { Dialog } from "./Dialog";
import { Button } from "./Button";
import { Icon } from "./Icon";
import { Switch, Slider } from "./md";
import { t as ti } from '../i18n';

interface SettingsDialogProps {
  open: boolean;
  onClose: () => void;
  showHiddenFiles: boolean;
  onToggleHiddenFiles: () => void;
  iconSize: number;
  onIconSizeChange: (size: number) => void;
  viewMode: "grid" | "list";
  onViewModeChange: (mode: "grid" | "list") => void;
  filledIcons: boolean;
  onToggleFilledIcons: () => void;
  onImportCss: () => void;
  customCssPath?: string;
}

const labelToKey: Record<string, string> = {
  Settings: "settings.title",
  Done: "settings.done",
  "Show Hidden Files": "settings.show_hidden",
  Appearance: "settings.appearance",
  "View Mode": "settings.view_mode",
  Grid: "settings.grid",
  List: "settings.list",
  "Icon Size": "settings.icon_size",
  "Filled Icons": "settings.filled_icons",
  Customization: "settings.customization",
  "Custom CSS": "settings.custom_css",
  "Import CSS": "settings.import_css",
};

const tSettings = (text: string) => {
  const key = labelToKey[text];
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  return key ? (ti as any)(key) : text;
};

export const SettingsDialog: React.FC<SettingsDialogProps> = ({
  open,
  onClose,
  showHiddenFiles,
  onToggleHiddenFiles,
  iconSize,
  onIconSizeChange,
  viewMode,
  onViewModeChange,
  filledIcons,
  onToggleFilledIcons,
  onImportCss,
  customCssPath,
}) => {
  return (
    <Dialog
      title={tSettings("Settings")}
      open={open}
      onClose={onClose}
      actions={
        <Button onClick={onClose} variant="filled">
          {tSettings("Done")}
        </Button>
      }
    >
      <div style={{ padding: "0 8px", minWidth: "300px" }}>
        <div
          onClick={onToggleHiddenFiles}
          style={{
            display: "flex",
            alignItems: "center",
            justifyContent: "space-between",
            padding: "12px 0",
            cursor: "pointer",
            userSelect: "none",
          }}
        >
          <div style={{ display: "flex", alignItems: "center", gap: "16px" }}>
            <Icon name={showHiddenFiles ? "visibility" : "visibility_off"} />
            <div style={{ fontSize: "16px" }}>
              {tSettings("Show Hidden Files")}
            </div>
          </div>
          {/* Material 3 Switch */}
          <Switch
            selected={showHiddenFiles}
            onClick={onToggleHiddenFiles}
          />
        </div>

        <div
          style={{
            padding: "12px 0",
            borderTop: "1px solid var(--md-sys-color-outline-variant)",
          }}
        >
          <div
            style={{
              fontSize: "14px",
              color: "var(--md-sys-color-primary)",
              fontWeight: 500,
              marginBottom: "8px",
            }}
          >
            {tSettings("Appearance")}
          </div>

          {/* View Mode */}
          <div style={{ marginBottom: "16px" }}>
            <div style={{ marginBottom: "8px" }}>{tSettings("View Mode")}</div>
            <div style={{ display: "flex", gap: "8px" }}>
              <Button
                variant={viewMode === "grid" ? "filled" : "outlined"}
                onClick={() => onViewModeChange("grid")}
              >
                <Icon name="grid_view" /> {tSettings("Grid")}
              </Button>
              <Button
                variant={viewMode === "list" ? "filled" : "outlined"}
                onClick={() => onViewModeChange("list")}
              >
                <Icon name="view_list" /> {tSettings("List")}
              </Button>
            </div>
          </div>

          {/* Icon Size */}
          <div style={{ marginBottom: "16px" }}>
            <div
              style={{
                marginBottom: "8px",
                display: "flex",
                justifyContent: "space-between",
              }}
            >
              <span>{tSettings("Icon Size")}</span>
              <span>{iconSize}px</span>
            </div>
            <Slider
              min={16}
              max={128}
              step={8}
              value={iconSize}
              onInput={(e) => onIconSizeChange(Number((e.target as HTMLInputElement).value))}
              style={{ width: "100%" }}
            />
          </div>

          {/* Filled Icons */}
          <div
            onClick={onToggleFilledIcons}
            style={{
              display: "flex",
              alignItems: "center",
              justifyContent: "space-between",
              cursor: "pointer",
              userSelect: "none",
            }}
          >
            <div style={{ display: "flex", alignItems: "center", gap: "16px" }}>
              <Icon name="favorite" filled={filledIcons} />
              <div style={{ fontSize: "16px" }}>
                {tSettings("Filled Icons")}
              </div>
            </div>
            <Switch
              selected={filledIcons}
              onClick={onToggleFilledIcons}
            />
          </div>
        </div>

        <div
          style={{
            padding: "12px 0",
            borderTop: "1px solid var(--md-sys-color-outline-variant)",
          }}
        >
          <div
            style={{
              fontSize: "14px",
              color: "var(--md-sys-color-primary)",
              fontWeight: 500,
              marginBottom: "8px",
            }}
          >
            {tSettings("Customization")}
          </div>
          <div
            style={{
              display: "flex",
              alignItems: "center",
              justifyContent: "space-between",
            }}
          >
            <div>
              <div style={{ fontSize: "16px" }}>{tSettings("Custom CSS")}</div>
              {customCssPath && (
                <div
                  style={{
                    fontSize: "12px",
                    color: "var(--md-sys-color-on-surface-variant)",
                    maxWidth: "200px",
                    overflow: "hidden",
                    textOverflow: "ellipsis",
                    whiteSpace: "nowrap",
                  }}
                >
                  {customCssPath}
                </div>
              )}
            </div>
            <Button variant="outlined" onClick={onImportCss}>
              {tSettings("Import CSS")}
            </Button>
          </div>
        </div>
      </div>
    </Dialog>
  );
};
