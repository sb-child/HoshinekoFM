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

function renderIcon(name: string, size: number): Promise<Blob> {
  return new Promise<Blob>((resolve, reject) => {
    const canvas = document.createElement('canvas');
    canvas.width = size;
    canvas.height = size;
    const ctx = canvas.getContext('2d')!;

    ctx.font = `${Math.round(size * 0.7)}px "Material Symbols Rounded"`;
    ctx.textAlign = 'center';
    ctx.textBaseline = 'middle';

    const style = getComputedStyle(document.documentElement);
    const color =
      style.getPropertyValue('--md-sys-color-on-surface-variant').trim() || '#444746';
    ctx.fillStyle = color;

    // Ligature rendering — the browser's text shaper handles the OpenType liga feature
    ctx.fillText(name, size / 2, size / 2 + 1);

    canvas.toBlob((blob) => {
      if (blob) { resolve(blob); } else { reject(new Error('toBlob returned null')); }
    }, 'image/png');
  });
}

let initialized = false;

export async function initDragIcons(): Promise<void> {
  if (initialized) return;
  initialized = true;

  const size = 96;

  // Wait for the Material Symbols font to be loaded
  await document.fonts.ready;

  // Render and send each icon to the main process for caching
  const tasks = ICON_NAMES.map(async (name) => {
    try {
      const blob = await renderIcon(name, size);
      const buf = await blob.arrayBuffer();
      const bytes = new Uint8Array(buf);

      // Build a base64 string from the bytes
      let binary = '';
      for (let i = 0; i < bytes.length; i++) {
        binary += String.fromCharCode(bytes[i]);
      }
      const base64 = btoa(binary);

      window.electron.cacheDragIcon(name, base64);
    } catch {
      // Silently skip — the generic fallback will be used
    }
  });

  await Promise.allSettled(tasks);
}
