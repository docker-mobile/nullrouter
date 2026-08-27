use nullrouter_dashboard_wasm::dashboard::{
    ConsoleLogLevel, ConsoleLogStreamStatus, console_log_dashboard_state,
};

#[test]
fn console_log_dashboard_state_exposes_upstream_stream_contract_when_unwired() {
    // Given: the WASM dashboard has no browser EventSource wiring for console logs yet.
    let state = console_log_dashboard_state();

    // When: the concrete console-log route consumes its typed state.
    let endpoint_paths = state
        .endpoints
        .iter()
        .map(|endpoint| endpoint.path)
        .collect::<Vec<_>>();

    // Then: it mirrors the upstream routes while truthfully reporting no live capture.
    assert_eq!(state.route_path, "/dashboard/console-log");
    assert_eq!(state.title, "Console Log");
    assert_eq!(state.empty_text, "No console logs yet.");
    assert_eq!(state.stream.status, ConsoleLogStreamStatus::Disconnected);
    assert!(!state.stream.connected);
    assert!(!state.live_capture_wired);
    assert_eq!(state.live_capture_label, "No live capture");
    assert_eq!(
        state.wiring_label,
        "EventSource stream unwired in this WASM slice"
    );
    assert_eq!(
        endpoint_paths,
        [
            "/api/translator/console-logs",
            "/api/translator/console-logs/stream",
        ]
    );
    assert!(state.logs.is_empty());
}

#[test]
fn console_log_dashboard_state_keeps_level_colors_and_trim_metadata_explicit() {
    // Given: the upstream console log buffer keeps the newest CONSOLE_LOG_CONFIG.maxLines lines.
    let state = console_log_dashboard_state();

    // When: level styles and retention metadata are exposed to the panel.
    let levels = state
        .levels
        .iter()
        .map(|level| (level.level, level.label, level.class_name))
        .collect::<Vec<_>>();

    // Then: the WASM route preserves level color semantics and the 200-line trim contract.
    assert_eq!(state.retention.max_lines, 200);
    assert_eq!(state.retention.retained_lines, 0);
    assert_eq!(state.retention.trim_label, "Newest 200 lines retained");
    assert_eq!(
        levels,
        [
            (ConsoleLogLevel::Log, "LOG", "nr-console-level-log"),
            (ConsoleLogLevel::Info, "INFO", "nr-console-level-info"),
            (ConsoleLogLevel::Warn, "WARN", "nr-console-level-warn"),
            (ConsoleLogLevel::Error, "ERROR", "nr-console-level-error"),
            (ConsoleLogLevel::Debug, "DEBUG", "nr-console-level-debug"),
        ]
    );
    assert_eq!(ConsoleLogStreamStatus::Connected.label(), "Connected");
    assert_eq!(ConsoleLogStreamStatus::Disconnected.label(), "Disconnected");
}
