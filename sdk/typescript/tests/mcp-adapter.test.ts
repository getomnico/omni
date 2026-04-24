import { describe, it, expect, beforeEach } from 'vitest';
import { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';
import { z } from 'zod';
import { McpAdapter } from '../src/mcp-adapter.js';
import { Connector } from '../src/connector.js';
import type { ConnectorManifest } from '../src/models.js';

function createTestMcpServer(): McpServer {
  const server = new McpServer({ name: 'test', version: '1.0.0' });

  server.registerTool(
    'greet',
    {
      description: 'Greet someone by name',
      annotations: { readOnlyHint: true },
      inputSchema: { name: z.string().describe('Person to greet') },
    },
    async (args) => ({
      content: [{ type: 'text' as const, text: `Hello, ${args.name}!` }],
    })
  );

  server.tool(
    'add',
    'Add two numbers',
    { a: z.number(), b: z.number() },
    async (args) => ({
      content: [{ type: 'text' as const, text: String(args.a + args.b) }],
    })
  );

  server.resource(
    'item',
    'test://item/{item_id}',
    async (uri) => ({
      contents: [{ uri: uri.href, text: `Item content`, mimeType: 'text/plain' }],
    })
  );

  server.prompt(
    'summarize',
    'Summarize the given text',
    { text: z.string() },
    async (args) => ({
      messages: [
        {
          role: 'user' as const,
          content: { type: 'text' as const, text: `Please summarize: ${args.text}` },
        },
      ],
    })
  );

  return server;
}

describe('McpAdapter', () => {
  let adapter: McpAdapter;

  beforeEach(() => {
    const server = createTestMcpServer();
    adapter = new McpAdapter(server);
  });

  it('converts MCP tools to action definitions', async () => {
    const actions = await adapter.getActionDefinitions();
    expect(actions.length).toBe(2);
    const names = actions.map((a) => a.name);
    expect(names).toContain('greet');
    expect(names).toContain('add');

    const greet = actions.find((a) => a.name === 'greet')!;
    expect(greet.description).toBe('Greet someone by name');
    expect(greet.input_schema.properties.name).toBeDefined();
    expect(greet.input_schema.properties.name.type).toBe('string');
    expect(greet.input_schema.required).toContain('name');
    expect(greet.mode).toBe('read');

    const add = actions.find((a) => a.name === 'add')!;
    expect(add.mode).toBe('write');
  });

  it('converts MCP resources to resource definitions', async () => {
    const resources = await adapter.getResourceDefinitions();
    expect(resources.length).toBe(1);
    expect(resources[0].name).toBe('item');
    expect(resources[0].uri_template).toBe('test://item/{item_id}');
  });

  it('converts MCP prompts to prompt definitions', async () => {
    const prompts = await adapter.getPromptDefinitions();
    expect(prompts.length).toBe(1);
    expect(prompts[0].name).toBe('summarize');
    expect(prompts[0].description).toBe('Summarize the given text');
  });

  it('executes a tool and returns success', async () => {
    const result = await adapter.executeTool('greet', { name: 'World' });
    expect(result.status).toBe('success');
    expect(result.result).toBeDefined();
    expect(result.result!.content).toContain('Hello, World!');
  });

  it('returns failure for nonexistent tool', async () => {
    const result = await adapter.executeTool('nonexistent', {});
    expect(result.status).toBe('error');
  });

  it('gets a prompt', async () => {
    const result = await adapter.getPrompt('summarize', { text: 'hello world' });
    expect(result.messages.length).toBeGreaterThanOrEqual(1);
    const msg = result.messages[0];
    expect(msg.role).toBe('user');
    expect((msg.content as { text: string }).text).toContain('hello world');
  });
});

describe('Connector MCP integration', () => {
  it('includes MCP tools in manifest as actions', async () => {
    class McpTestConnector extends Connector {
      readonly name = 'mcp-test';
      readonly version = '0.1.0';
      readonly sourceTypes = ['mcp_test'];

      get mcpServer(): McpServer {
        return createTestMcpServer();
      }

      async sync(): Promise<void> {}
    }

    const connector = new McpTestConnector();
    const manifest: ConnectorManifest = await connector.getManifest('http://test:8000');

    expect(manifest.mcp_enabled).toBe(true);
    const actionNames = manifest.actions.map((a) => a.name);
    expect(actionNames).toContain('greet');
    expect(actionNames).toContain('add');
    expect(manifest.resources.length).toBe(1);
    expect(manifest.prompts.length).toBe(1);
  });

  it('delegates action execution to MCP tool', async () => {
    class McpTestConnector extends Connector {
      readonly name = 'mcp-test';
      readonly version = '0.1.0';
      readonly sourceTypes = ['mcp_test'];

      get mcpServer(): McpServer {
        return createTestMcpServer();
      }

      async sync(): Promise<void> {}
    }

    const connector = new McpTestConnector();
    const result = await connector.executeAction(
      'greet',
      { name: 'Omni' },
      {} as Record<string, unknown>
    );
    expect(result).toBeInstanceOf(Response);
    expect(result.status).toBe(200);
    const body = JSON.parse(await result.text());
    expect(body.status).toBe('success');
  });

  it('returns not supported for unknown actions', async () => {
    class McpTestConnector extends Connector {
      readonly name = 'mcp-test';
      readonly version = '0.1.0';
      readonly sourceTypes = ['mcp_test'];

      get mcpServer(): McpServer {
        return createTestMcpServer();
      }

      async sync(): Promise<void> {}
    }

    const connector = new McpTestConnector();
    const result = await connector.executeAction(
      'unknown',
      {},
      {} as Record<string, unknown>
    );
    expect(result).toBeInstanceOf(Response);
    expect(result.status).toBe(404);
    const body = JSON.parse(await result.text());
    expect(body.status).toBe('error');
    expect(body.error).toContain('not supported');
  });

  it('non-MCP connector has mcp_enabled=false', async () => {
    class PlainConnector extends Connector {
      readonly name = 'plain';
      readonly version = '0.1.0';
      readonly sourceTypes = ['plain'];

      async sync(): Promise<void> {}
    }

    const connector = new PlainConnector();
    const manifest = await connector.getManifest('http://test:8000');
    expect(manifest.mcp_enabled).toBe(false);
    expect(manifest.resources).toEqual([]);
    expect(manifest.prompts).toEqual([]);
  });
});
