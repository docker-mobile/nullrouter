use crate::{
    StoreError,
    store::{ProviderConnection, StateSnapshot, StateStore, timestamp},
};

use super::types::{
    ANTHROPIC_COMPATIBLE, CUSTOM_EMBEDDING, OPENAI_COMPATIBLE, ProviderNode, ProviderNodeInput,
};

impl StateStore {
    pub(crate) fn list_provider_nodes(&self) -> Result<Vec<ProviderNode>, StoreError> {
        Ok(self.read_snapshot()?.provider_nodes)
    }

    pub(crate) fn get_provider_node(&self, id: &str) -> Result<Option<ProviderNode>, StoreError> {
        Ok(self
            .read_snapshot()?
            .provider_nodes
            .into_iter()
            .find(|node| node.id == id))
    }

    pub(crate) fn create_provider_node(
        &self,
        input: ProviderNodeInput,
    ) -> Result<ProviderNode, StoreError> {
        self.write_snapshot(|snapshot| {
            let now = timestamp();
            let node = ProviderNode {
                id: next_provider_node_id(snapshot, &input.node_type),
                node_type: input.node_type,
                name: input.name,
                prefix: input.prefix,
                api_type: input.api_type,
                base_url: input.base_url,
                created_at: now.clone(),
                updated_at: now,
            };
            snapshot.provider_nodes.push(node.clone());
            node
        })
    }

    pub(crate) fn update_provider_node(
        &self,
        id: &str,
        input: ProviderNodeInput,
    ) -> Result<Option<ProviderNode>, StoreError> {
        self.write_snapshot(|snapshot| {
            let node = snapshot
                .provider_nodes
                .iter_mut()
                .find(|node| node.id == id)?;
            node.node_type = input.node_type;
            node.name = input.name;
            node.prefix = input.prefix;
            node.api_type = input.api_type;
            node.base_url = input.base_url;
            node.updated_at = timestamp();
            Some(node.clone())
        })
    }

    pub(crate) fn delete_provider_node(&self, id: &str) -> Result<bool, StoreError> {
        self.write_snapshot(|snapshot| {
            let original_len = snapshot.provider_nodes.len();
            snapshot.provider_nodes.retain(|node| node.id != id);
            let deleted = snapshot.provider_nodes.len() != original_len;
            if deleted {
                delete_provider_connections(&mut snapshot.provider_connections, id);
            }
            deleted
        })
    }
}

fn next_provider_node_id(snapshot: &StateSnapshot, node_type: &str) -> String {
    let prefix = provider_node_id_prefix(node_type);
    let mut index = snapshot
        .provider_nodes
        .iter()
        .filter(|node| node.node_type == node_type)
        .count()
        + 1;
    loop {
        let candidate = format!("{prefix}{index}");
        if !snapshot
            .provider_nodes
            .iter()
            .any(|node| node.id == candidate)
        {
            return candidate;
        }
        index += 1;
    }
}

fn provider_node_id_prefix(node_type: &str) -> &'static str {
    match node_type {
        OPENAI_COMPATIBLE => "openai-compatible-",
        ANTHROPIC_COMPATIBLE => "anthropic-compatible-",
        CUSTOM_EMBEDDING => "custom-embedding-",
        _ => "provider-node-",
    }
}

fn delete_provider_connections(connections: &mut Vec<ProviderConnection>, provider_id: &str) {
    connections.retain(|connection| connection.provider != provider_id);
}
