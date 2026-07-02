import { t } from '../i18n';
import { showToast } from './toast';
import {
  checkConflicts,
  generateSafeName,
  splitNameExt,
  prepareDestParent,
  type ConflictEntry,
  type ConflictResult,
} from './fileConflict';

export function formatFileOpError(operation: string, fileRef: string, error: any): string {
  let code: string = error?.code || '';
  const msg = (error?.message || error?.toString() || '').toLowerCase();

  if (!code) {
    const m = msg.match(/error:\s*(\w+):/);
    if (m) code = m[1].toUpperCase();
  }

  switch (code) {
  case 'EEXIST':  return t('file_op.exists', operation, fileRef);
  case 'ENOENT':  return t('file_op.not_found', operation, fileRef);
  case 'EACCES':
  case 'EPERM':   return t('file_op.permission', operation, fileRef);
  case 'ENOSPC':  return t('file_op.no_space', operation, fileRef);
  case 'EROFS':   return t('file_op.read_only', operation, fileRef);
  case 'EISDIR':  return t('file_op.is_dir', operation, fileRef);
  case 'ENOTDIR': return t('file_op.not_dir', operation, fileRef);
  case 'EXDEV':   return t('file_op.cross_device', operation, fileRef);
  case 'EBUSY':   return t('file_op.busy', operation, fileRef);
  }

  if (msg.includes('same file') || msg.includes('same path') || msg.includes('source and destination')) {
    return t('file_op.same_target', operation, fileRef);
  }
  if (msg.includes('already exists') || msg.includes('exists')) {
    return t('file_op.exists', operation, fileRef);
  }
  if (msg.includes('no such file') || msg.includes('not found') || msg.includes('不存在')) {
    return t('file_op.not_found', operation, fileRef);
  }
  if (msg.includes('permission denied') || msg.includes('not permitted') || msg.includes('eacces') || msg.includes('eperm')) {
    return t('file_op.permission', operation, fileRef);
  }
  if (msg.includes('not a directory')) {
    return t('file_op.not_dir', operation, fileRef);
  }
  if (msg.includes('is a directory')) {
    return t('file_op.is_dir', operation, fileRef);
  }
  if (msg.includes('no space') || msg.includes('device') || msg.includes('enospc')) {
    return t('file_op.no_space', operation, fileRef);
  }
  if (msg.includes('read-only') || msg.includes('erofs')) {
    return t('file_op.read_only', operation, fileRef);
  }
  if (msg.includes('busy') || msg.includes('ebusy')) {
    return t('file_op.busy', operation, fileRef);
  }

  return t('file_op.generic', operation, fileRef, error?.message || error);
}

function fileName(path: string): string {
  return path.replace(/\\/g, '/').split('/').filter(Boolean).pop() || path;
}

function normalizePath(path: string): string {
  const parts = path.replace(/\\/g, '/').split('/');
  const result: string[] = [];
  for (const p of parts) {
    if (p === '.' || p === '') continue;
    if (p === '..') { result.pop(); continue; }
    result.push(p);
  }
  return (path.startsWith('/') ? '/' : '') + result.join('/');
}

export async function createFile(
  filePath: string,
  onSuccess?: () => void,
): Promise<void> {
  try {
    await window.electron.createFile(filePath);
    showToast(t('toast.file_created', fileName(filePath)), 'success');
    onSuccess?.();
  } catch (e) {
    showToast(formatFileOpError(t('operation.create_file'), fileName(filePath), e), 'error');
  }
}

export async function createDirectory(
  dirPath: string,
  onSuccess?: () => void,
): Promise<void> {
  try {
    await window.electron.createDirectory(dirPath);
    showToast(t('toast.folder_created', fileName(dirPath)), 'success');
    onSuccess?.();
  } catch (e) {
    showToast(formatFileOpError(t('operation.create_folder'), fileName(dirPath), e), 'error');
  }
}

export async function renameFile(
  oldPath: string,
  newPath: string,
  onSuccess?: () => void,
): Promise<void> {
  const oldParent = oldPath.substring(0, oldPath.lastIndexOf('/'));
  const newParent = newPath.substring(0, newPath.lastIndexOf('/'));
  const oldName = fileName(oldPath);
  const newName = fileName(newPath);
  try {
    const targetExists = await window.electron.exists(newPath);
    if (targetExists) {
      showToast(t('error.name_exists', newName), 'error');
      return;
    }
    if (newParent !== oldParent) {
      const ok = await prepareDestParent(newPath);
      if (!ok) return;
    }
    await window.electron.renameFile(oldPath, newPath);
    if (oldParent === newParent) {
      showToast(t('toast.rename_success', oldName, newName), 'success');
    } else {
      showToast(t('toast.rename_move_success', oldName, normalizePath(newPath)), 'success');
    }
    onSuccess?.();
  } catch (e) {
    showToast(formatFileOpError(t('operation.rename_op'), `${oldName} -> ${normalizePath(newPath)}`, e), 'error');
  }
}

export async function trashFile(
  filePath: string,
  onSuccess?: () => void,
): Promise<void> {
  try {
    await window.electron.trashFile(filePath);
    showToast(t('toast.file_deleted', fileName(filePath)), 'success');
    onSuccess?.();
  } catch (e) {
    showToast(formatFileOpError(t('operation.delete_op'), fileName(filePath), e), 'error');
  }
}

export async function trashFiles(
  paths: string[],
  onSuccess?: () => void,
): Promise<void> {
  let success = 0;
  let fail = 0;
  for (const p of paths) {
    try {
      await window.electron.trashFile(p);
      success++;
    } catch {
      fail++;
    }
  }
  if (success > 0) {
    showToast(t('toast.deleted_items', success), 'success');
    onSuccess?.();
  }
  if (fail > 0) {
    showToast(t('toast.failed_items', fail), 'error');
    showToast(t('toast.delete_fail_permission'), 'error');
  }
}

export async function copyFile(
  source: string,
  dest: string,
  onSuccess?: () => void,
): Promise<void> {
  try {
    const targetExists = await window.electron.exists(dest);
    if (targetExists) {
      showToast(t('error.copy_exists', fileName(dest)), 'error');
      return;
    }
    await window.electron.copyFile(source, dest);
    const destDir = dest.split('/').slice(0, -1).pop() || '';
    showToast(t('toast.copy_success', fileName(source), destDir, fileName(dest)), 'success');
    onSuccess?.();
  } catch (e) {
    showToast(formatFileOpError(t('operation.copy_op'), `${fileName(source)} -> ${fileName(dest)}`, e), 'error');
  }
}

export async function moveFile(
  source: string,
  dest: string,
  onSuccess?: () => void,
): Promise<void> {
  try {
    const targetExists = await window.electron.exists(dest);
    if (targetExists) {
      showToast(t('error.move_exists', fileName(dest)), 'error');
      return;
    }
    await window.electron.moveFile(source, dest);
    const destDir = dest.split('/').slice(0, -1).pop() || '';
    showToast(t('toast.move_success', fileName(source), destDir, fileName(dest)), 'success');
    onSuccess?.();
  } catch (e) {
    showToast(formatFileOpError(t('operation.move_op'), `${fileName(source)} -> ${fileName(dest)}`, e), 'error');
  }
}

export async function extractFile(
  filePath: string,
  onSuccess?: () => void,
): Promise<void> {
  try {
    const ok = await window.electron.extractFile(filePath);
    if (ok) {
      showToast(t('toast.file_extracted', fileName(filePath)), 'success');
      onSuccess?.();
    } else {
      showToast(t('error.unsupported_format'), 'error');
    }
  } catch (e) {
    showToast(formatFileOpError(t('operation.extract_op'), fileName(filePath), e), 'error');
  }
}

interface PasteEntry {
  path: string;
  name: string;
  isDir?: boolean;
}

export async function pasteFiles(
  entries: PasteEntry[],
  operation: 'copy' | 'cut',
  destDir: string,
  existingNames: string[],
  clearClipboard?: () => void,
  onSuccess?: () => void,
  onConflict?: (conflicts: ConflictEntry[]) => Promise<ConflictResult>,
): Promise<void> {
  const baseDir = destDir.endsWith('/') ? destDir : destDir + '/';

  const conflictEntries = await checkConflicts(
    entries.map((e) => ({ path: e.path, name: e.name, isDir: !!e.isDir })),
    destDir,
  );

  let renameMap: Map<string, string> | undefined;
  let conflictAction: 'skip' | 'auto-rename' = 'skip';

  if (conflictEntries.length > 0 && onConflict) {
    const result = await onConflict(conflictEntries);
    conflictAction = result.action;
    if (result.renames) renameMap = result.renames;
  }

  const conflictNames = new Set(conflictEntries.map((c) => c.entry.name));
  const usedNames = new Set(existingNames);

  const toProcess: { entry: PasteEntry; destName: string }[] = [];

  for (const entry of entries) {
    if (conflictNames.has(entry.name)) {
      if (conflictAction === 'skip') continue;
      if (renameMap) {
        const renamed = renameMap.get(entry.name);
        if (!renamed || !renamed.trim()) continue;
        toProcess.push({ entry, destName: renamed.trim() });
      } else {
        const { base, ext } = splitNameExt(entry.name, !!entry.isDir);
        const safe = generateSafeName(base, ext, usedNames, !!entry.isDir);
        usedNames.add(safe);
        toProcess.push({ entry, destName: safe });
      }
    } else {
      toProcess.push({ entry, destName: entry.name });
    }
  }

  let success = 0;
  let fail = 0;
  for (const { entry, destName } of toProcess) {
    const destPath = baseDir + destName;
    if (destName.includes('/') || destName.includes('..')) {
      const ok = await prepareDestParent(destPath);
      if (!ok) { fail++; continue; }
    }
    try {
      if (operation === 'copy') {
        await window.electron.copyFile(entry.path, destPath);
      } else {
        await window.electron.moveFile(entry.path, destPath);
      }
      success++;
    } catch (e) {
      fail++;
      console.error(`${operation} ${entry.name} 失败:`, e);
    }
  }

  if (success > 0) {
    showToast(t('toast.pasted_items', success), 'success');
    if (operation === 'cut') clearClipboard?.();
    onSuccess?.();
  }
  if (fail > 0) {
    showToast(t('toast.failed_items', fail), 'error');
  }
}

export async function openFile(
  filePath: string,
): Promise<void> {
  try {
    const err = await window.electron.openPath(filePath);
    if (err) {
      showToast(t('error.file_open_failed', fileName(filePath), err), 'error');
    }
  } catch (e) {
    showToast(formatFileOpError(t('operation.open_op'), fileName(filePath), e), 'error');
  }
}

export async function importFiles(
  fileEntries: { path: string }[],
  destDir: string,
  onSuccess?: () => void,
): Promise<void> {
  let count = 0;
  for (const entry of fileEntries) {
    const name = fileName(entry.path);
    const destPath = destDir.endsWith('/') ? destDir + name : destDir + '/' + name;
    try {
      await window.electron.copyFile(entry.path, destPath);
      count++;
    } catch (e) {
      console.error(`导入 ${name} 失败:`, e);
    }
  }
  if (count > 0) {
    showToast(t('toast.imported_files', count), 'success');
    onSuccess?.();
  }
}

export function copyToClipboard(
  count: number,
): void {
  showToast(t('toast.copied_items', count), 'info');
}

export function cutToClipboard(
  count: number,
): void {
  showToast(t('toast.cut_items', count), 'info');
}
