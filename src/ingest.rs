use crate::domain::CommunityPost;
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use feed_rs::model::Entry;
use reqwest::Client;
use serde::Deserialize;
use std::time::Duration;

const MAX_SUMMARY_CHARS: usize = 500;

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
    story_text: Option<String>,
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
            summary: normalize_summary(self.story_text.as_deref()),
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
    description: Option<String>,
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
            summary: normalize_summary(self.description.as_deref()),
            tags: self.tags,
        })
    }
}

fn normalize_feed_entry(entry: Entry) -> Option<Result<CommunityPost>> {
    let title = entry.title?.content;
    let summary = normalize_summary(
        entry
            .summary
            .as_ref()
            .map(|summary| summary.content.as_str())
            .or_else(|| {
                entry
                    .content
                    .as_ref()
                    .and_then(|content| content.body.as_deref())
            }),
    );
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
        summary,
        tags: entry
            .categories
            .into_iter()
            .map(|category| category.term)
            .collect(),
    }))
}

fn normalize_summary(summary: Option<&str>) -> String {
    let Some(summary) = summary else {
        return String::new();
    };
    let mut plain = String::with_capacity(summary.len());
    let mut in_tag = false;
    for character in summary.chars() {
        match character {
            '<' => {
                in_tag = true;
                plain.push(' ');
            }
            '>' if in_tag => {
                in_tag = false;
                plain.push(' ');
            }
            _ if !in_tag => plain.push(character),
            _ => {}
        }
    }
    let decoded = decode_html_entities(&plain);
    decoded
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MAX_SUMMARY_CHARS)
        .collect()
}

fn decode_html_entities(value: &str) -> String {
    let mut decoded = String::with_capacity(value.len());
    let mut characters = value.chars().peekable();
    while let Some(character) = characters.next() {
        if character != '&' {
            decoded.push(character);
            continue;
        }
        let mut candidate = characters.clone();
        let mut entity = String::new();
        let mut terminated = false;
        for _ in 0..10 {
            let Some(next) = candidate.next() else {
                break;
            };
            if next == ';' {
                terminated = true;
                break;
            }
            if next.is_whitespace() || next == '&' {
                break;
            }
            entity.push(next);
        }
        let replacement = terminated.then(|| decode_html_entity(&entity)).flatten();
        if let Some(replacement) = replacement {
            decoded.push(replacement);
            characters = candidate;
        } else {
            decoded.push('&');
        }
    }
    decoded
}

fn decode_html_entity(entity: &str) -> Option<char> {
    match entity {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" | "#39" => Some('\''),
        "nbsp" => Some(' '),
        value if value.starts_with("#x") || value.starts_with("#X") => {
            u32::from_str_radix(&value[2..], 16)
                .ok()
                .and_then(char::from_u32)
        }
        value if value.starts_with('#') => value[1..].parse().ok().and_then(char::from_u32),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn summary_normalization_strips_html_decodes_entities_and_collapses_space() {
        assert_eq!(
            normalize_summary(Some(
                " <p>Hello&nbsp; <b>Rust</b> &amp; friends.</p>\nNext "
            )),
            "Hello Rust & friends. Next"
        );
    }

    #[test]
    fn summary_normalization_is_unicode_safe_and_bounded() {
        let input = "🦀".repeat(MAX_SUMMARY_CHARS + 20);
        let output = normalize_summary(Some(&input));
        assert_eq!(output.chars().count(), MAX_SUMMARY_CHARS);
        assert!(output.chars().all(|character| character == '🦀'));
    }

    #[test]
    fn source_normalizers_capture_only_provided_author_text() {
        let hn: HackerNewsHit = serde_json::from_value(json!({
            "objectID": "1",
            "title": "Ask HN",
            "url": null,
            "author": "maya",
            "created_at": "2026-07-22T12:00:00Z",
            "points": 4,
            "story_text": "<p>HN author text</p>",
            "_tags": ["story"]
        }))
        .unwrap();
        assert_eq!(hn.normalize().unwrap().unwrap().summary, "HN author text");

        let lobsters: LobstersStory = serde_json::from_value(json!({
            "short_id": "abc",
            "title": "Text post",
            "url": "https://example.com/lobsters",
            "submitter_user": "lin",
            "created_at": "2026-07-22T12:00:00Z",
            "score": 3,
            "description": "<em>Lobsters author text</em>",
            "tags": ["rust"]
        }))
        .unwrap();
        assert_eq!(
            lobsters.normalize().unwrap().summary,
            "Lobsters author text"
        );

        let atom = br#"<?xml version="1.0" encoding="utf-8"?>
            <feed xmlns="http://www.w3.org/2005/Atom">
              <title>Product Hunt</title><id>feed</id><updated>2026-07-22T12:00:00Z</updated>
              <entry><title>Launch</title><id>ph-1</id><updated>2026-07-22T12:00:00Z</updated>
                <link rel="alternate" href="https://example.com/product" />
                <summary type="html">Product author text</summary>
              </entry>
            </feed>"#;
        let entry = feed_rs::parser::parse(atom.as_slice())
            .unwrap()
            .entries
            .into_iter()
            .next()
            .unwrap();
        assert_eq!(
            normalize_feed_entry(entry).unwrap().unwrap().summary,
            "Product author text"
        );
    }
}
