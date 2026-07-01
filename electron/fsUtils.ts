import { promises as fs } from 'fs';
import { writeFileSync, existsSync, mkdirSync } from 'fs';
import path from 'path';
import os from 'os';
import { execFile } from 'child_process';
import { promisify } from 'util';
import crypto from 'crypto';

const execFileAsync = promisify(execFile);

// ── MIME detection by magic bytes ──────────────────────────────────

// Read first 16 bytes of a file and return them as a Buffer
async function readHead(filePath: string, bytes = 16): Promise<Buffer | null> {
  try {
    const fd = await fs.open(filePath, 'r');
    const buf = Buffer.alloc(bytes);
    await fd.read(buf, 0, bytes, 0);
    await fd.close();
    return buf;
  } catch {
    return null;
  }
}

function bufStartsWith(buf: Buffer, pattern: number[]): boolean {
  for (let i = 0; i < pattern.length; i++) {
    if (buf[i] !== pattern[i]) return false;
  }
  return true;
}

/** Fast MIME detection by reading file magic bytes. Returns null on failure (caller may fall back to `file` command). */
export async function detectMimeByMagic(filePath: string): Promise<string | null> {
  const head = await readHead(filePath, 16);
  if (!head || head.length < 4) return null;

  // PNG
  if (bufStartsWith(head, [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A])) return 'image/png';
  // JPEG
  if (bufStartsWith(head, [0xFF, 0xD8, 0xFF])) return 'image/jpeg';
  // GIF87a / GIF89a
  if (bufStartsWith(head, [0x47, 0x49, 0x46, 0x38, 0x37, 0x61]) ||
      bufStartsWith(head, [0x47, 0x49, 0x46, 0x38, 0x39, 0x61])) return 'image/gif';
  // BMP
  if (bufStartsWith(head, [0x42, 0x4D])) return 'image/bmp';
  // TIFF (little-endian / big-endian)
  if (bufStartsWith(head, [0x49, 0x49, 0x2A, 0x00]) ||
      bufStartsWith(head, [0x4D, 0x4D, 0x00, 0x2A])) return 'image/tiff';
  // ICO
  if (bufStartsWith(head, [0x00, 0x00, 0x01, 0x00])) return 'image/x-icon';
  // WebP (RIFF .... WEBP)
  if (bufStartsWith(head, [0x52, 0x49, 0x46, 0x46]) && head.length >= 12 &&
      bufStartsWith(head.subarray(8, 12), [0x57, 0x45, 0x42, 0x50])) return 'image/webp';

  // PDF
  if (bufStartsWith(head, [0x25, 0x50, 0x44, 0x46])) return 'application/pdf';

  // ZIP (empty / spanned / normal)
  if (bufStartsWith(head, [0x50, 0x4B, 0x03, 0x04]) ||
      bufStartsWith(head, [0x50, 0x4B, 0x05, 0x06]) ||
      bufStartsWith(head, [0x50, 0x4B, 0x07, 0x08])) return 'application/zip';
  // GZip
  if (bufStartsWith(head, [0x1F, 0x8B])) return 'application/gzip';
  // BZip2
  if (bufStartsWith(head, [0x42, 0x5A, 0x68])) return 'application/x-bzip2';
  // XZ
  if (bufStartsWith(head, [0xFD, 0x37, 0x7A, 0x58, 0x5A, 0x00])) return 'application/x-xz';
  // 7z
  if (bufStartsWith(head, [0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C])) return 'application/x-7z-compressed';
  // RAR5
  if (bufStartsWith(head, [0x52, 0x61, 0x72, 0x21, 0x1A, 0x07, 0x00])) return 'application/vnd.rar';
  // RAR (older)
  if (bufStartsWith(head, [0x52, 0x61, 0x72, 0x21, 0x1A, 0x07, 0x01, 0x00])) return 'application/x-rar-compressed';

  // ELF (executable)
  if (bufStartsWith(head, [0x7F, 0x45, 0x4C, 0x46])) return 'application/x-elf';

  // MP3 (ID3 tag)
  if (bufStartsWith(head, [0x49, 0x44, 0x33])) return 'audio/mpeg';
  // OGG
  if (bufStartsWith(head, [0x4F, 0x67, 0x67, 0x53])) return 'audio/ogg';
  // FLAC
  if (bufStartsWith(head, [0x66, 0x4C, 0x61, 0x43])) return 'audio/flac';
  // WAV / AVI (RIFF)
  if (bufStartsWith(head, [0x52, 0x49, 0x46, 0x46]) && head.length >= 12) {
    const sub = head.subarray(8, 12).toString('ascii');
    if (sub === 'WAVE') return 'audio/wav';
    if (sub === 'AVI ') return 'video/x-msvideo';
  }
  // MP4 / QuickTime / HEIC (ftyp box)
  if (head.length >= 12) {
    const subtype = head.subarray(4, 8).toString('ascii');
    const brand = head.subarray(8, 12).toString('ascii');
    if (subtype === 'ftyp') {
      if (['isom', 'mp42', 'mp41', 'avc1'].includes(brand)) return 'video/mp4';
      if (['M4A ', 'M4B ', 'M4P '].includes(brand)) return 'audio/mp4';
      if (brand === 'heic' || brand === 'heix' || brand === 'mif1' || brand === 'msf1') return 'image/heic';
      if (brand === 'qt  ') return 'video/quicktime';
      return 'video/mp4';
    }
  }
  // WEBM / MKV (Matroska)
  if (bufStartsWith(head, [0x1A, 0x45, 0xDF, 0xA3])) return 'video/webm';

  // SVG — starts with <?xml or <svg
  // Read a larger chunk to detect SVG reliably
  if (head.length >= 4) {
    const ascii = head.toString('ascii').toLowerCase();
    if (ascii.startsWith('<?xml') || ascii.startsWith('<svg') || ascii.startsWith('<!doc')) {
      // Confirm by reading the full first 512 bytes
      const fullHead = await readHead(filePath, 512);
      if (fullHead) {
        const text = fullHead.toString('utf-8').toLowerCase();
        if (text.includes('<svg')) return 'image/svg+xml';
      }
      return 'text/plain';
    }
  }

  return null;
}

// ── Cached MIME detection (magic → `file --mime-type` fallback) ───

const mimeCache = new Map<string, string>();

export async function detectMime(filePath: string): Promise<string | null> {
  const cached = mimeCache.get(filePath);
  if (cached !== undefined) return cached;

  let mime = await detectMimeByMagic(filePath);

  // Fall back to `file --mime-type`
  if (!mime) {
    try {
      const { stdout } = await execFileAsync('file', ['--mime-type', '--brief', filePath]);
      mime = stdout.trim() || null;
    } catch {
      mime = null;
    }
  }

  mimeCache.set(filePath, mime ?? '');
  return mime ?? null;
}

export function clearMimeCache(): void {
  mimeCache.clear();
}

// ── Thumbnail generation & caching ────────────────────────────────

const THUMB_CACHE_DIR = path.join(os.homedir(), '.cache', 'material3', 'thumbnails');

function ensureThumbCacheDir(): void {
  if (!existsSync(THUMB_CACHE_DIR)) {
    mkdirSync(THUMB_CACHE_DIR, { recursive: true });
  }
}

function thumbCacheKey(key: string): string {
  const hash = crypto.createHash('md5').update(key).digest('hex');
  return path.join(THUMB_CACHE_DIR, `${hash}.png`);
}

/**
 * Generate a thumbnail for the given file (image only) and return the
 * cache file path.  Returns `null` if the file cannot be thumbnailed.
 *
 * Strategy:
 *   1. If ImageMagick `convert` is available, use it (fast, subprocess).
 *   2. Otherwise fall back to Electron's `nativeImage`.
 *
 * @param cropToSquare — when true, center-crop to a square (for drag icons).
 */
export async function getThumbnail(filePath: string, maxSize: number, cropToSquare = false): Promise<string | null> {
  ensureThumbCacheDir();
  const cacheKey = cropToSquare ? `${filePath}@${maxSize}-square` : `${filePath}@${maxSize}`;
  const cachePath = thumbCacheKey(cacheKey);

  // Return cached thumbnail if it exists
  if (existsSync(cachePath)) return cachePath;

  // Verify file is an image by magic bytes
  const mime = await detectMimeByMagic(filePath);
  if (!mime || !mime.startsWith('image/')) return null;

  // Try ImageMagick `convert` first
  try {
    const args: string[] = [
      filePath,
      '-auto-orient',
    ];
    if (cropToSquare) {
      args.push('-thumbnail', `${maxSize}x${maxSize}^`);
      args.push('-gravity', 'center');
      args.push('-extent', `${maxSize}x${maxSize}`);
    } else {
      args.push('-thumbnail', `${maxSize}x${maxSize}>`);
    }
    args.push('-strip', 'png:' + cachePath);
    await execFileAsync('convert', args);
    if (existsSync(cachePath)) return cachePath;
  } catch {
    // Fall through to nativeImage
  }

  // Fallback to Electron's nativeImage
  try {
    const { nativeImage } = await import('electron');
    const img = nativeImage.createFromPath(filePath);
    if (img.isEmpty()) return null;
    const resized = img.resize({ width: maxSize, height: maxSize });
    writeFileSync(cachePath, resized.toPNG());
    return cachePath;
  } catch {
    return null;
  }
}

const DRAG_ICON_DIR = path.join(os.homedir(), '.cache', 'material3', 'drag-icons');

/** Path for a cached Material Symbols drag icon by icon name */
export function getCachedDragIconPath(iconName: string): string {
  if (!existsSync(DRAG_ICON_DIR)) {
    mkdirSync(DRAG_ICON_DIR, { recursive: true });
  }
  return path.join(DRAG_ICON_DIR, `${iconName}.png`);
}

/**
 * Generate a drag icon (square PNG) for any file.
 * - For images: a square-cropped thumbnail (center crop).
 * - For non-images: Material Symbols icon pre-rendered by the frontend
 *   and cached on disk.  Falls back to a visible generic colored square.
 */
export async function getDragIcon(filePath: string, filled = false): Promise<string> {
  const mime = await detectMime(filePath);
  if (mime && mime.startsWith('image/')) {
    const thumb = await getThumbnail(filePath, 96, true);
    if (thumb) return thumb;
  }
  // Check if a pre-cached Material icon exists
  if (!mime) return getGenericFileIcon('Others');
  const iconName = getIconNameForMime(mime, false);
  const cacheKey = filled ? `${iconName}:filled` : iconName;
  const cached = getCachedDragIconPath(cacheKey);
  if (existsSync(cached)) return cached;
  // Try unfilled fallback
  const fallback = getCachedDragIconPath(iconName);
  if (existsSync(fallback)) return fallback;
  return getGenericFileIcon(mime);
}

/** Map MIME → Material Symbols icon name (mirrors frontend logic) */
function getIconNameForMime(mime: string | null, isDirectory: boolean): string {
  if (isDirectory || mime === 'inode/directory') return 'folder';
  if (!mime) return 'insert_drive_file';
  const cat = mime.split('/')[0];
  switch (cat) {
  case 'image': return 'image';
  case 'audio': return 'audio_file';
  case 'video': return 'movie';
  case 'text': return 'article';
  }
  switch (mime) {
  case 'application/pdf': return 'picture_as_pdf';
  case 'application/zip':
  case 'application/gzip':
  case 'application/x-bzip2':
  case 'application/x-xz':
  case 'application/x-7z-compressed':
  case 'application/vnd.rar':
  case 'application/x-rar-compressed':
  case 'application/x-tar':
    return 'folder_zip';
  case 'application/x-elf':
  case 'application/x-executable':
  case 'application/x-sharedlib':
    return 'terminal';
  }
  return 'insert_drive_file';
}

/** Generate a visible fallback PNG when the cached Material icon is not yet available. */
async function getGenericFileIcon(mime: string): Promise<string> {
  const cat = mime.split('/')[0];
  let color: string;
  switch (cat) {
  case 'image':  color = '#4A90D9'; break;
  case 'audio':  color = '#9B59B6'; break;
  case 'video':  color = '#E74C3C'; break;
  case 'text':   color = '#1ABC9C'; break;
  case 'inode':  color = '#3498DB'; break;
  default:
    switch (mime) {
    case 'application/pdf':                     color = '#E74C3C'; break;
    case 'application/zip':
    case 'application/gzip':
    case 'application/x-bzip2':
    case 'application/x-xz':
    case 'application/x-7z-compressed':
    case 'application/vnd.rar':
    case 'application/x-rar-compressed':
    case 'application/x-tar':
      color = '#F39C12'; break;
    case 'application/x-elf':
    case 'application/x-executable':
    case 'application/x-sharedlib':
      color = '#2C3E50'; break;
    default:                                    color = '#7F8C8D'; break;
    }
  }

  const { nativeImage } = await import('electron');
  const outPath = path.join(os.tmpdir(), `material3-generic-${Date.now()}.png`);
  try {
    const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="96" height="96">
      <rect width="96" height="96" fill="${color}"/>
    </svg>`;
    const dataUrl = `data:image/svg+xml;base64,${Buffer.from(svg).toString('base64')}`;
    const colored = nativeImage.createFromDataURL(dataUrl);
    writeFileSync(outPath, colored.toPNG());
    if (existsSync(outPath)) return outPath;
  } catch { /* fall through */ }
  return getFallbackIcon();
}

// ── Fallback 1×1 transparent PNG ──────────────────────────────────

const PNG_1x1 = Buffer.from(
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8/5+hHgAHggJ/PchI7wAAAABJRU5ErkJggg==',
  'base64',
);

let _fallbackIconPath: string | null = null;

function getFallbackIcon(): string {
  if (_fallbackIconPath && existsSync(_fallbackIconPath)) return _fallbackIconPath;
  _fallbackIconPath = path.join(os.tmpdir(), 'material3-fallback-icon.png');
  writeFileSync(_fallbackIconPath, PNG_1x1);
  return _fallbackIconPath;
}
