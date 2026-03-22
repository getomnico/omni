import { LinearClient, type Issue, type Project, type Document as LinearDocument, type ProjectUpdate, type Team, type Comment } from '@linear/sdk';

const PAGE_SIZE = 50;
const MAX_COMMENTS_PER_ISSUE = 50;

export class LinearApiClient {
  private client: LinearClient;

  constructor(apiKey: string) {
    this.client = new LinearClient({ apiKey });
  }

  async validateApiKey(): Promise<string> {
    const viewer = await this.client.viewer;
    return viewer.displayName ?? viewer.email;
  }

  async fetchTeams(): Promise<Team[]> {
    const teams: Team[] = [];
    let page = await this.client.teams({ first: PAGE_SIZE });
    while (page.nodes.length > 0) {
      teams.push(...page.nodes);
      if (!page.pageInfo.hasNextPage) break;
      page = await page.fetchNext();
    }
    return teams;
  }

  async fetchTeamMembers(teamId: string): Promise<{ email: string; displayName: string }[]> {
    const team = await this.client.team(teamId);
    const members: { email: string; displayName: string }[] = [];
    let page = await team.members({ first: PAGE_SIZE });
    while (page.nodes.length > 0) {
      for (const user of page.nodes) {
        if (user.email) {
          members.push({ email: user.email.toLowerCase(), displayName: user.displayName ?? user.email });
        }
      }
      if (!page.pageInfo.hasNextPage) break;
      page = await page.fetchNext();
    }
    return members;
  }

  async *fetchIssues(teamId: string, updatedAfter?: string): AsyncGenerator<Issue> {
    const filter: Record<string, unknown> = { team: { id: { eq: teamId } } };
    if (updatedAfter) {
      filter.updatedAt = { gt: updatedAfter };
    }

    let page = await this.client.issues({
      first: PAGE_SIZE,
      filter,
      orderBy: 'updatedAt' as never,
    });
    while (page.nodes.length > 0) {
      for (const issue of page.nodes) {
        yield issue;
      }
      if (!page.pageInfo.hasNextPage) break;
      page = await page.fetchNext();
    }
  }

  async fetchIssueComments(issueId: string): Promise<Comment[]> {
    const issue = await this.client.issue(issueId);
    const comments: Comment[] = [];
    let page = await issue.comments({ first: PAGE_SIZE });
    while (page.nodes.length > 0) {
      comments.push(...page.nodes);
      if (comments.length >= MAX_COMMENTS_PER_ISSUE) break;
      if (!page.pageInfo.hasNextPage) break;
      page = await page.fetchNext();
    }
    return comments.slice(0, MAX_COMMENTS_PER_ISSUE);
  }

  async *fetchProjects(updatedAfter?: string): AsyncGenerator<Project> {
    const filter: Record<string, unknown> = {};
    if (updatedAfter) {
      filter.updatedAt = { gt: updatedAfter };
    }

    let page = await this.client.projects({
      first: PAGE_SIZE,
      filter,
      orderBy: 'updatedAt' as never,
    });
    while (page.nodes.length > 0) {
      for (const project of page.nodes) {
        yield project;
      }
      if (!page.pageInfo.hasNextPage) break;
      page = await page.fetchNext();
    }
  }

  async fetchProjectUpdates(projectId: string): Promise<ProjectUpdate[]> {
    const project = await this.client.project(projectId);
    const updates: ProjectUpdate[] = [];
    let page = await project.projectUpdates({ first: 10 });
    while (page.nodes.length > 0) {
      updates.push(...page.nodes);
      if (!page.pageInfo.hasNextPage) break;
      page = await page.fetchNext();
    }
    return updates;
  }

  async *fetchDocuments(updatedAfter?: string): AsyncGenerator<LinearDocument> {
    const filter: Record<string, unknown> = {};
    if (updatedAfter) {
      filter.updatedAt = { gt: updatedAfter };
    }

    let page = await this.client.documents({
      first: PAGE_SIZE,
      filter,
      orderBy: 'updatedAt' as never,
    });
    while (page.nodes.length > 0) {
      for (const doc of page.nodes) {
        yield doc;
      }
      if (!page.pageInfo.hasNextPage) break;
      page = await page.fetchNext();
    }
  }

  async *fetchProjectUpdatesAll(updatedAfter?: string): AsyncGenerator<{ update: ProjectUpdate; projectName: string }> {
    for await (const project of this.fetchProjects(updatedAfter)) {
      const updates = await this.fetchProjectUpdates(project.id);
      for (const update of updates) {
        if (updatedAfter && update.createdAt < new Date(updatedAfter)) continue;
        yield { update, projectName: project.name };
      }
    }
  }
}
