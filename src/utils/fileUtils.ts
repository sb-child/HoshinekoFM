import type { IFile } from '../types/files';

export type FileGroup = 'Folders' | 'Code' | 'Media' | 'Documents' | 'Archives' | 'Executables' | 'Others';

export const GROUP_ORDER: FileGroup[] = [
  'Folders',
  'Media',
  'Documents',
  'Code',
  'Archives',
  'Executables',
  'Others'
];

// 1. 添加国际化汉化映射表
export const groupLocaleMap: Record<FileGroup, string> = {
  'Folders': '文件夹',
  'Media': '媒体文件',
  'Documents': '文档',
  'Code': '代码文件',
  'Archives': '压缩包',
  'Executables': '可执行文件',
  'Others': '其他文件'
};

// 2. 导出获取中文分组名称的辅助函数
export function getSemanticGroupLabel(group: FileGroup): string {
  return groupLocaleMap[group] || group;
}

const codeMimeSubtypes = new Set([
  'javascript', 'typescript', 'html', 'css', 'x-scss', 'x-python',
  'x-java', 'x-c', 'x-c++', 'x-csharp', 'x-go', 'x-rust', 'x-lua',
  'x-php', 'x-ruby', 'x-sql', 'x-shell', 'x-yaml', 'x-toml', 'x-perl',
  'x-swift', 'x-kotlin', 'x-dart', 'x-haskell', 'x-scala',
  'xml', 'json',
]);

const archiveMimeTypes = new Set([
  'application/zip', 'application/gzip', 'application/x-bzip2',
  'application/x-xz', 'application/x-7z-compressed', 'application/vnd.rar',
  'application/x-rar-compressed', 'application/x-tar',
  'application/x-iso9660-image',
]);

const docMimeTypes = new Set([
  'application/pdf', 'application/msword',
  'application/vnd.openxmlformats-officedocument.wordprocessingml.document',
  'application/vnd.ms-excel',
  'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet',
  'application/vnd.ms-powerpoint',
  'application/vnd.openxmlformats-officedocument.presentationml.presentation',
  'application/rtf',
]);

const execMimeTypes = new Set([
  'application/x-elf', 'application/x-executable',
  'application/x-sharedlib', 'application/x-python-bytecode',
]);

export function getSemanticGroup(file: IFile): FileGroup {
  if (file.isDirectory) return 'Folders';

  const mime = file.mime;
  if (!mime) return 'Others';

  const [cat, sub] = mime.split('/');

  switch (cat) {
  case 'image':
  case 'audio':
  case 'video':
    return 'Media';
  case 'text':
    return codeMimeSubtypes.has(sub) ? 'Code' : 'Documents';
  case 'application':
    if (codeMimeSubtypes.has(sub)) return 'Code';
    if (docMimeTypes.has(mime)) return 'Documents';
    if (archiveMimeTypes.has(mime)) return 'Archives';
    if (execMimeTypes.has(mime)) return 'Executables';
    return 'Others';
  default:
    return 'Others';
  }
}
