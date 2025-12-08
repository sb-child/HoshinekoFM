export interface IFile {
    name: string;
    path: string;
    isDirectory: boolean;
    size: number;
    mtime: Date;
}

export interface IFileSystemAPI {
    listDir: (path: string) => Promise<IFile[]>;
}
