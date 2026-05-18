//! W37: built-in source catalog. The catalog is a static slice of
//! [`ExternalSource`] data — adding or revising an entry is a code
//! change so the review status, terms URL, and pipeline shape stay
//! version-controlled.
//!
//! Run-time gating lives in [`crate::commands::external_source`]; this
//! module only owns the seed itself.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use crate::models::external_source::{
    ExternalSource, ExternalSourceAdapter, ExternalSourceCredentialPolicy, ExternalSourceDomain,
    ExternalSourceHttpRequest, ExternalSourceParam, ExternalSourceRateLimit,
    ExternalSourceReviewStatus, McpInstallEnvHint, McpInstallRecommendation,
};
use crate::models::pipeline::PipelineStep;

fn http_get(url: &str) -> ExternalSourceHttpRequest {
    ExternalSourceHttpRequest {
        method: "GET".to_string(),
        url: url.to_string(),
        query: BTreeMap::new(),
        headers: BTreeMap::new(),
        credential_header: None,
        credential_prefix: None,
        body_params: Vec::new(),
    }
}

fn with_query(
    mut req: ExternalSourceHttpRequest,
    pairs: &[(&str, &str)],
) -> ExternalSourceHttpRequest {
    for (key, value) in pairs {
        req.query.insert((*key).to_string(), (*value).to_string());
    }
    req
}

fn with_header(
    mut req: ExternalSourceHttpRequest,
    key: &str,
    value: &str,
) -> ExternalSourceHttpRequest {
    req.headers.insert(key.to_string(), value.to_string());
    req
}

fn required_param(name: &str, description: &str) -> ExternalSourceParam {
    ExternalSourceParam {
        name: name.to_string(),
        description: description.to_string(),
        schema: None,
        required: true,
        default: None,
    }
}

fn optional_param(name: &str, description: &str, schema: serde_json::Value) -> ExternalSourceParam {
    ExternalSourceParam {
        name: name.to_string(),
        description: description.to_string(),
        schema: Some(schema),
        required: false,
        default: None,
    }
}

/// W37++: empty HTTP request stub for non-HTTP adapters (McpRecommended).
fn http_unused() -> ExternalSourceHttpRequest {
    ExternalSourceHttpRequest {
        method: "GET".to_string(),
        url: String::new(),
        query: BTreeMap::new(),
        headers: BTreeMap::new(),
        credential_header: None,
        credential_prefix: None,
        body_params: Vec::new(),
    }
}

fn build_catalog() -> Vec<ExternalSource> {
    vec![
        // Hacker News via the Algolia search API. Keyless, CORS-open,
        // documented at https://hn.algolia.com/api. Pipeline trims the
        // huge hit payload to the columns chat actually needs.
        ExternalSource {
            id: "hacker_news_search".to_string(),
            display_name: "Hacker News Search".to_string(),
            description: "Search Hacker News stories/comments via the Algolia HN API. Keyless, public, rate-limited by Algolia.".to_string(),
            domain: ExternalSourceDomain::News,
            adapter: ExternalSourceAdapter::HttpJson,
            review_status: ExternalSourceReviewStatus::AllowedWithConditions,
            review_date: "2026-05-17".to_string(),
            adapter_license: "native".to_string(),
            terms_url: "https://hn.algolia.com/api".to_string(),
            review_notes: "Keyless public API. No write tools. Rate limits shared across users — handle 429 explicitly.".to_string(),
            attribution: Some("Data via Hacker News (news.ycombinator.com) and Algolia.".to_string()),
            credential_policy: ExternalSourceCredentialPolicy::None,
            credential_help: None,
            http: with_query(http_get("https://hn.algolia.com/api/v1/search"), &[
                ("query", "{query}"),
                ("hitsPerPage", "{limit}"),
                ("tags", "{tags}"),
            ]),
            params: vec![
                required_param("query", "Free-text search query (e.g. 'rust async runtime')."),
                optional_param(
                    "limit",
                    "Maximum hits to return (1-50, default 10).",
                    serde_json::json!({ "type": "integer", "minimum": 1, "maximum": 50 }),
                ),
                optional_param(
                    "tags",
                    "Optional Algolia tag filter (e.g. 'story', 'comment', 'front_page').",
                    serde_json::json!({ "type": "string" }),
                ),
            ],
            default_pipeline: vec![
                PipelineStep::Pick { path: "hits".to_string() },
                PipelineStep::Map {
                    fields: vec![
                        "title".to_string(),
                        "url".to_string(),
                        "points".to_string(),
                        "author".to_string(),
                        "num_comments".to_string(),
                        "created_at".to_string(),
                        "objectID".to_string(),
                    ],
                    rename: BTreeMap::new(),
                },
            ],
            rate_limit: Some(ExternalSourceRateLimit {
                plan_name: "Keyless".to_string(),
                free_quota: "Best-effort public — Algolia throttles by IP.".to_string(),
                paid_tier: None,
                queries_per_second: None,
                attribution_required: false,
                storage_rights_required: false,
            }),
            mcp_install: None,
        },
        // Wikipedia REST summary endpoint. Single-page lookup, Apache 2.0
        // license on the API metadata, CC BY-SA on the content body.
        ExternalSource {
            id: "wikipedia_summary".to_string(),
            display_name: "Wikipedia Summary".to_string(),
            description: "Fetch the summary card for a Wikipedia article via the public REST API.".to_string(),
            domain: ExternalSourceDomain::KnowledgeBase,
            adapter: ExternalSourceAdapter::HttpJson,
            review_status: ExternalSourceReviewStatus::AllowedWithConditions,
            review_date: "2026-05-17".to_string(),
            adapter_license: "native".to_string(),
            terms_url: "https://en.wikipedia.org/api/rest_v1/".to_string(),
            review_notes: "Keyless. Content licensed CC BY-SA — attribution required in UI when rendered.".to_string(),
            attribution: Some("Content from Wikipedia, available under CC BY-SA 4.0.".to_string()),
            credential_policy: ExternalSourceCredentialPolicy::None,
            credential_help: None,
            http: http_get("https://{lang}.wikipedia.org/api/rest_v1/page/summary/{title}"),
            params: vec![
                required_param("title", "Article title (URL-encoded automatically). Example: 'Rust_(programming_language)'."),
                optional_param(
                    "lang",
                    "Wikipedia language code (default 'en').",
                    serde_json::json!({ "type": "string", "default": "en" }),
                ),
            ],
            default_pipeline: vec![
                PipelineStep::Map {
                    fields: vec![
                        "title".to_string(),
                        "description".to_string(),
                        "extract".to_string(),
                        "content_urls".to_string(),
                        "timestamp".to_string(),
                    ],
                    rename: BTreeMap::new(),
                },
            ],
            rate_limit: Some(ExternalSourceRateLimit {
                plan_name: "Keyless".to_string(),
                free_quota: "200 req/s soft cap, fair-use".to_string(),
                paid_tier: None,
                queries_per_second: Some(200.0),
                attribution_required: true,
                storage_rights_required: false,
            }),
            mcp_install: None,
        },
        // CoinGecko public price endpoint. Keyless beta mode for quick
        // lookups; for production use the user should configure a key.
        ExternalSource {
            id: "coingecko_price".to_string(),
            display_name: "CoinGecko Spot Price".to_string(),
            description: "Look up current price for one or more coins via CoinGecko's public /simple/price endpoint.".to_string(),
            domain: ExternalSourceDomain::CryptoMarket,
            adapter: ExternalSourceAdapter::HttpJson,
            review_status: ExternalSourceReviewStatus::AllowedWithConditions,
            review_date: "2026-05-17".to_string(),
            adapter_license: "native".to_string(),
            terms_url: "https://www.coingecko.com/en/api/documentation".to_string(),
            review_notes: "Public keyless tier has aggressive rate limits and is best-effort. Use BYOK for sustained workloads.".to_string(),
            attribution: Some("Market data via CoinGecko.".to_string()),
            credential_policy: ExternalSourceCredentialPolicy::Optional,
            credential_help: Some("Optional CoinGecko API key (Demo or Pro). Stored locally; sent as the x-cg-demo-api-key header.".to_string()),
            http: ExternalSourceHttpRequest {
                method: "GET".to_string(),
                url: "https://api.coingecko.com/api/v3/simple/price".to_string(),
                query: {
                    let mut m = BTreeMap::new();
                    m.insert("ids".to_string(), "{ids}".to_string());
                    m.insert("vs_currencies".to_string(), "{vs_currencies}".to_string());
                    m.insert("include_24hr_change".to_string(), "true".to_string());
                    m
                },
                headers: BTreeMap::new(),
                credential_header: Some("x-cg-demo-api-key".to_string()),
                credential_prefix: None,
                body_params: Vec::new(),
            },
            params: vec![
                required_param(
                    "ids",
                    "Comma-separated CoinGecko coin ids (e.g. 'bitcoin,ethereum').",
                ),
                optional_param(
                    "vs_currencies",
                    "Comma-separated fiat/crypto quote currencies (default 'usd').",
                    serde_json::json!({ "type": "string", "default": "usd" }),
                ),
            ],
            default_pipeline: Vec::new(),
            rate_limit: Some(ExternalSourceRateLimit {
                plan_name: "Demo / Pro".to_string(),
                free_quota: "Public ~30 req/min unauthenticated; Demo key 50 req/min.".to_string(),
                paid_tier: Some("Pro tiers from $129/mo (500 req/min)".to_string()),
                queries_per_second: Some(0.83),
                attribution_required: true,
                storage_rights_required: false,
            }),
            mcp_install: None,
        },
        // GitHub public repo metadata. Read-only.
        ExternalSource {
            id: "github_public_repo".to_string(),
            display_name: "GitHub Public Repo".to_string(),
            description: "Fetch metadata for a public GitHub repository (stars, forks, default branch, license, latest push).".to_string(),
            domain: ExternalSourceDomain::DeveloperData,
            adapter: ExternalSourceAdapter::HttpJson,
            review_status: ExternalSourceReviewStatus::AllowedWithConditions,
            review_date: "2026-05-17".to_string(),
            adapter_license: "native".to_string(),
            terms_url: "https://docs.github.com/en/rest".to_string(),
            review_notes: "Read-only access to public repos via the REST API. Unauthenticated calls are rate-limited (60/hour). BYOK PAT recommended.".to_string(),
            attribution: None,
            credential_policy: ExternalSourceCredentialPolicy::Optional,
            credential_help: Some("Optional GitHub personal access token (classic or fine-grained, read-only). Sent as 'Authorization: Bearer <token>'.".to_string()),
            http: with_header(
                ExternalSourceHttpRequest {
                    method: "GET".to_string(),
                    url: "https://api.github.com/repos/{owner}/{repo}".to_string(),
                    query: BTreeMap::new(),
                    headers: BTreeMap::new(),
                    credential_header: Some("Authorization".to_string()),
                    credential_prefix: Some("Bearer ".to_string()),
                    body_params: Vec::new(),
                },
                "Accept",
                "application/vnd.github+json",
            ),
            params: vec![
                required_param("owner", "Repository owner / org login."),
                required_param("repo", "Repository name."),
            ],
            default_pipeline: vec![
                PipelineStep::Map {
                    fields: vec![
                        "full_name".to_string(),
                        "description".to_string(),
                        "stargazers_count".to_string(),
                        "forks_count".to_string(),
                        "open_issues_count".to_string(),
                        "default_branch".to_string(),
                        "license".to_string(),
                        "pushed_at".to_string(),
                        "html_url".to_string(),
                    ],
                    rename: BTreeMap::new(),
                },
            ],
            rate_limit: Some(ExternalSourceRateLimit {
                plan_name: "Unauthenticated / Personal Access Token".to_string(),
                free_quota: "60 req/hour anonymous, 5 000 req/hour with PAT.".to_string(),
                paid_tier: Some("GitHub Enterprise raises caps; Apps allow 15 000 req/hour.".to_string()),
                queries_per_second: Some(1.0),
                attribution_required: false,
                storage_rights_required: false,
            }),
            mcp_install: None,
        },
        // Brave Search — kept as needs_review. Feb-2026 repricing
        // dropped the free tier; everyone is metered with a credit
        // card on file. Catalog row records the facts but execution
        // stays disabled.
        ExternalSource {
            id: "brave_search_web".to_string(),
            display_name: "Brave Search (Web)".to_string(),
            description: "Brave Search Web API. Metered since Feb-2026; requires a credit card on file plus an API key. Kept needs_review until Datrina can render plan tiers + storage-rights affirmations inline.".to_string(),
            domain: ExternalSourceDomain::WebSearch,
            adapter: ExternalSourceAdapter::HttpJson,
            review_status: ExternalSourceReviewStatus::NeedsReview,
            review_date: "2026-05-17".to_string(),
            adapter_license: "native".to_string(),
            terms_url: "https://api-dashboard.search.brave.com/documentation/pricing".to_string(),
            review_notes: "Feb-2026 repricing: no free tier, $5/mo credit (~1k requests) then $5 per 1 000 Search requests, $4 per 1 000 Answers + $5 per million tokens. Free credit requires public attribution. Storing results requires an explicit storage-rights plan. Anonymous limit is 1 req/s; Pro AI plan up to 50 req/s. Datrina will keep this `needs_review` until the UI can surface storage-rights opt-in.".to_string(),
            attribution: Some("Search results provided by Brave Search.".to_string()),
            credential_policy: ExternalSourceCredentialPolicy::Required,
            credential_help: Some("Brave Search API key. Stored locally; sent as the X-Subscription-Token header.".to_string()),
            http: ExternalSourceHttpRequest {
                method: "GET".to_string(),
                url: "https://api.search.brave.com/res/v1/web/search".to_string(),
                query: {
                    let mut m = BTreeMap::new();
                    m.insert("q".to_string(), "{q}".to_string());
                    m.insert("count".to_string(), "{count}".to_string());
                    m
                },
                headers: {
                    let mut m = BTreeMap::new();
                    m.insert("Accept".to_string(), "application/json".to_string());
                    m
                },
                credential_header: Some("X-Subscription-Token".to_string()),
                credential_prefix: None,
                body_params: Vec::new(),
            },
            params: vec![
                required_param("q", "Search query."),
                optional_param(
                    "count",
                    "Number of results (1-20, default 10).",
                    serde_json::json!({ "type": "integer", "minimum": 1, "maximum": 20 }),
                ),
            ],
            default_pipeline: vec![PipelineStep::Pick {
                path: "web.results".to_string(),
            }],
            rate_limit: Some(ExternalSourceRateLimit {
                plan_name: "Metered (free tier removed Feb-2026)".to_string(),
                free_quota: "$5/mo credit (~1 000 Search requests). Credit card on file is mandatory.".to_string(),
                paid_tier: Some("$5 per 1 000 Search requests; Answers $4 per 1k + $5 per million tokens; Pro AI plan unlocks 50 req/s.".to_string()),
                queries_per_second: Some(1.0),
                attribution_required: true,
                storage_rights_required: true,
            }),
            mcp_install: None,
        },
        // DefiLlama TVL by protocol. Keyless public API.
        ExternalSource {
            id: "defillama_protocol".to_string(),
            display_name: "DefiLlama Protocol".to_string(),
            description: "Look up TVL, chain breakdown, and metadata for a DeFi protocol via DefiLlama.".to_string(),
            domain: ExternalSourceDomain::CryptoMarket,
            adapter: ExternalSourceAdapter::HttpJson,
            review_status: ExternalSourceReviewStatus::AllowedWithConditions,
            review_date: "2026-05-17".to_string(),
            adapter_license: "native".to_string(),
            terms_url: "https://defillama.com/docs/api".to_string(),
            review_notes: "Keyless public API; data community-contributed and best-effort. No write tools.".to_string(),
            attribution: Some("TVL data via DefiLlama.".to_string()),
            credential_policy: ExternalSourceCredentialPolicy::None,
            credential_help: None,
            http: http_get("https://api.llama.fi/protocol/{protocol}"),
            params: vec![required_param(
                "protocol",
                "Protocol slug (e.g. 'aave', 'lido'). See defillama.com URL slugs.",
            )],
            default_pipeline: vec![PipelineStep::Map {
                fields: vec![
                    "name".to_string(),
                    "category".to_string(),
                    "symbol".to_string(),
                    "url".to_string(),
                    "chains".to_string(),
                    "currentChainTvls".to_string(),
                    "tvl".to_string(),
                ],
                rename: BTreeMap::new(),
            }],
            rate_limit: Some(ExternalSourceRateLimit {
                plan_name: "Keyless public".to_string(),
                free_quota: "No published cap, fair-use community API.".to_string(),
                paid_tier: None,
                queries_per_second: None,
                attribution_required: true,
                storage_rights_required: false,
            }),
            mcp_install: None,
        },
        // MediaWiki Action API search.
        ExternalSource {
            id: "mediawiki_search".to_string(),
            display_name: "MediaWiki Search".to_string(),
            description: "Search a MediaWiki instance (default: en.wikipedia.org) via the Action API.".to_string(),
            domain: ExternalSourceDomain::KnowledgeBase,
            adapter: ExternalSourceAdapter::HttpJson,
            review_status: ExternalSourceReviewStatus::AllowedWithConditions,
            review_date: "2026-05-17".to_string(),
            adapter_license: "native".to_string(),
            terms_url: "https://www.mediawiki.org/wiki/API:Search".to_string(),
            review_notes: "Read-only profile. Content licensed CC BY-SA on Wikipedia — UI must attribute.".to_string(),
            attribution: Some("Search results from MediaWiki / Wikipedia (CC BY-SA 4.0).".to_string()),
            credential_policy: ExternalSourceCredentialPolicy::None,
            credential_help: None,
            http: {
                let mut m = BTreeMap::new();
                m.insert("action".to_string(), "query".to_string());
                m.insert("list".to_string(), "search".to_string());
                m.insert("format".to_string(), "json".to_string());
                m.insert("srsearch".to_string(), "{query}".to_string());
                m.insert("srlimit".to_string(), "{limit}".to_string());
                ExternalSourceHttpRequest {
                    method: "GET".to_string(),
                    url: "https://{host}/w/api.php".to_string(),
                    query: m,
                    headers: BTreeMap::new(),
                    credential_header: None,
                    credential_prefix: None,
                    body_params: Vec::new(),
                }
            },
            params: vec![
                required_param("query", "Search query."),
                optional_param(
                    "limit",
                    "Number of results (1-50, default 10).",
                    serde_json::json!({ "type": "integer", "minimum": 1, "maximum": 50 }),
                ),
                optional_param(
                    "host",
                    "Wiki host (default 'en.wikipedia.org'). Use to point at another MediaWiki.",
                    serde_json::json!({ "type": "string", "default": "en.wikipedia.org" }),
                ),
            ],
            default_pipeline: vec![
                PipelineStep::Pick {
                    path: "query.search".to_string(),
                },
                PipelineStep::Map {
                    fields: vec![
                        "title".to_string(),
                        "snippet".to_string(),
                        "pageid".to_string(),
                        "size".to_string(),
                        "timestamp".to_string(),
                    ],
                    rename: BTreeMap::new(),
                },
            ],
            rate_limit: Some(ExternalSourceRateLimit {
                plan_name: "Keyless".to_string(),
                free_quota: "200 req/s soft cap, fair-use".to_string(),
                paid_tier: None,
                queries_per_second: Some(200.0),
                attribution_required: true,
                storage_rights_required: false,
            }),
            mcp_install: None,
        },
        // Stack Exchange search.
        ExternalSource {
            id: "stackexchange_search".to_string(),
            display_name: "Stack Exchange Search".to_string(),
            description: "Search Stack Exchange sites (Stack Overflow by default) via the public API. Returns question excerpts.".to_string(),
            domain: ExternalSourceDomain::DeveloperData,
            adapter: ExternalSourceAdapter::HttpJson,
            review_status: ExternalSourceReviewStatus::AllowedWithConditions,
            review_date: "2026-05-17".to_string(),
            adapter_license: "native".to_string(),
            terms_url: "https://api.stackexchange.com/docs".to_string(),
            review_notes: "Read-only. Anonymous calls capped at 300/day per IP. User-Agent must identify the caller — Datrina's default UA satisfies this.".to_string(),
            attribution: Some("Content from Stack Exchange (CC BY-SA).".to_string()),
            credential_policy: ExternalSourceCredentialPolicy::Optional,
            credential_help: Some("Optional Stack Exchange API key (raises anonymous quota from 300 to 10 000/day). Stored locally; sent as the `key` query parameter.".to_string()),
            http: {
                let mut m = BTreeMap::new();
                m.insert("intitle".to_string(), "{query}".to_string());
                m.insert("site".to_string(), "{site}".to_string());
                m.insert("pagesize".to_string(), "{limit}".to_string());
                m.insert("order".to_string(), "desc".to_string());
                m.insert("sort".to_string(), "relevance".to_string());
                ExternalSourceHttpRequest {
                    method: "GET".to_string(),
                    url: "https://api.stackexchange.com/2.3/search".to_string(),
                    query: m,
                    headers: BTreeMap::new(),
                    credential_header: None,
                    credential_prefix: None,
                    body_params: Vec::new(),
                }
            },
            params: vec![
                required_param("query", "Search phrase (matches against question title)."),
                optional_param(
                    "site",
                    "Stack Exchange site (default 'stackoverflow').",
                    serde_json::json!({ "type": "string", "default": "stackoverflow" }),
                ),
                optional_param(
                    "limit",
                    "Page size (1-30, default 10).",
                    serde_json::json!({ "type": "integer", "minimum": 1, "maximum": 30 }),
                ),
            ],
            default_pipeline: vec![
                PipelineStep::Pick {
                    path: "items".to_string(),
                },
                PipelineStep::Map {
                    fields: vec![
                        "title".to_string(),
                        "link".to_string(),
                        "tags".to_string(),
                        "score".to_string(),
                        "answer_count".to_string(),
                        "is_answered".to_string(),
                        "creation_date".to_string(),
                    ],
                    rename: BTreeMap::new(),
                },
            ],
            rate_limit: Some(ExternalSourceRateLimit {
                plan_name: "Anonymous / Registered".to_string(),
                free_quota: "300 req/day anonymous; 10 000 req/day with API key.".to_string(),
                paid_tier: None,
                queries_per_second: Some(30.0),
                attribution_required: true,
                storage_rights_required: false,
            }),
            mcp_install: None,
        },
        // Hacker News top story ids (Firebase).
        ExternalSource {
            id: "hn_top_stories".to_string(),
            display_name: "Hacker News Top Stories".to_string(),
            description: "Fetch the current top-story ids from HN's Firebase API. Keyless.".to_string(),
            domain: ExternalSourceDomain::News,
            adapter: ExternalSourceAdapter::HttpJson,
            review_status: ExternalSourceReviewStatus::AllowedWithConditions,
            review_date: "2026-05-17".to_string(),
            adapter_license: "native".to_string(),
            terms_url: "https://github.com/HackerNews/API".to_string(),
            review_notes: "Public read-only Firebase endpoint. Returns a flat array of ids — chain with `hacker_news_search` or a follow-up fetch to enrich.".to_string(),
            attribution: Some("Data via Hacker News (news.ycombinator.com).".to_string()),
            credential_policy: ExternalSourceCredentialPolicy::None,
            credential_help: None,
            http: http_get("https://hacker-news.firebaseio.com/v0/topstories.json"),
            params: Vec::new(),
            default_pipeline: vec![PipelineStep::Limit { count: 20 }],
            rate_limit: Some(ExternalSourceRateLimit {
                plan_name: "Keyless".to_string(),
                free_quota: "Public Firebase endpoint, no published quota.".to_string(),
                paid_tier: None,
                queries_per_second: None,
                attribution_required: false,
                storage_rights_required: false,
            }),
            mcp_install: None,
        },
        // W37++: safe generic web page fetcher. Single URL param; runs
        // through `tool_engine.web_fetch` which adds robots.txt obedience
        // and a 500 KiB body cap on top of the regular HTTPS / private-IP
        // policy. No pipeline by default — the LLM sees `body.text` or
        // `body.json` directly.
        ExternalSource {
            id: "web_fetch".to_string(),
            display_name: "Safe Web Fetch".to_string(),
            description: "Fetch a single public URL with robots.txt obedience and a 500 KiB body cap. Returns text or JSON. Refuses local/private addresses.".to_string(),
            domain: ExternalSourceDomain::WebFetch,
            adapter: ExternalSourceAdapter::WebFetch,
            review_status: ExternalSourceReviewStatus::AllowedWithConditions,
            review_date: "2026-05-17".to_string(),
            adapter_license: "native".to_string(),
            terms_url: "https://www.rfc-editor.org/rfc/rfc9309".to_string(),
            review_notes: "Native adapter. Honours each host's robots.txt for the Datrina User-Agent, caps responses at 500 KiB, and blocks localhost/private IP destinations. Use through chat for one-off page lookups, not for crawls.".to_string(),
            attribution: None,
            credential_policy: ExternalSourceCredentialPolicy::None,
            credential_help: None,
            http: http_get("{url}"),
            params: vec![
                required_param("url", "Fully-qualified https:// URL to fetch."),
                optional_param(
                    "max_bytes",
                    "Override the default 500 KiB body cap (1 024 .. 1 048 576).",
                    serde_json::json!({ "type": "integer", "minimum": 1024, "maximum": 1_048_576 }),
                ),
            ],
            default_pipeline: Vec::new(),
            rate_limit: Some(ExternalSourceRateLimit {
                plan_name: "Native".to_string(),
                free_quota: "No Datrina-side rate limit; per-host robots.txt enforced.".to_string(),
                paid_tier: None,
                queries_per_second: None,
                attribution_required: false,
                storage_rights_required: false,
            }),
            mcp_install: None,
        },
        // W37++: recommended Fetch MCP. Catalog row only — install
        // through MCP Settings with the snippet the UI exposes.
        ExternalSource {
            id: "fetch_mcp_recommended".to_string(),
            display_name: "Fetch MCP Server (recommended)".to_string(),
            description: "Reference Fetch MCP server from modelcontextprotocol/servers. Adds robots-respecting URL fetch + Markdown extraction to chat once registered through MCP Settings.".to_string(),
            domain: ExternalSourceDomain::McpRecommended,
            adapter: ExternalSourceAdapter::McpRecommended,
            review_status: ExternalSourceReviewStatus::AllowedWithConditions,
            review_date: "2026-05-17".to_string(),
            adapter_license: "MIT (modelcontextprotocol/servers)".to_string(),
            terms_url: "https://github.com/modelcontextprotocol/servers/blob/main/src/fetch/README.md".to_string(),
            review_notes: "Python package distributed via PyPI. Honours robots.txt by default (disable with --ignore-robots-txt). The MCP server can access localhost/private IPs — keep that off unless you mean it. Datrina cannot install this for you; copy the command into MCP Settings.".to_string(),
            attribution: None,
            credential_policy: ExternalSourceCredentialPolicy::None,
            credential_help: None,
            http: http_unused(),
            params: Vec::new(),
            default_pipeline: Vec::new(),
            rate_limit: None,
            mcp_install: Some(McpInstallRecommendation {
                command: "uvx".to_string(),
                args: vec!["mcp-server-fetch".to_string()],
                env_hints: Vec::new(),
                package_kind: "pypi".to_string(),
                package_name: "mcp-server-fetch".to_string(),
            }),
        },
        // W37++: GitHub MCP server (official). Read-write tools — UI
        // warns to disable write tools by default at the MCP layer.
        ExternalSource {
            id: "github_mcp_recommended".to_string(),
            display_name: "GitHub MCP Server (recommended)".to_string(),
            description: "Official GitHub MCP server. Adds repo / issue / PR / release / Actions tools when registered through MCP Settings. Default to the read-only profile.".to_string(),
            domain: ExternalSourceDomain::McpRecommended,
            adapter: ExternalSourceAdapter::McpRecommended,
            review_status: ExternalSourceReviewStatus::AllowedWithConditions,
            review_date: "2026-05-17".to_string(),
            adapter_license: "MIT (github/github-mcp-server)".to_string(),
            terms_url: "https://github.com/github/github-mcp-server".to_string(),
            review_notes: "Ships with write tools that can mutate issues, PRs, releases, and Actions. Datrina recommends starting with the read-only toolset — turn writes back on per-tool from MCP Settings only after explicit consent.".to_string(),
            attribution: None,
            credential_policy: ExternalSourceCredentialPolicy::None,
            credential_help: None,
            http: http_unused(),
            params: Vec::new(),
            default_pipeline: Vec::new(),
            rate_limit: None,
            mcp_install: Some(McpInstallRecommendation {
                command: "npx".to_string(),
                args: vec![
                    "-y".to_string(),
                    "@modelcontextprotocol/server-github".to_string(),
                ],
                env_hints: vec![McpInstallEnvHint {
                    name: "GITHUB_PERSONAL_ACCESS_TOKEN".to_string(),
                    description: "Personal access token. Use a fine-grained, read-only token to start.".to_string(),
                    required: true,
                }],
                package_kind: "npm".to_string(),
                package_name: "@modelcontextprotocol/server-github".to_string(),
            }),
        },
        // W37++: MediaWiki MCP — community.
        ExternalSource {
            id: "mediawiki_mcp_recommended".to_string(),
            display_name: "MediaWiki MCP Server (recommended)".to_string(),
            description: "Community MediaWiki MCP server. Adds wiki search / page-read tools when registered through MCP Settings. Read-only profile recommended.".to_string(),
            domain: ExternalSourceDomain::McpRecommended,
            adapter: ExternalSourceAdapter::McpRecommended,
            review_status: ExternalSourceReviewStatus::AllowedWithConditions,
            review_date: "2026-05-17".to_string(),
            adapter_license: "MIT (ProfessionalWiki/MediaWiki-MCP-Server)".to_string(),
            terms_url: "https://github.com/ProfessionalWiki/MediaWiki-MCP-Server".to_string(),
            review_notes: "Community-maintained. Disable create/edit/delete tools at the MCP layer by default; only enable for trusted private wikis.".to_string(),
            attribution: None,
            credential_policy: ExternalSourceCredentialPolicy::None,
            credential_help: None,
            http: http_unused(),
            params: Vec::new(),
            default_pipeline: Vec::new(),
            rate_limit: None,
            mcp_install: Some(McpInstallRecommendation {
                command: "npx".to_string(),
                args: vec!["-y".to_string(), "@professional-wiki/mediawiki-mcp-server".to_string()],
                env_hints: vec![McpInstallEnvHint {
                    name: "MEDIAWIKI_API_URL".to_string(),
                    description: "Endpoint of the MediaWiki Action API (e.g. https://en.wikipedia.org/w/api.php).".to_string(),
                    required: true,
                }],
                package_kind: "npm".to_string(),
                package_name: "@professional-wiki/mediawiki-mcp-server".to_string(),
            }),
        },
    ]
}

static CATALOG: OnceLock<Vec<ExternalSource>> = OnceLock::new();

pub fn catalog() -> &'static [ExternalSource] {
    CATALOG.get_or_init(build_catalog).as_slice()
}

pub fn find(source_id: &str) -> Option<&'static ExternalSource> {
    catalog().iter().find(|entry| entry.id == source_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn catalog_is_non_empty_and_ids_are_unique() {
        let entries = catalog();
        assert!(
            entries.len() >= 9,
            "expected ≥9 catalog entries, got {}",
            entries.len()
        );
        let mut seen: HashSet<&str> = HashSet::new();
        for entry in entries {
            assert!(
                seen.insert(entry.id.as_str()),
                "duplicate catalog id '{}'",
                entry.id
            );
            assert!(
                !entry.display_name.is_empty(),
                "{} missing display name",
                entry.id
            );
            assert!(
                !entry.terms_url.is_empty(),
                "{} missing terms URL",
                entry.id
            );
            assert!(
                !entry.review_notes.is_empty(),
                "{} missing review notes",
                entry.id
            );
        }
    }

    #[test]
    fn every_runnable_entry_has_a_review_date() {
        for entry in catalog().iter().filter(|e| e.review_status.is_runnable()) {
            assert!(
                entry.review_date.starts_with("20"),
                "runnable source {} must carry a YYYY-MM-DD review_date, got '{}'",
                entry.id,
                entry.review_date
            );
        }
    }

    #[test]
    fn required_credential_entries_declare_a_header() {
        for entry in catalog().iter().filter(|e| {
            matches!(
                e.credential_policy,
                ExternalSourceCredentialPolicy::Required
            )
        }) {
            assert!(
                entry.http.credential_header.is_some(),
                "source {} requires a credential but lacks credential_header",
                entry.id
            );
        }
    }

    #[test]
    fn tool_name_is_prefixed_and_round_trips_via_parser() {
        for entry in catalog() {
            let tool_name = entry.tool_name();
            assert!(tool_name.starts_with("source_"));
            assert_eq!(
                crate::commands::external_source::parse_external_source_tool_name(&tool_name),
                Some(entry.id.as_str())
            );
        }
    }

    #[test]
    fn mcp_recommended_entries_carry_install_metadata() {
        let mcp_entries: Vec<_> = catalog()
            .iter()
            .filter(|e| matches!(e.adapter, ExternalSourceAdapter::McpRecommended))
            .collect();
        assert!(
            !mcp_entries.is_empty(),
            "catalog must include at least one McpRecommended entry"
        );
        for entry in mcp_entries {
            let install = entry
                .mcp_install
                .as_ref()
                .unwrap_or_else(|| panic!("{} missing mcp_install metadata", entry.id));
            assert!(
                !install.command.is_empty() && !install.package_name.is_empty(),
                "{} missing command/package metadata",
                entry.id
            );
        }
    }

    #[test]
    fn brave_search_records_storage_rights_constraint() {
        let entry = find("brave_search_web").expect("brave entry exists");
        let rate = entry.rate_limit.as_ref().expect("brave has rate metadata");
        assert!(
            rate.storage_rights_required,
            "brave requires storage rights"
        );
        assert!(rate.attribution_required, "brave requires attribution");
        assert!(
            entry.review_notes.contains("Feb-2026"),
            "brave review_notes must record the Feb-2026 repricing context"
        );
    }
}
