use anyhow::{Context, Result};
use reqwest::RequestBuilder;
use serde::Deserialize;
use serde_json::{json, Value as JsonValue};

use crate::credentials::DarwinboxCredentials;

#[derive(Debug, Clone, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub expires_in: Option<u64>,
    #[serde(default)]
    pub token_type: Option<String>,
}

pub fn add_api_key_and_dataset(
    mut body: JsonValue,
    credentials: &DarwinboxCredentials,
    include_dataset_key: bool,
) -> JsonValue {
    let obj = body.as_object_mut();
    if let Some(obj) = obj {
        if let Some(api_key) = credentials.api_key() {
            obj.entry("api_key".to_string())
                .or_insert_with(|| json!(api_key));
        }
        if include_dataset_key {
            obj.entry("datasetKey".to_string())
                .or_insert_with(|| json!(credentials.dataset_key()));
        }
    }
    body
}

pub fn apply_basic_auth(
    request: RequestBuilder,
    credentials: &DarwinboxCredentials,
) -> RequestBuilder {
    match credentials {
        DarwinboxCredentials::Basic {
            username, password, ..
        } => request.basic_auth(username, Some(password)),
        _ => request,
    }
}

pub async fn fetch_token(
    client: &reqwest::Client,
    base_url: &str,
    credentials: &DarwinboxCredentials,
) -> Result<Option<TokenResponse>> {
    let payload = match credentials {
        DarwinboxCredentials::Basic { .. } => return Ok(None),
        DarwinboxCredentials::DynamicToken {
            client_id,
            client_secret,
            grant_type,
            code,
            refresh_token,
            ..
        } => {
            let mut body = json!({
                "client_id": client_id,
                "client_secret": client_secret,
                "grant_type": grant_type,
            });
            if grant_type == "refresh_token" {
                body["refresh_token"] = json!(refresh_token
                    .as_deref()
                    .context("refresh_token grant requires refresh_token")?);
            } else {
                body["code"] = json!(code
                    .as_deref()
                    .context("authorization_code grant requires code")?);
            }
            ("/oauth/v1token", body)
        }
        DarwinboxCredentials::ClientCredentials {
            client_id,
            client_secret,
            ..
        } => (
            "/oauth/v2token",
            json!({
                "client_id": client_id,
                "client_secret": client_secret,
                "grant_type": "client_credentials",
            }),
        ),
    };

    let url = format!("{}{}", base_url.trim_end_matches('/'), payload.0);
    let response = client
        .post(url)
        .header("Content-Type", "application/json")
        .json(&payload.1)
        .send()
        .await
        .context("failed to request Darwinbox token")?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Darwinbox token request failed with HTTP {status}: {body}");
    }

    let token = response
        .json::<TokenResponse>()
        .await
        .context("failed to parse Darwinbox token response")?;
    Ok(Some(token))
}
