use serde::Serialize;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct MitmDashboardState {
    pub route_path: &'static str,
    pub title: &'static str,
    pub risk_warning: &'static str,
    pub unsupported_notice: &'static str,
    pub live_control_wired: bool,
    pub server: MitmServerState,
    pub tools: &'static [MitmToolState],
    pub hosts_instruction: &'static str,
    pub mapping_notice: &'static str,
    pub mapping_placeholder: &'static str,
    pub select_label: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct MitmServerState {
    pub title: &'static str,
    pub status_label: &'static str,
    pub running: bool,
    pub checks: &'static [MitmStatusCheck],
    pub purpose: &'static str,
    pub how_it_works: &'static str,
    pub base_url: MitmFieldState,
    pub api_key: MitmFieldState,
    pub action: MitmAction,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct MitmStatusCheck {
    pub label: &'static str,
    pub ok: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct MitmFieldState {
    pub label: &'static str,
    pub value: &'static str,
    pub placeholder: &'static str,
    pub enabled: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct MitmAction {
    pub label: &'static str,
    pub status_label: &'static str,
    pub enabled: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct MitmToolState {
    pub id: &'static str,
    pub name: &'static str,
    pub image: &'static str,
    pub intercept_label: &'static str,
    pub dns_instruction: &'static str,
    pub hosts: &'static [&'static str],
    pub models: &'static [MitmModelMapping],
    pub server_running: bool,
    pub dns_active: bool,
    pub server_status_label: &'static str,
    pub dns_status_label: &'static str,
    pub mapping_inputs_enabled: bool,
    pub model_select_enabled: bool,
    pub dns_action: MitmAction,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct MitmModelMapping {
    pub name: &'static str,
    pub alias: &'static str,
}
