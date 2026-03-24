import { randomBytes, createCipheriv, createDecipheriv, hkdfSync } from 'crypto'

export interface EncryptedData {
    data: string // Base64 encoded encrypted data (ciphertext + 16-byte GCM auth tag)
    nonce: string // Base64 encoded 12-byte nonce
    salt: string // Base64 encoded 16-byte salt
}

export interface EncryptedConfig {
    encrypted_data: EncryptedData
    version: number
}

let _masterKey: Buffer | null = null

function getMasterKey(): Buffer {
    if (_masterKey) return _masterKey

    const encryptionKey = process.env.ENCRYPTION_KEY || ''
    const encryptionSalt = process.env.ENCRYPTION_SALT || ''

    if (!encryptionKey) {
        throw new Error('ENCRYPTION_KEY environment variable not set')
    }
    if (!encryptionSalt) {
        throw new Error('ENCRYPTION_SALT environment variable not set')
    }
    if (encryptionKey.length < 32) {
        throw new Error('ENCRYPTION_KEY must be at least 32 characters long')
    }
    if (encryptionSalt.length < 16) {
        throw new Error('ENCRYPTION_SALT must be at least 16 characters long')
    }

    _masterKey = Buffer.from(
        hkdfSync(
            'sha256',
            Buffer.from(encryptionKey, 'utf-8'),
            Buffer.from(encryptionSalt, 'utf-8'),
            Buffer.from('omni-encryption-key', 'utf-8'),
            32,
        ),
    )
    return _masterKey
}

function deriveOperationKey(masterKey: Buffer, operationSalt: Buffer): Buffer {
    return Buffer.from(
        hkdfSync(
            'sha256',
            masterKey,
            operationSalt,
            Buffer.from('omni-operation-key', 'utf-8'),
            32,
        ),
    )
}

export function encrypt(plaintext: string): EncryptedData {
    const masterKey = getMasterKey()

    const nonce = randomBytes(12)
    const salt = randomBytes(16)

    const operationKey = deriveOperationKey(masterKey, salt)

    const cipher = createCipheriv('aes-256-gcm', operationKey, nonce)
    const encrypted = Buffer.concat([cipher.update(plaintext, 'utf-8'), cipher.final()])
    const authTag = cipher.getAuthTag()

    // Concatenate ciphertext + auth tag (matches ring's seal_in_place_append_tag)
    const combined = Buffer.concat([encrypted, authTag])

    return {
        data: combined.toString('base64'),
        nonce: nonce.toString('base64'),
        salt: salt.toString('base64'),
    }
}

export function decrypt(encryptedData: EncryptedData): string {
    const masterKey = getMasterKey()

    const combined = Buffer.from(encryptedData.data, 'base64')
    const nonce = Buffer.from(encryptedData.nonce, 'base64')
    const salt = Buffer.from(encryptedData.salt, 'base64')

    if (nonce.length !== 12) throw new Error('Invalid nonce length')
    if (salt.length !== 16) throw new Error('Invalid salt length')

    // Split ciphertext and auth tag (last 16 bytes)
    const ciphertext = combined.subarray(0, combined.length - 16)
    const authTag = combined.subarray(combined.length - 16)

    const operationKey = deriveOperationKey(masterKey, salt)

    const decipher = createDecipheriv('aes-256-gcm', operationKey, nonce)
    decipher.setAuthTag(authTag)

    const decrypted = Buffer.concat([decipher.update(ciphertext), decipher.final()])
    return decrypted.toString('utf-8')
}

export function encryptConfig(config: Record<string, unknown>): EncryptedConfig {
    const plaintext = JSON.stringify(config)
    const encryptedData = encrypt(plaintext)
    return {
        encrypted_data: encryptedData,
        version: 1,
    }
}

export function decryptConfig(dbValue: unknown): Record<string, unknown> {
    if (dbValue === null || dbValue === undefined) return {}

    const obj = dbValue as Record<string, unknown>

    // If it has encrypted_data, decrypt it
    if (obj.encrypted_data && typeof obj.encrypted_data === 'object') {
        const encryptedData = obj.encrypted_data as EncryptedData
        const plaintext = decrypt(encryptedData)
        return JSON.parse(plaintext) as Record<string, unknown>
    }

    // Backward compatibility: return as-is if not encrypted
    return obj
}
