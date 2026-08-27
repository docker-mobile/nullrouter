use super::types::MediaProviderAction;

pub(super) const KIND_ACTIONS: [MediaProviderAction; 4] = [
    MediaProviderAction {
        label: "Toggle Provider",
        status_label: "Preview only",
        enabled: false,
    },
    MediaProviderAction {
        label: "Create Combo",
        status_label: "Mutation disabled",
        enabled: false,
    },
    MediaProviderAction {
        label: "Add Custom Embedding",
        status_label: "Mutation disabled",
        enabled: false,
    },
    MediaProviderAction {
        label: "Refresh Connections",
        status_label: "Provider API offline",
        enabled: false,
    },
];

pub(super) const DETAIL_CONNECTION_ACTIONS: [MediaProviderAction; 4] = [
    MediaProviderAction {
        label: "Add Connection",
        status_label: "Preview only",
        enabled: false,
    },
    MediaProviderAction {
        label: "Test Connection One-by-One",
        status_label: "Execution unavailable",
        enabled: false,
    },
    MediaProviderAction {
        label: "Enable Provider",
        status_label: "Mutation disabled",
        enabled: false,
    },
    MediaProviderAction {
        label: "Edit Models",
        status_label: "Persistence unsupported",
        enabled: false,
    },
];

pub(super) const DETAIL_TEST_ACTIONS: [MediaProviderAction; 3] = [
    MediaProviderAction {
        label: "Run Example",
        status_label: "Execution unavailable",
        enabled: false,
    },
    MediaProviderAction {
        label: "Copy Curl",
        status_label: "Preview only",
        enabled: false,
    },
    MediaProviderAction {
        label: "Save Settings",
        status_label: "Persistence unsupported",
        enabled: false,
    },
];

pub(super) const COMBO_ACTIONS: [MediaProviderAction; 6] = [
    MediaProviderAction {
        label: "Save Settings",
        status_label: "Persistence unsupported",
        enabled: false,
    },
    MediaProviderAction {
        label: "Add Provider",
        status_label: "Mutation disabled",
        enabled: false,
    },
    MediaProviderAction {
        label: "Move Provider",
        status_label: "Mutation disabled",
        enabled: false,
    },
    MediaProviderAction {
        label: "Remove Provider",
        status_label: "Mutation disabled",
        enabled: false,
    },
    MediaProviderAction {
        label: "Run",
        status_label: "Execution unavailable",
        enabled: false,
    },
    MediaProviderAction {
        label: "Delete",
        status_label: "Mutation disabled",
        enabled: false,
    },
];
