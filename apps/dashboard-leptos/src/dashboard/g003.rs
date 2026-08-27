use serde::Serialize;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct DashboardPanelState {
    pub title: &'static str,
    pub route_path: &'static str,
    pub api_status: &'static str,
    pub persistence_status: &'static str,
    pub controls_enabled: bool,
    pub empty_title: &'static str,
    pub empty_detail: &'static str,
    pub rows: &'static [DashboardPanelRow],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct DashboardPanelRow {
    pub label: &'static str,
    pub value: &'static str,
}

pub const fn basic_chat_state() -> DashboardPanelState {
    DashboardPanelState {
        title: "Basic Chat",
        route_path: "/dashboard/basic-chat",
        api_status: "Chat API not connected",
        persistence_status: "Not persisted",
        controls_enabled: false,
        empty_title: "No chat session loaded",
        empty_detail: "Prompt history, streaming, and send actions remain disabled until host chat state is wired.",
        rows: &[
            DashboardPanelRow {
                label: "Model selector",
                value: "Default only",
            },
            DashboardPanelRow {
                label: "Send control",
                value: "Disabled",
            },
            DashboardPanelRow {
                label: "History",
                value: "Not persisted",
            },
        ],
    }
}

pub const fn proxy_pools_state() -> DashboardPanelState {
    DashboardPanelState {
        title: "Proxy Pools",
        route_path: "/dashboard/proxy-pools",
        api_status: "Proxy API not connected",
        persistence_status: "Not persisted",
        controls_enabled: false,
        empty_title: "No proxy pools configured",
        empty_detail: "Pool rotation and health checks are visible as disabled defaults until the proxy service is connected.",
        rows: &[
            DashboardPanelRow {
                label: "Pool count",
                value: "0",
            },
            DashboardPanelRow {
                label: "Rotation",
                value: "Disabled",
            },
            DashboardPanelRow {
                label: "Health checks",
                value: "Not connected",
            },
        ],
    }
}

pub const fn translator_state() -> DashboardPanelState {
    DashboardPanelState {
        title: "Translator",
        route_path: "/dashboard/translator",
        api_status: "Translation API not connected",
        persistence_status: "Not persisted",
        controls_enabled: false,
        empty_title: "No translation loaded",
        empty_detail: "Language selection and submit controls are disabled until translator execution is available.",
        rows: &[
            DashboardPanelRow {
                label: "Source language",
                value: "Auto default",
            },
            DashboardPanelRow {
                label: "Target language",
                value: "Unset",
            },
            DashboardPanelRow {
                label: "Submit",
                value: "Disabled",
            },
        ],
    }
}

pub const fn console_log_state() -> DashboardPanelState {
    DashboardPanelState {
        title: "Console Log",
        route_path: "/dashboard/console-log",
        api_status: "Log stream not connected",
        persistence_status: "Not persisted",
        controls_enabled: false,
        empty_title: "No console entries",
        empty_detail: "The panel does not invent runtime logs while the host event stream is unavailable.",
        rows: &[
            DashboardPanelRow {
                label: "Runtime stream",
                value: "Not connected",
            },
            DashboardPanelRow {
                label: "Buffered entries",
                value: "0",
            },
            DashboardPanelRow {
                label: "Export",
                value: "Disabled",
            },
        ],
    }
}

pub const fn media_providers_web_state() -> DashboardPanelState {
    DashboardPanelState {
        title: "Web Media Providers",
        route_path: "/dashboard/media-providers/web",
        api_status: "Provider API not connected",
        persistence_status: "Not persisted",
        controls_enabled: false,
        empty_title: "No web provider connections",
        empty_detail: "Search and fetch provider cards stay in default state until credential data is hydrated.",
        rows: &[
            DashboardPanelRow {
                label: "Web search",
                value: "No connections",
            },
            DashboardPanelRow {
                label: "Web fetch",
                value: "No connections",
            },
            DashboardPanelRow {
                label: "Combos",
                value: "Preview only",
            },
        ],
    }
}

pub const fn profile_state() -> DashboardPanelState {
    DashboardPanelState {
        title: "Profile",
        route_path: "/dashboard/profile",
        api_status: "Profile API not connected",
        persistence_status: "Not persisted",
        controls_enabled: false,
        empty_title: "No profile loaded",
        empty_detail: "Account identity, avatar, and preferences are held empty until authentication is wired.",
        rows: &[
            DashboardPanelRow {
                label: "Signed in",
                value: "No",
            },
            DashboardPanelRow {
                label: "Preferences",
                value: "Default",
            },
            DashboardPanelRow {
                label: "Save action",
                value: "Disabled",
            },
        ],
    }
}
