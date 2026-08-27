pub(super) struct MediaKindDefinition {
    pub(super) id: &'static str,
    pub(super) label: &'static str,
    pub(super) icon: &'static str,
    pub(super) endpoint_method: &'static str,
    pub(super) endpoint_path: &'static str,
}

pub(super) struct MediaProviderDefinition {
    pub(super) id: &'static str,
    pub(super) name: &'static str,
    pub(super) description: &'static str,
    pub(super) color: &'static str,
    pub(super) text_icon: &'static str,
    pub(super) service_kinds: &'static [&'static str],
    pub(super) no_auth: bool,
}

pub(super) struct MediaProviderComboDefinition {
    pub(super) id: &'static str,
    pub(super) name: &'static str,
    pub(super) kind_id: &'static str,
    pub(super) models: &'static [&'static str],
    pub(super) routing: &'static str,
}

pub(super) const fn provider_definitions() -> &'static [MediaProviderDefinition] {
    &MEDIA_PROVIDERS
}

pub(super) const fn combo_definitions() -> &'static [MediaProviderComboDefinition] {
    &MEDIA_COMBOS
}

pub(super) const MEDIA_PROVIDER_KINDS: [MediaKindDefinition; 9] = [
    MediaKindDefinition {
        id: "embedding",
        label: "Embedding",
        icon: "data_array",
        endpoint_method: "POST",
        endpoint_path: "/v1/embeddings",
    },
    MediaKindDefinition {
        id: "image",
        label: "Text to Image",
        icon: "brush",
        endpoint_method: "POST",
        endpoint_path: "/v1/images/generations",
    },
    MediaKindDefinition {
        id: "imageToText",
        label: "Image to Text",
        icon: "image_search",
        endpoint_method: "POST",
        endpoint_path: "/v1/images/understanding",
    },
    MediaKindDefinition {
        id: "tts",
        label: "Text To Speech",
        icon: "record_voice_over",
        endpoint_method: "POST",
        endpoint_path: "/v1/audio/speech",
    },
    MediaKindDefinition {
        id: "stt",
        label: "Speech To Text",
        icon: "mic",
        endpoint_method: "POST",
        endpoint_path: "/v1/audio/transcriptions",
    },
    MediaKindDefinition {
        id: "webSearch",
        label: "Web Search",
        icon: "travel_explore",
        endpoint_method: "POST",
        endpoint_path: "/v1/search",
    },
    MediaKindDefinition {
        id: "webFetch",
        label: "Web Fetch",
        icon: "language",
        endpoint_method: "POST",
        endpoint_path: "/v1/web/fetch",
    },
    MediaKindDefinition {
        id: "video",
        label: "Video",
        icon: "movie",
        endpoint_method: "POST",
        endpoint_path: "/v1/video/generations",
    },
    MediaKindDefinition {
        id: "music",
        label: "Music",
        icon: "music_note",
        endpoint_method: "POST",
        endpoint_path: "/v1/audio/music",
    },
];

const MEDIA_PROVIDERS: [MediaProviderDefinition; 10] = [
    MediaProviderDefinition {
        id: "openai",
        name: "OpenAI",
        description: "OpenAI media endpoints for image, speech, transcription, and embeddings.",
        color: "#10b981",
        text_icon: "OA",
        service_kinds: &["embedding", "image", "tts", "stt"],
        no_auth: false,
    },
    MediaProviderDefinition {
        id: "anthropic",
        name: "Anthropic",
        description: "Claude-compatible text reasoning with no media endpoint fixture here.",
        color: "#f59e0b",
        text_icon: "AN",
        service_kinds: &[],
        no_auth: false,
    },
    MediaProviderDefinition {
        id: "gemini",
        name: "Gemini",
        description: "Google media bridge for embeddings and image understanding.",
        color: "#60a5fa",
        text_icon: "GE",
        service_kinds: &["embedding", "imageToText"],
        no_auth: false,
    },
    MediaProviderDefinition {
        id: "mistral",
        name: "Mistral",
        description: "Embedding provider preview for Mistral-compatible vectors.",
        color: "#f97316",
        text_icon: "MI",
        service_kinds: &["embedding"],
        no_auth: false,
    },
    MediaProviderDefinition {
        id: "elevenlabs",
        name: "ElevenLabs",
        description: "Speech generation and transcription provider preview.",
        color: "#a78bfa",
        text_icon: "EL",
        service_kinds: &["tts", "stt"],
        no_auth: false,
    },
    MediaProviderDefinition {
        id: "tavily",
        name: "Tavily",
        description: "Web search provider for research-oriented requests.",
        color: "#14b8a6",
        text_icon: "TV",
        service_kinds: &["webSearch"],
        no_auth: false,
    },
    MediaProviderDefinition {
        id: "exa",
        name: "Exa",
        description: "Neural web search and content discovery provider.",
        color: "#8b5cf6",
        text_icon: "EX",
        service_kinds: &["webSearch"],
        no_auth: false,
    },
    MediaProviderDefinition {
        id: "firecrawl",
        name: "Firecrawl",
        description: "Web fetch and extraction provider for URL content.",
        color: "#ef4444",
        text_icon: "FC",
        service_kinds: &["webFetch"],
        no_auth: false,
    },
    MediaProviderDefinition {
        id: "replicate",
        name: "Replicate",
        description: "Image and video generation provider fixture.",
        color: "#9ca3af",
        text_icon: "RP",
        service_kinds: &["image", "video"],
        no_auth: false,
    },
    MediaProviderDefinition {
        id: "pollinations",
        name: "Pollinations",
        description: "No-auth image generation provider preview.",
        color: "#f472b6",
        text_icon: "PO",
        service_kinds: &["image"],
        no_auth: true,
    },
];

const MEDIA_COMBOS: [MediaProviderComboDefinition; 5] = [
    MediaProviderComboDefinition {
        id: "combo_1",
        name: "embedding_combo",
        kind_id: "embedding",
        models: &["openai/text-embedding-3-small", "mistral/embed"],
        routing: "State-backed combo preview",
    },
    MediaProviderComboDefinition {
        id: "search-combo",
        name: "search-combo",
        kind_id: "webSearch",
        models: &["openai/gpt-5-search", "tavily/search"],
        routing: "Fallback order preview",
    },
    MediaProviderComboDefinition {
        id: "fetch-combo",
        name: "fetch-combo",
        kind_id: "webFetch",
        models: &["firecrawl/fetch", "openai/gpt-5"],
        routing: "Fallback order preview",
    },
    MediaProviderComboDefinition {
        id: "image-combo",
        name: "image-combo",
        kind_id: "image",
        models: &["openai/gpt-image-1", "replicate/flux"],
        routing: "Fallback order preview",
    },
    MediaProviderComboDefinition {
        id: "tts-combo",
        name: "tts-combo",
        kind_id: "tts",
        models: &["openai/tts-1", "elevenlabs/voice"],
        routing: "Fallback order preview",
    },
];
