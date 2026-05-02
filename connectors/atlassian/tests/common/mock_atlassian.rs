use anyhow::Result;
use async_trait::async_trait;
use futures::stream::Stream;
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Mutex;

use omni_atlassian_connector::models::{
    ConfluenceCqlPage, ConfluencePage, ConfluenceSpace, ConfluenceSpacePermission, JiraField,
    JiraIssue, JiraProjectRolesResponse, JiraRoleActorsResponse, JiraSearchResponse,
};
use omni_atlassian_connector::AtlassianApi;
use omni_atlassian_connector::AtlassianCredentials;

#[derive(Debug, Clone)]
pub struct MethodCall {
    pub method: String,
    pub args: Vec<String>,
}

pub struct MockAtlassianApi {
    pub spaces: Mutex<Vec<ConfluenceSpace>>,
    pub pages: Mutex<Vec<Vec<ConfluencePage>>>,
    pub cql_pages: Mutex<Vec<ConfluenceCqlPage>>,
    pub jira_projects: Mutex<Vec<serde_json::Value>>,
    pub jira_search_response: Mutex<Option<JiraSearchResponse>>,
    pub jira_fields: Mutex<Vec<JiraField>>,
    pub single_page: Mutex<Option<ConfluencePage>>,
    pub single_issue: Mutex<Option<JiraIssue>>,
    pub webhook_register_result: Mutex<Option<u64>>,
    pub webhook_exists: Mutex<bool>,
    pub calls: Mutex<Vec<MethodCall>>,
    pub space_permissions: Mutex<HashMap<String, Vec<ConfluenceSpacePermission>>>,
    pub project_roles: Mutex<HashMap<String, String>>,
    pub role_actors: Mutex<HashMap<String, JiraRoleActorsResponse>>,
    pub bulk_users: Mutex<Vec<(String, String)>>,
    pub group_members: Mutex<HashMap<String, Vec<String>>>,
}

impl MockAtlassianApi {
    pub fn new() -> Self {
        Self {
            spaces: Mutex::new(vec![]),
            pages: Mutex::new(vec![]),
            cql_pages: Mutex::new(vec![]),
            jira_projects: Mutex::new(vec![]),
            jira_search_response: Mutex::new(None),
            jira_fields: Mutex::new(vec![]),
            single_page: Mutex::new(None),
            single_issue: Mutex::new(None),
            webhook_register_result: Mutex::new(None),
            webhook_exists: Mutex::new(false),
            calls: Mutex::new(vec![]),
            space_permissions: Mutex::new(HashMap::new()),
            project_roles: Mutex::new(HashMap::new()),
            role_actors: Mutex::new(HashMap::new()),
            bulk_users: Mutex::new(vec![]),
            group_members: Mutex::new(HashMap::new()),
        }
    }

    pub fn record_call(&self, method: &str, args: Vec<String>) {
        self.calls.lock().unwrap().push(MethodCall {
            method: method.to_string(),
            args,
        });
    }

    pub fn get_calls_for(&self, method: &str) -> Vec<MethodCall> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.method == method)
            .cloned()
            .collect()
    }
}

#[async_trait]
impl AtlassianApi for MockAtlassianApi {
    fn get_confluence_pages<'a>(
        &'a self,
        _creds: &'a AtlassianCredentials,
        space_id: &'a str,
    ) -> Pin<Box<dyn Stream<Item = Result<ConfluencePage>> + Send + 'a>> {
        self.record_call("get_confluence_pages", vec![space_id.to_string()]);

        let pages_lists = self.pages.lock().unwrap();
        // Find pages for this space by matching space_id
        let pages: Vec<ConfluencePage> = pages_lists
            .iter()
            .flat_map(|list| list.iter().filter(|p| p.space_id == space_id).cloned())
            .collect();

        Box::pin(futures::stream::iter(pages.into_iter().map(Ok)))
    }

    fn search_confluence_pages_by_cql<'a>(
        &'a self,
        _creds: &'a AtlassianCredentials,
        cql: &'a str,
    ) -> Pin<Box<dyn Stream<Item = Result<ConfluenceCqlPage>> + Send + 'a>> {
        self.record_call("search_confluence_pages_by_cql", vec![cql.to_string()]);

        let pages = self.cql_pages.lock().unwrap().clone();
        Box::pin(futures::stream::iter(pages.into_iter().map(Ok)))
    }

    async fn get_confluence_spaces(
        &self,
        _creds: &AtlassianCredentials,
    ) -> Result<Vec<ConfluenceSpace>> {
        self.record_call("get_confluence_spaces", vec![]);
        Ok(self.spaces.lock().unwrap().clone())
    }

    async fn get_confluence_page_by_id(
        &self,
        _creds: &AtlassianCredentials,
        page_id: &str,
        _expand: &[&str],
    ) -> Result<ConfluencePage> {
        self.record_call("get_confluence_page_by_id", vec![page_id.to_string()]);
        self.single_page
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Page not found"))
    }

    async fn get_jira_issues(
        &self,
        _creds: &AtlassianCredentials,
        jql: &str,
        _max_results: u32,
        _next_page_token: Option<&str>,
        _fields: &[String],
    ) -> Result<JiraSearchResponse> {
        self.record_call("get_jira_issues", vec![jql.to_string()]);
        Ok(self
            .jira_search_response
            .lock()
            .unwrap()
            .clone()
            .unwrap_or(JiraSearchResponse {
                issues: vec![],
                is_last: true,
                next_page_token: None,
            }))
    }

    async fn get_jira_issue_by_key(
        &self,
        _creds: &AtlassianCredentials,
        issue_key: &str,
        _fields: &[String],
    ) -> Result<JiraIssue> {
        self.record_call("get_jira_issue_by_key", vec![issue_key.to_string()]);
        self.single_issue
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Issue not found"))
    }

    async fn get_jira_fields(&self, _creds: &AtlassianCredentials) -> Result<Vec<JiraField>> {
        self.record_call("get_jira_fields", vec![]);
        Ok(self.jira_fields.lock().unwrap().clone())
    }

    async fn get_jira_projects(
        &self,
        _creds: &AtlassianCredentials,
        _expand: &[&str],
    ) -> Result<Vec<serde_json::Value>> {
        self.record_call("get_jira_projects", vec![]);
        Ok(self.jira_projects.lock().unwrap().clone())
    }

    async fn register_webhook(
        &self,
        _creds: &AtlassianCredentials,
        webhook_url: &str,
    ) -> Result<u64> {
        self.record_call("register_webhook", vec![webhook_url.to_string()]);
        self.webhook_register_result
            .lock()
            .unwrap()
            .ok_or_else(|| anyhow::anyhow!("register_webhook not configured"))
    }

    async fn delete_webhook(&self, _creds: &AtlassianCredentials, webhook_id: u64) -> Result<()> {
        self.record_call("delete_webhook", vec![webhook_id.to_string()]);
        Ok(())
    }

    async fn get_webhook(&self, _creds: &AtlassianCredentials, webhook_id: u64) -> Result<bool> {
        self.record_call("get_webhook", vec![webhook_id.to_string()]);
        Ok(*self.webhook_exists.lock().unwrap())
    }

    async fn get_confluence_space_permissions(
        &self,
        _creds: &AtlassianCredentials,
        space_id: &str,
    ) -> Result<Vec<ConfluenceSpacePermission>> {
        self.record_call(
            "get_confluence_space_permissions",
            vec![space_id.to_string()],
        );
        let perms = self.space_permissions.lock().unwrap();
        Ok(perms.get(space_id).cloned().unwrap_or_default())
    }

    async fn get_confluence_group_members(
        &self,
        _creds: &AtlassianCredentials,
        group_id: &str,
    ) -> Result<Vec<String>> {
        self.record_call("get_confluence_group_members", vec![group_id.to_string()]);
        let members = self.group_members.lock().unwrap();
        Ok(members.get(group_id).cloned().unwrap_or_default())
    }

    async fn get_jira_project_roles(
        &self,
        _creds: &AtlassianCredentials,
        project_key: &str,
    ) -> Result<JiraProjectRolesResponse> {
        self.record_call("get_jira_project_roles", vec![project_key.to_string()]);
        Ok(JiraProjectRolesResponse {
            roles: self.project_roles.lock().unwrap().clone(),
        })
    }

    async fn get_jira_project_role_actors(
        &self,
        _creds: &AtlassianCredentials,
        project_key: &str,
        role_id: &str,
    ) -> Result<JiraRoleActorsResponse> {
        self.record_call(
            "get_jira_project_role_actors",
            vec![project_key.to_string(), role_id.to_string()],
        );
        let actors = self.role_actors.lock().unwrap();
        actors
            .get(role_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Role not found"))
    }

    async fn get_jira_users_bulk(
        &self,
        _creds: &AtlassianCredentials,
        account_ids: &[String],
    ) -> Result<Vec<(String, String)>> {
        self.record_call("get_jira_users_bulk", account_ids.iter().cloned().collect());
        let all_users = self.bulk_users.lock().unwrap();
        let result: Vec<(String, String)> = all_users
            .iter()
            .filter(|(id, _)| account_ids.contains(id))
            .cloned()
            .collect();
        Ok(result)
    }
}
