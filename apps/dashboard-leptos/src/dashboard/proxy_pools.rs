use serde::Serialize;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProxyPoolsState {
    pub route_path: &'static str,
    pub title: &'static str,
    pub api_state_label: &'static str,
    pub totals: ProxyPoolTotals,
    pub header_actions: &'static [ProxyPoolAction],
    pub relay_actions: &'static [RelayProviderAction],
    pub selection: ProxyPoolSelectionState,
    pub empty: ProxyPoolEmptyState,
    pub entries: Vec<ProxyPoolEntry>,
    pub sample_entry: ProxyPoolEntry,
    pub modals: ProxyPoolModals,
    pub visible_hooks: &'static [&'static str],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct ProxyPoolTotals {
    pub total: usize,
    pub active: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct ProxyPoolAction {
    pub label: &'static str,
    pub status_label: &'static str,
    pub enabled: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct RelayProviderAction {
    pub label: &'static str,
    pub modal_title: &'static str,
    pub default_project: &'static str,
    pub deployment_wired: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct ProxyPoolSelectionState {
    pub select_all_label: &'static str,
    pub selected_label: &'static str,
    pub health_label: &'static str,
    pub health_progress_label: &'static str,
    pub bulk_actions: &'static [ProxyPoolAction],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct ProxyPoolEmptyState {
    pub title: &'static str,
    pub detail: &'static str,
    pub action_label: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProxyPoolEntry {
    pub id: &'static str,
    pub name: &'static str,
    pub test_status: ProxyPoolTestStatus,
    pub is_active: bool,
    pub proxy_type: ProxyPoolType,
    pub bound_connection_count: u8,
    pub proxy_url: &'static str,
    pub no_proxy: Option<&'static str>,
    pub last_tested_label: &'static str,
    pub last_error: Option<&'static str>,
    pub strict_proxy: bool,
    pub actions: ProxyPoolRowActions,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub enum ProxyPoolTestStatus {
    Active,
    Error,
    Unknown,
}

impl ProxyPoolTestStatus {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Error => "error",
            Self::Unknown => "unknown",
        }
    }

    pub const fn class_name(self) -> &'static str {
        match self {
            Self::Active => "is-connected",
            Self::Error => "is-degraded",
            Self::Unknown => "is-idle",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub enum ProxyPoolType {
    Direct,
    VercelRelay,
    CloudflareRelay,
    DenoRelay,
}

impl ProxyPoolType {
    pub const fn badge_label(self) -> Option<&'static str> {
        match self {
            Self::Direct => None,
            Self::VercelRelay => Some("vercel relay"),
            Self::CloudflareRelay => Some("cloudflare relay"),
            Self::DenoRelay => Some("deno relay"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct ProxyPoolRowActions {
    pub toggle_label: &'static str,
    pub test_label: &'static str,
    pub edit_label: &'static str,
    pub delete_label: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct ProxyPoolModals {
    pub batch_import: ProxyPoolModalState,
    pub form: ProxyPoolModalState,
    pub vercel: ProxyPoolModalState,
    pub cloudflare: ProxyPoolModalState,
    pub deno: ProxyPoolModalState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct ProxyPoolModalState {
    pub title: &'static str,
    pub body_title: &'static str,
    pub body: &'static str,
    pub fields: &'static [ProxyPoolField],
    pub primary_label: &'static str,
    pub secondary_label: &'static str,
    pub unsupported_label: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct ProxyPoolField {
    pub label: &'static str,
    pub placeholder: &'static str,
    pub hint: &'static str,
}

const HEADER_ACTIONS: [ProxyPoolAction; 3] = [
    ProxyPoolAction {
        label: "Deploy Relay",
        status_label: "Menu preview",
        enabled: false,
    },
    ProxyPoolAction {
        label: "Batch Import",
        status_label: "Import disabled",
        enabled: false,
    },
    ProxyPoolAction {
        label: "Add Proxy Pool",
        status_label: "Create disabled",
        enabled: false,
    },
];

const RELAY_ACTIONS: [RelayProviderAction; 3] = [
    RelayProviderAction {
        label: "Cloudflare Relay",
        modal_title: "Deploy Cloudflare Relay",
        default_project: "cloudflare-relay",
        deployment_wired: false,
    },
    RelayProviderAction {
        label: "Vercel Relay",
        modal_title: "Deploy Vercel Relay",
        default_project: "vercel-relay",
        deployment_wired: false,
    },
    RelayProviderAction {
        label: "Deno Relay",
        modal_title: "Deploy Deno Relay",
        default_project: "deno-relay",
        deployment_wired: false,
    },
];

const BULK_ACTIONS: [ProxyPoolAction; 4] = [
    ProxyPoolAction {
        label: "Activate",
        status_label: "Bulk update disabled",
        enabled: false,
    },
    ProxyPoolAction {
        label: "Deactivate",
        status_label: "Bulk update disabled",
        enabled: false,
    },
    ProxyPoolAction {
        label: "Delete",
        status_label: "Bulk delete disabled",
        enabled: false,
    },
    ProxyPoolAction {
        label: "Clear",
        status_label: "Selection reset preview",
        enabled: false,
    },
];

const BATCH_FIELDS: [ProxyPoolField; 1] = [ProxyPoolField {
    label: "Paste Proxy List (One per line)",
    placeholder: "http://user:pass@127.0.0.1:7897\n127.0.0.1:7897:user:pass",
    hint: "Supported formats: protocol://user:pass@host:port, host:port:user:pass",
}];

const FORM_FIELDS: [ProxyPoolField; 5] = [
    ProxyPoolField {
        label: "Name",
        placeholder: "Office Proxy",
        hint: "Display name for this proxy pool entry.",
    },
    ProxyPoolField {
        label: "Proxy URL",
        placeholder: "http://127.0.0.1:7897",
        hint: "HTTP, HTTPS, SOCKS, or relay URL used for upstream requests.",
    },
    ProxyPoolField {
        label: "No Proxy",
        placeholder: "localhost,127.0.0.1,.internal",
        hint: "Comma-separated hosts/domains to bypass proxy",
    },
    ProxyPoolField {
        label: "Active",
        placeholder: "enabled by default",
        hint: "Inactive pools are ignored by runtime resolution.",
    },
    ProxyPoolField {
        label: "Strict Proxy",
        placeholder: "off",
        hint: "Fail request if proxy is unreachable instead of falling back to direct.",
    },
];

const VERCEL_FIELDS: [ProxyPoolField; 2] = [
    ProxyPoolField {
        label: "Vercel API Token",
        placeholder: "your-vercel-api-token",
        hint: "Token is used once for deployment and not stored.",
    },
    ProxyPoolField {
        label: "Project Name",
        placeholder: "my-relay",
        hint: "Unique name for your Vercel project. Leave empty for auto-generated name.",
    },
];

const CLOUDFLARE_FIELDS: [ProxyPoolField; 3] = [
    ProxyPoolField {
        label: "Account ID",
        placeholder: "your-cloudflare-account-id",
        hint: "Found on the right side of the Cloudflare dashboard overview page.",
    },
    ProxyPoolField {
        label: "API Token",
        placeholder: "your-cloudflare-api-token",
        hint: "Requires Workers Scripts: Edit permission.",
    },
    ProxyPoolField {
        label: "Worker Name",
        placeholder: "my-relay",
        hint: "Unique name for your Cloudflare Worker. Leave empty for auto-generated name.",
    },
];

const DENO_FIELDS: [ProxyPoolField; 3] = [
    ProxyPoolField {
        label: "Deno Deploy API Token",
        placeholder: "ddo_xxxxxxxxxxxxxxxx",
        hint: "Token is used once for deployment, not stored.",
    },
    ProxyPoolField {
        label: "Organization Domain",
        placeholder: "your-org.deno.net",
        hint: "Your relay URL will be in the format: https://my-relay.your-org.deno.net",
    },
    ProxyPoolField {
        label: "App Name",
        placeholder: "deno-relay",
        hint: "Unique app name. Leave empty for auto-generated name.",
    },
];

const VISIBLE_HOOKS: [&str; 6] = [
    "nr-proxy-pools-panel",
    "nr-proxy-relay-menu",
    "nr-proxy-bulk-bar",
    "nr-proxy-empty",
    "nr-proxy-row",
    "nr-proxy-modal-grid",
];

pub const fn proxy_pools_dashboard_state() -> ProxyPoolsState {
    ProxyPoolsState {
        route_path: "/dashboard/proxy-pools",
        title: "Proxy Pools",
        api_state_label: "Host proxy-pools API is unavailable in this WASM preview",
        totals: ProxyPoolTotals {
            total: 0,
            active: 0,
        },
        header_actions: &HEADER_ACTIONS,
        relay_actions: &RELAY_ACTIONS,
        selection: ProxyPoolSelectionState {
            select_all_label: "Select all",
            selected_label: "0 selected",
            health_label: "Health Check",
            health_progress_label: "Checking 0/0",
            bulk_actions: &BULK_ACTIONS,
        },
        empty: ProxyPoolEmptyState {
            title: "No proxy pool entries yet",
            detail: "Create a proxy pool entry, then assign it to connections.",
            action_label: "Add Proxy Pool",
        },
        entries: Vec::new(),
        sample_entry: proxy_pool_sample_entry(),
        modals: proxy_pool_modals(),
        visible_hooks: &VISIBLE_HOOKS,
    }
}

pub fn proxy_pools_sample_state() -> ProxyPoolsState {
    let sample_entry = proxy_pool_sample_entry();

    ProxyPoolsState {
        totals: ProxyPoolTotals {
            total: 1,
            active: 1,
        },
        selection: ProxyPoolSelectionState {
            select_all_label: "Unselect all",
            selected_label: "1 selected",
            health_label: "Health Check",
            health_progress_label: "Checking 1/1",
            bulk_actions: &BULK_ACTIONS,
        },
        entries: vec![sample_entry.clone()],
        sample_entry,
        ..proxy_pools_dashboard_state()
    }
}

pub const fn proxy_pool_sample_entry() -> ProxyPoolEntry {
    ProxyPoolEntry {
        id: "proxy-pool-sample-cloudflare",
        name: "Cloudflare edge relay",
        test_status: ProxyPoolTestStatus::Active,
        is_active: true,
        proxy_type: ProxyPoolType::CloudflareRelay,
        bound_connection_count: 2,
        proxy_url: "https://cloudflare-relay.example.workers.dev",
        no_proxy: Some("localhost,127.0.0.1,.internal"),
        last_tested_label: "Last tested: Jul 12, 2026, 09:10",
        last_error: None,
        strict_proxy: true,
        actions: ProxyPoolRowActions {
            toggle_label: "Disable",
            test_label: "Test proxy",
            edit_label: "Edit",
            delete_label: "Delete",
        },
    }
}

pub const fn proxy_pool_modals() -> ProxyPoolModals {
    ProxyPoolModals {
        batch_import: ProxyPoolModalState {
            title: "Batch Import Proxies",
            body_title: "Paste proxy list",
            body: "Parse import lines locally, then create entries after the proxy-pools API is wired.",
            fields: &BATCH_FIELDS,
            primary_label: "Import",
            secondary_label: "Cancel",
            unsupported_label: "Batch import is disabled until /api/proxy-pools is available.",
        },
        form: ProxyPoolModalState {
            title: "Add/Edit Proxy Pool",
            body_title: "Proxy pool details",
            body: "Create and edit controls mirror upstream defaults without writing host state.",
            fields: &FORM_FIELDS,
            primary_label: "Save",
            secondary_label: "Cancel",
            unsupported_label: "Save disabled: /api/proxy-pools persistence is unavailable here.",
        },
        vercel: ProxyPoolModalState {
            title: "Deploy Vercel Relay",
            body_title: "What is Vercel Relay?",
            body: "Deploys an edge relay function to Vercel and forwards AI provider requests through Vercel edge IPs.",
            fields: &VERCEL_FIELDS,
            primary_label: "Deploy",
            secondary_label: "Cancel",
            unsupported_label: "Deployment not wired in the WASM dashboard.",
        },
        cloudflare: ProxyPoolModalState {
            title: "Deploy Cloudflare Relay",
            body_title: "What is Cloudflare Relay?",
            body: "Deploys a Cloudflare Worker proxy relay for global edge routing and IP masking.",
            fields: &CLOUDFLARE_FIELDS,
            primary_label: "Deploy Worker",
            secondary_label: "Cancel",
            unsupported_label: "Deployment not wired in the WASM dashboard.",
        },
        deno: ProxyPoolModalState {
            title: "Deploy Deno Relay",
            body_title: "What is Deno Relay?",
            body: "Deploys a relay worker to Deno Deploy's global edge network.",
            fields: &DENO_FIELDS,
            primary_label: "Deploy Relay",
            secondary_label: "Cancel",
            unsupported_label: "Deployment not wired in the WASM dashboard.",
        },
    }
}
