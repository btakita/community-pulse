use crate::domain::CommunityPost;
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use feed_rs::model::Entry;
use reqwest::Client;
use serde::Deserialize;
use std::time::Duration;

#[async_trait]
pub trait Ingester: Send + Sync {
    fn name(&self) -> &'static str;
    async fn fetch(&self, client: &Client) -> Result<Vec<CommunityPost>>;
}

pub struct HackerNewsIngester;
pub struct LobstersIngester;
pub struct ProductHuntIngester;

#[async_trait]
impl Ingester for HackerNewsIngester {
    fn name(&self) -> &'static str {
        "Hacker News"
    }

    async fn fetch(&self, client: &Client) -> Result<Vec<CommunityPost>> {
        let response = client
            .get("https://hn.algolia.com/api/v1/search_by_date")
            .query(&[("tags", "story"), ("hitsPerPage", "100")])
            .send()
            .await?
            .error_for_status()?
            .json::<HackerNewsResponse>()
            .await?;
        response
            .hits
            .into_iter()
            .filter_map(HackerNewsHit::normalize)
            .collect()
    }
}

#[async_trait]
impl Ingester for LobstersIngester {
    fn name(&self) -> &'static str {
        "Lobsters"
    }

    async fn fetch(&self, client: &Client) -> Result<Vec<CommunityPost>> {
        let stories = client
            .get("https://lobste.rs/newest.json")
            .send()
            .await?
            .error_for_status()?
            .json::<Vec<LobstersStory>>()
            .await?;
        stories.into_iter().map(LobstersStory::normalize).collect()
    }
}

#[async_trait]
impl Ingester for ProductHuntIngester {
    fn name(&self) -> &'static str {
        "Product Hunt"
    }

    async fn fetch(&self, client: &Client) -> Result<Vec<CommunityPost>> {
        let bytes = client
            .get("https://www.producthunt.com/feed")
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?;
        let feed = feed_rs::parser::parse(bytes.as_ref()).context("parse Product Hunt feed")?;
        feed.entries
            .into_iter()
            .filter_map(normalize_feed_entry)
            .collect()
    }
}

pub async fn fetch_all() -> Vec<(&'static str, Result<Vec<CommunityPost>>)> {
    let client = Client::builder()
        .user_agent("community-pulse/0.1 (+https://github.com/btakita/community-pulse)")
        .timeout(Duration::from_secs(20))
        .build()
        .expect("build HTTP client");
    let ingesters: Vec<Box<dyn Ingester>> = vec![
        Box::new(HackerNewsIngester),
        Box::new(LobstersIngester),
        Box::new(ProductHuntIngester),
    ];
    let futures = ingesters.into_iter().map(|ingester| {
        let client = client.clone();
        async move { (ingester.name(), ingester.fetch(&client).await) }
    });
    futures_util::future::join_all(futures).await
}

#[derive(Deserialize)]
struct HackerNewsResponse {
    hits: Vec<HackerNewsHit>,
}

#[derive(Deserialize)]
struct HackerNewsHit {
    #[serde(rename = "objectID")]
    object_id: String,
    title: Option<String>,
    url: Option<String>,
    author: String,
    created_at: DateTime<Utc>,
    points: Option<i64>,
    #[serde(default)]
    _tags: Vec<String>,
}

impl HackerNewsHit {
    fn normalize(self) -> Option<Result<CommunityPost>> {
        let title = self.title?;
        let url = self
            .url
            .unwrap_or_else(|| format!("https://news.ycombinator.com/item?id={}", self.object_id));
        Some(Ok(CommunityPost {
            id: format!("hn-{}", self.object_id),
            source: "Hacker News".to_owned(),
            title,
            url,
            author: self.author,
            published_at: self.created_at,
            points: self.points.unwrap_or_default(),
            tags: self._tags,
        }))
    }
}

#[derive(Deserialize)]
struct LobstersStory {
    short_id: String,
    title: String,
    url: String,
    submitter_user: String,
    created_at: DateTime<Utc>,
    score: i64,
    #[serde(default)]
    tags: Vec<String>,
}

impl LobstersStory {
    fn normalize(self) -> Result<CommunityPost> {
        Ok(CommunityPost {
            id: format!("lobsters-{}", self.short_id),
            source: "Lobsters".to_owned(),
            title: self.title,
            url: self.url,
            author: self.submitter_user,
            published_at: self.created_at,
            points: self.score,
            tags: self.tags,
        })
    }
}

fn normalize_feed_entry(entry: Entry) -> Option<Result<CommunityPost>> {
    let title = entry.title?.content;
    let url = entry
        .links
        .iter()
        .find(|link| link.rel.as_deref() == Some("alternate"))
        .or_else(|| entry.links.first())?
        .href
        .clone();
    let published_at = entry.published.or(entry.updated).unwrap_or_else(Utc::now);
    Some(Ok(CommunityPost {
        id: format!("product-hunt-{}", entry.id),
        source: "Product Hunt".to_owned(),
        title,
        url,
        author: entry
            .authors
            .first()
            .map(|author| author.name.clone())
            .unwrap_or_else(|| "Product Hunt".to_owned()),
        published_at,
        points: 0,
        tags: entry
            .categories
            .into_iter()
            .map(|category| category.term)
            .collect(),
    }))
}
