# Community Pulse research task

Research `{{topic}}` using the pulse MCP endpoint at
`http://127.0.0.1:{{port}}/mcp`.

1. Use `get_series` and `topic_posts` for the topic.
2. Read the top source threads with your web tools.
3. Explain what is actually happening, who is affected, whether the spike is
   organic (source diversity and velocity shape), and what to watch next.
4. Submit a report with citations through `submit_research`. Always include:
   - `verdict`: `organic`, `manufactured`, or `unclear`.
   - `summary`: one card-ready sentence of at most 140 characters.
   - `watch`: up to three topic slugs worth watching next.
   Keep the markdown report under 400 words.
5. Always produce a self-contained web report before calling `submit_research`.
   Claude publishes a private `https://claude.ai/...` Artifact and submits its
   URL as `web_report`. Codex writes HTML below `research/reports/` and submits
   its absolute path as `web_report`.
6. When a structured section includes a chart, pass the charted numbers through
   `series: { label, points, baseline? }`; `get_series` output can be used
   directly. Do not submit a chart screenshot. Optional figures must be saved
   below `research/reports/assets/` and submitted as
   `images: [{ path, caption }]`.

Do not make the report public. It remains private, agent-attributed research for
the user to review.
