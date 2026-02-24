/// The current Codex CLI version as embedded at compile time.
pub const CODEX_CLI_VERSION: &str = env!("CARGO_PKG_VERSION");

const CODEXN_DISPLAY_VERSION_ENV: &str = "CODEXN_DISPLAY_VERSION";
static DISPLAY_VERSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();

pub fn codex_cli_display_version() -> &'static str {
    DISPLAY_VERSION
        .get_or_init(|| {
            std::env::var(CODEXN_DISPLAY_VERSION_ENV)
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| CODEX_CLI_VERSION.to_string())
        })
        .as_str()
}
