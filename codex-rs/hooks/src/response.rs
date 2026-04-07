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

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum NeroHookMsgFormat {
    #[default]
    Block,
    Inline,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NeroHookMsgStatus {
    pub kind: String,
    pub text: String,
    #[serde(default)]
    pub meta: Option<serde_json::Value>,
}

fn deserialize_nero_hook_msg_format_or_default<'de, D>(
    deserializer: D,
) -> Result<NeroHookMsgFormat, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = Option::<serde_json::Value>::deserialize(deserializer)?;
    let Some(raw) = raw else {
        return Ok(NeroHookMsgFormat::Block);
    };
    match raw.as_str() {
        Some("block") => Ok(NeroHookMsgFormat::Block),
        Some("inline") => Ok(NeroHookMsgFormat::Inline),
        _ => Ok(NeroHookMsgFormat::Block),
    }
}

fn deserialize_optional_nero_hook_msg_status_lossy<'de, D>(
    deserializer: D,
) -> Result<Option<NeroHookMsgStatus>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = Option::<serde_json::Value>::deserialize(deserializer)?;
    let Some(raw) = raw else {
        return Ok(None);
    };
    let Some(obj) = raw.as_object() else {
        return Ok(None);
    };
    let Some(kind) = obj.get("kind").and_then(|v| v.as_str()) else {
        return Ok(None);
    };
    let Some(text) = obj.get("text").and_then(|v| v.as_str()) else {
        return Ok(None);
    };
    let meta = obj
        .get("meta")
        .and_then(|value| value.as_object().map(|_| value.clone()));
    Ok(Some(NeroHookMsgStatus {
        kind: kind.to_string(),
        text: text.to_string(),
        meta,
    }))
}

fn deserialize_u64_or_default<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Option::<u64>::deserialize(deserializer)?.unwrap_or(0))
}

fn deserialize_optional_nonneg_u64_lossy<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = Option::<serde_json::Value>::deserialize(deserializer)?;
    let Some(raw) = raw else {
        return Ok(None);
    };
    Ok(raw.as_u64())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NeroHookAction {
    NeroHookMsg {
        mode: NeroHookMsgMode,
        show: NeroHookMsgShow,
        #[serde(default, deserialize_with = "deserialize_u64_or_default")]
        freq: u64,
        #[serde(
            default,
            deserialize_with = "deserialize_nero_hook_msg_format_or_default"
        )]
        format: NeroHookMsgFormat,
        #[serde(
            default,
            deserialize_with = "deserialize_optional_nero_hook_msg_status_lossy"
        )]
        status: Option<NeroHookMsgStatus>,
        msg: NeroHookMsgContent,
    },
    VisibleNote {
        message: String,
    },
    AutoUserReply {
        message: String,
        #[serde(default, deserialize_with = "deserialize_optional_nonneg_u64_lossy")]
        expected_wait_seconds: Option<u64>,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedNeroHookActions {
    pub actions: Vec<NeroHookAction>,
    pub ignored_unknown_actions: usize,
}

#[derive(Debug, Deserialize)]
struct NeroHookActionEnvelope {
    #[serde(default)]
    actions: Vec<serde_json::Value>,
}

fn parse_nero_hook_action_envelope(
    json_payload: &str,
) -> Result<ParsedNeroHookActions, serde_json::Error> {
    let envelope: NeroHookActionEnvelope = serde_json::from_str(json_payload)?;
    let mut parsed = ParsedNeroHookActions::default();

    for raw_action in envelope.actions {
        match serde_json::from_value::<NeroHookAction>(raw_action.clone()) {
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

pub fn parse_nero_hook_actions_from_stdout(
    stdout: &str,
) -> Result<ParsedNeroHookActions, serde_json::Error> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return Ok(ParsedNeroHookActions::default());
    }

    let strict_err = match parse_nero_hook_action_envelope(trimmed) {
        Ok(parsed) => return Ok(parsed),
        Err(err) => err,
    };

    // Compatibility recovery for hooks that accidentally emit plaintext log lines
    // before a canonical JSON envelope in the final line.
    let lines: Vec<&str> = trimmed
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    if let [prefix @ .., candidate] = lines.as_slice()
        && !prefix.is_empty()
    {
        let prefix_contains_jsonish_tokens = prefix
            .iter()
            .any(|line| line.contains('{') || line.contains('}'));
        if !prefix_contains_jsonish_tokens
            && candidate.starts_with('{')
            && candidate.ends_with('}')
            && (candidate.starts_with("{\"actions\"") || candidate.starts_with("{ \"actions\""))
            && let Ok(parsed) = parse_nero_hook_action_envelope(candidate)
        {
            warn!(
                stdout_len = trimmed.len(),
                recovered_json_len = candidate.len(),
                "recovered hook actions from trailing json line"
            );
            return Ok(parsed);
        }
    }

    Err(strict_err)
}

fn is_unknown_action_type(value: &serde_json::Value) -> bool {
    let Some(kind) = action_type(value) else {
        return false;
    };
    !matches!(kind, "nero_hook_msg" | "visible_note" | "auto_user_reply")
}

fn action_type(value: &serde_json::Value) -> Option<&str> {
    let obj = value.as_object()?;
    let type_value = obj.get("type")?;
    type_value.as_str()
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn parse_empty_stdout_returns_no_actions() {
        let parsed = parse_nero_hook_actions_from_stdout("   \n").expect("parse");
        assert_eq!(parsed, ParsedNeroHookActions::default());
    }

    #[test]
    fn missing_actions_field_defaults_to_no_actions() {
        let parsed = parse_nero_hook_actions_from_stdout("{}").expect("parse");
        assert_eq!(parsed, ParsedNeroHookActions::default());
    }

    #[test]
    fn parse_valid_actions() {
        let parsed = parse_nero_hook_actions_from_stdout(
            r#"{
              "actions": [
                {"type": "visible_note", "message": "[nero-hook] ok"},
                {"type": "nero_hook_msg", "mode": "tui-short", "show": {"agent": true, "tui": true}, "msg": {"full": "FULL", "short": "SHORT"}},
                {"type": "auto_user_reply", "message": "continue"}
              ]
            }"#,
        )
        .expect("parse");

        assert_eq!(
            parsed.actions,
            vec![
                NeroHookAction::VisibleNote {
                    message: "[nero-hook] ok".to_string()
                },
                NeroHookAction::NeroHookMsg {
                    mode: NeroHookMsgMode::TuiShort,
                    show: NeroHookMsgShow {
                        agent: true,
                        tui: true
                    },
                    freq: 0,
                    format: NeroHookMsgFormat::Block,
                    status: None,
                    msg: NeroHookMsgContent {
                        full: "FULL".to_string(),
                        short: "SHORT".to_string()
                    }
                },
                NeroHookAction::AutoUserReply {
                    message: "continue".to_string(),
                    expected_wait_seconds: None
                }
            ]
        );
        assert_eq!(parsed.ignored_unknown_actions, 0);
    }

    #[test]
    fn parse_invalid_json_errors() {
        let err = parse_nero_hook_actions_from_stdout("{not-json").expect_err("invalid json");
        let msg = err.to_string();
        assert!(msg.contains("expected") || msg.contains("key"));
    }

    #[test]
    fn parse_recovers_json_after_plaintext_prefix_line() {
        let parsed = parse_nero_hook_actions_from_stdout(
            r#"legacy warning line
{"actions":[{"type":"visible_note","message":"ok"}]}"#,
        )
        .expect("parse");

        assert_eq!(
            parsed.actions,
            vec![NeroHookAction::VisibleNote {
                message: "ok".to_string()
            }]
        );
        assert_eq!(parsed.ignored_unknown_actions, 0);
    }

    #[test]
    fn parse_wrapped_json_in_noise_still_errors() {
        let err = parse_nero_hook_actions_from_stdout(
            r#"prefix >>> {"actions":[{"type":"visible_note","message":"ctx"}]} <<< suffix"#,
        )
        .expect_err("wrapped json should not be recovered");
        assert!(!err.to_string().is_empty());
    }

    #[test]
    fn unknown_action_type_is_ignored_and_counted() {
        let parsed = parse_nero_hook_actions_from_stdout(
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
            vec![NeroHookAction::VisibleNote {
                message: "a".to_string()
            }]
        );
        assert_eq!(parsed.ignored_unknown_actions, 1);
    }

    #[test]
    fn mixed_known_and_unknown_action_types_keep_known_actions_and_count_unknowns() {
        let parsed = parse_nero_hook_actions_from_stdout(
            r#"{
              "actions": [
                {"type": "visible_note", "message": "keep me"},
                {"type": "mystery_action_alpha", "message": "legacy"},
                {"type": "nero_hook_msg", "mode": "tui-short", "show": {"agent": true, "tui": false}, "msg": {"full": "FULL", "short": "SHORT"}},
                {"type": "opaque_action_beta", "payload": {"tui_message": "short", "agent_message": "full"}},
                {"type": "future_magic_action", "message": "future"},
                {"type": "auto_user_reply", "message": "continue"}
              ]
            }"#,
        )
        .expect("parse");

        assert_eq!(
            parsed.actions,
            vec![
                NeroHookAction::VisibleNote {
                    message: "keep me".to_string()
                },
                NeroHookAction::NeroHookMsg {
                    mode: NeroHookMsgMode::TuiShort,
                    show: NeroHookMsgShow {
                        agent: true,
                        tui: false
                    },
                    freq: 0,
                    format: NeroHookMsgFormat::Block,
                    status: None,
                    msg: NeroHookMsgContent {
                        full: "FULL".to_string(),
                        short: "SHORT".to_string()
                    }
                },
                NeroHookAction::AutoUserReply {
                    message: "continue".to_string(),
                    expected_wait_seconds: None
                }
            ]
        );
        assert_eq!(parsed.ignored_unknown_actions, 3);
    }

    #[test]
    fn parse_auto_user_reply_preserves_expected_wait_seconds() {
        let parsed = parse_nero_hook_actions_from_stdout(
            r#"{
              "actions": [
                {"type": "auto_user_reply", "message": "continue", "expected_wait_seconds": 2}
              ]
            }"#,
        )
        .expect("parse");

        assert_eq!(
            parsed.actions,
            vec![NeroHookAction::AutoUserReply {
                message: "continue".to_string(),
                expected_wait_seconds: Some(2),
            }]
        );
        assert_eq!(parsed.ignored_unknown_actions, 0);
    }

    #[test]
    fn parse_auto_user_reply_drops_invalid_expected_wait_seconds() {
        let parsed = parse_nero_hook_actions_from_stdout(
            r#"{
              "actions": [
                {"type": "auto_user_reply", "message": "continue", "expected_wait_seconds": true},
                {"type": "auto_user_reply", "message": "continue again", "expected_wait_seconds": -1}
              ]
            }"#,
        )
        .expect("parse");

        assert_eq!(
            parsed.actions,
            vec![
                NeroHookAction::AutoUserReply {
                    message: "continue".to_string(),
                    expected_wait_seconds: None,
                },
                NeroHookAction::AutoUserReply {
                    message: "continue again".to_string(),
                    expected_wait_seconds: None,
                }
            ]
        );
        assert_eq!(parsed.ignored_unknown_actions, 0);
    }

    #[test]
    fn malformed_known_action_errors() {
        let err = parse_nero_hook_actions_from_stdout(
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
        let parsed = parse_nero_hook_actions_from_stdout(
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
            vec![NeroHookAction::NeroHookMsg {
                mode: NeroHookMsgMode::TuiShort,
                show: NeroHookMsgShow {
                    agent: true,
                    tui: true
                },
                freq: 0,
                format: NeroHookMsgFormat::Block,
                status: None,
                msg: NeroHookMsgContent {
                    full: "f".to_string(),
                    short: "s".to_string()
                }
            }]
        );
    }

    #[test]
    fn nero_hook_msg_parses_freq_when_present() {
        let parsed = parse_nero_hook_actions_from_stdout(
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
            vec![NeroHookAction::NeroHookMsg {
                mode: NeroHookMsgMode::TuiShort,
                show: NeroHookMsgShow {
                    agent: true,
                    tui: true
                },
                freq: 120,
                format: NeroHookMsgFormat::Block,
                status: None,
                msg: NeroHookMsgContent {
                    full: "f".to_string(),
                    short: "s".to_string()
                }
            }]
        );
    }

    #[test]
    fn nero_hook_msg_parses_null_freq_as_zero() {
        let parsed = parse_nero_hook_actions_from_stdout(
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
            vec![NeroHookAction::NeroHookMsg {
                mode: NeroHookMsgMode::TuiShort,
                show: NeroHookMsgShow {
                    agent: true,
                    tui: true
                },
                freq: 0,
                format: NeroHookMsgFormat::Block,
                status: None,
                msg: NeroHookMsgContent {
                    full: "f".to_string(),
                    short: "s".to_string()
                }
            }]
        );
    }

    #[test]
    fn nero_hook_msg_parses_format_and_status_when_present() {
        let parsed = parse_nero_hook_actions_from_stdout(
            r#"{
              "actions": [
                {
                  "type": "nero_hook_msg",
                  "mode": "tui-short",
                  "show": {"agent": true, "tui": true},
                  "format": "inline",
                  "status": {"kind": "countdown", "text": "next update in 7s"},
                  "msg": {"full": "f", "short": "s"}
                }
              ]
            }"#,
        )
        .expect("parse");

        assert_eq!(
            parsed.actions,
            vec![NeroHookAction::NeroHookMsg {
                mode: NeroHookMsgMode::TuiShort,
                show: NeroHookMsgShow {
                    agent: true,
                    tui: true
                },
                freq: 0,
                format: NeroHookMsgFormat::Inline,
                status: Some(NeroHookMsgStatus {
                    kind: "countdown".to_string(),
                    text: "next update in 7s".to_string(),
                    meta: None,
                }),
                msg: NeroHookMsgContent {
                    full: "f".to_string(),
                    short: "s".to_string()
                }
            }]
        );
    }

    #[test]
    fn nero_hook_msg_parses_status_meta_when_present() {
        let parsed = parse_nero_hook_actions_from_stdout(
            r#"{
              "actions": [
                {
                  "type": "nero_hook_msg",
                  "mode": "synced",
                  "show": {"agent": true, "tui": false},
                  "status": {
                    "kind": "auto",
                    "text": "continue: score=9 threshold=5 round=0",
                    "meta": {
                      "auto_decision": {
                        "decision": "continue",
                        "reason_code": "continue"
                      }
                    }
                  },
                  "msg": {"full": "f", "short": "s"}
                }
              ]
            }"#,
        )
        .expect("parse");

        let NeroHookAction::NeroHookMsg { status, .. } = &parsed.actions[0] else {
            panic!("expected nero_hook_msg action");
        };
        let status = status.as_ref().expect("status");
        let meta = status.meta.as_ref().expect("status meta");
        let decision = meta
            .get("auto_decision")
            .and_then(|value| value.as_object())
            .and_then(|value| value.get("decision"))
            .and_then(|value| value.as_str());
        assert_eq!(decision, Some("continue"));
    }

    #[test]
    fn nero_hook_msg_unknown_format_defaults_to_block() {
        let parsed = parse_nero_hook_actions_from_stdout(
            r#"{
              "actions": [
                {
                  "type": "nero_hook_msg",
                  "mode": "tui-short",
                  "show": {"agent": true, "tui": true},
                  "format": "future-fancy",
                  "msg": {"full": "f", "short": "s"}
                }
              ]
            }"#,
        )
        .expect("parse");

        assert_eq!(
            parsed.actions,
            vec![NeroHookAction::NeroHookMsg {
                mode: NeroHookMsgMode::TuiShort,
                show: NeroHookMsgShow {
                    agent: true,
                    tui: true
                },
                freq: 0,
                format: NeroHookMsgFormat::Block,
                status: None,
                msg: NeroHookMsgContent {
                    full: "f".to_string(),
                    short: "s".to_string()
                }
            }]
        );
    }

    #[test]
    fn nero_hook_msg_non_string_format_defaults_to_block() {
        let parsed = parse_nero_hook_actions_from_stdout(
            r#"{
              "actions": [
                {
                  "type": "nero_hook_msg",
                  "mode": "tui-short",
                  "show": {"agent": true, "tui": true},
                  "format": 123,
                  "msg": {"full": "f", "short": "s"}
                }
              ]
            }"#,
        )
        .expect("parse");

        assert_eq!(
            parsed.actions,
            vec![NeroHookAction::NeroHookMsg {
                mode: NeroHookMsgMode::TuiShort,
                show: NeroHookMsgShow {
                    agent: true,
                    tui: true
                },
                freq: 0,
                format: NeroHookMsgFormat::Block,
                status: None,
                msg: NeroHookMsgContent {
                    full: "f".to_string(),
                    short: "s".to_string()
                }
            }]
        );
    }

    #[test]
    fn nero_hook_msg_malformed_status_is_ignored() {
        let parsed = parse_nero_hook_actions_from_stdout(
            r#"{
              "actions": [
                {
                  "type": "nero_hook_msg",
                  "mode": "tui-short",
                  "show": {"agent": true, "tui": true},
                  "status": {"kind": "countdown", "text": 7},
                  "msg": {"full": "f", "short": "s"}
                }
              ]
            }"#,
        )
        .expect("parse");

        assert_eq!(
            parsed.actions,
            vec![NeroHookAction::NeroHookMsg {
                mode: NeroHookMsgMode::TuiShort,
                show: NeroHookMsgShow {
                    agent: true,
                    tui: true
                },
                freq: 0,
                format: NeroHookMsgFormat::Block,
                status: None,
                msg: NeroHookMsgContent {
                    full: "f".to_string(),
                    short: "s".to_string()
                }
            }]
        );
    }
}
