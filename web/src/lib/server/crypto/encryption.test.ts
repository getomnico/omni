import { describe, it, expect } from 'vitest'

// Must be set before importing the module (master key is lazily derived on first use)
process.env.ENCRYPTION_KEY = 'test_master_key_that_is_long_enough_32_chars'
process.env.ENCRYPTION_SALT = 'test_salt_16_chars'

import { encrypt, decrypt, encryptConfig, decryptConfig } from './encryption'

describe('encryption', () => {
    it('should encrypt and decrypt a string roundtrip', () => {
        const plaintext = 'Hello, World! This is sensitive data.'

        const encrypted = encrypt(plaintext)

        expect(encrypted.data).toBeTruthy()
        expect(encrypted.nonce).toBeTruthy()
        expect(encrypted.salt).toBeTruthy()

        const decrypted = decrypt(encrypted)
        expect(decrypted).toBe(plaintext)
    })

    it('should produce different ciphertexts for the same plaintext', () => {
        const plaintext = 'Same data for both encryptions'

        const encrypted1 = encrypt(plaintext)
        const encrypted2 = encrypt(plaintext)

        expect(encrypted1.data).not.toBe(encrypted2.data)
        expect(encrypted1.salt).not.toBe(encrypted2.salt)

        expect(decrypt(encrypted1)).toBe(plaintext)
        expect(decrypt(encrypted2)).toBe(plaintext)
    })

    it('should fail to decrypt with tampered data', () => {
        const encrypted = encrypt('test data')
        encrypted.data = encrypted.data.slice(0, -4) + 'AAAA'

        expect(() => decrypt(encrypted)).toThrow()
    })

    it('should reject invalid nonce length', () => {
        const encrypted = encrypt('test')
        encrypted.nonce = Buffer.from([1, 2, 3]).toString('base64')

        expect(() => decrypt(encrypted)).toThrow('Invalid nonce length')
    })

    it('should reject invalid salt length', () => {
        const encrypted = encrypt('test')
        encrypted.salt = Buffer.from([1, 2, 3]).toString('base64')

        expect(() => decrypt(encrypted)).toThrow('Invalid salt length')
    })
})

describe('encryptConfig / decryptConfig', () => {
    it('should encrypt and decrypt a config object roundtrip', () => {
        const config = {
            apiKey: 'sk-test-123456',
            model: 'gpt-4',
            apiUrl: 'https://api.example.com',
        }

        const encrypted = encryptConfig(config)

        expect(encrypted.encrypted_data).toBeTruthy()
        expect(encrypted.version).toBe(1)

        const decrypted = decryptConfig(encrypted)
        expect(decrypted).toEqual(config)
    })

    it('should pass through unencrypted config (backward compat)', () => {
        const config = {
            apiKey: 'sk-test-123456',
            model: 'gpt-4',
        }

        const decrypted = decryptConfig(config)
        expect(decrypted).toEqual(config)
    })

    it('should return empty object for null/undefined', () => {
        expect(decryptConfig(null)).toEqual({})
        expect(decryptConfig(undefined)).toEqual({})
    })

    it('should handle empty config object', () => {
        const encrypted = encryptConfig({})
        const decrypted = decryptConfig(encrypted)
        expect(decrypted).toEqual({})
    })
})
