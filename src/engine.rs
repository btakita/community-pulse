use crate::domain::{
    ATTENTION_BUDGET, CommunityPost, DigestCard, EvidencePost, InterestModel, TopicScore,
    TrendEvidence,
};
use anyhow::{Context as _, Result, bail};
use chrono::{DateTime, Duration, Utc};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;

pub struct PulseEngine {
    connection: Connection,
}

type HeadlineCandidate = (String, String, String);
type HeadlineCandidates = Vec<HeadlineCandidate>;

impl PulseEngine {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let connection = Connection::open(path).context("open pulse database")?;
        let engine = Self { connection };
        engine.initialize()?;
        Ok(engine)
    }

    pub fn in_memory() -> Result<Self> {
        let connection = Connection::open_in_memory().context("open in-memory pulse database")?;
        let engine = Self { connection };
        engine.initialize()?;
        Ok(engine)
    }

    fn initialize(&self) -> Result<()> {
        self.connection.execute_batch(
            r#"
            PRAGMA foreign_keys = ON;
            PRAGMA journal_mode = WAL;

            CREATE TABLE IF NOT EXISTS posts (
                id           TEXT PRIMARY KEY,
                source       TEXT NOT NULL,
                title        TEXT NOT NULL,
                url          TEXT NOT NULL,
                author       TEXT NOT NULL,
                published_at INTEGER NOT NULL,
                points       INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS topics (
                id      TEXT PRIMARY KEY,
                display TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS post_topics (
                post_id  TEXT NOT NULL REFERENCES posts(id) ON DELETE CASCADE,
                topic_id TEXT NOT NULL REFERENCES topics(id) ON DELETE CASCADE,
                PRIMARY KEY (post_id, topic_id)
            );

            CREATE INDEX IF NOT EXISTS idx_posts_published_at
                ON posts(published_at);
            CREATE INDEX IF NOT EXISTS idx_post_topics_topic
                ON post_topics(topic_id);

            CREATE TABLE IF NOT EXISTS score_snapshots (
                topic_id         TEXT NOT NULL REFERENCES topics(id) ON DELETE CASCADE,
                captured_at      INTEGER NOT NULL,
                mentions_1h      INTEGER NOT NULL,
                mentions_6h      INTEGER NOT NULL,
                mentions_24h     INTEGER NOT NULL,
                baseline_mean    REAL NOT NULL,
                baseline_stddev  REAL NOT NULL,
                z_score          REAL NOT NULL,
                trend_score      REAL NOT NULL,
                PRIMARY KEY (topic_id, captured_at)
            );

            CREATE TABLE IF NOT EXISTS interests (
                topic_id TEXT PRIMARY KEY,
                weight   REAL NOT NULL
            );

            CREATE TABLE IF NOT EXISTS subscriptions (
                topic_id      TEXT PRIMARY KEY,
                subscribed_at INTEGER NOT NULL
            );
            "#,
        )?;
        Ok(())
    }

    pub fn post_count(&self) -> Result<usize> {
        let count = self
            .connection
            .query_row("SELECT COUNT(*) FROM posts", [], |row| row.get::<_, i64>(0))?;
        Ok(count as usize)
    }

    pub fn ingest(&mut self, posts: &[CommunityPost]) -> Result<usize> {
        let transaction = self.connection.transaction()?;
        let mut ingested = 0;
        for post in posts {
            ingested += Self::upsert_post(&transaction, post)?;
        }
        transaction.commit()?;
        Ok(ingested)
    }

    fn upsert_post(transaction: &Transaction<'_>, post: &CommunityPost) -> Result<usize> {
        let changed = transaction.execute(
            r#"
            INSERT INTO posts (id, source, title, url, author, published_at, points)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            ON CONFLICT(id) DO UPDATE SET
                source = excluded.source,
                title = excluded.title,
                url = excluded.url,
                author = excluded.author,
                published_at = excluded.published_at,
                points = excluded.points
            "#,
            params![
                post.id,
                post.source,
                post.title,
                post.url,
                post.author,
                post.published_at.timestamp(),
                post.points,
            ],
        )?;

        transaction.execute("DELETE FROM post_topics WHERE post_id = ?1", [&post.id])?;
        for topic in extract_topics(&post.title, &post.tags) {
            transaction.execute(
                "INSERT OR IGNORE INTO topics (id, display) VALUES (?1, ?2)",
                params![topic, display_topic(&topic)],
            )?;
            transaction.execute(
                "INSERT OR IGNORE INTO post_topics (post_id, topic_id) VALUES (?1, ?2)",
                params![post.id, topic],
            )?;
        }
        Ok(usize::from(changed > 0))
    }

    pub fn load_fixture(&mut self, now: DateTime<Utc>) -> Result<usize> {
        let fixture: Fixture =
            serde_json::from_str(include_str!("../fixtures/community-pulse.json"))
                .context("parse bundled fixture")?;

        let transaction = self.connection.transaction()?;
        transaction.execute_batch(
            "DELETE FROM score_snapshots; DELETE FROM post_topics; DELETE FROM posts; DELETE FROM topics; DELETE FROM interests; DELETE FROM subscriptions;",
        )?;
        let mut count = 0;
        for fixture_post in fixture.posts {
            let post = fixture_post.at(now);
            count += Self::upsert_post(&transaction, &post)?;
        }
        transaction.commit()?;

        // Three snapshots make the evidence chart useful immediately. The current
        // score remains authoritative; earlier snapshots are presentation history.
        self.recompute(now - Duration::hours(2))?;
        self.recompute(now - Duration::hours(1))?;
        self.recompute(now)?;
        Ok(count)
    }

    pub fn recompute(&mut self, now: DateTime<Utc>) -> Result<Vec<TopicScore>> {
        let topics = {
            let mut statement = self
                .connection
                .prepare("SELECT id, display FROM topics ORDER BY id")?;
            statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };

        let transaction = self.connection.transaction()?;
        let mut scores = Vec::with_capacity(topics.len());
        for (id, display) in topics {
            let score = compute_topic_score(&transaction, &id, &display, now)?;
            transaction.execute(
                r#"
                INSERT INTO score_snapshots (
                    topic_id, captured_at, mentions_1h, mentions_6h, mentions_24h,
                    baseline_mean, baseline_stddev, z_score, trend_score
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                ON CONFLICT(topic_id, captured_at) DO UPDATE SET
                    mentions_1h = excluded.mentions_1h,
                    mentions_6h = excluded.mentions_6h,
                    mentions_24h = excluded.mentions_24h,
                    baseline_mean = excluded.baseline_mean,
                    baseline_stddev = excluded.baseline_stddev,
                    z_score = excluded.z_score,
                    trend_score = excluded.trend_score
                "#,
                params![
                    score.id,
                    score.captured_at.timestamp(),
                    score.mentions_1h as i64,
                    score.mentions_6h as i64,
                    score.mentions_24h as i64,
                    score.baseline_mean,
                    score.baseline_stddev,
                    score.z_score,
                    score.trend_score,
                ],
            )?;
            scores.push(score);
        }
        transaction.commit()?;
        scores.sort_by(|left, right| right.trend_score.total_cmp(&left.trend_score));
        Ok(scores)
    }

    pub fn get_pulse(
        &self,
        interests: &InterestModel,
        limit: Option<usize>,
        now: DateTime<Utc>,
    ) -> Result<Vec<DigestCard>> {
        let requested = limit.unwrap_or(ATTENTION_BUDGET).clamp(1, ATTENTION_BUDGET);
        let mut statement = self.connection.prepare(
            r#"
            SELECT t.id, t.display, s.mentions_1h, s.mentions_6h, s.mentions_24h,
                   s.z_score, s.trend_score
            FROM topics t
            JOIN score_snapshots s ON s.topic_id = t.id
            WHERE s.captured_at = (
                SELECT MAX(latest.captured_at)
                FROM score_snapshots latest
                WHERE latest.topic_id = t.id
            )
            "#,
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)? as usize,
                row.get::<_, i64>(3)? as usize,
                row.get::<_, i64>(4)? as usize,
                row.get::<_, f64>(5)?,
                row.get::<_, f64>(6)?,
            ))
        })?;

        let mut cards = Vec::new();
        for row in rows {
            let (id, topic, mentions_1h, mentions_6h, mentions_24h, z_score, trend_score) = row?;
            let affinity = interests.affinity(&id);
            if affinity == 0.0 {
                continue;
            }
            cards.push(DigestCard {
                id,
                topic,
                headline: String::new(),
                sources: Vec::new(),
                score: trend_score * affinity,
                trend_score,
                interest_affinity: affinity,
                z_score,
                mentions_1h,
                mentions_6h,
                mentions_24h,
                sparkline: Vec::new(),
            });
        }
        cards.sort_by(|left, right| right.score.total_cmp(&left.score));
        let mut used_posts = HashSet::new();
        for card in &mut cards {
            let candidates = self.headline_candidates(&card.id)?;
            card.sources = candidates
                .iter()
                .map(|(_, _, source)| source.clone())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            card.headline = candidates
                .into_iter()
                .find_map(|(post_id, title, _)| used_posts.insert(post_id).then_some(title))
                .unwrap_or_else(|| format!("{} is gaining attention", card.topic));
            card.sparkline = hourly_series(&self.connection, &card.id, now, 12)?;
        }
        cards.truncate(requested);
        Ok(cards)
    }

    fn headline_candidates(&self, topic: &str) -> Result<HeadlineCandidates> {
        let mut statement = self.connection.prepare(
            r#"
                SELECT p.id, p.title, p.source
                FROM posts p
                JOIN post_topics pt ON pt.post_id = p.id
                WHERE pt.topic_id = ?1
            ORDER BY p.published_at DESC, p.points DESC
            LIMIT 8
            "#,
        )?;
        let rows = statement.query_map([topic], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        let mut candidates = Vec::new();
        for row in rows {
            let (post_id, title, source) = row?;
            candidates.push((post_id, title, source));
        }
        Ok(candidates)
    }

    pub fn explain_trend(&self, topic: &str, now: DateTime<Utc>) -> Result<TrendEvidence> {
        let snapshot = self
            .connection
            .query_row(
                r#"
                SELECT t.display, s.mentions_1h, s.mentions_6h, s.mentions_24h,
                       s.baseline_mean, s.baseline_stddev, s.z_score
                FROM topics t
                JOIN score_snapshots s ON s.topic_id = t.id
                WHERE t.id = ?1
                ORDER BY s.captured_at DESC
                LIMIT 1
                "#,
                [topic],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)? as usize,
                        row.get::<_, i64>(2)? as usize,
                        row.get::<_, i64>(3)? as usize,
                        row.get::<_, f64>(4)?,
                        row.get::<_, f64>(5)?,
                        row.get::<_, f64>(6)?,
                    ))
                },
            )
            .optional()?;
        let Some((display, mentions_1h, mentions_6h, mentions_24h, mean, stddev, z_score)) =
            snapshot
        else {
            bail!("unknown trend: {topic}");
        };

        let mut post_statement = self.connection.prepare(
            r#"
            SELECT p.source, p.title, p.url, p.published_at
            FROM posts p
            JOIN post_topics pt ON pt.post_id = p.id
            WHERE pt.topic_id = ?1
            ORDER BY p.published_at DESC, p.points DESC
            LIMIT 5
            "#,
        )?;
        let posts = post_statement
            .query_map([topic], |row| {
                let timestamp = row.get::<_, i64>(3)?;
                Ok(EvidencePost {
                    source: row.get(0)?,
                    title: row.get(1)?,
                    url: row.get(2)?,
                    published_at: DateTime::from_timestamp(timestamp, 0).unwrap_or_default(),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let sparkline = hourly_series(&self.connection, topic, now, 12)?;

        Ok(TrendEvidence {
            id: topic.to_owned(),
            topic: display,
            mentions_1h,
            mentions_6h,
            mentions_24h,
            baseline_mean: mean,
            baseline_stddev: stddev,
            z_score,
            sparkline,
            posts,
        })
    }

    pub fn set_interest(&self, topic: &str, weight: f64) -> Result<()> {
        if weight.abs() < f64::EPSILON {
            self.connection
                .execute("DELETE FROM interests WHERE topic_id = ?1", [topic])?;
        } else {
            self.connection.execute(
                r#"
                INSERT INTO interests (topic_id, weight) VALUES (?1, ?2)
                ON CONFLICT(topic_id) DO UPDATE SET weight = excluded.weight
                "#,
                params![topic, weight.clamp(-1.0, 2.0)],
            )?;
        }
        Ok(())
    }

    pub fn load_interests(&self) -> Result<InterestModel> {
        let mut statement = self
            .connection
            .prepare("SELECT topic_id, weight FROM interests ORDER BY topic_id")?;
        let values = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?))
            })?
            .collect::<rusqlite::Result<BTreeMap<_, _>>>()?;
        Ok(InterestModel(values))
    }

    pub fn subscribe_topic(&self, topic: &str, now: DateTime<Utc>) -> Result<Vec<String>> {
        self.connection.execute(
            r#"
            INSERT INTO subscriptions (topic_id, subscribed_at) VALUES (?1, ?2)
            ON CONFLICT(topic_id) DO NOTHING
            "#,
            params![topic, now.timestamp()],
        )?;
        self.subscriptions()
    }

    pub fn subscriptions(&self) -> Result<Vec<String>> {
        let mut statement = self
            .connection
            .prepare("SELECT topic_id FROM subscriptions ORDER BY subscribed_at, topic_id")?;
        Ok(statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn suggested_topics(&self, limit: usize) -> Result<Vec<String>> {
        let mut statement = self.connection.prepare(
            r#"
            SELECT topic_id
            FROM score_snapshots s
            WHERE captured_at = (
                SELECT MAX(latest.captured_at)
                FROM score_snapshots latest
                WHERE latest.topic_id = s.topic_id
            )
            ORDER BY trend_score DESC
            LIMIT ?1
            "#,
        )?;
        Ok(statement
            .query_map([limit as i64], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }
}

fn compute_topic_score(
    connection: &Connection,
    id: &str,
    display: &str,
    now: DateTime<Utc>,
) -> Result<TopicScore> {
    let mentions_1h = count_mentions(connection, id, now - Duration::hours(1), now)?;
    let mentions_6h = count_mentions(connection, id, now - Duration::hours(6), now)?;
    let mentions_24h = count_mentions(connection, id, now - Duration::hours(24), now)?;

    let mut baseline = Vec::with_capacity(27);
    for bucket in 1..=27 {
        let end = now - Duration::hours(6 * bucket);
        let start = end - Duration::hours(6);
        baseline.push(count_mentions(connection, id, start, end)? as f64);
    }
    let mean = baseline.iter().sum::<f64>() / baseline.len() as f64;
    let variance = baseline
        .iter()
        .map(|value| (value - mean).powi(2))
        .sum::<f64>()
        / baseline.len() as f64;
    let stddev = variance.sqrt();
    let denominator = stddev.max(1.0);
    let z_score = (mentions_6h as f64 - mean) / denominator;

    let source_count = connection.query_row(
        r#"
        SELECT COUNT(DISTINCT p.source)
        FROM posts p
        JOIN post_topics pt ON pt.post_id = p.id
        WHERE pt.topic_id = ?1 AND p.published_at >= ?2 AND p.published_at <= ?3
        "#,
        params![id, (now - Duration::hours(24)).timestamp(), now.timestamp()],
        |row| row.get::<_, i64>(0),
    )? as f64;

    let velocity = mentions_1h as f64 * 4.0 + mentions_6h as f64 * 0.8 + mentions_24h as f64 * 0.15;
    let trend_score = velocity * (1.0 + z_score.max(0.0) * 0.25) + source_count * 0.5;

    Ok(TopicScore {
        id: id.to_owned(),
        topic: display.to_owned(),
        mentions_1h,
        mentions_6h,
        mentions_24h,
        baseline_mean: mean,
        baseline_stddev: stddev,
        z_score,
        trend_score,
        captured_at: now,
    })
}

fn count_mentions(
    connection: &Connection,
    topic: &str,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Result<usize> {
    Ok(connection.query_row(
        r#"
        SELECT COUNT(*)
        FROM posts p
        JOIN post_topics pt ON pt.post_id = p.id
        WHERE pt.topic_id = ?1 AND p.published_at >= ?2 AND p.published_at < ?3
        "#,
        params![topic, start.timestamp(), end.timestamp()],
        |row| row.get::<_, i64>(0),
    )? as usize)
}

fn hourly_series(
    connection: &Connection,
    topic: &str,
    now: DateTime<Utc>,
    buckets: usize,
) -> Result<Vec<usize>> {
    (0..buckets)
        .rev()
        .map(|offset| {
            let start = now - Duration::hours(offset as i64 + 1);
            let end = now - Duration::hours(offset as i64);
            count_mentions(connection, topic, start, end)
        })
        .collect()
}

pub fn extract_topics(title: &str, tags: &[String]) -> Vec<String> {
    const KNOWN: &[(&str, &[&str])] = &[
        ("rust", &["rust", "cargo", "rustlang"]),
        (
            "wasm-runtimes",
            &["wasm", "webassembly", "wasmtime", "wasmer"],
        ),
        (
            "local-first",
            &["local-first", "local first", "offline-first", "crdt"],
        ),
        (
            "ai-infra",
            &["ai infra", "inference", "llm", "gpu", "agent"],
        ),
        (
            "databases",
            &["database", "sqlite", "postgres", "query engine"],
        ),
        ("privacy", &["privacy", "encrypted", "security"]),
        ("crypto", &["crypto", "bitcoin", "ethereum", "blockchain"]),
    ];
    const STOP_WORDS: &[&str] = &[
        "about", "after", "before", "build", "from", "have", "into", "more", "open", "release",
        "their", "this", "using", "what", "with", "your",
    ];

    let title_lower = title.to_lowercase();
    let tag_text = tags.join(" ").to_lowercase();
    let mut topics = BTreeSet::new();
    for (topic, aliases) in KNOWN {
        if aliases
            .iter()
            .any(|alias| title_lower.contains(alias) || tag_text.contains(alias))
        {
            topics.insert((*topic).to_owned());
        }
    }

    if topics.is_empty() {
        let stop_words = STOP_WORDS.iter().copied().collect::<HashSet<_>>();
        for word in title_lower
            .split(|character: char| !character.is_alphanumeric() && character != '-')
            .filter(|word| word.len() >= 5 && !stop_words.contains(*word))
            .take(2)
        {
            topics.insert(slugify(word));
        }
    }
    topics.into_iter().collect()
}

fn slugify(value: &str) -> String {
    value
        .trim()
        .to_lowercase()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

pub fn canonical_topic(value: &str) -> String {
    slugify(value)
}

pub fn display_topic(topic: &str) -> String {
    topic
        .split('-')
        .map(|part| {
            let mut characters = part.chars();
            match characters.next() {
                Some(first) => first.to_uppercase().collect::<String>() + characters.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Debug, Deserialize)]
struct Fixture {
    posts: Vec<FixturePost>,
}

#[derive(Debug, Deserialize)]
struct FixturePost {
    id: String,
    source: String,
    title: String,
    url: String,
    author: String,
    age_minutes: i64,
    points: i64,
    tags: Vec<String>,
}

impl FixturePost {
    fn at(self, now: DateTime<Utc>) -> CommunityPost {
        CommunityPost {
            id: self.id,
            source: self.source,
            title: self.title,
            url: self.url,
            author: self.author,
            published_at: now - Duration::minutes(self.age_minutes),
            points: self.points,
            tags: self.tags,
        }
    }
}
