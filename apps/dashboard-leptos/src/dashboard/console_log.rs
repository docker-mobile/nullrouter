use serde::Serialize;

const MAX_LINES: usize = 200;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct ConsoleLogState {
    pub route_path: &'static str,
    pub title: &'static str,
    pub subtitle: &'static str,
    pub empty_text: &'static str,
    pub stream: ConsoleLogStreamState,
    pub retention: ConsoleLogRetention,
    pub clear_action: ConsoleLogAction,
    pub endpoints: &'static [ConsoleLogEndpoint],
    pub levels: &'static [ConsoleLogLevelStyle],
    pub logs: &'static [ConsoleLogLine],
    pub live_capture_wired: bool,
    pub live_capture_label: &'static str,
    pub wiring_label: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct ConsoleLogStreamState {
    pub status: ConsoleLogStreamStatus,
    pub connected: bool,
    pub label: &'static str,
    pub detail: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConsoleLogStreamStatus {
    Connected,
    Disconnected,
}

impl ConsoleLogStreamStatus {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Connected => "Connected",
            Self::Disconnected => "Disconnected",
        }
    }

    pub const fn class_name(self) -> &'static str {
        match self {
            Self::Connected => "is-connected",
            Self::Disconnected => "is-idle",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct ConsoleLogRetention {
    pub max_lines: usize,
    pub retained_lines: usize,
    pub retained_label: &'static str,
    pub max_label: &'static str,
    pub trim_label: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct ConsoleLogAction {
    pub label: &'static str,
    pub status_label: &'static str,
    pub enabled: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct ConsoleLogEndpoint {
    pub label: &'static str,
    pub method: &'static str,
    pub path: &'static str,
    pub wired: bool,
    pub status_label: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct ConsoleLogLevelStyle {
    pub level: ConsoleLogLevel,
    pub label: &'static str,
    pub class_name: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ConsoleLogLevel {
    Log,
    Info,
    Warn,
    Error,
    Debug,
}

impl ConsoleLogLevel {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Log => "LOG",
            Self::Info => "INFO",
            Self::Warn => "WARN",
            Self::Error => "ERROR",
            Self::Debug => "DEBUG",
        }
    }

    pub const fn class_name(self) -> &'static str {
        match self {
            Self::Log => "nr-console-level-log",
            Self::Info => "nr-console-level-info",
            Self::Warn => "nr-console-level-warn",
            Self::Error => "nr-console-level-error",
            Self::Debug => "nr-console-level-debug",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct ConsoleLogLine {
    pub level: ConsoleLogLevel,
    pub text: &'static str,
}

const STREAM: ConsoleLogStreamState = ConsoleLogStreamState {
    status: ConsoleLogStreamStatus::Disconnected,
    connected: false,
    label: "Stream status",
    detail: "EventSource stream unwired in this WASM slice",
};

const RETENTION: ConsoleLogRetention = ConsoleLogRetention {
    max_lines: MAX_LINES,
    retained_lines: 0,
    retained_label: "0 retained",
    max_label: "200 max",
    trim_label: "Newest 200 lines retained",
};

const CLEAR_ACTION: ConsoleLogAction = ConsoleLogAction {
    label: "Clear",
    status_label: "Clear endpoint unwired",
    enabled: false,
};

const ENDPOINTS: [ConsoleLogEndpoint; 2] = [
    ConsoleLogEndpoint {
        label: "History",
        method: "GET/DELETE",
        path: "/api/translator/console-logs",
        wired: false,
        status_label: "Clear endpoint unwired",
    },
    ConsoleLogEndpoint {
        label: "Stream",
        method: "GET",
        path: "/api/translator/console-logs/stream",
        wired: false,
        status_label: "EventSource stream unwired",
    },
];

const LEVELS: [ConsoleLogLevelStyle; 5] = [
    ConsoleLogLevelStyle {
        level: ConsoleLogLevel::Log,
        label: ConsoleLogLevel::Log.label(),
        class_name: ConsoleLogLevel::Log.class_name(),
    },
    ConsoleLogLevelStyle {
        level: ConsoleLogLevel::Info,
        label: ConsoleLogLevel::Info.label(),
        class_name: ConsoleLogLevel::Info.class_name(),
    },
    ConsoleLogLevelStyle {
        level: ConsoleLogLevel::Warn,
        label: ConsoleLogLevel::Warn.label(),
        class_name: ConsoleLogLevel::Warn.class_name(),
    },
    ConsoleLogLevelStyle {
        level: ConsoleLogLevel::Error,
        label: ConsoleLogLevel::Error.label(),
        class_name: ConsoleLogLevel::Error.class_name(),
    },
    ConsoleLogLevelStyle {
        level: ConsoleLogLevel::Debug,
        label: ConsoleLogLevel::Debug.label(),
        class_name: ConsoleLogLevel::Debug.class_name(),
    },
];

const LOGS: [ConsoleLogLine; 0] = [];

pub const fn console_log_dashboard_state() -> ConsoleLogState {
    ConsoleLogState {
        route_path: "/dashboard/console-log",
        title: "Console Log",
        subtitle: "Translator console output mirror",
        empty_text: "No console logs yet.",
        stream: STREAM,
        retention: RETENTION,
        clear_action: CLEAR_ACTION,
        endpoints: &ENDPOINTS,
        levels: &LEVELS,
        logs: &LOGS,
        live_capture_wired: false,
        live_capture_label: "No live capture",
        wiring_label: "EventSource stream unwired in this WASM slice",
    }
}
