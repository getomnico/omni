// TODO: Rewrite to use StdioClientTransport (subprocess) instead of InMemoryTransport.
// The Python SDK has already been migrated. See sdk/python/omni_connector/mcp_adapter.py.
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { InMemoryTransport } from '@modelcontextprotocol/sdk/inMemory.js';
import type { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';
import type {
  ActionDefinition,
  ActionResponse,
  McpPromptDefinition,
  McpResourceDefinition,
} from './models.js';
import { ActionResponse } from './models.js';
import { getLogger } from './logger.js';

const logger = getLogger('sdk:mcp-adapter');

/**
 * Bridges an MCP McpServer into Omni's connector protocol.
 *
 * Uses InMemoryTransport to create an in-process client-server pair,
 * then interacts with the server through the standard MCP Client API.
 */
export class McpAdapter {
  private server: McpServer;
  private client: Client | null = null;

  constructor(mcpServer: McpServer) {
    this.server = mcpServer;
  }

  private async ensureConnected(): Promise<Client> {
    if (this.client) {
      return this.client;
    }
    const [clientTransport, serverTransport] = InMemoryTransport.createLinkedPair();
    await this.server.connect(serverTransport);
    this.client = new Client({ name: 'omni-mcp-adapter', version: '1.0.0' });
    await this.client.connect(clientTransport);
    return this.client;
  }

  async getActionDefinitions(): Promise<ActionDefinition[]> {
    const client = await this.ensureConnected();
    const { tools } = await client.listTools();
    const actions: ActionDefinition[] = [];

    for (const tool of tools) {
      const isReadOnly = tool.annotations?.readOnlyHint === true;
      actions.push({
        name: tool.name,
        description: tool.description ?? '',
        input_schema: tool.inputSchema ?? { type: 'object', properties: {} },
        mode: isReadOnly ? 'read' : 'write',
      });
    }

    return actions;
  }

  async getResourceDefinitions(): Promise<McpResourceDefinition[]> {
    const client = await this.ensureConnected();
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

  async getPromptDefinitions(): Promise<McpPromptDefinition[]> {
    const client = await this.ensureConnected();
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

  async executeTool(
    name: string,
    args: Record<string, unknown>
  ): Promise<ActionResponse> {
    const client = await this.ensureConnected();

    try {
      const result = await client.callTool({ name, arguments: args });
      if (result.isError) {
        const errorText = (result.content as Array<{ type: string; text?: string }>)
          .filter((c) => c.type === 'text')
          .map((c) => c.text ?? '')
          .join('\n');
        return ActionResponse.failure(errorText || 'Tool execution failed');
      }

      if (result.structuredContent && typeof result.structuredContent === 'object') {
        return ActionResponse.success(
          result.structuredContent as Record<string, unknown>
        );
      }

      const textParts: string[] = [];
      for (const block of result.content as Array<{ type: string; text?: string; mimeType?: string }>) {
        if (block.type === 'text' && block.text) {
          textParts.push(block.text);
        } else if (block.type === 'image') {
          textParts.push(`[binary: ${block.mimeType ?? 'unknown'}]`);
        }
      }
      return ActionResponse.success({ content: textParts.join('\n') });
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      logger.error({ err }, `MCP tool ${name} failed`);
      return ActionResponse.failure(message);
    }
  }

  async readResource(
    uri: string
  ): Promise<{ contents: Array<Record<string, unknown>> }> {
    const client = await this.ensureConnected();
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
  }

  async getPrompt(
    name: string,
    args?: Record<string, unknown>
  ): Promise<{ description?: string; messages: Array<Record<string, unknown>> }> {
    const client = await this.ensureConnected();
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
  }
}
