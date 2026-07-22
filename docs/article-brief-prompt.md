# Community Pulse article brief

Research the specific Community Pulse evidence item below and submit an article-level brief.

- Topic: `{{topic}}`
- Exact article/post URL: `{{article_url}}`
- MCP endpoint: `http://127.0.0.1:{{port}}/mcp`

Use the Community Pulse MCP tools to inspect the topic and its evidence. Fetch and read both the article/post at the exact URL and the associated community discussion. For self-posts or thread-first sources, treat the post body as the article and still inspect the full discussion.

Write a concise brief with these five sections, in this order:

1. What it is
2. Substance
3. Community reaction
4. Credibility and caveats
5. What is driving the trend and what to watch

Every factual claim and every quoted reaction must be traceable to an exact direct URL. Link to the original article location, discussion thread, or individual comment/permalink as precisely as the source permits. Do not invent anchors or use a community home page as evidence. Clearly label any synthesis that is inference rather than a sourced claim.

Call `submit_research` exactly once with:

- `topic_id` set to `{{topic}}` and `article_url` set exactly to `{{article_url}}`.
- A useful title and the complete markdown brief.
- `citations` containing every exact article, thread, and comment URL used by the brief.
- A summary of at most 140 characters.
- Structured `sections` in addition to markdown, always. Use the canonical kinds `what`, `substance`, `reaction`, `credibility`, and `watch`; include a body for each applicable section.
- Quotes only when they add evidence. Each structured quote must include its exact source URL, contain at most 280 characters, and appear in the section it supports. Include at most three quotes per section. Every quote URL must also be present in `citations`.
- `web_report` when available: publish a Claude artifact and submit its URL. If artifact publishing is unavailable—or when running Codex—write self-contained HTML under `research/reports/` and submit its absolute path or `file://` URL. Never fabricate a report location.

The markdown is the compatibility fallback and must remain complete even when structured `sections` are supplied. This is a user-initiated brief for one evidence item; do not batch other articles, alter ranking, or submit topic-level enrichment as part of this run.
