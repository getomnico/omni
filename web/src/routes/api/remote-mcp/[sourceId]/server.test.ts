import { describe, expect, it } from 'vitest'
import { AuthType } from '$lib/types'
import { remoteMcpPutTransition } from './+server'

describe('remote MCP PUT state transitions', () => {
    it('preserves OAuth active state and credentials for name-only or write-tools edits', () => {
        const transition = remoteMcpPutTransition(
            true,
            {
                endpoint_url: 'https://mcp.example.com/mcp',
                auth_type: AuthType.OAUTH,
                write_tools_enabled: true,
            },
            {
                endpoint_url: 'https://mcp.example.com/mcp',
                auth_type: AuthType.OAUTH,
                write_tools_enabled: false,
            },
        )

        expect(transition).toEqual({
            shouldBeActive: true,
            shouldDeleteCredentials: false,
            oauthBootstrapRequired: false,
        })
    })

    it('deactivates and drops credentials when OAuth endpoint changes', () => {
        const transition = remoteMcpPutTransition(
            true,
            {
                endpoint_url: 'https://mcp.example.com/mcp',
                auth_type: AuthType.OAUTH,
                write_tools_enabled: true,
            },
            {
                endpoint_url: 'https://new-mcp.example.com/mcp',
                auth_type: AuthType.OAUTH,
                write_tools_enabled: true,
            },
        )

        expect(transition).toEqual({
            shouldBeActive: false,
            shouldDeleteCredentials: true,
            oauthBootstrapRequired: true,
        })
    })

    it('activates public and bearer transitions after successful probe', () => {
        expect(
            remoteMcpPutTransition(
                false,
                {
                    endpoint_url: 'https://mcp.example.com/mcp',
                    auth_type: AuthType.OAUTH,
                    write_tools_enabled: true,
                },
                {
                    endpoint_url: 'https://mcp.example.com/mcp',
                    auth_type: null,
                    write_tools_enabled: true,
                },
            ).shouldBeActive,
        ).toBe(true)

        expect(
            remoteMcpPutTransition(
                false,
                {
                    endpoint_url: 'https://mcp.example.com/mcp',
                    auth_type: AuthType.OAUTH,
                    write_tools_enabled: true,
                },
                {
                    endpoint_url: 'https://mcp.example.com/mcp',
                    auth_type: AuthType.BEARER_TOKEN,
                    write_tools_enabled: true,
                },
            ).shouldBeActive,
        ).toBe(true)
    })
})
