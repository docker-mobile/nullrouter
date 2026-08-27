use super::registry::provider_definitions;
use super::types::MediaProviderPlaceholder;

pub(super) fn unknown_kind_placeholder(
    kind_id: &str,
    known: bool,
) -> Option<MediaProviderPlaceholder> {
    if known {
        None
    } else {
        Some(MediaProviderPlaceholder {
            title: "Unknown media provider kind",
            detail: format!("No upstream media provider kind named `{kind_id}` is available."),
        })
    }
}

pub(super) fn detail_placeholder(
    kind_id: &str,
    provider_id: &str,
    known_kind: bool,
    supported: bool,
) -> Option<MediaProviderPlaceholder> {
    if supported {
        None
    } else if !known_kind {
        Some(MediaProviderPlaceholder {
            title: "Unknown media provider kind",
            detail: format!("No upstream media provider kind named `{kind_id}` is available."),
        })
    } else if provider_definitions()
        .iter()
        .any(|provider| provider.id == provider_id)
    {
        Some(MediaProviderPlaceholder {
            title: "Provider does not support this kind",
            detail: format!("`{provider_id}` is known, but it is not listed for `{kind_id}`."),
        })
    } else {
        Some(MediaProviderPlaceholder {
            title: "Unknown provider",
            detail: format!("No upstream media provider named `{provider_id}` is available."),
        })
    }
}

pub(super) fn combo_placeholder(combo_id: &str, known: bool) -> Option<MediaProviderPlaceholder> {
    if known {
        None
    } else {
        Some(MediaProviderPlaceholder {
            title: "Combo not found",
            detail: format!("No upstream combo named `{combo_id}` is available."),
        })
    }
}
