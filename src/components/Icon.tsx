import React from "react";

interface IconProps {
  /** Material Symbols icon name (ligature) */
  name: string;
  /** Additional CSS class names */
  className?: string;
  /** Whether the icon should be filled (FILL=1) */
  filled?: boolean;
  /** Custom icon size in pixels */
  size?: number;
  /** Additional inline styles */
  style?: React.CSSProperties;
  /** Slot attribute for shadow DOM placement */
  slot?: string;
}

/** Internal type for md-icon custom element */
type MdIconElement = React.DetailedHTMLProps<
  React.HTMLAttributes<HTMLElement> & { filled?: boolean },
  HTMLElement
>;

const createMdIcon = (props: MdIconElement & { children?: React.ReactNode }): React.ReactElement =>
  React.createElement("md-icon", props as Record<string, unknown>);

/**
 * Material Web icon component wrapping {@link https://material-web.dev/components/icon/ | `<md-icon>`}.
 * Renders Material Symbols Rounded icons by default (set via global CSS `--md-icon-font`).
 */
export const Icon: React.FC<IconProps> = ({
  name,
  className = "",
  filled = false,
  size,
  style = {},
  slot,
}) => {
  // Derive --md-icon-size from `size` prop, or from `style.fontSize` as fallback.
  // `<md-icon>` constrains its intrinsic size to `var(--md-icon-size, 24px)`,
  // so if fontSize is set via style without --md-icon-size, the icon gets clipped.
  const resolvedSize = size ?? (typeof style.fontSize === "number"
    ? style.fontSize
    : typeof style.fontSize === "string"
      ? parseFloat(style.fontSize)
      : undefined);
  const sizeStyle = (resolvedSize !== undefined && !Number.isNaN(resolvedSize)
    ? { "--md-icon-size": `${resolvedSize}px` }
    : {}) as React.CSSProperties;

  return createMdIcon({
    className: className || undefined,
    style: { ...sizeStyle, ...style },
    filled: filled || undefined,
    slot,
    children: name,
  });
};
