use super::provider_groups;
use serde::Serialize;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct UsageProviderNode {
    pub id: String,
    pub name: String,
    pub accent: String,
    pub slot_class: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RecentRequest {
    pub provider: String,
    pub route: String,
    pub status: String,
    pub age: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct UsageSnapshot {
    pub stream_connected: bool,
    pub active_requests: u8,
    pub requests_today: u32,
    pub tokens_today: u32,
    pub estimated_cost: &'static str,
    pub topology_providers: Vec<UsageProviderNode>,
    pub recent_requests: Vec<RecentRequest>,
}

pub fn usage_snapshot() -> UsageSnapshot {
    UsageSnapshot {
        stream_connected: false,
        active_requests: 0,
        requests_today: 0,
        tokens_today: 0,
        estimated_cost: "$0.00",
        topology_providers: topology_providers(),
        recent_requests: Vec::new(),
    }
}

fn topology_providers() -> Vec<UsageProviderNode> {
    provider_groups()
        .into_iter()
        .flat_map(|group| group.providers)
        .take(6)
        .enumerate()
        .map(|(index, provider)| UsageProviderNode {
            id: provider.id,
            name: provider.name,
            accent: provider.accent,
            slot_class: slot_class(index),
        })
        .collect()
}

const fn slot_class(index: usize) -> &'static str {
    match index {
        0 => "slot-one",
        1 => "slot-two",
        2 => "slot-three",
        3 => "slot-four",
        4 => "slot-five",
        _ => "slot-six",
    }
}
