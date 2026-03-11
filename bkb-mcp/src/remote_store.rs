use anyhow::{Context, Result};
use async_trait::async_trait;
use reqwest::Client;

use chrono::{DateTime, Utc};

use bkb_core::model::{
	CommitContext, DocumentContext, Reference, SearchParams, SearchResults, Timeline,
};
use bkb_core::store::KnowledgeStore;

/// KnowledgeStore implementation that proxies to the BKB HTTP API.
pub struct RemoteApiStore {
	client: Client,
	base_url: String,
}

impl RemoteApiStore {
	pub fn new(base_url: &str) -> Self {
		Self { client: Client::new(), base_url: base_url.trim_end_matches('/').to_string() }
	}
}

#[async_trait]
impl KnowledgeStore for RemoteApiStore {
	async fn search(&self, params: SearchParams) -> Result<SearchResults> {
		let mut url = format!("{}/search?q={}", self.base_url, urlencoded(&params.query));

		if let Some(ref source_types) = params.source_type {
			let types_str: String =
				source_types.iter().map(|t| t.as_str()).collect::<Vec<_>>().join(",");
			url.push_str(&format!("&source_type={}", types_str));
		}

		if let Some(ref repos) = params.source_repo {
			url.push_str(&format!("&source_repo={}", repos.join(",")));
		}

		if let Some(ref author) = params.author {
			url.push_str(&format!("&author={}", urlencoded(author)));
		}

		if let Some(ref after) = params.after {
			url.push_str(&format!("&after={}", after.to_rfc3339()));
		}

		if let Some(ref before) = params.before {
			url.push_str(&format!("&before={}", before.to_rfc3339()));
		}

		if params.semantic {
			url.push_str("&semantic=true");
		}

		if let Some(limit) = params.limit {
			url.push_str(&format!("&limit={}", limit));
		}

		let response = self.client.get(&url).send().await.context("failed to query BKB API")?;

		if !response.status().is_success() {
			let body = response.text().await.unwrap_or_default();
			anyhow::bail!("BKB API returned error: {}", body);
		}

		response.json().await.context("failed to parse search response")
	}

	async fn get_document(&self, id: &str) -> Result<Option<DocumentContext>> {
		let url = format!("{}/document/{}", self.base_url, urlencoded(id));

		let response = self.client.get(&url).send().await.context("failed to query BKB API")?;

		if response.status() == reqwest::StatusCode::NOT_FOUND {
			return Ok(None);
		}

		if !response.status().is_success() {
			let body = response.text().await.unwrap_or_default();
			anyhow::bail!("BKB API returned error: {}", body);
		}

		let ctx: DocumentContext =
			response.json().await.context("failed to parse document response")?;
		Ok(Some(ctx))
	}

	async fn get_references(
		&self, entity: &str, ref_type: Option<&str>, limit: u32,
	) -> Result<Vec<Reference>> {
		let mut url =
			format!("{}/references/{}?limit={}", self.base_url, urlencoded(entity), limit);

		if let Some(rt) = ref_type {
			url.push_str(&format!("&ref_type={}", urlencoded(rt)));
		}

		let response = self.client.get(&url).send().await.context("failed to query BKB API")?;

		if !response.status().is_success() {
			let body = response.text().await.unwrap_or_default();
			anyhow::bail!("BKB API returned error: {}", body);
		}

		response.json().await.context("failed to parse references response")
	}

	async fn lookup_bip(&self, number: u32) -> Result<Option<DocumentContext>> {
		let url = format!("{}/bip/{}", self.base_url, number);

		let response = self.client.get(&url).send().await.context("failed to query BKB API")?;

		if response.status() == reqwest::StatusCode::NOT_FOUND {
			return Ok(None);
		}

		if !response.status().is_success() {
			let body = response.text().await.unwrap_or_default();
			anyhow::bail!("BKB API returned error: {}", body);
		}

		let ctx: DocumentContext = response.json().await.context("failed to parse BIP response")?;
		Ok(Some(ctx))
	}

	async fn lookup_bolt(&self, number: u32) -> Result<Option<DocumentContext>> {
		let url = format!("{}/bolt/{}", self.base_url, number);

		let response = self.client.get(&url).send().await.context("failed to query BKB API")?;

		if response.status() == reqwest::StatusCode::NOT_FOUND {
			return Ok(None);
		}

		if !response.status().is_success() {
			let body = response.text().await.unwrap_or_default();
			anyhow::bail!("BKB API returned error: {}", body);
		}

		let ctx: DocumentContext =
			response.json().await.context("failed to parse BOLT response")?;
		Ok(Some(ctx))
	}

	async fn lookup_blip(&self, number: u32) -> Result<Option<DocumentContext>> {
		let url = format!("{}/blip/{}", self.base_url, number);

		let response = self.client.get(&url).send().await.context("failed to query BKB API")?;

		if response.status() == reqwest::StatusCode::NOT_FOUND {
			return Ok(None);
		}

		if !response.status().is_success() {
			let body = response.text().await.unwrap_or_default();
			anyhow::bail!("BKB API returned error: {}", body);
		}

		let ctx: DocumentContext =
			response.json().await.context("failed to parse bLIP response")?;
		Ok(Some(ctx))
	}

	async fn lookup_lud(&self, number: u32) -> Result<Option<DocumentContext>> {
		let url = format!("{}/lud/{}", self.base_url, number);

		let response = self.client.get(&url).send().await.context("failed to query BKB API")?;

		if response.status() == reqwest::StatusCode::NOT_FOUND {
			return Ok(None);
		}

		if !response.status().is_success() {
			let body = response.text().await.unwrap_or_default();
			anyhow::bail!("BKB API returned error: {}", body);
		}

		let ctx: DocumentContext = response.json().await.context("failed to parse LUD response")?;
		Ok(Some(ctx))
	}

	async fn lookup_nut(&self, number: u32) -> Result<Option<DocumentContext>> {
		let url = format!("{}/nut/{}", self.base_url, number);

		let response = self.client.get(&url).send().await.context("failed to query BKB API")?;

		if response.status() == reqwest::StatusCode::NOT_FOUND {
			return Ok(None);
		}

		if !response.status().is_success() {
			let body = response.text().await.unwrap_or_default();
			anyhow::bail!("BKB API returned error: {}", body);
		}

		let ctx: DocumentContext = response.json().await.context("failed to parse NUT response")?;
		Ok(Some(ctx))
	}

	async fn timeline(
		&self, concept: &str, after: Option<DateTime<Utc>>, before: Option<DateTime<Utc>>,
	) -> Result<Timeline> {
		let mut url = format!("{}/timeline/{}", self.base_url, urlencoded(concept));

		if let Some(ref after) = after {
			url.push_str(&format!("?after={}", after.to_rfc3339()));
		}

		if let Some(ref before) = before {
			let sep = if url.contains('?') { '&' } else { '?' };
			url.push_str(&format!("{}before={}", sep, before.to_rfc3339()));
		}

		let response = self.client.get(&url).send().await.context("failed to query BKB API")?;

		if !response.status().is_success() {
			let body = response.text().await.unwrap_or_default();
			anyhow::bail!("BKB API returned error: {}", body);
		}

		response.json().await.context("failed to parse timeline response")
	}

	async fn find_commit(&self, query: &str, repo: Option<&str>) -> Result<Vec<CommitContext>> {
		let mut url = format!("{}/find_commit?q={}", self.base_url, urlencoded(query));

		if let Some(repo) = repo {
			url.push_str(&format!("&repo={}", urlencoded(repo)));
		}

		let response = self.client.get(&url).send().await.context("failed to query BKB API")?;

		if !response.status().is_success() {
			let body = response.text().await.unwrap_or_default();
			anyhow::bail!("BKB API returned error: {}", body);
		}

		response.json().await.context("failed to parse find_commit response")
	}
}

/// Simple URL encoding for query parameter values.
fn urlencoded(s: &str) -> String {
	url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}
