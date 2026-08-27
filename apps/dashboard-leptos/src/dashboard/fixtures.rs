use super::{EndpointBadge, EndpointRow, ProviderHealth, ProviderStatus, ProviderTile};

pub(super) fn api_key_providers() -> Vec<ProviderTile> {
    vec![
        ProviderTile {
            id: "openai".to_owned(),
            name: "OpenAI Compatible".to_owned(),
            description: "Custom base URLs, OpenAI chat, and responses format.".to_owned(),
            auth_label: "API key".to_owned(),
            accent: "#10b981".to_owned(),
            status: idle_status(),
        },
        ProviderTile {
            id: "anthropic".to_owned(),
            name: "Anthropic Compatible".to_owned(),
            description: "Claude-format upstreams with model mapping.".to_owned(),
            auth_label: "API key".to_owned(),
            accent: "#f59e0b".to_owned(),
            status: idle_status(),
        },
        ProviderTile {
            id: "gemini".to_owned(),
            name: "Gemini".to_owned(),
            description: "Google model bridge with native schema translation.".to_owned(),
            auth_label: "API key".to_owned(),
            accent: "#60a5fa".to_owned(),
            status: ProviderStatus {
                connected: 0,
                error: 1,
                total: 1,
                health: ProviderHealth::Degraded,
            },
        },
        ProviderTile {
            id: "openrouter".to_owned(),
            name: "OpenRouter".to_owned(),
            description: "OpenAI-compatible aggregation for fallback routing.".to_owned(),
            auth_label: "API key".to_owned(),
            accent: "#e56a4a".to_owned(),
            status: idle_status(),
        },
    ]
}

const ENDPOINT_ROWS: [EndpointRow; 5] = [
    EndpointRow {
        label: "OpenAI",
        value: "http://localhost:3000/v1",
        badge: EndpointBadge::Local,
    },
    EndpointRow {
        label: "Claude",
        value: "http://localhost:3000/anthropic",
        badge: EndpointBadge::Local,
    },
    EndpointRow {
        label: "Gemini",
        value: "http://localhost:3000/gemini",
        badge: EndpointBadge::Local,
    },
    EndpointRow {
        label: "CF",
        value: "https://example.trycloudflare.com/v1",
        badge: EndpointBadge::Cloudflare,
    },
    EndpointRow {
        label: "TS",
        value: "https://nine-router.tailnet.ts.net/v1",
        badge: EndpointBadge::Tailscale,
    },
];

pub(super) const fn endpoint_rows() -> &'static [EndpointRow] {
    &ENDPOINT_ROWS
}

const fn idle_status() -> ProviderStatus {
    ProviderStatus {
        connected: 0,
        error: 0,
        total: 0,
        health: ProviderHealth::Idle,
    }
}
