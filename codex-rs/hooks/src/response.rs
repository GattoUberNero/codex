use serde::Deserialize;
use serde::Deserializer;
use serde::Serialize;
use tracing::warn;

#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NeroHookMsgMode {
    Synced,
    #[serde(alias = "tui_short")]
    TuiShort,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NeroHookMsgShow {
    pub agent: bool,
    pub tui: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NeroHookMsgContent {
    pub full: String,
    pub short: String,
}

fn deserialize_u64_or_default<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Option::<u64>::deserialize(deserializer)?.unwrap_or(0))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HookAction {
    NeroHookMsg {
        mode: NeroHookMsgMode,
        show: NeroHookMsgShow,
        #[serde(default, deserialize_with = "deserialize_u64_or_default")]
        freq: u64,
        msg: NeroHookMsgContent,
    },
    VisibleNote { message: String },
    ContextNote { message: String },
    DualNote {
        tui_message: String,
        agent_message: String,
    },
    AutoUserReply { message: String },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedHookActions {
    pub actions: Vec<HookAction>,
    pub ignored_unknown_actions: usize,
}

#[derive(Debug, Deserialize)]
struct HookActionEnvelope {
    #[serde(default)]
    actions: Vec<serde_json::Value>,
}

pub fn parse_hook_actions_from_stdout(stdout: &str) -> Result<ParsedHookActions, serde_json::Error> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return Ok(ParsedHookActions::default());
    }

    let envelope: HookActionEnvelope = serde_json::from_str(trimmed)?;
    let mut parsed = ParsedHookActions::default();

    for raw_action in envelope.actions {
        match serde_json::from_value::<HookAction>(raw_action.clone()) {
            Ok(action) => parsed.actions.push(action),
            Err(_) if is_unknown_action_type(&raw_action) => {
                warn!(raw_action = %raw_action, "ignoring unknown hook action type");
                parsed.ignored_unknown_actions += 1;
            }
            Err(err) => return Err(err),
        }
    }

    Ok(parsed)
}

fn is_unknown_action_type(value: &serde_json::Value) -> bool {
    let Some(obj) = value.as_object() else {
        return false;
    };
    let Some(type_value) = obj.get("type") else {
        return false;
    };
    let Some(kind) = type_value.as_str() else {
        return false;
    };
    !matches!(
        kind,
        "nero_hook_msg" | "visible_note" | "context_note" | "dual_note" | "auto_user_reply"
    )
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn parse_empty_stdout_returns_no_actions() {
        let parsed = parse_hook_actions_from_stdout("   \n").expect("parse");
        assert_eq!(parsed, ParsedHookActions::default());
    }

    #[test]
    fn missing_actions_field_defaults_to_no_actions() {
        let parsed = parse_hook_actions_from_stdout("{}").expect("parse");
        assert_eq!(parsed, ParsedHookActions::default());
    }

    #[test]
    fn parse_valid_actions() {
        let parsed = parse_hook_actions_from_stdout(
            r#"{
              "actions": [
                {"type": "visible_note", "message": "[nero-hook] ok"},
                {"type": "context_note", "message": "internal note"},
                {"type": "dual_note", "tui_message": "short", "agent_message": "full"},
                {"type": "nero_hook_msg", "mode": "tui-short", "show": {"agent": true, "tui": true}, "msg": {"full": "FULL", "short": "SHORT"}},
                {"type": "auto_user_reply", "message": "continue"}
              ]
            }"#,
        )
        .expect("parse");

        assert_eq!(
            parsed.actions,
            vec![
                HookAction::VisibleNote {
                    message: "[nero-hook] ok".to_string()
                },
                HookAction::ContextNote {
                    message: "internal note".to_string()
                },
                HookAction::DualNote {
                    tui_message: "short".to_string(),
                    agent_message: "full".to_string()
                },
                HookAction::NeroHookMsg {
                    mode: NeroHookMsgMode::TuiShort,
                    show: NeroHookMsgShow {
                        agent: true,
                        tui: true
                    },
                    freq: 0,
                    msg: NeroHookMsgContent {
                        full: "FULL".to_string(),
                        short: "SHORT".to_string()
                    }
                },
                HookAction::AutoUserReply {
                    message: "continue".to_string()
                }
            ]
        );
        assert_eq!(parsed.ignored_unknown_actions, 0);
    }

    #[test]
    fn parse_invalid_json_errors() {
        let err = parse_hook_actions_from_stdout("{not-json").expect_err("invalid json");
        let msg = err.to_string();
        assert!(msg.contains("expected") || msg.contains("key"));
    }

    #[test]
    fn unknown_action_type_is_ignored_and_counted() {
        let parsed = parse_hook_actions_from_stdout(
            r#"{
              "actions": [
                {"type": "visible_note", "message": "a"},
                {"type": "future_magic_action", "message": "b"}
              ]
            }"#,
        )
        .expect("parse");

        assert_eq!(
            parsed.actions,
            vec![HookAction::VisibleNote {
                message: "a".to_string()
            }]
        );
        assert_eq!(parsed.ignored_unknown_actions, 1);
    }

    #[test]
    fn malformed_known_action_errors() {
        let err = parse_hook_actions_from_stdout(
            r#"{
              "actions": [
                {"type": "visible_note"}
              ]
            }"#,
        )
        .expect_err("missing message should error");

        assert!(err.to_string().contains("message"));
    }

    #[test]
    fn nero_hook_msg_mode_accepts_tui_short_alias() {
        let parsed = parse_hook_actions_from_stdout(
            r#"{
              "actions": [
                {
                  "type": "nero_hook_msg",
                  "mode": "tui_short",
                  "show": {"agent": true, "tui": true},
                  "msg": {"full": "f", "short": "s"}
                }
              ]
            }"#,
        )
        .expect("parse");

        assert_eq!(
            parsed.actions,
            vec![HookAction::NeroHookMsg {
                mode: NeroHookMsgMode::TuiShort,
                show: NeroHookMsgShow {
                    agent: true,
                    tui: true
                },
                freq: 0,
                msg: NeroHookMsgContent {
                    full: "f".to_string(),
                    short: "s".to_string()
                }
            }]
        );
    }

    #[test]
    fn nero_hook_msg_parses_freq_when_present() {
        let parsed = parse_hook_actions_from_stdout(
            r#"{
              "actions": [
                {
                  "type": "nero_hook_msg",
                  "mode": "tui-short",
                  "show": {"agent": true, "tui": true},
                  "freq": 120,
                  "msg": {"full": "f", "short": "s"}
                }
              ]
            }"#,
        )
        .expect("parse");

        assert_eq!(
            parsed.actions,
            vec![HookAction::NeroHookMsg {
                mode: NeroHookMsgMode::TuiShort,
                show: NeroHookMsgShow {
                    agent: true,
                    tui: true
                },
                freq: 120,
                msg: NeroHookMsgContent {
                    full: "f".to_string(),
                    short: "s".to_string()
                }
            }]
        );
    }

    #[test]
    fn nero_hook_msg_parses_null_freq_as_zero() {
        let parsed = parse_hook_actions_from_stdout(
            r#"{
              "actions": [
                {
                  "type": "nero_hook_msg",
                  "mode": "tui-short",
                  "show": {"agent": true, "tui": true},
                  "freq": null,
                  "msg": {"full": "f", "short": "s"}
                }
              ]
            }"#,
        )
        .expect("parse");

        assert_eq!(
            parsed.actions,
            vec![HookAction::NeroHookMsg {
                mode: NeroHookMsgMode::TuiShort,
                show: NeroHookMsgShow {
                    agent: true,
                    tui: true
                },
                freq: 0,
                msg: NeroHookMsgContent {
                    full: "f".to_string(),
                    short: "s".to_string()
                }
            }]
        );
    }
}
