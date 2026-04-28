import { describe, it, expect, beforeAll, afterAll } from 'vitest';
import { spawn, type ChildProcess } from 'node:child_process';
import { createServer as createNetServer } from 'node:net';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import { McpAdapter, type HttpMcpServer, type StdioMcpServer } from '../src/mcp-adapter.js';
import { Connector } from '../src/connector.js';
import type { ConnectorManifest } from '../src/models.js';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const FIXTURE = join(__dirname, 'fixtures', 'test-mcp-server.mjs');

const STDIO_SERVER: StdioMcpServer = {
  transport: 'stdio',
  command: process.execPath,
  args: [FIXTURE],
};

function freePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const srv = createNetServer();
    srv.unref();
    srv.on('error', reject);
    srv.listen(0, () => {
      const addr = srv.address();
      if (addr && typeof addr === 'object') {
        const { port } = addr;
        srv.close(() => resolve(port));
      } else {
        reject(new Error('could not get port'));
      }
    });
  });
}

async function waitForHttp(url: string, timeoutMs = 8000): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      // Any TCP-level reply means the server is up. The MCP endpoint rejects
      // bare GETs but that's fine — we only need to confirm the socket is open.
      await fetch(url, { method: 'GET' });
      return;
    } catch {
      await new Promise((r) => setTimeout(r, 100));
    }
  }
  throw new Error(`HTTP fixture did not become ready at ${url}`);
}

describe('McpAdapter (stdio)', () => {
  it('lists tools', async () => {
    const adapter = new McpAdapter(STDIO_SERVER);
    const actions = await adapter.getActionDefinitions({ TEST_MODE: '1' });
    const names = actions.map((a) => a.name).sort();
    expect(names).toEqual(['add', 'greet']);
    const greet = actions.find((a) => a.name === 'greet')!;
    expect(greet.mode).toBe('read');
    expect(greet.description).toBe('Greet someone by name');
    const add = actions.find((a) => a.name === 'add')!;
    expect(add.mode).toBe('write');
  });

  it('lists resources', async () => {
    const adapter = new McpAdapter(STDIO_SERVER);
    const resources = await adapter.getResourceDefinitions({ TEST_MODE: '1' });
    expect(resources).toHaveLength(1);
    expect(resources[0].uri_template).toBe('test://item/{item_id}');
  });

  it('lists prompts', async () => {
    const adapter = new McpAdapter(STDIO_SERVER);
    const prompts = await adapter.getPromptDefinitions({ TEST_MODE: '1' });
    expect(prompts).toHaveLength(1);
    expect(prompts[0].name).toBe('summarize');
    expect(prompts[0].arguments[0].name).toBe('text');
    expect(prompts[0].arguments[0].required).toBe(true);
  });

  it('executes a tool', async () => {
    const adapter = new McpAdapter(STDIO_SERVER);
    const result = await adapter.executeTool('greet', { name: 'World' }, {
      TEST_MODE: '1',
    });
    expect(result.status).toBe('success');
    expect(result.result?.content).toContain('Hello, World!');
  });

  it('returns error for nonexistent tool', async () => {
    const adapter = new McpAdapter(STDIO_SERVER);
    const result = await adapter.executeTool('nonexistent', {}, { TEST_MODE: '1' });
    expect(result.status).toBe('error');
  });

  it('reads a resource', async () => {
    const adapter = new McpAdapter(STDIO_SERVER);
    const result = await adapter.readResource('test://item/42', { TEST_MODE: '1' });
    expect(result.contents.length).toBeGreaterThanOrEqual(1);
  });

  it('gets a prompt', async () => {
    const adapter = new McpAdapter(STDIO_SERVER);
    const result = await adapter.getPrompt(
      'summarize',
      { text: 'hello world' },
      { TEST_MODE: '1' }
    );
    expect(result.messages.length).toBeGreaterThanOrEqual(1);
    const msg = result.messages[0];
    expect(msg.role).toBe('user');
    expect((msg.content as { text: string }).text).toContain('hello world');
  });

  it('caches discovered definitions', async () => {
    const adapter = new McpAdapter(STDIO_SERVER);
    await adapter.discover({ TEST_MODE: '1' });
    // No env — returns from cache without spawning
    const actions = await adapter.getActionDefinitions();
    expect(actions.map((a) => a.name).sort()).toEqual(['add', 'greet']);
    const resources = await adapter.getResourceDefinitions();
    expect(resources).toHaveLength(1);
    const prompts = await adapter.getPromptDefinitions();
    expect(prompts).toHaveLength(1);
  });

  it('returns empty without auth and without cache', async () => {
    const adapter = new McpAdapter(STDIO_SERVER);
    expect(await adapter.getActionDefinitions()).toEqual([]);
    expect(await adapter.getResourceDefinitions()).toEqual([]);
    expect(await adapter.getPromptDefinitions()).toEqual([]);
  });
});

describe('McpAdapter (Streamable HTTP)', () => {
  let proc: ChildProcess | null = null;
  let url: string;

  beforeAll(async () => {
    const port = await freePort();
    proc = spawn(process.execPath, [FIXTURE, 'http', String(port)], {
      stdio: ['ignore', 'ignore', 'ignore'],
    });
    url = `http://127.0.0.1:${port}/mcp`;
    await waitForHttp(url);
  }, 15000);

  afterAll(async () => {
    if (proc && proc.pid !== undefined) {
      proc.kill('SIGTERM');
      await new Promise((r) => setTimeout(r, 100));
      if (!proc.killed) {
        proc.kill('SIGKILL');
      }
    }
  });

  it('lists tools', async () => {
    const adapter = new McpAdapter({ transport: 'http', url });
    const actions = await adapter.getActionDefinitions(undefined, { 'X-Test': '1' });
    expect(actions.map((a) => a.name).sort()).toEqual(['add', 'greet']);
  });

  it('executes a tool', async () => {
    const adapter = new McpAdapter({ transport: 'http', url });
    const result = await adapter.executeTool(
      'greet',
      { name: 'Remote' },
      undefined,
      { 'X-Test': '1' }
    );
    expect(result.status).toBe('success');
    expect(result.result?.content).toContain('Hello, Remote!');
  });

  it('reads a resource', async () => {
    const adapter = new McpAdapter({ transport: 'http', url });
    const result = await adapter.readResource('test://item/99', undefined, {
      'X-Test': '1',
    });
    expect(result.contents.length).toBeGreaterThanOrEqual(1);
  });

  it('gets a prompt', async () => {
    const adapter = new McpAdapter({ transport: 'http', url });
    const result = await adapter.getPrompt(
      'summarize',
      { text: 'remote text' },
      undefined,
      { 'X-Test': '1' }
    );
    expect(result.messages.length).toBeGreaterThanOrEqual(1);
  });

  it('caches discovered definitions', async () => {
    const adapter = new McpAdapter({ transport: 'http', url });
    await adapter.discover(undefined, { 'X-Test': '1' });
    expect(
      (await adapter.getActionDefinitions()).map((a) => a.name).sort()
    ).toEqual(['add', 'greet']);
    expect(await adapter.getResourceDefinitions()).toHaveLength(1);
    expect(await adapter.getPromptDefinitions()).toHaveLength(1);
  });

  it('merges static + per-call headers', async () => {
    const server: HttpMcpServer = {
      transport: 'http',
      url,
      headers: { 'X-Static': 'yes' },
    };
    const adapter = new McpAdapter(server);
    const actions = await adapter.getActionDefinitions(undefined, {
      'X-Per-Call': 'yes',
    });
    expect(actions).toHaveLength(2);
  });
});

describe('Connector MCP integration', () => {
  it('stdio: includes MCP tools in manifest', async () => {
    class StdioMcpConnector extends Connector {
      readonly name = 'mcp-test-stdio';
      readonly version = '0.1.0';
      readonly sourceTypes = ['mcp_test'];

      get mcpServer(): StdioMcpServer {
        return STDIO_SERVER;
      }

      async sync(): Promise<void> {}
    }

    const connector = new StdioMcpConnector();
    await connector.bootstrapMcp({});
    const manifest: ConnectorManifest = await connector.getManifest('http://test:8000');
    expect(manifest.mcp_enabled).toBe(true);
    const actionNames = manifest.actions.map((a) => a.name);
    expect(actionNames).toContain('greet');
    expect(actionNames).toContain('add');
    expect(manifest.resources).toHaveLength(1);
    expect(manifest.prompts).toHaveLength(1);
  });

  it('stdio: delegates action execution to MCP tool', async () => {
    class StdioMcpConnector extends Connector {
      readonly name = 'mcp-test-stdio';
      readonly version = '0.1.0';
      readonly sourceTypes = ['mcp_test'];

      get mcpServer(): StdioMcpServer {
        return STDIO_SERVER;
      }

      async sync(): Promise<void> {}
    }

    const connector = new StdioMcpConnector();
    const result = await connector.executeAction('greet', { name: 'Omni' }, {});
    expect(result).toBeInstanceOf(Response);
    expect(result.status).toBe(200);
    const body = JSON.parse(await result.text());
    expect(body.status).toBe('success');
  });

  it('returns not supported for unknown actions', async () => {
    class StdioMcpConnector extends Connector {
      readonly name = 'mcp-test-stdio';
      readonly version = '0.1.0';
      readonly sourceTypes = ['mcp_test'];

      get mcpServer(): StdioMcpServer {
        return STDIO_SERVER;
      }

      async sync(): Promise<void> {}
    }

    const connector = new StdioMcpConnector();
    const result = await connector.executeAction('unknown', {}, {});
    expect(result.status).toBe(404);
    const body = JSON.parse(await result.text());
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
