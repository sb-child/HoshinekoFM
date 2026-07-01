import { watch, FSWatcher } from 'fs';
import path from 'path';

interface WatcherEntry {
  watcher: FSWatcher;
  timer: ReturnType<typeof setTimeout> | null;
}

const watchers = new Map<string, WatcherEntry>();

export function startWatching(
  dir: string,
  onChange: (changedDir: string) => void,
): void {
  if (watchers.has(dir)) return;

  const watcher = watch(dir, (_event, _filename) => {
    const entry = watchers.get(dir);
    if (!entry) return;

    if (entry.timer) clearTimeout(entry.timer);
    entry.timer = setTimeout(() => {
      entry.timer = null;
      onChange(dir);
    }, 300);
  });

  watchers.set(dir, { watcher, timer: null });
}

export function stopWatching(dir: string): void {
  const entry = watchers.get(dir);
  if (!entry) return;
  if (entry.timer) clearTimeout(entry.timer);
  entry.watcher.close();
  watchers.delete(dir);
}

export function stopAllWatching(): void {
  for (const [dir] of watchers) {
    stopWatching(dir);
  }
}
