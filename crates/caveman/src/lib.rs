//! NullStack Caveman — terse *output* compression.
//!
//! Own-brand port of OmniRoute/9Router's Caveman (⭐52K, 6 levels) but
//! Rust-native, prompt-injection via `crates/translate` system injection,
//! not Node. Stacks with RTK (`rtk`) as `rtk → caveman` for 78-95% total
//! (avg 89.2% = 1-(1-0.80)*(1-0.46) per `COMPRESSION_ENGINES.md:227`).

/// Caveman intensity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Level {
    Off,
    Lite,
    #[default]
    Standard,
    Ultra,
    /// CJK Wenyan variant.
    WenyanLite,
    Wenyan,
    WenyanUltra,
}

impl Level {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Lite => "lite",
            Self::Standard => "standard",
            Self::Ultra => "ultra",
            Self::WenyanLite => "wenyan-lite",
            Self::Wenyan => "wenyan",
            Self::WenyanUltra => "wenyan-ultra",
        }
    }

    #[must_use]
    pub fn savings_hint(self) -> &'static str {
        match self {
            Self::Off => "0%",
            Self::Lite => "~30%",
            Self::Standard => "46% input / 65% output",
            Self::Ultra => "up to 75%",
            Self::WenyanLite => "~30% (wenyan)",
            Self::Wenyan => "46% (wenyan)",
            Self::WenyanUltra => "65% (wenyan)",
        }
    }
}

/// System prompt injected for the level. Preserved verbatim from our
/// brand voice — not a copy of OmniRoute's `cavemanPrompts.js`.
pub fn system_prompt(level: Level) -> Option<&'static str> {
    match level {
        Level::Off => None,
        Level::Lite => Some(
            "Be terse. Remove filler and hedges. Keep code, paths, URLs, errors verbatim. \
             Preserve the user's language.",
        ),
        Level::Standard => Some(
            "You are NullStack Caveman (standard). Output tersely: strip filler/hedges, \
             keep technical substance, code blocks/paths/commands/errors/URLs verbatim, \
             security warnings verbatim, no invented abbreviations, preserve user language, \
             no narration or emoji. Stacks with RTK.",
        ),
        Level::Ultra => Some(
            "You are NullStack Caveman (ultra). Maximally terse: delete every non-essential \
             token, keep only substance and verbatim code/paths/errors/URLs. Preserve \
             validation/security/a11y notes. No self-reference.",
        ),
        Level::WenyanLite | Level::Wenyan | Level::WenyanUltra => Some(
            "You are NullStack Caveman (wenyan). 用文言文风格极度简练地回答，保留代码、路径、错误原文与用户语言。",
        ),
    }
}

/// Decide whether to inject for this request. Fail-open.
#[must_use]
pub fn should_inject(level: Level) -> bool {
    level != Level::Off
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_exists_for_standard() {
        assert!(system_prompt(Level::Standard).is_some());
    }

    #[test]
    fn off_has_no_prompt() {
        assert!(system_prompt(Level::Off).is_none());
    }
}
