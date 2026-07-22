use crate::domain::{
    CommunityPost, DEFAULT_BUDGET, DigestCard, EvidencePost, InterestModel, MAX_BUDGET,
    ResearchPost, ResearchReport, ResearchSeries, ResearchSubmission, ResearchTopic, TopicScore,
    TrendEvidence,
};
use anyhow::{Context as _, Result, bail};
use chrono::{DateTime, Duration, Utc};
use rusqlite::types::Type;
use rusqlite::{Connection, OptionalExtension, Row, Transaction, params};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;

pub struct PulseEngine {
    connection: Connection,
}

type HeadlineCandidate = (String, String, String, String);
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
        let requested = limit.unwrap_or(self.budget()?).clamp(1, MAX_BUDGET);
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
                headline_url: String::new(),
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
                .map(|(_, _, source, _)| source.clone())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            if let Some((_, title, _, url)) = candidates
                .into_iter()
                .find(|(post_id, _, _, _)| used_posts.insert(post_id.clone()))
            {
                card.headline = title;
                card.headline_url = url;
            } else {
                card.headline = format!("{} is gaining attention", card.topic);
            }
            card.sparkline = hourly_series(&self.connection, &card.id, now, 12)?;
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

    fn headline_candidates(&self, topic: &str) -> Result<HeadlineCandidates> {
        let mut statement = self.connection.prepare(
            r#"
                SELECT p.id, p.title, p.source, p.url
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
                row.get::<_, String>(3)?,
            ))
        })?;
        let mut candidates = Vec::new();
        for row in rows {
            let (post_id, title, source, url) = row?;
            candidates.push((post_id, title, source, url));
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
            SELECT p.source, p.title, p.url, p.points, p.published_at
            FROM posts p
            JOIN post_topics pt ON pt.post_id = p.id
            WHERE pt.topic_id = ?1
              AND p.published_at >= ?2
              AND p.published_at <= ?3
            ORDER BY p.published_at DESC, p.points DESC
            LIMIT ?4
            "#,
        )?;
        Ok(statement
            .query_map(
                params![
                    topic,
                    (now - Duration::hours(window_hours as i64)).timestamp(),
                    now.timestamp(),
                    limit as i64
                ],
                |row| {
                    Ok(ResearchPost {
                        source: row.get(0)?,
                        title: row.get(1)?,
                        url: row.get(2)?,
                        points: row.get(3)?,
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
    bucket_series(connection, topic, now, buckets, 1)
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
