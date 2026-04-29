import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import type {
  ActionDefinition,
  McpPromptDefinition,
  McpResourceDefinition,
} from './models.js';
import { ActionResponse } from './models.js';
import { getLogger } from './logger.js';

const logger = getLogger('sdk:mcp-adapter');

/**
 * Configuration for an MCP server reached via stdio (subprocess).
 */
export interface StdioMcpServer {
  transport: 'stdio';
  command: string;
  args?: string[];
  env?: Record<string, string>;
  cwd?: string;
}

/**
 * Configuration for a remote MCP server reached via Streamable HTTP.
 */
export interface HttpMcpServer {
  transport: 'http';
  url: string;
  headers?: Record<string, string>;
  /** Forwarded to StreamableHTTPClientTransport's `requestInit` (merged with `headers`). */
  requestInit?: RequestInit;
}

export type McpServer = StdioMcpServer | HttpMcpServer;

/**
 * Bridges an external MCP server into Omni's connector protocol.
 *
 * Supports two transports:
 * - stdio: spawns the MCP server as a subprocess and talks JSON-RPC over
 *   stdin/stdout.
 * - Streamable HTTP: connects to a remote MCP endpoint per the MCP spec.
 *
 * Each operation opens a fresh client/transport pair and tears it down
 * afterwards. Tool/resource/prompt definitions are cached after the first
 * successful discovery so manifest builds don't require live auth.
 */
export class McpAdapter {
  private server: McpServer;
  private cachedActions: ActionDefinition[] | null = null;
  private cachedResources: McpResourceDefinition[] | null = null;
  private cachedPrompts: McpPromptDefinition[] | null = null;

  constructor(server: McpServer) {
    this.server = server;
  }

  private async withSession<T>(
    env: Record<string, string> | undefined,
    headers: Record<string, string> | undefined,
    fn: (client: Client) => Promise<T>
  ): Promise<T> {
    const client = new Client({ name: 'omni-mcp-adapter', version: '1.0.0' });
    if (this.server.transport === 'stdio') {
      const { StdioClientTransport } = await import(
        '@modelcontextprotocol/sdk/client/stdio.js'
      );
      const mergedEnv = { ...(this.server.env ?? {}), ...(env ?? {}) };
      const transport = new StdioClientTransport({
        command: this.server.command,
        args: this.server.args,
        env: Object.keys(mergedEnv).length > 0 ? mergedEnv : undefined,
        cwd: this.server.cwd,
      });
      logger.debug(
        `Spawning MCP subprocess: ${this.server.command} ${(this.server.args ?? []).join(' ')}`
      );
      await client.connect(transport);
      try {
        return await fn(client);
      } finally {
        await client.close();
      }
    } else {
      const { StreamableHTTPClientTransport } = await import(
        '@modelcontextprotocol/sdk/client/streamableHttp.js'
      );
      const mergedHeaders = { ...(this.server.headers ?? {}), ...(headers ?? {}) };
      const baseInit = this.server.requestInit ?? {};
      const baseHeaders = (baseInit.headers ?? {}) as Record<string, string>;
      const transport = new StreamableHTTPClientTransport(new URL(this.server.url), {
        requestInit: {
          ...baseInit,
          headers: { ...baseHeaders, ...mergedHeaders },
        },
      });
      logger.debug(`Opening MCP HTTP session: ${this.server.url}`);
      await client.connect(transport);
      try {
        return await fn(client);
      } finally {
        await client.close();
      }
    }
  }

  async discover(
    env?: Record<string, string>,
    headers?: Record<string, string>
  ): Promise<void> {
    await this.withSession(env, headers, async (client) => {
      this.cachedActions = await this.fetchActions(client);
      this.cachedResources = await this.fetchResources(client);
      this.cachedPrompts = await this.fetchPrompts(client);
    });
    logger.info(
      `MCP discovery complete: ${this.cachedActions?.length ?? 0} tools, ` +
        `${this.cachedResources?.length ?? 0} resources, ` +
        `${this.cachedPrompts?.length ?? 0} prompts`
    );
  }

  async getActionDefinitions(
    env?: Record<string, string>,
    headers?: Record<string, string>
  ): Promise<ActionDefinition[]> {
    if (env !== undefined || headers !== undefined) {
      try {
        const actions = await this.withSession(env, headers, (c) =>
          this.fetchActions(c)
        );
        this.cachedActions = actions;
        return actions;
      } catch (err) {
        if (this.cachedActions !== null) {
          logger.debug(
            `Live action fetch failed, returning ${this.cachedActions.length} cached`
          );
          return this.cachedActions;
        }
        throw err;
      }
    }
    return this.cachedActions ?? [];
  }

  async getResourceDefinitions(
    env?: Record<string, string>,
    headers?: Record<string, string>
  ): Promise<McpResourceDefinition[]> {
    if (env !== undefined || headers !== undefined) {
      try {
        const resources = await this.withSession(env, headers, (c) =>
          this.fetchResources(c)
        );
        this.cachedResources = resources;
        return resources;
      } catch (err) {
        if (this.cachedResources !== null) {
          return this.cachedResources;
        }
        throw err;
      }
    }
    return this.cachedResources ?? [];
  }

  async getPromptDefinitions(
    env?: Record<string, string>,
    headers?: Record<string, string>
  ): Promise<McpPromptDefinition[]> {
    if (env !== undefined || headers !== undefined) {
      try {
        const prompts = await this.withSession(env, headers, (c) =>
          this.fetchPrompts(c)
        );
        this.cachedPrompts = prompts;
        return prompts;
      } catch (err) {
        if (this.cachedPrompts !== null) {
          return this.cachedPrompts;
        }
        throw err;
      }
    }
    return this.cachedPrompts ?? [];
  }

  async executeTool(
    name: string,
    args: Record<string, unknown>,
    env?: Record<string, string>,
    headers?: Record<string, string>
  ): Promise<ActionResponse> {
    try {
      return await this.withSession(env, headers, async (client) => {
        const result = await client.callTool({ name, arguments: args });
        if (result.isError) {
          const errorText = (
            result.content as Array<{ type: string; text?: string }>
          )
            .filter((c) => c.type === 'text')
            .map((c) => c.text ?? '')
            .join('\n');
          return ActionResponse.failure(errorText || 'Tool execution failed');
        }

        if (
          result.structuredContent &&
          typeof result.structuredContent === 'object'
        ) {
          return ActionResponse.success(
            result.structuredContent as Record<string, unknown>
          );
        }

        const textParts: string[] = [];
        for (const block of result.content as Array<{
          type: string;
          text?: string;
          mimeType?: string;
        }>) {
          if (block.type === 'text' && block.text) {
            textParts.push(block.text);
          } else if (block.type === 'image') {
            textParts.push(`[binary: ${block.mimeType ?? 'unknown'}]`);
          }
        }
        return ActionResponse.success({ content: textParts.join('\n') });
      });
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      logger.error({ err }, `MCP tool ${name} failed`);
      return ActionResponse.failure(message);
    }
  }

  async readResource(
    uri: string,
    env?: Record<string, string>,
    headers?: Record<string, string>
  ): Promise<{ contents: Array<Record<string, unknown>> }> {
    return await this.withSession(env, headers, async (client) => {
      const result = await client.readResource({ uri });
      const items: Array<Record<string, unknown>> = [];
      for (const item of result.contents) {
        const entry: Record<string, unknown> = { uri: item.uri ?? uri };
        if ('text' in item && item.text != null) {
          entry.text = item.text;
        }
        if ('blob' in item && item.blob != null) {
          entry.blob = item.blob;
        }
        if (item.mimeType) {
          entry.mime_type = item.mimeType;
        }
        items.push(entry);
      }
      return { contents: items };
    });
  }

  async getPrompt(
    name: string,
    args?: Record<string, unknown>,
    env?: Record<string, string>,
    headers?: Record<string, string>
  ): Promise<{ description?: string; messages: Array<Record<string, unknown>> }> {
    return await this.withSession(env, headers, async (client) => {
      const result = await client.getPrompt({
        name,
        arguments: args as Record<string, string> | undefined,
      });
      const messages: Array<Record<string, unknown>> = [];
      for (const msg of result.messages) {
        let contentData: Record<string, unknown>;
        if (msg.content.type === 'text') {
          contentData = { type: 'text', text: msg.content.text };
        } else {
          contentData = { type: msg.content.type };
        }
        messages.push({ role: msg.role, content: contentData });
      }
      return { description: result.description, messages };
    });
  }

  // -- internal helpers --

  private async fetchActions(client: Client): Promise<ActionDefinition[]> {
    const { tools } = await client.listTools();
    const actions: ActionDefinition[] = [];
    for (const tool of tools) {
      const isReadOnly = tool.annotations?.readOnlyHint === true;
      actions.push({
        name: tool.name,
        description: tool.description ?? '',
        input_schema: tool.inputSchema ?? { type: 'object', properties: {} },
        mode: isReadOnly ? 'read' : 'write',
        source_types: [],
        admin_only: false,
      });
    }
    return actions;
  }

  private async fetchResources(client: Client): Promise<McpResourceDefinition[]> {
    const definitions: McpResourceDefinition[] = [];

    const { resourceTemplates } = await client.listResourceTemplates();
    for (const tmpl of resourceTemplates) {
      definitions.push({
        uri_template: tmpl.uriTemplate,
        name: tmpl.name,
        description: tmpl.description,
        mime_type: tmpl.mimeType,
      });
    }

    const { resources } = await client.listResources();
    for (const res of resources) {
      definitions.push({
        uri_template: res.uri,
        name: res.name,
        description: res.description,
        mime_type: res.mimeType,
      });
    }

    return definitions;
  }

  private async fetchPrompts(client: Client): Promise<McpPromptDefinition[]> {
    const { prompts } = await client.listPrompts();
    return prompts.map((prompt) => ({
      name: prompt.name,
      description: prompt.description,
      arguments: (prompt.arguments ?? []).map((arg) => ({
        name: arg.name,
        description: arg.description,
        required: arg.required ?? false,
      })),
    }));
  }
}
