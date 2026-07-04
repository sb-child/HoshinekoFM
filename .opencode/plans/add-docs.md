# Plan: Add documentation to file entry types and file operation handlers

## Overview

The `IFile` interface and file operation handlers currently lack explanatory documentation. This plan covers adding JSDoc comments to field definitions and function-level documentation for the file operation handlers.

---

## File 1: `src/types/files.ts` — Document the `IFile` interface

Add JSDoc block comment on the interface, then a `/** */` on each field.

### Interface-level doc

```
/**
 * Represents a file, directory, or device entry in the file explorer listing.
 * Returned by `listDirectoryContents` in the Electron main process and consumed
 * by the React frontend.
 */
```

### Field-level docs

| Field | Doc comment |
|---|---|
| `name` | Display name of the file/directory entry (not full path) |
| `path` | Full absolute path on disk |
| `isDirectory` | `true` if this entry is a directory |
| `size` | File size in bytes; `0` for directories, special files, and broken symlinks |
| `mtime` | Last modification timestamp (`Date(0)` for special files and broken symlinks) |
| `mime` | MIME type string, e.g. `"text/plain"`, `"inode/directory"`, `"inode/blockdevice"`, `"inode/symlink"`; `null` if detection fails |
| `symlinkTarget?` | If the entry is a symbolic link, the resolved absolute target path |
| `isMountpoint?` | `true` when this directory is a filesystem mount point (from `/proc/mounts`) |
| `mountSource?` | Source device path or filesystem name backing the mount (e.g. `"/dev/sda1"`, `"tmpfs"`, `"none"`) |
| `mountFstype?` | Filesystem type string (e.g. `"ext4"`, `"ntfs"`, `"vfat"`, `"btrfs"`) |
| `devicePath?` | For block/char device entries in `/dev`, the full `/dev/...` path |
| `isMountable?` | `true` if this block device has partitions or is a DM device and can be mounted via udisks |
| `parentDisk?` | For partition entries (e.g. `/dev/sda1`), the parent disk path (e.g. `/dev/sda`) |
| `isExternal?` | `true` if the block device is removable/USB (read from `/sys/block/<name>/removable`) |
| `mountedAt?` | If this device is currently mounted, the filesystem mountpoint directory |
| `canAutoMount?` | `true` if the block device is mountable and is **not** a DM (device-mapper) device; indicates it's safe for auto-mount via udisks |

---

## File 2: `electron/handlers/fs.ts` — Document `listDirectoryContents` and handlers

### `listDirectoryContents(targetPath: string)`

Function-level JSDoc describing the five-phase logic:

```
/**
 * Lists all entries under `targetPath` with full metadata.
 *
 * Phases:
 * 1. **readdir** — Enumerate directory entries via `fs.readdir({ withFileTypes: true })`.
 * 2. **Per-entry stat/mime** — For each entry:
 *    - Special files (block, char, fifo, socket) are classified with `inode/*` MIME types. Block
 *      devices additionally query `/sys/class/block/` for partition/DM status (→ `isMountable`,
 *      `canAutoMount`) and `/sys/block/` for removable status (→ `isExternal`, `parentDisk`).
 *    - Symlinks are resolved with `fs.readlink`, then `fs.stat` on the resolved path to get the
 *      real target's metadata. Broken symlinks (`ENOENT`) are reported as `inode/symlink`.
 *    - Regular files/directories get size/mtime via `fs.stat` and MIME via `detectMime`.
 * 3. **Build device ↔ mount map** — Parse `/proc/mounts` (via `getMountMap`) and index by
 *    device source path. Also resolve symlinks in device paths (e.g. `/dev/disk/by-uuid/...`
 *    → `/dev/sda1`).
 * 4. **Enrich entries with mount info** — Second pass over results:
 *    - Directories matching a mountpoint get `isMountpoint`, `mountSource`, `mountFstype`.
 *    - Entries with a `devicePath` or `symlinkTarget` that match a device in the mount map get
 *      `mountFstype` and `mountedAt`.
 *    - Block device symlinks in `/dev` get `isMountable`, `canAutoMount`, and `isExternal`
 *      computed from `/sys/class/block/<targetName>` and `/sys/block/<targetName>`.
 *
 * @returns Filtered array (null entries removed) of file entry objects.
 */
```

### IPC Handler docs

| Handler | Doc |
|---|---|
| `fs:list-dir` | Lists directory contents. If permission is denied, falls back to the nearest accessible parent directory and includes an `error` field with the original path. |
| `fs:get-places` | Returns the standard user directory shortcuts (Home, Desktop, Documents, Downloads, Music, Pictures, Videos) for the sidebar. |
| `fs:copy` | Copies `source` to `dest` recursively (`fs.cp` with `recursive: true`). Does not overwrite existing files (`force: false`). |
| `fs:move` | Moves/renames `source` to `dest` via `fs.rename`. |
| `fs:trash` | Moves a single file/directory to the system trash via `shell.trashItem`. |
| `fs:trash-batch` | Moves multiple files to the system trash. Returns a list of paths that failed. |
| `fs:rename` | Renames `oldPath` to `newPath` via `fs.rename`. |
| `fs:mkdir` | Creates a directory recursively (`fs.mkdir({ recursive: true })`). |
| `fs:create-file` | Creates an empty file at `filePath`. |
| `fs:open` | Opens a file/directory with the system default handler via `shell.openPath`. Returns an error string if failed. |
| `fs:extract` | Extracts archives: `.zip` via `unzip`, `.tar`/`.gz`/`.xz` via `tar`. Extracts to the archive's parent directory. Returns `false` for unsupported extensions. |
| `fs:exists` | Checks if a single path exists (`fs.access`). |
| `fs:exists-batch` | Checks existence of multiple paths in parallel. Returns a `Record<string, boolean>`. |
| `fs:get-parent` | Returns the parent directory path (`path.dirname`). |
| `fs:get-home` | Returns the user's home directory (`app.getPath('home')`). |
| `fs:get-symlink-target` | Resolves a symlink and returns `{ isSymlink, target, targetExists }`. |
| `fs:check-symlinks` | Batch version of symlink resolution for multiple paths. |
| `fs:read-file` | Reads a file's contents as UTF-8 text. Returns `null` on failure. |
| `system:get-directory-size` | Computes directory size in bytes using `du -sb`. Returns `0` on failure or zero match. |

---

## Implementation order

1. `src/types/files.ts` — Add JSDoc to `IFile` interface and fields
2. `electron/handlers/fs.ts` — Add function-level JSDoc to `listDirectoryContents`
3. `electron/handlers/fs.ts` — Add inline comments on handler shortcuts for each IPC handler
