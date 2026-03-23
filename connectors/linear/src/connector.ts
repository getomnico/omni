import {
  Connector,
  type SyncContext,
  type ActionDefinition,
  type ActionResponse,
  type SearchOperator,
  createActionResponseSuccess,
  createActionResponseFailure,
  createActionResponseNotSupported,
  getLogger,
} from '@getomnico/connector';
import { LinearApiClient } from './client.js';
import {
  mapIssueToDocument,
  generateIssueContent,
  mapProjectToDocument,
  generateProjectContent,
  mapLinearDocumentToDocument,
  generateDocumentContent,
  mapProjectUpdateToDocument,
  generateProjectUpdateContent,
} from './mappers.js';
import type { LinearSyncState, LinearSourceConfig, LinearCredentials } from './types.js';

const logger = getLogger('linear');
const CHECKPOINT_INTERVAL = 100;

export class LinearConnector extends Connector<LinearSourceConfig, LinearCredentials, LinearSyncState> {
  readonly name = 'linear';
  readonly version = '1.0.0';
  readonly sourceTypes = ['linear'];

  get description(): string {
    return 'Connect to Linear issues, projects, and documents';
  }

  get displayName(): string {
    return 'Linear';
  }

  readonly syncModes = ['full', 'incremental'];

  readonly actions: ActionDefinition[] = [
    {
      name: 'list_teams',
      description: 'List all accessible teams in the Linear workspace',
      parameters: {},
      mode: 'read',
    },
  ];

  readonly searchOperators: SearchOperator[] = [
    { operator: 'status', attribute_key: 'status', value_type: 'text' },
    { operator: 'priority', attribute_key: 'priority', value_type: 'text' },
    { operator: 'label', attribute_key: 'labels', value_type: 'text' },
    { operator: 'assignee', attribute_key: 'assignee', value_type: 'person' },
    { operator: 'team', attribute_key: 'team', value_type: 'text' },
    { operator: 'project', attribute_key: 'project_name', value_type: 'text' },
  ];

  readonly extraSchema = {
    type: 'object',
    properties: {
      linear: {
        type: 'object',
        properties: {
          team_id: { type: 'string' },
          project_id: { type: 'string' },
        },
      },
    },
  };

  readonly attributesSchema = {
    type: 'object',
    properties: {
      status: { type: 'string' },
      priority: { type: 'string' },
      labels: { type: 'string' },
      assignee: { type: 'string' },
      assignee_email: { type: 'string', format: 'email' },
      team: { type: 'string' },
      identifier: { type: 'string' },
      project_name: { type: 'string' },
      health: { type: 'string' },
      lead: { type: 'string' },
    },
  };

  async sync(
    config: LinearSourceConfig,
    credentials: LinearCredentials,
    state: LinearSyncState | null,
    ctx: SyncContext,
  ): Promise<void> {
    const { api_key: apiKey } = credentials;
    if (!apiKey) {
      await ctx.fail("Missing 'api_key' in credentials");
      return;
    }

    const client = new LinearApiClient(apiKey);

    try {
      const userName = await client.validateApiKey();
      logger.info(`Starting Linear sync as '${userName}'`);
    } catch (e) {
      await ctx.fail(`Authentication failed: ${e}`);
      return;
    }

    const lastSyncAt = state?.last_sync_at;
    const isIncremental = !!lastSyncAt;
    let docsSinceCheckpoint = 0;

    try {
      const teams = await client.fetchTeams();
      const teamFilter = config.team_keys;
      const filteredTeams = teamFilter
        ? teams.filter(t => teamFilter.includes(t.key))
        : teams;

      // Build team ID → key lookup
      const teamIdToKey = new Map<string, string>();
      for (const team of filteredTeams) {
        teamIdToKey.set(team.id, team.key);
      }

      // Sync group memberships for each team
      logger.info('Syncing team memberships...');
      for (const team of filteredTeams) {
        try {
          const members = await client.fetchTeamMembers(team.id);
          const memberEmails = members.map(m => m.email);
          await ctx.emitGroupMembership(
            `linear-team:${team.key}`,
            memberEmails,
            team.name,
          );
          logger.info(`Synced ${memberEmails.length} members for team ${team.name}`);
        } catch (e) {
          logger.warn(`Failed to sync members for team ${team.name}: ${e}`);
        }
      }

      // Sync issues per team
      for (const team of filteredTeams) {
        if (ctx.isCancelled()) {
          await ctx.fail('Cancelled by user');
          return;
        }
        logger.info(`Syncing issues for team: ${team.name} (${team.key})`);

        for await (const issue of client.fetchIssues(team.id, lastSyncAt)) {
          if (ctx.isCancelled()) {
            await ctx.fail('Cancelled by user');
            return;
          }
          await ctx.incrementScanned();
          try {
            const comments = await client.fetchIssueComments(issue.id);
            const content = await generateIssueContent(issue, comments);
            const contentId = await ctx.contentStorage.save(content, 'text/markdown');
            const doc = await mapIssueToDocument(issue, comments, contentId, team.key);
            if (isIncremental) {
              await ctx.emitUpdated(doc);
            } else {
              await ctx.emit(doc);
            }
            docsSinceCheckpoint++;
            if (docsSinceCheckpoint >= CHECKPOINT_INTERVAL) {
              await ctx.saveState({ last_sync_at: new Date().toISOString() });
              docsSinceCheckpoint = 0;
            }
          } catch (e) {
            const eid = `linear:issue:${issue.id}`;
            logger.warn(`Error processing ${eid}: ${e}`);
            ctx.emitError(eid, String(e));
          }
        }
      }

      // Sync projects
      logger.info('Syncing projects...');
      for await (const project of client.fetchProjects(lastSyncAt)) {
        if (ctx.isCancelled()) {
          await ctx.fail('Cancelled by user');
          return;
        }
        await ctx.incrementScanned();
        try {
          const projectTeams = await project.teams();
          const projectTeamKeys = projectTeams.nodes
            .map(t => teamIdToKey.get(t.id))
            .filter((k): k is string => k !== undefined);

          const updates = await client.fetchProjectUpdates(project.id);
          const content = await generateProjectContent(project, updates);
          const contentId = await ctx.contentStorage.save(content, 'text/markdown');
          const doc = await mapProjectToDocument(project, updates, contentId, projectTeamKeys);
          if (isIncremental) {
            await ctx.emitUpdated(doc);
          } else {
            await ctx.emit(doc);
          }
          docsSinceCheckpoint++;

          // Sync project updates as separate documents
          for (const update of updates) {
            if (lastSyncAt && update.createdAt < new Date(lastSyncAt)) continue;
            await ctx.incrementScanned();
            try {
              const updateContent = await generateProjectUpdateContent(update, project.name);
              const updateContentId = await ctx.contentStorage.save(updateContent, 'text/markdown');
              const updateDoc = await mapProjectUpdateToDocument(update, project.name, updateContentId, projectTeamKeys);
              if (isIncremental) {
                await ctx.emitUpdated(updateDoc);
              } else {
                await ctx.emit(updateDoc);
              }
              docsSinceCheckpoint++;
            } catch (e) {
              const eid = `linear:project_update:${update.id}`;
              logger.warn(`Error processing ${eid}: ${e}`);
              ctx.emitError(eid, String(e));
            }
          }

          if (docsSinceCheckpoint >= CHECKPOINT_INTERVAL) {
            await ctx.saveState({ last_sync_at: new Date().toISOString() });
            docsSinceCheckpoint = 0;
          }
        } catch (e) {
          const eid = `linear:project:${project.id}`;
          logger.warn(`Error processing ${eid}: ${e}`);
          ctx.emitError(eid, String(e));
        }
      }

      // Sync documents
      logger.info('Syncing documents...');
      for await (const doc of client.fetchDocuments(lastSyncAt)) {
        if (ctx.isCancelled()) {
          await ctx.fail('Cancelled by user');
          return;
        }
        await ctx.incrementScanned();
        try {
          // Resolve team keys from the document's project (if any)
          const docProject = await doc.project;
          let docTeamKeys: string[] = [];
          if (docProject) {
            const docProjectTeams = await docProject.teams();
            docTeamKeys = docProjectTeams.nodes
              .map(t => teamIdToKey.get(t.id))
              .filter((k): k is string => k !== undefined);
          }

          const content = await generateDocumentContent(doc);
          const contentId = await ctx.contentStorage.save(content, 'text/markdown');
          const omniDoc = await mapLinearDocumentToDocument(doc, contentId, docTeamKeys);
          if (isIncremental) {
            await ctx.emitUpdated(omniDoc);
          } else {
            await ctx.emit(omniDoc);
          }
          docsSinceCheckpoint++;
          if (docsSinceCheckpoint >= CHECKPOINT_INTERVAL) {
            await ctx.saveState({ last_sync_at: new Date().toISOString() });
            docsSinceCheckpoint = 0;
          }
        } catch (e) {
          const eid = `linear:document:${doc.id}`;
          logger.warn(`Error processing ${eid}: ${e}`);
          ctx.emitError(eid, String(e));
        }
      }

      await ctx.complete({ last_sync_at: new Date().toISOString() });
      logger.info(`Sync completed: ${ctx.documentsScanned} scanned, ${ctx.documentsEmitted} emitted`);
    } catch (e) {
      logger.error({ err: e }, 'Sync failed with unexpected error');
      await ctx.fail(String(e));
    }
  }

  async executeAction(
    action: string,
    _params: Record<string, unknown>,
    credentials: LinearCredentials,
  ): Promise<ActionResponse> {
    if (action !== 'list_teams') {
      return createActionResponseNotSupported(action);
    }

    const { api_key: apiKey } = credentials;
    if (!apiKey) {
      return createActionResponseFailure("Missing 'api_key' in credentials");
    }

    try {
      const client = new LinearApiClient(apiKey);
      const teams = await client.fetchTeams();
      return createActionResponseSuccess({
        teams: teams.map(t => ({ key: t.key, name: t.name })),
      });
    } catch (e) {
      return createActionResponseFailure(String(e));
    }
  }
}
