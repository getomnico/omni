#!/usr/bin/env npx tsx
/**
 * Example RSS Feed Connector for Omni.
 *
 * This connector demonstrates how to use the @getomnico/connector SDK to build
 * a custom connector that syncs RSS feed articles into Omni.
 *
 * Usage:
 *   npm install rss-parser
 *   CONNECTOR_MANAGER_URL=http://localhost:8080 npx tsx rss-connector.ts
 *
 * The connector expects source config with:
 *   {
 *     "feed_url": "https://example.com/feed.xml"
 *   }
 */

import {
  Connector,
  type SyncContext,
  type ActionDefinition,
  type Document,
  ActionResponse,
} from '../src/index.js';

// You'll need to install rss-parser: npm install rss-parser
// import Parser from 'rss-parser';

interface FeedItem {
  id?: string;
  link?: string;
  title?: string;
  content?: string;
  contentSnippet?: string;
  pubDate?: string;
  creator?: string;
  isoDate?: string;
}

interface Feed {
  title?: string;
  items: FeedItem[];
}

class RSSConnector extends Connector {
  name = 'rss';
  version = '1.0.0';
  syncModes = ['full', 'incremental'];

  actions: ActionDefinition[] = [
    {
      name: 'validate_feed',
      description: 'Validate that an RSS feed URL is accessible and parseable',
      parameters: {
        feed_url: {
          type: 'string',
          required: true,
          description: 'The RSS feed URL to validate',
        },
      },
    },
  ];

  async sync(
    sourceConfig: Record<string, unknown>,
    _credentials: Record<string, unknown>,
    state: Record<string, unknown> | null,
    ctx: SyncContext
  ): Promise<void> {
    const feedUrl = sourceConfig.feed_url as string | undefined;
    if (!feedUrl) {
      await ctx.fail('Missing feed_url in source config');
      return;
    }

    let lastSyncTime: Date | null = null;
    if (state?.last_sync_time) {
      lastSyncTime = new Date(state.last_sync_time as string);
    }

    console.log(`Fetching RSS feed: ${feedUrl}`);

    let feed: Feed;
    try {
      feed = await this.parseFeed(feedUrl);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      await ctx.fail(`Failed to parse feed: ${message}`);
      return;
    }

    let docsSinceCheckpoint = 0;
    const currentTime = new Date();

    for (const entry of feed.items) {
      if (ctx.isCancelled()) {
        await ctx.fail('Cancelled by user');
        return;
      }

      await ctx.incrementScanned();

      const published = entry.isoDate ? new Date(entry.isoDate) : null;
      if (lastSyncTime && published && published <= lastSyncTime) {
        continue;
      }

      const entryId = entry.id ?? entry.link ?? this.hashEntry(entry);
      if (!entryId) {
        ctx.emitError('unknown', 'Entry has no id or link');
        continue;
      }

      const content = entry.content ?? entry.contentSnippet ?? entry.title ?? '';

      let contentId: string;
      try {
        contentId = await ctx.contentStorage.save(content, 'text/html');
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        ctx.emitError(entryId, `Failed to store content: ${message}`);
        continue;
      }

      const doc: Document = {
        external_id: entryId,
        title: entry.title ?? 'Untitled',
        content_id: contentId,
        metadata: {
          author: entry.creator,
          created_at: entry.isoDate,
          updated_at: entry.isoDate,
          url: entry.link,
          mime_type: 'text/html',
          extra: {
            feed_title: feed.title,
            feed_url: feedUrl,
          },
        },
        permissions: { public: true, users: [], groups: [] },
        attributes: {
          source_type: 'rss',
          feed_url: feedUrl,
        },
      };

      await ctx.emit(doc);
      docsSinceCheckpoint++;

      if (docsSinceCheckpoint >= 50) {
        await ctx.saveState({ last_sync_time: currentTime.toISOString() });
        docsSinceCheckpoint = 0;
      }
    }

    await ctx.complete({ last_sync_time: currentTime.toISOString() });
    console.log(
      `Sync completed: ${ctx.documentsScanned} scanned, ${ctx.documentsEmitted} emitted`
    );
  }

  async executeAction(
    action: string,
    params: Record<string, unknown>,
    _credentials: Record<string, unknown>
  ): Promise<Response> {
    if (action === 'validate_feed') {
      const feedUrl = params.feed_url as string | undefined;
      if (!feedUrl) {
        return ActionResponse.failure('Missing feed_url parameter').toResponse(400);
      }

      try {
        const feed = await this.parseFeed(feedUrl);
        return ActionResponse.success({
          valid: true,
          title: feed.title ?? 'Unknown',
          entry_count: feed.items.length,
        }).toResponse();
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        return ActionResponse.failure(`Failed to fetch feed: ${message}`).toResponse(500);
      }
    }

    return ActionResponse.notSupported(action).toResponse(404);
  }

  /**
   * Parse an RSS feed from a URL.
   *
   * In a real implementation, you would use a library like rss-parser:
   *   import Parser from 'rss-parser';
   *   const parser = new Parser();
   *   return parser.parseURL(url);
   *
   * For this example, we'll use a simple fetch + regex approach.
   */
  private async parseFeed(url: string): Promise<Feed> {
    const response = await fetch(url);
    if (!response.ok) {
      throw new Error(`HTTP ${response.status}: ${response.statusText}`);
    }

    const xml = await response.text();

    // Simple regex-based XML parsing (for demonstration only)
    // In production, use a proper XML parser or rss-parser library
    const titleMatch = xml.match(/<title>([^<]*)<\/title>/);
    const items: FeedItem[] = [];

    const itemRegex = /<item>([\s\S]*?)<\/item>/g;
    let match;
    while ((match = itemRegex.exec(xml)) !== null) {
      const itemXml = match[1];
      items.push({
        title: this.extractTag(itemXml, 'title'),
        link: this.extractTag(itemXml, 'link'),
        content: this.extractTag(itemXml, 'content:encoded') ??
                 this.extractTag(itemXml, 'description'),
        pubDate: this.extractTag(itemXml, 'pubDate'),
        isoDate: this.parseDate(this.extractTag(itemXml, 'pubDate')),
        creator: this.extractTag(itemXml, 'dc:creator') ??
                 this.extractTag(itemXml, 'author'),
      });
    }

    return {
      title: titleMatch?.[1],
      items,
    };
  }

  private extractTag(xml: string, tag: string): string | undefined {
    const regex = new RegExp(`<${tag}[^>]*>([\\s\\S]*?)</${tag}>`, 'i');
    const match = xml.match(regex);
    if (match) {
      return match[1]
        .replace(/<!\[CDATA\[/g, '')
        .replace(/\]\]>/g, '')
        .trim();
    }
    return undefined;
  }

  private parseDate(dateStr: string | undefined): string | undefined {
    if (!dateStr) return undefined;
    try {
      return new Date(dateStr).toISOString();
    } catch {
      return undefined;
    }
  }

  private hashEntry(entry: FeedItem): string {
    const content = `${entry.title ?? ''}${entry.contentSnippet ?? ''}`;
    let hash = 0;
    for (let i = 0; i < content.length; i++) {
      const char = content.charCodeAt(i);
      hash = (hash << 5) - hash + char;
      hash = hash & hash;
    }
    return Math.abs(hash).toString(16).substring(0, 16);
  }
}

const port = parseInt(process.env.PORT ?? '8000', 10);
new RSSConnector().serve({ port });
