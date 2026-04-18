import { createHash } from 'crypto'

export function sha256(input: Uint8Array): Uint8Array {
    return new Uint8Array(createHash('sha256').update(input).digest())
}

export function encodeHexLowerCase(input: Uint8Array): string {
    return Buffer.from(input).toString('hex')
}

export function encodeBase64url(input: Uint8Array): string {
    return Buffer.from(input).toString('base64url')
}
