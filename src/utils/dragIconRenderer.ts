import { isTauri } from '@tauri-apps/api/core';

const ICON_NAMES = [
  'folder',
  'image',
  'audio_file',
  'movie',
  'picture_as_pdf',
  'article',
  'folder_zip',
  'insert_drive_file',
  'terminal',
] as const;

function renderIcon(name: string, size: number, filled: boolean): Promise<Blob> {
  return new Promise<Blob>((resolve, reject) => {
    const fontSize = Math.round(size * 0.7);

    const canvas = document.createElement('canvas');
    canvas.width = size;
    canvas.height = size;
    const ctx = canvas.getContext('2d')!;
    ctx.textAlign = 'center';
    ctx.textBaseline = 'middle';
    ctx.font = `${fontSize}px "Material Symbols Rounded Variable"`;

    const fillValue = filled ? 1 : 0;
    if ('fontVariationSettings' in ctx) {
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      (ctx as any).fontVariationSettings = `'FILL' ${fillValue}`;
    }

    const style = getComputedStyle(document.documentElement);
    ctx.fillStyle = style.getPropertyValue('--md-sys-color-on-surface-variant').trim() || '#444746';
    ctx.fillText(name, size / 2, size / 2 + 1);

    canvas.toBlob((b) => {
      if (b) resolve(b); else reject(new Error('toBlob returned null'));
    }, 'image/png');
  });
}

let initialized = false;

/**
 * 预渲染拖放图标并缓存到后端（Electron 时代 API）。
 *
 * 在 Tauri 迁移阶段，浏览器 mock 环境下不需要真正的拖放图标，
 * 此函数在非 Tauri 环境中跳过，避免不必要的 canvas 操作。
 */
export async function initDragIcons(): Promise<void> {
  if (initialized) return;
  initialized = true;

  // 仅在 Tauri 环境中执行（需要原生拖放图标）
  if (!isTauri()) {
    console.log("[mock] initDragIcons: skipped (browser)");
    return;
  }

  try {
    const size = 96;

    if (!document?.fonts?.ready) {
      console.warn("[mock] initDragIcons: document.fonts not available, skipping");
      return;
    }
    await document.fonts.ready;

    const tasks: Promise<void>[] = [];
    for (const name of ICON_NAMES) {
      for (const filled of [false, true]) {
        const key = filled ? `${name}:filled` : name;
        tasks.push(
          renderIcon(name, size, filled)
            .then(async (blob) => {
              const buf = await blob.arrayBuffer();
              const bytes = new Uint8Array(buf);
              let binary = '';
              for (let i = 0; i < bytes.length; i++) {
                binary += String.fromCharCode(bytes[i]);
              }
              const base64 = btoa(binary);
              window.electron.cacheDragIcon(key, base64);
            })
            .catch(() => {})
        );
      }
    }

    await Promise.allSettled(tasks);
  } catch (e) {
    console.warn("[mock] initDragIcons: failed", e);
  }
}
