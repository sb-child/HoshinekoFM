import type { IFile } from '../types/files';

const mimeDescriptionMap: Record<string, string> = {
  'inode/directory': '文件夹',
  'inode/symlink': '符号链接',
  'inode/blockdevice': '块设备',
  'inode/chardevice': '字符设备',
  'inode/fifo': '命名管道',
  'inode/socket': '套接字',

  'text/plain': '文本文件',
  'text/html': 'HTML 文件',
  'text/css': '样式表',
  'text/javascript': 'JavaScript 文件',
  'text/xml': 'XML 文件',
  'text/csv': 'CSV 文件',
  'text/markdown': 'Markdown 文件',
  'text/x-python': 'Python 文件',
  'text/x-c': 'C 源文件',
  'text/x-c++': 'C++ 源文件',
  'text/x-java': 'Java 源文件',
  'text/x-go': 'Go 源文件',
  'text/x-rust': 'Rust 源文件',
  'text/x-shell': 'Shell 脚本',
  'text/x-yaml': 'YAML 文件',
  'text/x-toml': 'TOML 文件',

  'image/png': 'PNG 图像',
  'image/jpeg': 'JPEG 图像',
  'image/gif': 'GIF 图像',
  'image/svg+xml': 'SVG 图像',
  'image/webp': 'WebP 图像',
  'image/bmp': 'BMP 图像',
  'image/tiff': 'TIFF 图像',
  'image/x-icon': '图标文件',
  'image/heic': 'HEIC 图像',

  'audio/mpeg': 'MP3 音频',
  'audio/ogg': 'OGG 音频',
  'audio/flac': 'FLAC 音频',
  'audio/wav': 'WAV 音频',
  'audio/mp4': 'AAC 音频',

  'video/mp4': 'MP4 视频',
  'video/webm': 'WebM 视频',
  'video/x-msvideo': 'AVI 视频',
  'video/quicktime': 'QuickTime 视频',

  'application/pdf': 'PDF 文档',
  'application/zip': 'ZIP 压缩包',
  'application/gzip': 'GZip 压缩包',
  'application/x-bzip2': 'BZip2 压缩包',
  'application/x-xz': 'XZ 压缩包',
  'application/x-7z-compressed': '7z 压缩包',
  'application/vnd.rar': 'RAR 压缩包',
  'application/x-rar-compressed': 'RAR 压缩包',
  'application/x-tar': 'TAR 归档',
  'application/x-iso9660-image': '光盘映像',

  'application/vnd.oasis.opendocument.text': 'ODT 文档',
  'application/vnd.oasis.opendocument.spreadsheet': 'ODS 表格',
  'application/vnd.oasis.opendocument.presentation': 'ODP 演示文稿',
  'application/vnd.openxmlformats-officedocument.wordprocessingml.document': 'DOCX 文档',
  'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet': 'XLSX 表格',
  'application/vnd.openxmlformats-officedocument.presentationml.presentation': 'PPTX 演示文稿',
  'application/msword': 'DOC 文档',
  'application/vnd.ms-excel': 'XLS 表格',
  'application/vnd.ms-powerpoint': 'PPT 演示文稿',
  'application/rtf': 'RTF 文档',

  'application/x-elf': '可执行文件',
  'application/x-executable': '可执行文件',
  'application/x-sharedlib': '共享库',
  'application/x-python-bytecode': 'Python 字节码',

  'application/json': 'JSON 文件',
  'application/xml': 'XML 文件',
};

const categoryDescriptions: Record<string, string> = {
  'text': '文本文件',
  'image': '图像文件',
  'audio': '音频文件',
  'video': '视频文件',
  'font': '字体文件',
  'inode': '系统文件',
  'application': '其他文件',
};

export function getFileTypeDescription(file: IFile): string {
  if (file.isDirectory) return '文件夹';

  const mime = file.mime;
  if (mime && mimeDescriptionMap[mime]) {
    return mimeDescriptionMap[mime];
  }

  if (mime) {
    const cat = mime.split('/')[0];
    if (categoryDescriptions[cat]) return categoryDescriptions[cat];
  }

  const ext = file.name.split('.').pop()?.toUpperCase();
  return ext ? `${ext} 文件` : '其他文件';
}

export function getMimeIcon(description: string): string {
  if (description === '文件夹') return 'folder';
  if (description === '符号链接') return 'link';
  if (description === '块设备') return 'hard_drive';
  if (description === '字符设备') return 'keyboard';
  if (description === '命名管道') return 'swap_vert';
  if (description === '套接字') return 'hub';
  return 'insert_drive_file';
}
