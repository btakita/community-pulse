use crate::domain::{
    ChartBucket, ChartPost, CommunityPost, DEFAULT_BUDGET, DigestCard, EvidencePost, InterestModel,
    MAX_BUDGET, ResearchPost, ResearchReport, ResearchSeries, ResearchSubmission, ResearchTopic,
    SourceMentionCount, SourceWeight, TopicScore, TrendEvidence,
};
use anyhow::{Context as _, Result, bail};
use chrono::{DateTime, Duration, Timelike, Utc};
use rusqlite::types::Type;
use rusqlite::{Connection, OptionalExtension, Row, Transaction, params};
use serde::Deserialize;
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

pub struct PulseEngine {
    connection: Connection,
}

type HeadlineCandidate = (String, String, String, String, String);
type HeadlineCandidates = Vec<HeadlineCandidate>;

const VELOCITY_1H_WEIGHT: f64 = 4.0;
const VELOCITY_6H_WEIGHT: f64 = 0.8;
const VELOCITY_24H_WEIGHT: f64 = 0.15;
const Z_BOOST_DIVISOR: f64 = 4.0;
const SOURCE_DIVERSITY_DIVISOR: f64 = 2.0;
const BASELINE_BUCKET_HOURS: i64 = 6;
const BASELINE_BUCKETS: usize = 27;
const BASELINE_STDDEV_FLOOR: f64 = 1.0;
const SOURCE_WEIGHT_DAYS: f64 = 7.0;
const SOURCE_WEIGHT_WINDOW_HOURS: i64 = 24 * 7;
const SOURCE_WEIGHT_MIN: f64 = 0.25;
const SOURCE_WEIGHT_MAX: f64 = 4.0;

#[derive(Clone, Debug, PartialEq)]
pub struct MethodologyCopy {
    pub formula: String,
    pub ingest: String,
    pub categorize: String,
    pub score: String,
    pub surface: String,
    pub source_weights: Vec<SourceWeight>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RankBreakdownCopy {
    pub rank_line: String,
    pub trend_line: String,
    pub recomputed_rank: f64,
    pub recomputed_trend: f64,
}

pub fn methodology_copy(
    attention_budget: usize,
    source_weights: &[SourceWeight],
) -> MethodologyCopy {
    let live_weights = if source_weights.is_empty() {
        "this week: no source volume yet".to_owned()
    } else {
        format!(
            "this week: {}",
            source_weights
                .iter()
                .map(|source| format!(
                    "{} ×{:.2}",
                    source_short_label(&source.source),
                    source.weight
                ))
                .collect::<Vec<_>>()
                .join(" · ")
        )
    };
    MethodologyCopy {
        formula: format!(
            "rank = trend × interest · trend = velocity × (1 + z⁺/{Z_BOOST_DIVISOR:.0}) + sources/{SOURCE_DIVERSITY_DIVISOR:.0} · capped at your budget"
        ),
        ingest: "INGEST — Normalize Hacker News, Lobsters, and Product Hunt posts, with a politeness floor between source fetches.".to_owned(),
        categorize: "CATEGORIZE — Curated aliases match title + tags (for example, ‘wasmtime’ → wasm-runtimes). One post can enter several topics; unmatched posts use title keywords. This is keyword matching, not ML, so misclassification is possible; aliases are inspectable.".to_owned(),
        score: format!(
            "SCORE — Source-normalized mentions feed velocity (1h × {VELOCITY_1H_WEIGHT:.0}, 6h × {VELOCITY_6H_WEIGHT}, 24h × {VELOCITY_24H_WEIGHT}) and z’s current/baseline values; weights use rolling 7-day source volumes, clamped {SOURCE_WEIGHT_MIN:.2}–{SOURCE_WEIGHT_MAX:.1}. {live_weights}. z compares the current weighted 6h value with the topic’s own {BASELINE_BUCKETS} prior {BASELINE_BUCKET_HOURS}h windows, with σ floored at {BASELINE_STDDEV_FLOOR:.0}; source diversity adds sources/{SOURCE_DIVERSITY_DIVISOR:.0}; then trend is multiplied by your interest affinity. Interest states are − mute, 0 neutral, + boost, and ++ strong."
        ),
        surface: format!(
            "SURFACE — Rank signals and show at most {attention_budget}, your live attention budget."
        ),
        source_weights: source_weights.to_vec(),
    }
}

pub fn rank_breakdown_copy(card: &DigestCard) -> RankBreakdownCopy {
    let velocity = velocity_score(
        card.weighted_mentions_1h,
        card.weighted_mentions_6h,
        card.weighted_mentions_24h,
    );
    let z_boost = card.z_score.max(0.0);
    let source_bonus = card.sources.len() as f64 / SOURCE_DIVERSITY_DIVISOR;
    let recomputed_trend = trend_score(velocity, card.z_score, card.sources.len());
    let recomputed_rank = rank_score(recomputed_trend, card.interest_affinity);

    RankBreakdownCopy {
        rank_line: format!(
            "rank {:.1} = trend {:.1} × interest {:.2}",
            card.score, card.trend_score, card.interest_affinity
        ),
        trend_line: format!(
            "trend {:.1} = weighted velocity {:.1} (1h {:.2} · 6h {:.2} · 24h {:.2}) × (1 + z⁺ {:.1}/{Z_BOOST_DIVISOR:.0}) + sources {source_bonus:.1}",
            card.trend_score,
            velocity,
            card.weighted_mentions_1h,
            card.weighted_mentions_6h,
            card.weighted_mentions_24h,
            z_boost
        ),
        recomputed_rank,
        recomputed_trend,
    }
}

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
                points       INTEGER NOT NULL,
                summary      TEXT NOT NULL DEFAULT ''
            );

            CREATE TABLE IF NOT EXISTS topics (
                id      TEXT PRIMARY KEY,
                display TEXT NOT NULL
            );

CREATE TABLE IF NOT EXISTS post_topics (
post_id  TEXT NOT NULL REFERENCES posts(id) ON DELETE CASCADE,
topic_id TEXT NOT NULL REFERENCES topics(id) ON DELETE CASCADE,
matched_alias TEXT NOT NULL DEFAULT '',
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
                weighted_mentions_1h REAL NOT NULL DEFAULT 0,
                weighted_mentions_6h REAL NOT NULL DEFAULT 0,
                weighted_mentions_24h REAL NOT NULL DEFAULT 0,
                weighted_baseline_mean REAL NOT NULL DEFAULT 0,
                weighted_baseline_stddev REAL NOT NULL DEFAULT 0,
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

            CREATE TABLE IF NOT EXISTS settings (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS research_reports (
                id          INTEGER PRIMARY KEY,
                topic_id    TEXT NOT NULL,
                agent       TEXT NOT NULL,
                title       TEXT NOT NULL,
                markdown    TEXT NOT NULL,
                citations   TEXT NOT NULL,
                web_report  TEXT,
                article_url TEXT,
                sections_json TEXT NOT NULL DEFAULT '[]',
                verdict     TEXT,
                summary     TEXT,
                watch_json  TEXT NOT NULL DEFAULT '[]',
                created_at  INTEGER NOT NULL,
                status      TEXT NOT NULL CHECK(status IN ('submitted', 'superseded'))
            );

            CREATE INDEX IF NOT EXISTS idx_research_reports_topic_created
            ON research_reports(topic_id, created_at DESC, id DESC);
            "#,
        )?;
        ensure_column(
            &self.connection,
            "posts",
            "summary",
            "TEXT NOT NULL DEFAULT ''",
        )?;
        ensure_column(
            &self.connection,
            "post_topics",
            "matched_alias",
            "TEXT NOT NULL DEFAULT ''",
        )?;
        for column in [
            "weighted_mentions_1h",
            "weighted_mentions_6h",
            "weighted_mentions_24h",
            "weighted_baseline_mean",
            "weighted_baseline_stddev",
        ] {
            ensure_column(
                &self.connection,
                "score_snapshots",
                column,
                "REAL NOT NULL DEFAULT 0",
            )?;
        }
        ensure_column(&self.connection, "research_reports", "verdict", "TEXT")?;
        ensure_column(&self.connection, "research_reports", "summary", "TEXT")?;
        ensure_column(&self.connection, "research_reports", "article_url", "TEXT")?;
        ensure_column(
            &self.connection,
            "research_reports",
            "sections_json",
            "TEXT NOT NULL DEFAULT '[]'",
        )?;
        ensure_column(
            &self.connection,
            "research_reports",
            "watch_json",
            "TEXT NOT NULL DEFAULT '[]'",
        )?;
        Ok(())
    }

    pub fn budget(&self) -> Result<usize> {
        let stored = self
            .connection
            .query_row(
                "SELECT value FROM settings WHERE key = 'attention_budget'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        Ok(stored
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(DEFAULT_BUDGET)
            .clamp(1, MAX_BUDGET))
    }

    pub fn set_budget(&self, budget: usize) -> Result<usize> {
        let budget = budget.clamp(1, MAX_BUDGET);
        self.connection.execute(
            r#"
            INSERT INTO settings (key, value) VALUES ('attention_budget', ?1)
            ON CONFLICT(key) DO UPDATE SET value = excluded.value
            "#,
            [budget.to_string()],
        )?;
        Ok(budget)
    }

    pub fn post_count(&self) -> Result<usize> {
        let count = self
            .connection
            .query_row("SELECT COUNT(*) FROM posts", [], |row| row.get::<_, i64>(0))?;
        Ok(count as usize)
    }

    pub fn source_weights(&self, now: DateTime<Utc>) -> Result<Vec<SourceWeight>> {
        compute_source_weights(&self.connection, now)
    }

    pub fn snapshot_to(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        if path.exists() {
            bail!("snapshot target already exists: {}", path.display());
        }
        self.connection
            .execute("VACUUM INTO ?1", [path.to_string_lossy().as_ref()])
            .with_context(|| format!("snapshot database to {}", path.display()))?;
        Ok(())
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
            INSERT INTO posts (id, source, title, url, author, published_at, points, summary)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            ON CONFLICT(id) DO UPDATE SET
                source = excluded.source,
                title = excluded.title,
                url = excluded.url,
                author = excluded.author,
                published_at = excluded.published_at,
                points = excluded.points,
                summary = excluded.summary
            "#,
            params![
                post.id,
                post.source,
                post.title,
                post.url,
                post.author,
                post.published_at.timestamp(),
                post.points,
                post.summary,
            ],
        )?;

        transaction.execute("DELETE FROM post_topics WHERE post_id = ?1", [&post.id])?;
        for (topic, matched_alias) in extract_topic_matches(&post.title, &post.tags) {
            transaction.execute(
                "INSERT OR IGNORE INTO topics (id, display) VALUES (?1, ?2)",
                params![topic, display_topic(&topic)],
            )?;
            transaction.execute(
                "INSERT OR IGNORE INTO post_topics (post_id, topic_id, matched_alias) VALUES (?1, ?2, ?3)",
                params![post.id, topic, matched_alias],
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

        let source_weights = compute_source_weights(&self.connection, now)?;
        let transaction = self.connection.transaction()?;
        let mut scores = Vec::with_capacity(topics.len());
        for (id, display) in topics {
            let score = compute_topic_score(&transaction, &id, &display, now, &source_weights)?;
            transaction.execute(
                r#"
                INSERT INTO score_snapshots (
                topic_id, captured_at, mentions_1h, mentions_6h, mentions_24h,
                baseline_mean, baseline_stddev, weighted_mentions_1h,
                weighted_mentions_6h, weighted_mentions_24h,
                weighted_baseline_mean, weighted_baseline_stddev, z_score, trend_score
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
                ON CONFLICT(topic_id, captured_at) DO UPDATE SET
                    mentions_1h = excluded.mentions_1h,
                    mentions_6h = excluded.mentions_6h,
                    mentions_24h = excluded.mentions_24h,
            baseline_mean = excluded.baseline_mean,
            baseline_stddev = excluded.baseline_stddev,
            weighted_mentions_1h = excluded.weighted_mentions_1h,
            weighted_mentions_6h = excluded.weighted_mentions_6h,
            weighted_mentions_24h = excluded.weighted_mentions_24h,
            weighted_baseline_mean = excluded.weighted_baseline_mean,
            weighted_baseline_stddev = excluded.weighted_baseline_stddev,
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
                    score.weighted_mentions_1h,
                    score.weighted_mentions_6h,
                    score.weighted_mentions_24h,
                    score.weighted_baseline_mean,
                    score.weighted_baseline_stddev,
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
        let requested = limit.unwrap_or(self.budget()?).clamp(1, MAX_BUDGET);
        let mut statement = self.connection.prepare(
            r#"
            SELECT t.id, t.display, s.mentions_1h, s.mentions_6h, s.mentions_24h,
                   s.baseline_mean, s.baseline_stddev, s.weighted_mentions_1h,
                   s.weighted_mentions_6h, s.weighted_mentions_24h,
                   s.weighted_baseline_mean, s.weighted_baseline_stddev,
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
                row.get::<_, f64>(7)?,
                row.get::<_, f64>(8)?,
                row.get::<_, f64>(9)?,
                row.get::<_, f64>(10)?,
                row.get::<_, f64>(11)?,
                row.get::<_, f64>(12)?,
                row.get::<_, f64>(13)?,
            ))
        })?;

        let mut cards = Vec::new();
        for row in rows {
            let (
                id,
                topic,
                mentions_1h,
                mentions_6h,
                mentions_24h,
                baseline_mean,
                baseline_stddev,
                weighted_mentions_1h,
                weighted_mentions_6h,
                weighted_mentions_24h,
                weighted_baseline_mean,
                weighted_baseline_stddev,
                z_score,
                trend_score,
            ) = row?;
            let affinity = interests.affinity(&id);
            if affinity == 0.0 {
                continue;
            }
            cards.push(DigestCard {
                id,
                topic,
                headline: String::new(),
                headline_url: String::new(),
                headline_source: String::new(),
                headline_summary: String::new(),
                sources: Vec::new(),
                score: rank_score(trend_score, affinity),
                trend_score,
                interest_affinity: affinity,
                baseline_mean,
                baseline_stddev,
                weighted_mentions_1h,
                weighted_mentions_6h,
                weighted_mentions_24h,
                weighted_baseline_mean,
                weighted_baseline_stddev,
                z_score,
                mentions_1h,
                mentions_6h,
                mentions_24h,
                sparkline: Vec::new(),
                chart_buckets: Vec::new(),
            });
        }
        cards.sort_by(|left, right| right.score.total_cmp(&left.score));
        let mut used_posts = HashSet::new();
        for card in &mut cards {
            let candidates = self.headline_candidates(&card.id, now)?;
            card.sources =
                topic_sources_since(&self.connection, &card.id, now - Duration::hours(24), now)?;
            if let Some((_, title, source, url, summary)) = candidates
                .into_iter()
                .find(|(post_id, _, _, _, _)| used_posts.insert(post_id.clone()))
            {
                card.headline = title;
                card.headline_url = url;
                card.headline_source = source;
                card.headline_summary = summary;
            } else {
                card.headline = format!("{} is gaining attention", card.topic);
            }
            card.chart_buckets = hourly_series(&self.connection, &card.id, now, 12, false)?;
            card.sparkline = card
                .chart_buckets
                .iter()
                .map(|bucket| bucket.mentions)
                .collect();
        }
        cards.truncate(requested);
        Ok(cards)
    }

    pub fn digest_delta_chips(&self, interests: &InterestModel) -> Result<Vec<String>> {
        let mut times = self.connection.prepare(
            "SELECT DISTINCT captured_at FROM score_snapshots ORDER BY captured_at DESC LIMIT 2",
        )?;
        let captures = times
            .query_map([], |row| row.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if captures.len() < 2 {
            return Ok(vec!["Fresh pulse".to_owned()]);
        }

        let current = self.ranked_scores_at(interests, captures[0])?;
        let previous = self.ranked_scores_at(interests, captures[1])?;
        let previous_by_id = previous
            .iter()
            .map(|(id, topic, score)| (id.as_str(), (topic.as_str(), *score)))
            .collect::<std::collections::HashMap<_, _>>();
        let current_ids = current
            .iter()
            .map(|(id, _, _)| id.as_str())
            .collect::<HashSet<_>>();
        let entered = current
            .iter()
            .filter(|(id, _, _)| !previous_by_id.contains_key(id.as_str()))
            .map(|(_, topic, _)| topic.as_str())
            .collect::<Vec<_>>();

        let mut chips = Vec::new();
        if !entered.is_empty() {
            chips.push(format!("{} new · {}", entered.len(), entered.join(", ")));
        }
        if let Some((_, topic, change)) = current
            .iter()
            .filter_map(|(id, topic, score)| {
                previous_by_id
                    .get(id.as_str())
                    .map(|(_, previous)| (id, topic, score - previous))
            })
            .filter(|(_, _, change)| *change < 0.0)
            .min_by(|left, right| left.2.total_cmp(&right.2))
        {
            chips.push(format!("{topic} cooled {change:.1}"));
        }
        if let Some((_, topic, score)) = previous
            .iter()
            .filter(|(id, _, _)| !current_ids.contains(id.as_str()))
            .max_by(|left, right| left.2.total_cmp(&right.2))
        {
            chips.push(format!("{topic} left top 5 · {score:.1}"));
        }
        if chips.is_empty() {
            chips.push("Top 5 holding steady".to_owned());
        }
        chips.truncate(3);
        Ok(chips)
    }

    fn ranked_scores_at(
        &self,
        interests: &InterestModel,
        captured_at: i64,
    ) -> Result<Vec<(String, String, f64)>> {
        let mut statement = self.connection.prepare(
            r#"
            SELECT t.id, t.display, s.trend_score
            FROM topics t
            JOIN score_snapshots s ON s.topic_id = t.id
            WHERE s.captured_at = ?1
            "#,
        )?;
        let rows = statement.query_map([captured_at], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, f64>(2)?,
            ))
        })?;
        let mut scores = Vec::new();
        for row in rows {
            let (id, topic, trend_score) = row?;
            let affinity = interests.affinity(&id);
            if affinity > 0.0 {
                scores.push((id, topic, trend_score * affinity));
            }
        }
        scores.sort_by(|left, right| right.2.total_cmp(&left.2));
        scores.truncate(self.budget()?);
        Ok(scores)
    }

    fn headline_candidates(&self, topic: &str, now: DateTime<Utc>) -> Result<HeadlineCandidates> {
        let mut statement = self.connection.prepare(
            r#"
            WITH candidates AS (
                SELECT p.id, p.title, p.source, p.url, p.summary, p.published_at,
                       CAST((
                           SELECT COUNT(*) FROM posts peers
                           WHERE peers.source = p.source
                             AND peers.published_at >= ?2 AND peers.published_at <= ?3
                             AND peers.points <= p.points
                       ) AS REAL) / NULLIF((
                           SELECT COUNT(*) FROM posts peers
                           WHERE peers.source = p.source
                             AND peers.published_at >= ?2 AND peers.published_at <= ?3
                       ), 0) AS points_percentile
                FROM posts p
                JOIN post_topics pt ON pt.post_id = p.id
                WHERE pt.topic_id = ?1
            )
            SELECT id, title, source, url, summary
            FROM candidates
            ORDER BY published_at DESC, points_percentile DESC
            LIMIT 8
            "#,
        )?;
        let rows = statement.query_map(
            params![
                topic,
                (now - Duration::hours(SOURCE_WEIGHT_WINDOW_HOURS)).timestamp(),
                now.timestamp()
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )?;
        let mut candidates = Vec::new();
        for row in rows {
            let (post_id, title, source, url, summary) = row?;
            candidates.push((post_id, title, source, url, summary));
        }
        Ok(candidates)
    }

    pub fn explain_trend(&self, topic: &str, now: DateTime<Utc>) -> Result<TrendEvidence> {
        let snapshot = self
            .connection
            .query_row(
                r#"
            SELECT t.display, s.mentions_1h, s.mentions_6h, s.mentions_24h,
                   s.baseline_mean, s.baseline_stddev, s.weighted_mentions_6h,
                   s.weighted_baseline_mean, s.weighted_baseline_stddev, s.z_score
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
                        row.get::<_, f64>(7)?,
                        row.get::<_, f64>(8)?,
                        row.get::<_, f64>(9)?,
                    ))
                },
            )
            .optional()?;
        let Some((
            display,
            mentions_1h,
            mentions_6h,
            mentions_24h,
            mean,
            stddev,
            weighted_mentions_6h,
            weighted_mean,
            weighted_stddev,
            z_score,
        )) = snapshot
        else {
            bail!("unknown trend: {topic}");
        };

        let mut post_statement = self.connection.prepare(
            r#"
            SELECT p.source, p.title, p.url, p.summary, pt.matched_alias, p.published_at,
                   p.points,
                   CAST((
                       SELECT COUNT(*) FROM posts peers
                       WHERE peers.source = p.source
                         AND peers.published_at >= ?2 AND peers.published_at <= ?3
                         AND peers.points <= p.points
                   ) AS REAL) / NULLIF((
                       SELECT COUNT(*) FROM posts peers
                       WHERE peers.source = p.source
                         AND peers.published_at >= ?2 AND peers.published_at <= ?3
                   ), 0) AS points_percentile
            FROM posts p
            JOIN post_topics pt ON pt.post_id = p.id
            WHERE pt.topic_id = ?1
            ORDER BY p.published_at DESC, points_percentile DESC
            LIMIT 5
            "#,
        )?;
        let posts = post_statement
            .query_map(
                params![
                    topic,
                    (now - Duration::hours(SOURCE_WEIGHT_WINDOW_HOURS)).timestamp(),
                    now.timestamp()
                ],
                |row| {
                    let timestamp = row.get::<_, i64>(5)?;
                    Ok(EvidencePost {
                        source: row.get(0)?,
                        title: row.get(1)?,
                        url: row.get(2)?,
                        summary: row.get(3)?,
                        matched_alias: row.get(4)?,
                        points: row.get(6)?,
                        points_percentile: row.get(7)?,
                        published_at: DateTime::from_timestamp(timestamp, 0).unwrap_or_default(),
                    })
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let chart_buckets = hourly_series(&self.connection, topic, now, 12, true)?;
        let sparkline = chart_buckets.iter().map(|bucket| bucket.mentions).collect();

        Ok(TrendEvidence {
            id: topic.to_owned(),
            topic: display,
            mentions_1h,
            mentions_6h,
            mentions_24h,
            baseline_mean: mean,
            baseline_stddev: stddev,
            weighted_mentions_6h,
            weighted_baseline_mean: weighted_mean,
            weighted_baseline_stddev: weighted_stddev,
            z_score,
            sparkline,
            chart_buckets,
            posts,
        })
    }

    pub fn list_topics(
        &self,
        now: DateTime<Utc>,
        window_hours: usize,
        min_z: Option<f64>,
    ) -> Result<Vec<ResearchTopic>> {
        let captured_at = self.connection.query_row(
            "SELECT MAX(captured_at) FROM score_snapshots",
            [],
            |row| row.get::<_, Option<i64>>(0),
        )?;
        let Some(captured_at) = captured_at else {
            return Ok(Vec::new());
        };
        let mut statement = self.connection.prepare(
            r#"
            SELECT t.id, t.display, s.z_score, s.trend_score
            FROM topics t
            JOIN score_snapshots s ON s.topic_id = t.id
            WHERE s.captured_at = ?1
            ORDER BY s.trend_score DESC, t.id
            "#,
        )?;
        let rows = statement.query_map([captured_at], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, f64>(2)?,
                row.get::<_, f64>(3)?,
            ))
        })?;
        let start = now - Duration::hours(window_hours as i64);
        let min_z = min_z.unwrap_or(f64::NEG_INFINITY);
        let mut topics = Vec::new();
        for row in rows {
            let (id, display, z, trend) = row?;
            if z < min_z {
                continue;
            }
            topics.push(ResearchTopic {
                mentions: count_mentions(&self.connection, &id, start, now)?,
                sources: topic_sources_since(&self.connection, &id, start, now)?,
                id,
                display,
                z,
                trend,
                window_hours,
            });
        }
        Ok(topics)
    }

    pub fn topic_posts(
        &self,
        topic: &str,
        now: DateTime<Utc>,
        window_hours: usize,
        limit: usize,
    ) -> Result<Vec<ResearchPost>> {
        let mut statement = self.connection.prepare(
            r#"
            SELECT p.source, p.title, p.url, p.points, p.published_at,
                   CAST((
                       SELECT COUNT(*) FROM posts peers
                       WHERE peers.source = p.source
                         AND peers.published_at >= ?5 AND peers.published_at <= ?6
                         AND peers.points <= p.points
                   ) AS REAL) / NULLIF((
                       SELECT COUNT(*) FROM posts peers
                       WHERE peers.source = p.source
                         AND peers.published_at >= ?5 AND peers.published_at <= ?6
                   ), 0) AS points_percentile
            FROM posts p
            JOIN post_topics pt ON pt.post_id = p.id
            WHERE pt.topic_id = ?1
              AND p.published_at >= ?2
              AND p.published_at <= ?3
            ORDER BY p.published_at DESC, points_percentile DESC
            LIMIT ?4
            "#,
        )?;
        Ok(statement
            .query_map(
                params![
                    topic,
                    (now - Duration::hours(window_hours as i64)).timestamp(),
                    now.timestamp(),
                    limit as i64,
                    (now - Duration::hours(SOURCE_WEIGHT_WINDOW_HOURS)).timestamp(),
                    now.timestamp()
                ],
                |row| {
                    Ok(ResearchPost {
                        source: row.get(0)?,
                        title: row.get(1)?,
                        url: row.get(2)?,
                        points: row.get(3)?,
                        points_percentile: row.get(5)?,
                        published_at: DateTime::from_timestamp(row.get(4)?, 0).unwrap_or_default(),
                    })
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn topic_has_post_url(&self, topic: &str, url: &str) -> Result<bool> {
        Ok(self.connection.query_row(
            r#"
                SELECT EXISTS (
                    SELECT 1
                    FROM posts p
                    JOIN post_topics pt ON pt.post_id = p.id
                    WHERE pt.topic_id = ?1 AND p.url = ?2
                )
                "#,
            params![topic, url],
            |row| row.get(0),
        )?)
    }

    pub fn get_series(
        &self,
        topic: &str,
        now: DateTime<Utc>,
        buckets: usize,
        bucket_hours: usize,
    ) -> Result<ResearchSeries> {
        let baseline = self
            .connection
            .query_row(
                r#"
                SELECT baseline_mean, baseline_stddev
                FROM score_snapshots
                WHERE topic_id = ?1
                ORDER BY captured_at DESC
                LIMIT 1
                "#,
                [topic],
                |row| Ok((row.get::<_, f64>(0)?, row.get::<_, f64>(1)?)),
            )
            .optional()?;
        let Some((baseline_mean, baseline_stddev)) = baseline else {
            bail!("unknown trend: {topic}");
        };
        Ok(ResearchSeries {
            id: topic.to_owned(),
            buckets,
            bucket_hours,
            counts: bucket_series(&self.connection, topic, now, buckets, bucket_hours)?,
            baseline_mean,
            baseline_stddev,
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

    pub fn set_muted(&self, topic: &str, muted: bool) -> Result<()> {
        let weight = if muted { -1.0 } else { 0.0 };
        self.connection.execute(
            r#"
                INSERT INTO interests (topic_id, weight) VALUES (?1, ?2)
                ON CONFLICT(topic_id) DO UPDATE SET weight = excluded.weight
                "#,
            params![topic, weight],
        )?;
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

    #[allow(clippy::too_many_arguments)]
    pub fn submit_research(
        &self,
        submission: &ResearchSubmission,
        now: DateTime<Utc>,
    ) -> Result<ResearchReport> {
        let citations_json = serde_json::to_string(&submission.citations)?;
        let sections_json = serde_json::to_string(&submission.sections)?;
        let watch_json = serde_json::to_string(&submission.enrichment.watch)?;
        self.connection.execute(
            r#"
                INSERT INTO research_reports
                    (topic_id, agent, title, markdown, citations, web_report, article_url,
                     sections_json, verdict, summary, watch_json, created_at, status)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 'submitted')
                "#,
            params![
                submission.topic_id,
                submission.agent,
                submission.title,
                submission.markdown,
                citations_json,
                submission.web_report,
                submission.article_url,
                sections_json,
                submission.enrichment.verdict,
                submission.enrichment.summary,
                watch_json,
                now.timestamp()
            ],
        )?;
        Ok(ResearchReport {
            id: self.connection.last_insert_rowid(),
            topic_id: submission.topic_id.clone(),
            agent: submission.agent.clone(),
            title: submission.title.clone(),
            markdown: submission.markdown.clone(),
            citations: submission.citations.clone(),
            web_report: submission.web_report.clone(),
            article_url: submission.article_url.clone(),
            sections: submission.sections.clone(),
            verdict: submission.enrichment.verdict.clone(),
            summary: submission.enrichment.summary.clone(),
            watch: submission.enrichment.watch.clone(),
            created_at: now,
            status: "submitted".to_owned(),
        })
    }

    pub fn list_research(&self, topic_id: Option<&str>) -> Result<Vec<ResearchReport>> {
        const SELECT_REPORTS: &str = r#"
            SELECT id, topic_id, agent, title, markdown, citations, web_report, article_url,
                   sections_json, verdict, summary, watch_json, created_at, status
            FROM research_reports
        "#;
        let mut statement = if topic_id.is_some() {
            self.connection.prepare(&format!(
                "{SELECT_REPORTS} WHERE topic_id = ?1 ORDER BY created_at DESC, id DESC"
            ))?
        } else {
            self.connection.prepare(&format!(
                "{SELECT_REPORTS} ORDER BY created_at DESC, id DESC"
            ))?
        };
        let reports = if let Some(topic_id) = topic_id {
            statement
                .query_map([topic_id], research_report_from_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?
        } else {
            statement
                .query_map([], research_report_from_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        Ok(reports)
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

fn research_report_from_row(row: &Row<'_>) -> rusqlite::Result<ResearchReport> {
    let citations_json = row.get::<_, String>(5)?;
    let citations = serde_json::from_str(&citations_json).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(5, Type::Text, Box::new(error))
    })?;
    let sections_json = row.get::<_, String>(8)?;
    let sections = serde_json::from_str(&sections_json).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(8, Type::Text, Box::new(error))
    })?;
    let watch_json = row.get::<_, String>(11)?;
    let watch = serde_json::from_str(&watch_json).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(11, Type::Text, Box::new(error))
    })?;
    let created_at = DateTime::from_timestamp(row.get(12)?, 0).unwrap_or_default();
    Ok(ResearchReport {
        id: row.get(0)?,
        topic_id: row.get(1)?,
        agent: row.get(2)?,
        title: row.get(3)?,
        markdown: row.get(4)?,
        citations,
        web_report: row.get(6)?,
        article_url: row.get(7)?,
        sections,
        verdict: row.get(9)?,
        summary: row.get(10)?,
        watch,
        created_at,
        status: row.get(13)?,
    })
}

fn ensure_column(
    connection: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<()> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if !columns.iter().any(|existing| existing == column) {
        connection.execute_batch(&format!(
            "ALTER TABLE {table} ADD COLUMN {column} {definition};"
        ))?;
    }
    Ok(())
}

fn compute_topic_score(
    connection: &Connection,
    id: &str,
    display: &str,
    now: DateTime<Utc>,
    source_weights: &[SourceWeight],
) -> Result<TopicScore> {
    let mentions_1h = count_mentions(connection, id, now - Duration::hours(1), now)?;
    let mentions_6h = count_mentions(connection, id, now - Duration::hours(6), now)?;
    let mentions_24h = count_mentions(connection, id, now - Duration::hours(24), now)?;
    let weighted_mentions_1h = weighted_mentions(
        connection,
        id,
        now - Duration::hours(1),
        now,
        source_weights,
    )?;
    let weighted_mentions_6h = weighted_mentions(
        connection,
        id,
        now - Duration::hours(6),
        now,
        source_weights,
    )?;
    let weighted_mentions_24h = weighted_mentions(
        connection,
        id,
        now - Duration::hours(24),
        now,
        source_weights,
    )?;

    let mut baseline = Vec::with_capacity(BASELINE_BUCKETS);
    let mut weighted_baseline = Vec::with_capacity(BASELINE_BUCKETS);
    for bucket in 1..=BASELINE_BUCKETS {
        let end = now - Duration::hours(BASELINE_BUCKET_HOURS * bucket as i64);
        let start = end - Duration::hours(BASELINE_BUCKET_HOURS);
        baseline.push(count_mentions(connection, id, start, end)? as f64);
        weighted_baseline.push(weighted_mentions(
            connection,
            id,
            start,
            end,
            source_weights,
        )?);
    }
    let (mean, stddev) = mean_and_stddev(&baseline);
    let (weighted_mean, weighted_stddev) = mean_and_stddev(&weighted_baseline);
    let denominator = weighted_stddev.max(BASELINE_STDDEV_FLOOR);
    let z_score = (weighted_mentions_6h - weighted_mean) / denominator;

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

    let velocity = velocity_score(
        weighted_mentions_1h,
        weighted_mentions_6h,
        weighted_mentions_24h,
    );
    let trend_score = trend_score(velocity, z_score, source_count as usize);

    Ok(TopicScore {
        id: id.to_owned(),
        topic: display.to_owned(),
        mentions_1h,
        mentions_6h,
        mentions_24h,
        baseline_mean: mean,
        baseline_stddev: stddev,
        weighted_mentions_1h,
        weighted_mentions_6h,
        weighted_mentions_24h,
        weighted_baseline_mean: weighted_mean,
        weighted_baseline_stddev: weighted_stddev,
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

fn compute_source_weights(
    connection: &Connection,
    now: DateTime<Utc>,
) -> Result<Vec<SourceWeight>> {
    let mut statement = connection.prepare(
        r#"
        SELECT source, COUNT(*)
        FROM posts
        WHERE published_at >= ?1 AND published_at < ?2
        GROUP BY source
        ORDER BY source
        "#,
    )?;
    let counts = statement
        .query_map(
            params![
                (now - Duration::hours(SOURCE_WEIGHT_WINDOW_HOURS)).timestamp(),
                now.timestamp()
            ],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as u64)),
        )?
        .collect::<rusqlite::Result<BTreeMap<_, _>>>()?;
    Ok(source_weights_from_counts(&counts))
}

fn source_weights_from_counts(counts: &BTreeMap<String, u64>) -> Vec<SourceWeight> {
    if counts.is_empty() {
        return Vec::new();
    }
    let global_average_daily_posts =
        counts.values().sum::<u64>() as f64 / counts.len() as f64 / SOURCE_WEIGHT_DAYS;
    counts
        .iter()
        .map(|(source, count)| {
            let average_daily_posts = *count as f64 / SOURCE_WEIGHT_DAYS;
            SourceWeight {
                source: source.clone(),
                average_daily_posts,
                weight: (global_average_daily_posts / average_daily_posts)
                    .clamp(SOURCE_WEIGHT_MIN, SOURCE_WEIGHT_MAX),
            }
        })
        .collect()
}

fn weighted_mentions(
    connection: &Connection,
    topic: &str,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    source_weights: &[SourceWeight],
) -> Result<f64> {
    let mut statement = connection.prepare(
        r#"
        SELECT p.source, COUNT(*)
        FROM posts p
        JOIN post_topics pt ON pt.post_id = p.id
        WHERE pt.topic_id = ?1 AND p.published_at >= ?2 AND p.published_at < ?3
        GROUP BY p.source
        "#,
    )?;
    let counts = statement
        .query_map(params![topic, start.timestamp(), end.timestamp()], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as u64))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(counts
        .into_iter()
        .map(|(source, count)| {
            let weight = source_weights
                .iter()
                .find(|candidate| candidate.source == source)
                .map_or(1.0, |candidate| candidate.weight);
            count as f64 * weight
        })
        .sum())
}

fn mean_and_stddev(values: &[f64]) -> (f64, f64) {
    if values.is_empty() {
        return (0.0, 0.0);
    }
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    let variance = values
        .iter()
        .map(|value| (value - mean).powi(2))
        .sum::<f64>()
        / values.len() as f64;
    (mean, variance.sqrt())
}

fn source_short_label(source: &str) -> &str {
    match source {
        "Hacker News" => "HN",
        "Product Hunt" => "PH",
        other => other,
    }
}

fn hourly_series(
    connection: &Connection,
    topic: &str,
    now: DateTime<Utc>,
    buckets: usize,
    include_posts: bool,
) -> Result<Vec<ChartBucket>> {
    let end_anchor = now
        .with_minute(0)
        .and_then(|value| value.with_second(0))
        .and_then(|value| value.with_nanosecond(0))
        .unwrap_or(now)
        + Duration::hours(1);
    let mut series = Vec::with_capacity(buckets);
    for offset in (0..buckets).rev() {
        let end = end_anchor - Duration::hours(offset as i64);
        let start = end - Duration::hours(1);
        let mentions = count_mentions(connection, topic, start, end)?;
        let source_counts = source_mention_counts(connection, topic, start, end)?;
        let posts = if include_posts {
            chart_bucket_posts(connection, topic, start, end)?
        } else {
            Vec::new()
        };
        series.push(ChartBucket {
            start,
            end,
            mentions,
            source_counts,
            posts,
        });
    }
    Ok(series)
}

fn source_mention_counts(
    connection: &Connection,
    topic: &str,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Result<Vec<SourceMentionCount>> {
    let mut statement = connection.prepare(
        r#"
        SELECT p.source, COUNT(*) AS mention_count
        FROM posts p
        JOIN post_topics pt ON pt.post_id = p.id
        WHERE pt.topic_id = ?1 AND p.published_at >= ?2 AND p.published_at < ?3
        GROUP BY p.source
        ORDER BY mention_count DESC, p.source
        "#,
    )?;
    Ok(statement
        .query_map(params![topic, start.timestamp(), end.timestamp()], |row| {
            Ok(SourceMentionCount {
                source: row.get(0)?,
                count: row.get::<_, i64>(1)? as usize,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}

fn chart_bucket_posts(
    connection: &Connection,
    topic: &str,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Result<Vec<ChartPost>> {
    let mut statement = connection.prepare(
        r#"
        SELECT p.source, p.title, p.url,
               CAST((
                   SELECT COUNT(*) FROM posts peers
                   WHERE peers.source = p.source
                     AND peers.published_at >= ?4 AND peers.published_at <= ?5
                     AND peers.points <= p.points
               ) AS REAL) / NULLIF((
                   SELECT COUNT(*) FROM posts peers
                   WHERE peers.source = p.source
                     AND peers.published_at >= ?4 AND peers.published_at <= ?5
               ), 0) AS points_percentile
        FROM posts p
        JOIN post_topics pt ON pt.post_id = p.id
        WHERE pt.topic_id = ?1 AND p.published_at >= ?2 AND p.published_at < ?3
        ORDER BY p.published_at DESC, points_percentile DESC
        LIMIT 2
        "#,
    )?;
    Ok(statement
        .query_map(
            params![
                topic,
                start.timestamp(),
                end.timestamp(),
                (end - Duration::hours(SOURCE_WEIGHT_WINDOW_HOURS)).timestamp(),
                end.timestamp()
            ],
            |row| {
                Ok(ChartPost {
                    source: row.get(0)?,
                    title: row.get(1)?,
                    url: row.get(2)?,
                })
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}

fn velocity_score(mentions_1h: f64, mentions_6h: f64, mentions_24h: f64) -> f64 {
    mentions_1h * VELOCITY_1H_WEIGHT
        + mentions_6h * VELOCITY_6H_WEIGHT
        + mentions_24h * VELOCITY_24H_WEIGHT
}

fn trend_score(velocity: f64, z_score: f64, source_count: usize) -> f64 {
    velocity * (1.0 + z_score.max(0.0) / Z_BOOST_DIVISOR)
        + source_count as f64 / SOURCE_DIVERSITY_DIVISOR
}

fn rank_score(trend_score: f64, interest_affinity: f64) -> f64 {
    trend_score * interest_affinity
}

fn bucket_series(
    connection: &Connection,
    topic: &str,
    now: DateTime<Utc>,
    buckets: usize,
    bucket_hours: usize,
) -> Result<Vec<usize>> {
    (0..buckets)
        .rev()
        .map(|offset| {
            let end = now - Duration::hours((offset * bucket_hours) as i64);
            let start = end - Duration::hours(bucket_hours as i64);
            count_mentions(connection, topic, start, end)
        })
        .collect()
}

fn topic_sources_since(
    connection: &Connection,
    topic: &str,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Result<Vec<String>> {
    let mut statement = connection.prepare(
        r#"
        SELECT DISTINCT p.source
        FROM posts p
        JOIN post_topics pt ON pt.post_id = p.id
        WHERE pt.topic_id = ?1
          AND p.published_at >= ?2
          AND p.published_at <= ?3
        ORDER BY p.source
        "#,
    )?;
    Ok(statement
        .query_map(params![topic, start.timestamp(), end.timestamp()], |row| {
            row.get::<_, String>(0)
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn extract_topics(title: &str, tags: &[String]) -> Vec<String> {
    extract_topic_matches(title, tags)
        .into_iter()
        .map(|(topic, _)| topic)
        .collect()
}

pub fn extract_topic_matches(title: &str, tags: &[String]) -> Vec<(String, String)> {
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
    let mut topics = BTreeMap::new();
    for (topic, aliases) in KNOWN {
        if let Some(alias) = aliases
            .iter()
            .filter(|alias| title_lower.contains(*alias) || tag_text.contains(*alias))
            .max_by_key(|alias| alias.len())
        {
            topics.insert((*topic).to_owned(), (*alias).to_owned());
        }
    }

    if topics.is_empty() {
        let stop_words = STOP_WORDS.iter().copied().collect::<HashSet<_>>();
        for word in title_lower
            .split(|character: char| !character.is_alphanumeric() && character != '-')
            .filter(|word| word.len() >= 5 && !stop_words.contains(*word))
            .take(2)
        {
            topics.insert(slugify(word), word.to_owned());
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
    #[serde(default)]
    summary: String,
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
            summary: self.summary,
            tags: self.tags,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BASELINE_BUCKET_HOURS, BASELINE_BUCKETS, BASELINE_STDDEV_FLOOR, PulseEngine,
        SOURCE_DIVERSITY_DIVISOR, SOURCE_WEIGHT_MAX, SOURCE_WEIGHT_MIN, VELOCITY_1H_WEIGHT,
        VELOCITY_6H_WEIGHT, VELOCITY_24H_WEIGHT, Z_BOOST_DIVISOR, compute_topic_score,
        methodology_copy, rank_breakdown_copy, rank_score, source_weights_from_counts, trend_score,
        velocity_score,
    };
    use crate::domain::{CommunityPost, DigestCard, SourceWeight};
    use chrono::{Duration, TimeZone, Utc};
    use std::collections::BTreeMap;

    #[test]
    fn methodology_formula_and_steps_use_the_scorers_constants() {
        let copy = methodology_copy(8, &[]);

        assert!(copy.formula.contains(&format!("z⁺/{Z_BOOST_DIVISOR:.0}")));
        assert!(
            copy.formula
                .contains(&format!("sources/{SOURCE_DIVERSITY_DIVISOR:.0}"))
        );
        assert!(
            copy.score
                .contains(&format!("1h × {VELOCITY_1H_WEIGHT:.0}"))
        );
        assert!(copy.score.contains(&format!("6h × {VELOCITY_6H_WEIGHT}")));
        assert!(copy.score.contains(&format!("24h × {VELOCITY_24H_WEIGHT}")));
        assert!(copy.score.contains(&format!(
            "{BASELINE_BUCKETS} prior {BASELINE_BUCKET_HOURS}h windows"
        )));
        assert!(
            copy.score
                .contains(&format!("σ floored at {BASELINE_STDDEV_FLOOR:.0}"))
        );
        assert!(copy.surface.contains("at most 8"));
        assert!(!copy.surface.contains("at most 5"));
    }

    #[test]
    fn worked_example_recomputes_to_the_displayed_rank() {
        let velocity = velocity_score(2.0, 5.0, 9.0);
        let trend = trend_score(velocity, 1.9, 3);
        let interest = 1.10;
        let rank = rank_score(trend, interest);
        let card = DigestCard {
            id: "wasm-runtimes".to_owned(),
            topic: "WASM runtimes".to_owned(),
            headline: String::new(),
            headline_url: String::new(),
            headline_source: String::new(),
            headline_summary: String::new(),
            sources: vec!["HN".to_owned(), "Lobsters".to_owned(), "PH".to_owned()],
            score: rank,
            trend_score: trend,
            interest_affinity: interest,
            baseline_mean: 0.0,
            baseline_stddev: 1.0,
            weighted_mentions_1h: 2.0,
            weighted_mentions_6h: 5.0,
            weighted_mentions_24h: 9.0,
            weighted_baseline_mean: 0.0,
            weighted_baseline_stddev: 1.0,
            z_score: 1.9,
            mentions_1h: 2,
            mentions_6h: 5,
            mentions_24h: 9,
            sparkline: Vec::new(),
            chart_buckets: Vec::new(),
        };

        let breakdown = rank_breakdown_copy(&card);
        assert_eq!(
            format!("{:.1}", breakdown.recomputed_rank),
            format!("{:.1}", card.score)
        );
        assert_eq!(
            format!("{:.1}", breakdown.recomputed_trend),
            format!("{:.1}", card.trend_score)
        );
        assert_eq!(
            breakdown.rank_line,
            format!("rank {rank:.1} = trend {trend:.1} × interest {interest:.2}")
        );
        assert!(breakdown.trend_line.contains("+ sources 1.5"));
    }

    #[test]
    fn extraction_records_the_alias_that_explains_a_topic_match() {
        let matches = super::extract_topic_matches("Wasmtime reaches production", &[]);
        assert!(matches.contains(&("wasm-runtimes".to_owned(), "wasmtime".to_owned())));
    }

    #[test]
    fn source_weights_use_rolling_volume_and_clamp_both_bounds() {
        let counts = BTreeMap::from([
            ("A".to_owned(), 1),
            ("B".to_owned(), 1),
            ("C".to_owned(), 1),
            ("D".to_owned(), 1),
            ("Whale".to_owned(), 1_000),
        ]);

        let weights = source_weights_from_counts(&counts);
        assert_eq!(
            weights
                .iter()
                .find(|item| item.source == "A")
                .unwrap()
                .weight,
            SOURCE_WEIGHT_MAX
        );
        assert_eq!(
            weights
                .iter()
                .find(|item| item.source == "Whale")
                .unwrap()
                .weight,
            SOURCE_WEIGHT_MIN
        );
    }

    #[test]
    fn fixture_weights_are_live_visible_and_shared_with_methodology() {
        let now = Utc.with_ymd_and_hms(2026, 7, 22, 12, 0, 0).unwrap();
        let mut engine = PulseEngine::in_memory().unwrap();
        engine.load_fixture(now).unwrap();

        let weights = engine.source_weights(now).unwrap();
        assert!(weights.iter().any(|item| (item.weight - 1.0).abs() > 0.01));
        let copy = methodology_copy(5, &weights);
        assert_eq!(copy.source_weights, weights);
        for source in &weights {
            assert!(copy.score.contains(&format!("×{:.2}", source.weight)));
        }
    }

    #[test]
    fn all_one_weights_reproduce_the_raw_z_score() {
        let now = Utc.with_ymd_and_hms(2026, 7, 22, 12, 0, 0).unwrap();
        let mut engine = PulseEngine::in_memory().unwrap();
        let mut posts = vec![
            post(
                "current-1",
                "HN",
                "Rust current",
                now - Duration::minutes(10),
                &["rust"],
            ),
            post(
                "current-2",
                "HN",
                "Rust current",
                now - Duration::minutes(20),
                &["rust"],
            ),
        ];
        for bucket in 1..=BASELINE_BUCKETS {
            posts.push(post(
                &format!("baseline-{bucket}"),
                "HN",
                "Rust baseline",
                now - Duration::hours(BASELINE_BUCKET_HOURS * bucket as i64) - Duration::minutes(1),
                &["rust"],
            ));
        }
        engine.ingest(&posts).unwrap();
        let weights = vec![SourceWeight {
            source: "HN".to_owned(),
            average_daily_posts: 1.0,
            weight: 1.0,
        }];

        let score = compute_topic_score(&engine.connection, "rust", "Rust", now, &weights).unwrap();
        let old_z = (score.mentions_6h as f64 - score.baseline_mean)
            / score.baseline_stddev.max(BASELINE_STDDEV_FLOOR);
        assert!((score.z_score - old_z).abs() < 1e-12);
        assert_eq!(score.weighted_mentions_6h, score.mentions_6h as f64);
        assert_eq!(score.weighted_baseline_mean, score.baseline_mean);
    }

    #[test]
    fn equal_per_capita_activity_ranks_equally_across_sources() {
        let now = Utc.with_ymd_and_hms(2026, 7, 22, 12, 0, 0).unwrap();
        let mut engine = PulseEngine::in_memory().unwrap();
        let mut posts = Vec::new();
        for index in 0..4 {
            posts.push(post(
                &format!("a-rust-{index}"),
                "A",
                "Rust activity",
                now - Duration::minutes(10 + index),
                &["rust"],
            ));
            posts.push(post(
                &format!("a-bg-{index}"),
                "A",
                "Background alpha",
                now - Duration::hours(48 + index),
                &[],
            ));
        }
        posts.push(post(
            "b-privacy",
            "B",
            "Privacy activity",
            now - Duration::minutes(10),
            &["privacy"],
        ));
        posts.push(post(
            "b-bg",
            "B",
            "Background beta",
            now - Duration::hours(48),
            &[],
        ));
        engine.ingest(&posts).unwrap();
        let weights = engine.source_weights(now).unwrap();

        let rust = compute_topic_score(&engine.connection, "rust", "Rust", now, &weights).unwrap();
        let privacy =
            compute_topic_score(&engine.connection, "privacy", "Privacy", now, &weights).unwrap();
        assert!((rust.weighted_mentions_6h - privacy.weighted_mentions_6h).abs() < 1e-12);
        assert!((rust.trend_score - privacy.trend_score).abs() < 1e-12);
    }

    fn post(
        id: &str,
        source: &str,
        title: &str,
        published_at: chrono::DateTime<Utc>,
        tags: &[&str],
    ) -> CommunityPost {
        CommunityPost {
            id: id.to_owned(),
            source: source.to_owned(),
            title: title.to_owned(),
            url: format!("https://example.com/{id}"),
            author: "tester".to_owned(),
            published_at,
            points: 1,
            summary: String::new(),
            tags: tags.iter().map(|tag| (*tag).to_owned()).collect(),
        }
    }
}
