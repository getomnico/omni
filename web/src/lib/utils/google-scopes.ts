export const GOOGLE_DRIVE_READ_SCOPE = 'https://www.googleapis.com/auth/drive.readonly'
export const GOOGLE_ADMIN_DIRECTORY_GROUP_READ_SCOPE =
    'https://www.googleapis.com/auth/admin.directory.group.readonly'

export const GOOGLE_SA_DIRECT_DRIVE_SCOPES = [GOOGLE_DRIVE_READ_SCOPE]
export const GOOGLE_SA_DIRECT_SCOPES = [
    ...GOOGLE_SA_DIRECT_DRIVE_SCOPES,
    GOOGLE_ADMIN_DIRECTORY_GROUP_READ_SCOPE,
]
