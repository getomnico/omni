import { describe, expect, it } from 'vitest'
import { isPublicIp } from './remoteImage'

describe('isPublicIp', () => {
    it('accepts public addresses', () => {
        expect(isPublicIp('93.184.216.34')).toBe(true)
        expect(isPublicIp('2606:2800:220:1:248:1893:25c8:1946')).toBe(true)
    })

    it.each([
        ['10.0.0.1'],
        ['127.0.0.1'],
        ['0.0.0.0'],
        ['169.254.1.1'],
        ['172.16.0.1'],
        ['172.31.255.255'],
        ['192.168.1.30'],
        ['100.64.0.1'],
        ['100.127.255.255'],
        ['::1'],
        ['::'],
        ['fe80::1'],
        ['fd00::1'],
        ['::ffff:192.168.1.30'],
        ['::ffff:127.0.0.1'],
    ])('rejects private/reserved %s', (ip) => {
        expect(isPublicIp(ip)).toBe(false)
    })

    it('rejects garbage', () => {
        expect(isPublicIp('not-an-ip')).toBe(false)
    })
})
