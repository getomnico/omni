# Google Ads Skill

Use this skill when the user asks about Google Ads campaigns, ad groups, ads, keywords, budgets, search terms, conversion performance, exports, or account diagnostics.

## Core Principle

Google Ads in Omni has two kinds of data:

1. **Indexed structure/configuration** — searchable Omni documents for durable, non-metric account structure.
2. **Live reports/actions** — fresh Google Ads API data for metrics, time series, exports, and analysis.

Do **not** treat indexed Google Ads documents as a source of performance metrics. Metrics such as impressions, clicks, cost, CTR, CPC, conversions, conversion value, CPA, ROAS, and impression share must come from live Google Ads actions/reports.

## Read-Only Scope

Only perform read/analysis workflows. Do not create, update, pause, enable, delete, mutate, or recommend executing mutations unless the user explicitly asks for a future implementation plan. If a query or action would mutate Google Ads, refuse and offer a read-only alternative.

## First Steps for Analysis

1. Identify the relevant Google Ads source/customer/campaign context.
2. If the user has not specified a date range, ask for one or choose a sensible default and state it clearly (for example, last 30 days with prior 30-day comparison).
3. Clarify the desired grain and dimensions when needed:
   - account/customer
   - campaign
   - ad group
   - ad/ad creative
   - keyword
   - search term
   - landing page/final URL
   - device, network, geo, date/week/month/hour, audience, demographic
4. Use indexed docs for structure discovery and live reports for metrics.
5. Summarize findings with customer ID, date range, filters, row count, and caveats.

## Use Indexed Google Ads Documents For

Search indexed docs when the question is about setup, names, statuses, relationships, or configuration:

- customers/accounts
- campaigns
- campaign budgets
- ad groups
- ads and responsive search ad text
- assets
- keyword criteria
- conversion actions
- user lists/audiences where indexed
- recommendations where indexed
- labels and resource IDs where available

Useful search filters/operators may include:

- `in:google_ads`
- `customer:<customer_id>`
- `campaign:<campaign_id>`
- `ad_group:<ad_group_id>`
- `status:<status>`
- `channel:<channel_type>`
- `entity:<entity_type>`
- `label:<label_resource_or_name>`

Examples:

- `in:google_ads entity:campaign paused campaigns`
- `in:google_ads customer:1234567890 entity:keyword_view brand keywords`
- `in:google_ads campaign:111222333 ads final urls`

## Use Live Google Ads Actions/Reports For

Use live Google Ads actions when the user asks for performance, trends, exports, or metrics:

- campaign/ad group/ad/keyword performance
- search term mining
- budget pacing
- conversions, CPA, ROAS, conversion value
- CTR/CPC/cost/click/impression trends
- device/network/geo/hour/day/week/month segmentation
- landing page performance
- auction/impression-share analysis
- change history and performance anomaly debugging
- CSV/XLSX exports for spreadsheet analysis

Prefer curated report actions when available. Use raw GAQL only when a curated action does not cover the requested workflow.

## GAQL Guidance

When using raw GAQL:

- Use only `SELECT` queries.
- Include an explicit date filter or `segments.date DURING ...` for metric reports.
- Keep resources and fields compatible; Google Ads rejects incompatible field combinations.
- Include only fields needed for the analysis to keep exports manageable.
- Use row limits and explain if the output may be sampled/truncated by a limit.
- Do not include mutation operations or multiple statements.

Common date clauses:

```sql
WHERE segments.date DURING LAST_30_DAYS
```

```sql
WHERE segments.date BETWEEN '2026-01-01' AND '2026-01-31'
```

Campaign performance shape:

```sql
SELECT
  segments.date,
  campaign.id,
  campaign.name,
  campaign.status,
  metrics.impressions,
  metrics.clicks,
  metrics.cost_micros,
  metrics.conversions,
  metrics.conversions_value
FROM campaign
WHERE segments.date DURING LAST_30_DAYS
ORDER BY metrics.cost_micros DESC
```

Search term mining shape:

```sql
SELECT
  campaign.id,
  campaign.name,
  ad_group.id,
  ad_group.name,
  search_term_view.search_term,
  metrics.impressions,
  metrics.clicks,
  metrics.cost_micros,
  metrics.conversions,
  metrics.conversions_value
FROM search_term_view
WHERE segments.date DURING LAST_30_DAYS
ORDER BY metrics.cost_micros DESC
```

## Export Workflow

For CSV/XLSX exports:

1. Confirm customer ID, date range, dimensions, metrics, filters, and desired format.
2. Generate the report live from Google Ads.
3. For spreadsheet work, load the `excel` skill before inspecting or transforming an XLSX file.
4. Include a short explanation of exported columns, row count, date range, customer ID, and any limits.
5. Avoid huge downloads. Prefer focused dimensions/date ranges and respect connector row caps; ask the user to narrow scope if they need more than the export limit.

## Analysis Patterns

### Campaign health triage

Look for campaigns with:

- high cost and low/no conversions
- declining impressions or clicks
- low CTR relative to account/campaign peers
- high CPC or CPA
- budget-limited behavior
- paused/removed/closed statuses
- serving or policy issues
- recent changes around an anomaly window

### Budget pacing

Compare spend to expected pacing for the selected period. Check campaign-budget relationships, daily budgets, cost by day, and conversion outcomes. Flag budgets with high spend/no conversions and budgets with little delivery.

### Search term mining

Sort search terms by cost, clicks, conversions, and conversion value. Identify:

- expensive terms with no conversions
- high-converting queries for keyword expansion
- irrelevant queries for negative keyword review
- query themes that differ from campaign intent

Only suggest candidates for analyst review; do not add negatives or keywords.

### Creative analysis

Compare ads/assets by impressions, clicks, CTR, conversions, and cost. Include the ad text/headlines/descriptions from indexed structure when explaining likely drivers. Check status/policy fields where available.

### Root-cause analysis

For sudden changes:

1. Establish metric trend and anomaly date.
2. Segment by campaign/ad group/device/network/search term if needed.
3. Compare before/after windows.
4. Check indexed statuses/configuration and live change history if available.
5. Summarize likely causes and evidence.

## Reporting Caveats

Always mention relevant caveats:

- Google Ads account timezone and currency matter.
- Date ranges in Google Ads reports use the account timezone.
- `cost_micros` must be divided by 1,000,000 for currency units.
- Some metrics are attribution-window dependent and may change retroactively.
- Different Google Ads UI reports may use different filters, conversion columns, or attribution settings.
- Multi-account/MCC rollups may involve different currencies/timezones.

## Response Style

Be analyst-friendly and evidence-driven:

- State assumptions and filters.
- Show concise tables for top movers, winners, and problem areas.
- Include formulas for derived metrics when useful.
- Recommend next read-only checks or exports.
- Separate facts from hypotheses.
