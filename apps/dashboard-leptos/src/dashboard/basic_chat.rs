use std::collections::BTreeMap;

use serde::Serialize;

use super::{ModelTile, model_catalog};

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BasicChatState {
    pub route_path: &'static str,
    pub provider_boundary_title: &'static str,
    pub provider_boundary_detail: &'static str,
    pub model_menu_title: &'static str,
    pub model_menu_subtitle: &'static str,
    pub active_model_label: String,
    pub active_model_detail: String,
    pub provider_groups: Vec<BasicChatProviderGroup>,
    pub history: BasicChatHistoryState,
    pub empty_title: &'static str,
    pub empty_detail: &'static str,
    pub composer: BasicChatComposerState,
    pub execution_wired: bool,
    pub persistence_wired: bool,
    pub transcript_hooks: &'static [&'static str],
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BasicChatProviderGroup {
    pub provider_id: String,
    pub provider_name: String,
    pub models: Vec<BasicChatModelOption>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BasicChatModelOption {
    pub id: String,
    pub name: String,
    pub request_model: String,
    pub provider_id: String,
    pub provider_name: String,
    pub source_label: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BasicChatHistoryState {
    pub title: &'static str,
    pub clear_label: &'static str,
    pub empty_label: &'static str,
    pub sessions: Vec<BasicChatSessionPreview>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct BasicChatSessionPreview {
    pub title: &'static str,
    pub preview: &'static str,
    pub updated_label: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BasicChatComposerState {
    pub placeholder: &'static str,
    pub attachment_label: &'static str,
    pub send_label: &'static str,
    pub stop_label: &'static str,
    pub model_label: String,
    pub can_attach: bool,
    pub can_send: bool,
    pub can_stop: bool,
}

const TRANSCRIPT_HOOKS: [&str; 4] = [
    "nr-chat-transcript",
    "nr-chat-message assistant",
    "nr-chat-message user",
    "nr-chat-streaming",
];

const LOCAL_SESSION: BasicChatSessionPreview = BasicChatSessionPreview {
    title: "New chat",
    preview: "Empty chat",
    updated_label: "Now",
};

pub fn basic_chat_dashboard_state() -> BasicChatState {
    basic_chat_state_from_groups(provider_groups_from_models(model_catalog()))
}

pub fn basic_chat_no_provider_state() -> BasicChatState {
    basic_chat_state_from_groups(Vec::new())
}

fn basic_chat_state_from_groups(provider_groups: Vec<BasicChatProviderGroup>) -> BasicChatState {
    let selected_model = provider_groups
        .first()
        .and_then(|group| group.models.first());
    let active_model_label = selected_model.map_or_else(
        || "No model".to_owned(),
        |model| model.request_model.clone(),
    );
    let active_model_detail = selected_model.map_or_else(
        || "Choose from connected providers".to_owned(),
        |model| format!("{} catalog default", model.provider_name),
    );
    let history = history_state(selected_model.is_some());
    let composer = BasicChatComposerState {
        placeholder: "Message AI",
        attachment_label: "Attach image",
        send_label: "Send message",
        stop_label: "Stop response",
        model_label: active_model_label.clone(),
        can_attach: false,
        can_send: false,
        can_stop: false,
    };

    BasicChatState {
        route_path: "/dashboard/basic-chat",
        provider_boundary_title: "No providers connected yet",
        provider_boundary_detail: "Connect a provider before sending messages. The WASM dashboard keeps the composer disabled until host provider state is available.",
        model_menu_title: "Models",
        model_menu_subtitle: "Only from connected providers",
        active_model_label,
        active_model_detail,
        provider_groups,
        history,
        empty_title: "Start a conversation",
        empty_detail: "Select a model from the local catalog preview, then provider execution will be available after the dashboard chat API is wired.",
        composer,
        execution_wired: false,
        persistence_wired: false,
        transcript_hooks: &TRANSCRIPT_HOOKS,
    }
}

fn provider_groups_from_models(models: Vec<ModelTile>) -> Vec<BasicChatProviderGroup> {
    let names = provider_names(&models);
    let mut groups: Vec<BasicChatProviderGroup> = Vec::new();

    for model in models {
        let provider_name = names
            .get(model.provider.as_str())
            .cloned()
            .unwrap_or_else(|| provider_label(&model.provider));
        let option = BasicChatModelOption {
            id: model.id.clone(),
            name: model.id.clone(),
            request_model: model.id,
            provider_id: model.provider.clone(),
            provider_name: provider_name.clone(),
            source_label: "Catalog default",
        };

        if let Some(group) = groups
            .iter_mut()
            .find(|group| group.provider_id == model.provider)
        {
            group.models.push(option);
        } else {
            groups.push(BasicChatProviderGroup {
                provider_id: model.provider,
                provider_name,
                models: vec![option],
            });
        }
    }

    groups
}

fn history_state(has_model: bool) -> BasicChatHistoryState {
    BasicChatHistoryState {
        title: "Recent chats",
        clear_label: "Clear",
        empty_label: "No conversations yet",
        sessions: if has_model {
            vec![LOCAL_SESSION]
        } else {
            Vec::new()
        },
    }
}

fn provider_names(models: &[ModelTile]) -> BTreeMap<String, String> {
    models
        .iter()
        .map(|model| (model.provider.clone(), provider_label(&model.provider)))
        .collect()
}

fn provider_label(provider_id: &str) -> String {
    provider_id
        .split(['-', '_'])
        .map(capitalize)
        .collect::<Vec<_>>()
        .join(" ")
}

fn capitalize(value: &str) -> String {
    let mut chars = value.chars();
    chars.next().map_or_else(String::new, |first| {
        first
            .to_uppercase()
            .chain(chars.flat_map(char::to_lowercase))
            .collect()
    })
}
