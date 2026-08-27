use nullrouter_dashboard_wasm::dashboard::{
    ProxyPoolTestStatus, ProxyPoolType, proxy_pools_dashboard_state, proxy_pools_sample_state,
};

#[test]
fn proxy_pools_dashboard_state_exposes_upstream_management_controls_when_unwired() {
    // Given: the WASM dashboard has no host proxy-pools API feed.
    let state = proxy_pools_dashboard_state();

    // When: the Proxy Pools route consumes its deterministic default state.
    let relay_labels = state
        .relay_actions
        .iter()
        .map(|action| action.label)
        .collect::<Vec<_>>();
    let action_labels = state
        .header_actions
        .iter()
        .map(|action| action.label)
        .collect::<Vec<_>>();

    // Then: the upstream management surface is present without claiming persistence.
    assert_eq!(state.route_path, "/dashboard/proxy-pools");
    assert_eq!(state.title, "Proxy Pools");
    assert_eq!(state.totals.total, 0);
    assert_eq!(state.totals.active, 0);
    assert!(state.entries.is_empty());
    assert_eq!(state.empty.title, "No proxy pool entries yet");
    assert_eq!(
        state.empty.detail,
        "Create a proxy pool entry, then assign it to connections."
    );
    assert!(action_labels.contains(&"Deploy Relay"));
    assert!(action_labels.contains(&"Batch Import"));
    assert!(action_labels.contains(&"Add Proxy Pool"));
    assert_eq!(
        relay_labels,
        ["Cloudflare Relay", "Vercel Relay", "Deno Relay"]
    );
    assert!(
        state
            .relay_actions
            .iter()
            .all(|action| !action.deployment_wired)
    );
    assert_eq!(state.selection.select_all_label, "Select all");
    assert_eq!(state.selection.selected_label, "0 selected");
    assert_eq!(state.selection.health_label, "Health Check");
    assert_eq!(state.selection.health_progress_label, "Checking 0/0");
    assert!(
        state
            .selection
            .bulk_actions
            .iter()
            .any(|action| action.label == "Activate")
    );
}

#[test]
fn proxy_pools_modal_defaults_include_labels_placeholders_and_unwired_status() {
    // Given: proxy-pool modal fixtures mirror the upstream page's default forms.
    let state = proxy_pools_dashboard_state();

    // When: modal data is serialized at the dashboard boundary.
    let json = serde_json::to_string(&state.modals);

    // Then: each modal has the key default labels and truthful disabled status copy.
    assert!(json.is_ok());
    assert_eq!(state.modals.batch_import.title, "Batch Import Proxies");
    assert!(state.modals.batch_import.fields.iter().any(|field| {
        field.label == "Paste Proxy List (One per line)"
            && field.placeholder.contains("127.0.0.1:7897:user:pass")
    }));
    assert_eq!(state.modals.form.title, "Add/Edit Proxy Pool");
    assert!(state.modals.form.fields.iter().any(|field| {
        field.label == "Proxy URL" && field.placeholder == "http://127.0.0.1:7897"
    }));
    assert!(state.modals.form.fields.iter().any(|field| {
        field.label == "No Proxy" && field.placeholder == "localhost,127.0.0.1,.internal"
    }));
    assert_eq!(state.modals.vercel.title, "Deploy Vercel Relay");
    assert_eq!(state.modals.vercel.primary_label, "Deploy");
    assert!(state.modals.vercel.fields.iter().any(|field| {
        field.label == "Vercel API Token" && field.placeholder == "your-vercel-api-token"
    }));
    assert_eq!(state.modals.cloudflare.title, "Deploy Cloudflare Relay");
    assert_eq!(state.modals.cloudflare.primary_label, "Deploy Worker");
    assert!(state.modals.cloudflare.fields.iter().any(|field| {
        field.label == "API Token" && field.placeholder == "your-cloudflare-api-token"
    }));
    assert_eq!(state.modals.deno.title, "Deploy Deno Relay");
    assert_eq!(state.modals.deno.primary_label, "Deploy Relay");
    assert!(state.modals.deno.fields.iter().any(|field| {
        field.label == "Deno Deploy API Token" && field.placeholder == "ddo_xxxxxxxxxxxxxxxx"
    }));
    assert_eq!(
        state.modals.cloudflare.unsupported_label,
        "Deployment not wired in the WASM dashboard."
    );
}

#[test]
fn proxy_pools_sample_state_exposes_row_badges_and_actions_for_tests() {
    // Given: tests need a deterministic non-empty row state separate from host data.
    let state = proxy_pools_sample_state();

    // When: the sample row is inspected by the route and tests.
    let mut entries = state.entries.iter();
    let entry = entries.next();

    // Then: row status, active/type/bound badges, metadata, and actions are stable.
    assert_eq!(state.totals.total, 1);
    assert_eq!(state.totals.active, 1);
    assert_eq!(state.selection.selected_label, "1 selected");
    assert_eq!(state.selection.health_progress_label, "Checking 1/1");
    assert!(entry.is_some());
    assert!(entries.next().is_none());
    if let Some(entry) = entry {
        assert_eq!(entry.name, "Cloudflare edge relay");
        assert_eq!(entry.test_status, ProxyPoolTestStatus::Active);
        assert_eq!(entry.test_status.label(), "active");
        assert!(entry.is_active);
        assert_eq!(entry.proxy_type, ProxyPoolType::CloudflareRelay);
        assert_eq!(entry.proxy_type.badge_label(), Some("cloudflare relay"));
        assert_eq!(entry.bound_connection_count, 2);
        assert_eq!(
            entry.proxy_url,
            "https://cloudflare-relay.example.workers.dev"
        );
        assert_eq!(entry.no_proxy, Some("localhost,127.0.0.1,.internal"));
        assert_eq!(entry.last_tested_label, "Last tested: Jul 12, 2026, 09:10");
        assert!(entry.last_error.is_none());
        assert!(entry.strict_proxy);
        assert_eq!(entry.actions.toggle_label, "Disable");
        assert_eq!(entry.actions.test_label, "Test proxy");
        assert_eq!(entry.actions.edit_label, "Edit");
        assert_eq!(entry.actions.delete_label, "Delete");
    }
}
