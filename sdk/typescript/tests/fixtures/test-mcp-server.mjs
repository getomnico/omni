// Minimal MCP server used as a subprocess for testing the SDK's adapter.
// Supports both transports:
//   node test-mcp-server.mjs                  # stdio (default)
//   node test-mcp-server.mjs http <port>      # Streamable HTTP
import { McpServer, ResourceTemplate } from '@modelcontextprotocol/sdk/server/mcp.js';
import { StdioServerTransport } from '@modelcontextprotocol/sdk/server/stdio.js';
import { z } from 'zod';

function buildServer() {
  const server = new McpServer({ name: 'test', version: '1.0.0' });

  server.registerTool(
    'greet',
    {
      description: 'Greet someone by name',
      annotations: { readOnlyHint: true },
      inputSchema: { name: z.string().describe('Person to greet') },
    },
    async (args) => ({
      content: [{ type: 'text', text: `Hello, ${args.name}!` }],
    })
  );

  server.tool(
    'add',
    'Add two numbers',
    { a: z.number(), b: z.number() },
    async (args) => ({
      content: [{ type: 'text', text: String(args.a + args.b) }],
    })
  );

  server.resource(
    'item',
    new ResourceTemplate('test://item/{item_id}', { list: undefined }),
    async (uri, args) => ({
      contents: [
        {
          uri: uri.href,
          text: `Item ${args.item_id}`,
          mimeType: 'text/plain',
        },
      ],
    })
  );

  server.prompt(
    'summarize',
    'Summarize the given text',
    { text: z.string() },
    async (args) => ({
      messages: [
        {
          role: 'user',
          content: { type: 'text', text: `Please summarize: ${args.text}` },
        },
      ],
    })
  );

  return server;
}

async function runStdio() {
  const server = buildServer();
  const transport = new StdioServerTransport();
  await server.connect(transport);
}

async function runHttp(port) {
  const { StreamableHTTPServerTransport } = await import(
    '@modelcontextprotocol/sdk/server/streamableHttp.js'
  );
  const http = await import('node:http');

  const httpServer = http.createServer(async (req, res) => {
    if (!req.url || !req.url.startsWith('/mcp')) {
      res.statusCode = 404;
      res.end('not found');
      return;
    }
    const chunks = [];
    for await (const chunk of req) {
      chunks.push(chunk);
    }
    const bodyRaw = Buffer.concat(chunks).toString('utf8');
    const body = bodyRaw ? JSON.parse(bodyRaw) : undefined;

    // Stateless: spin up a fresh server + transport per request. Simpler than
    // tracking sessions across requests for a test fixture.
    const transport = new StreamableHTTPServerTransport({
      sessionIdGenerator: undefined,
    });
    const server = buildServer();
    await server.connect(transport);
    res.on('close', () => {
      transport.close();
      server.close();
    });
    await transport.handleRequest(req, res, body);
  });

  await new Promise((resolve) => httpServer.listen(port, '127.0.0.1', resolve));
  process.stdin.resume();
}

const mode = process.argv[2];
if (mode === 'http') {
  const port = Number(process.argv[3] ?? 8765);
  runHttp(port).catch((err) => {
    console.error('http fixture error:', err);
    process.exit(1);
  });
} else {
  runStdio().catch((err) => {
    console.error('stdio fixture error:', err);
    process.exit(1);
  });
}
