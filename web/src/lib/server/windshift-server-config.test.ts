import { describe, expect, it } from 'vitest'
import { validateWindshiftServerUrl } from './windshift-server-config'

describe('validateWindshiftServerUrl', () => {
    it('accepts a public https URL', async () => {
        await expect(
            validateWindshiftServerUrl('Windshift URL', 'https://8.8.8.8'),
        ).resolves.toBeUndefined()
    })

    it('accepts a public http URL', async () => {
        await expect(
            validateWindshiftServerUrl('Windshift URL', 'http://1.1.1.1'),
        ).resolves.toBeUndefined()
    })

    it('skips empty URLs (optional internal URL)', async () => {
        await expect(
            validateWindshiftServerUrl('Windshift internal URL', ''),
        ).resolves.toBeUndefined()
        await expect(
            validateWindshiftServerUrl('Windshift internal URL', '   '),
        ).resolves.toBeUndefined()
    })

    it('rejects loopback and private addresses (SSRF)', async () => {
        await expect(
            validateWindshiftServerUrl('Windshift URL', 'http://127.0.0.1:8080'),
        ).rejects.toThrow('Windshift URL is not allowed')
        await expect(
            validateWindshiftServerUrl('Windshift URL', 'http://169.254.169.254/'),
        ).rejects.toThrow('Windshift URL is not allowed')
        await expect(
            validateWindshiftServerUrl('Windshift URL', 'http://192.168.1.10:8080'),
        ).rejects.toThrow('Windshift URL is not allowed')
    })

    it('rejects credentials, fragments, and non-http schemes', async () => {
        await expect(
            validateWindshiftServerUrl('Windshift URL', 'https://user:pass@8.8.8.8'),
        ).rejects.toThrow('Windshift URL is not allowed')
        await expect(
            validateWindshiftServerUrl('Windshift URL', 'https://8.8.8.8#frag'),
        ).rejects.toThrow('Windshift URL is not allowed')
        await expect(validateWindshiftServerUrl('Windshift URL', 'ftp://8.8.8.8')).rejects.toThrow(
            'Windshift URL is not allowed',
        )
    })
})
