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

export async function initDragIcons(): Promise<void> {
  if (initialized) return;
  initialized = true;

  const size = 96;

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
}
