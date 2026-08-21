export interface FolderPathFilter {
    id: string
    name: string
    path: string
    driveId: string
    kind: 'shared_drive_root' | 'folder'
}
