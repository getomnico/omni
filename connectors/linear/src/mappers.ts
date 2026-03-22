import type { Issue, Project, Document as LinearDocument, ProjectUpdate, Comment, Team } from '@linear/sdk';
import type { Document, DocumentMetadata, DocumentPermissions } from '@getomnico/connector';

const GROUP_PREFIX = 'linear-team:';

function teamPermissions(teamKeys: string[]): DocumentPermissions {
  if (teamKeys.length === 0) {
    return { public: true, users: [], groups: [] };
  }
  return { public: false, users: [], groups: teamKeys.map(k => `${GROUP_PREFIX}${k}`) };
}

const MAX_CONTENT_LENGTH = 100_000;

function truncate(content: string): string {
  if (content.length > MAX_CONTENT_LENGTH) {
    return content.slice(0, MAX_CONTENT_LENGTH) + '\n... (truncated)';
  }
  return content;
}

function toISOString(date: Date | undefined | null): string | undefined {
  if (!date) return undefined;
  return date.toISOString();
}

// --- Issues ---

export async function mapIssueToDocument(
  issue: Issue,
  comments: Comment[],
  contentId: string,
  teamKey: string,
): Promise<Document> {
  const state = await issue.state;
  const assignee = await issue.assignee;
  const team = await issue.team;
  const project = await issue.project;
  const creator = await issue.creator;
  const issueLabels = await issue.labels();

  const labels = issueLabels.nodes.map(l => l.name);

  const attributes: Record<string, unknown> = {
    source_type: 'linear',
    status: state?.name ?? '',
    priority: issue.priorityLabel ?? '',
    labels: labels.join(','),
    assignee: assignee?.displayName ?? '',
    assignee_email: assignee?.email ?? '',
    team: team?.name ?? '',
    identifier: issue.identifier,
  };
  if (project) {
    attributes.project_name = project.name;
  }

  const pathParts = [team?.name, project?.name, issue.identifier].filter(Boolean);

  const metadata: DocumentMetadata = {
    author: creator?.displayName,
    created_at: toISOString(issue.createdAt),
    updated_at: toISOString(issue.updatedAt),
    content_type: 'issue',
    url: issue.url,
    mime_type: 'text/markdown',
    path: pathParts.join(' / '),
    extra: {
      linear: {
        team_id: team?.id,
        project_id: project?.id,
      },
    },
  };

  return {
    external_id: `linear:issue:${issue.id}`,
    title: `${issue.identifier} - ${issue.title}`,
    content_id: contentId,
    metadata,
    permissions: teamPermissions([teamKey]),
    attributes,
  };
}

export async function generateIssueContent(
  issue: Issue,
  comments: Comment[],
): Promise<string> {
  const state = await issue.state;
  const assignee = await issue.assignee;
  const team = await issue.team;
  const project = await issue.project;
  const issueLabels = await issue.labels();
  const labels = issueLabels.nodes.map(l => l.name);

  const lines: string[] = [];
  lines.push(`${issue.identifier}: ${issue.title}`);
  lines.push(`Status: ${state?.name ?? 'Unknown'} | Priority: ${issue.priorityLabel ?? 'None'} | Team: ${team?.name ?? 'Unknown'}`);
  if (assignee) {
    lines.push(`Assignee: ${assignee.displayName}`);
  }
  if (labels.length > 0) {
    lines.push(`Labels: ${labels.join(', ')}`);
  }
  if (project) {
    lines.push(`Project: ${project.name}`);
  }
  lines.push('');
  if (issue.description) {
    lines.push(issue.description);
  }

  if (comments.length > 0) {
    lines.push('');
    lines.push('--- Comments ---');
    for (const comment of comments) {
      const author = await comment.user;
      const dateStr = comment.createdAt.toISOString().split('T')[0];
      lines.push(`${author?.displayName ?? 'Unknown'} (${dateStr}):`);
      if (comment.body) {
        lines.push(comment.body);
      }
      lines.push('');
    }
  }

  return truncate(lines.join('\n'));
}

// --- Projects ---

export async function mapProjectToDocument(
  project: Project,
  recentUpdates: ProjectUpdate[],
  contentId: string,
  teamKeys: string[],
): Promise<Document> {
  const lead = await project.lead;
  const creator = await project.creator;

  let latestHealth = '';
  if (recentUpdates.length > 0) {
    latestHealth = recentUpdates[0]!.health ?? '';
  }

  const attributes: Record<string, unknown> = {
    source_type: 'linear',
    status: project.state ?? '',
    health: latestHealth,
    lead: lead?.displayName ?? '',
  };

  const metadata: DocumentMetadata = {
    author: lead?.displayName ?? creator?.displayName,
    created_at: toISOString(project.createdAt),
    updated_at: toISOString(project.updatedAt),
    content_type: 'project',
    url: project.url,
    mime_type: 'text/markdown',
    path: `Projects / ${project.name}`,
  };

  return {
    external_id: `linear:project:${project.id}`,
    title: project.name,
    content_id: contentId,
    metadata,
    permissions: teamPermissions(teamKeys),
    attributes,
  };
}

export async function generateProjectContent(
  project: Project,
  recentUpdates: ProjectUpdate[],
): Promise<string> {
  const lead = await project.lead;
  const teams = await project.teams();
  const teamNames = teams.nodes.map(t => t.name);

  const lines: string[] = [];
  lines.push(`Project: ${project.name}`);
  lines.push(`Status: ${project.state ?? 'Unknown'} | Lead: ${lead?.displayName ?? 'Unassigned'}`);
  if (teamNames.length > 0) {
    lines.push(`Teams: ${teamNames.join(', ')}`);
  }
  if (project.targetDate) {
    lines.push(`Target Date: ${project.targetDate}`);
  }
  lines.push('');
  if (project.description) {
    lines.push(project.description);
  }

  if (recentUpdates.length > 0) {
    lines.push('');
    lines.push('--- Recent Updates ---');
    for (const update of recentUpdates) {
      const user = await update.user;
      const dateStr = update.createdAt.toISOString().split('T')[0];
      lines.push(`${user?.displayName ?? 'Unknown'} (${dateStr}) - Health: ${update.health ?? 'Unknown'}`);
      if (update.body) {
        lines.push(update.body);
      }
      lines.push('');
    }
  }

  return truncate(lines.join('\n'));
}

// --- Documents ---

export async function mapLinearDocumentToDocument(
  doc: LinearDocument,
  contentId: string,
  teamKeys: string[],
): Promise<Document> {
  const creator = await doc.creator;
  const project = await doc.project;

  const attributes: Record<string, unknown> = {
    source_type: 'linear',
  };
  if (project) {
    attributes.project_name = project.name;
  }

  const pathParts = ['Documents', project?.name, doc.title].filter(Boolean);

  const metadata: DocumentMetadata = {
    author: creator?.displayName,
    created_at: toISOString(doc.createdAt),
    updated_at: toISOString(doc.updatedAt),
    content_type: 'document',
    url: doc.url,
    mime_type: 'text/markdown',
    path: pathParts.join(' / '),
  };

  return {
    external_id: `linear:document:${doc.id}`,
    title: doc.title,
    content_id: contentId,
    metadata,
    permissions: teamPermissions(teamKeys),
    attributes,
  };
}

export async function generateDocumentContent(doc: LinearDocument): Promise<string> {
  const project = await doc.project;
  const lines: string[] = [];
  lines.push(doc.title);
  if (project) {
    lines.push(`Project: ${project.name}`);
  }
  lines.push('');
  if (doc.content) {
    lines.push(doc.content);
  }
  return truncate(lines.join('\n'));
}

// --- Project Updates ---

export async function mapProjectUpdateToDocument(
  update: ProjectUpdate,
  projectName: string,
  contentId: string,
  teamKeys: string[],
): Promise<Document> {
  const user = await update.user;
  const project = await update.project;
  const dateStr = update.createdAt.toISOString().split('T')[0];

  const attributes: Record<string, unknown> = {
    source_type: 'linear',
    health: update.health ?? '',
    project_name: projectName,
  };

  const metadata: DocumentMetadata = {
    author: user?.displayName,
    created_at: toISOString(update.createdAt),
    updated_at: toISOString(update.updatedAt),
    content_type: 'project_update',
    url: update.url,
    mime_type: 'text/markdown',
    path: `Projects / ${projectName} / Updates`,
  };

  return {
    external_id: `linear:project_update:${update.id}`,
    title: `Project Update: ${projectName} - ${dateStr}`,
    content_id: contentId,
    metadata,
    permissions: teamPermissions(teamKeys),
    attributes,
  };
}

export async function generateProjectUpdateContent(
  update: ProjectUpdate,
  projectName: string,
): Promise<string> {
  const user = await update.user;
  const lines: string[] = [];
  lines.push(`Project Update: ${projectName}`);
  lines.push(`Health: ${update.health ?? 'Unknown'} | By: ${user?.displayName ?? 'Unknown'}`);
  lines.push('');
  if (update.body) {
    lines.push(update.body);
  }
  return truncate(lines.join('\n'));
}
