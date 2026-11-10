//! What a harness starts on when the form leaves a setting on "Harness
//! default": the model and effort level its own configuration names.
//!
//! Claude Code keeps them in the settings files of its configuration
//! directory, `CLAUDE_CONFIG_DIR` or `~/.claude` (`model`, `effortLevel`, and
//! per-model overrides under `modelSettings`); Codex keeps them at the top of
//! `~/.codex/config.toml` (`model`, `model_reasoning_effort`). Nothing here guesses a CLI's built-in default:
//! when a file names no value, the form shows none.

use std::fs;

use serde_json::Value;

use crate::{
    harness::{Harness, claude_config_dirs, claude_settings, first_setting},
    paths::home,
};

/// One harness's configuration as far as the form's defaults are concerned.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum HarnessConfig {
    /// Claude Code settings, the most specific file first.
    Claude(Vec<Value>),
    /// The top-level keys of Codex's config file.
    Codex {
        model: Option<String>,
        effort: Option<String>,
    },
    /// A harness whose configuration Corgi does not read.
    #[default]
    Unknown,
}

/// The values the form shows in parentheses behind "Harness default".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HarnessDefaults {
    pub model: Option<String>,
    pub effort: Option<String>,
}

impl HarnessConfig {
    /// Reads the configuration of `harness` from the user's own files.
    pub fn load(harness: &Harness) -> Self {
        match harness {
            Harness::Claude => Self::Claude(claude_settings(&claude_config_dirs())),
            Harness::Codex => home()
                .and_then(|home| fs::read_to_string(home.join(".codex").join("config.toml")).ok())
                .map(|raw| Self::codex_from_toml(&raw))
                .unwrap_or(Self::Unknown),
            _ => Self::Unknown,
        }
    }

    /// Codex's top-level `model` and `model_reasoning_effort`, read from the
    /// lines before the first `[section]`. The file is TOML, but these two
    /// keys are plain quoted strings, so no TOML parser is needed.
    pub fn codex_from_toml(raw: &str) -> Self {
        let mut model = None;
        let mut effort = None;
        for line in raw.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                break;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value
                .split('#')
                .next()
                .unwrap_or_default()
                .trim()
                .trim_matches(|character| character == '"' || character == '\'')
                .to_string();
            if value.is_empty() {
                continue;
            }
            match key.trim() {
                "model" => model = Some(value),
                "model_reasoning_effort" => effort = Some(value),
                _ => {}
            }
        }
        Self::Codex { model, effort }
    }

    /// What the harness starts on with the form's Model row on its default
    /// and, given the model that will run (`selected`, or the default when
    /// empty), the effort level its configuration names for it.
    pub fn defaults(&self, selected_model: &str) -> HarnessDefaults {
        match self {
            Self::Claude(settings) => {
                let model = first_setting(settings, |value| &value["model"]);
                let running = if selected_model.trim().is_empty() {
                    model.clone()
                } else {
                    Some(selected_model.trim().to_string())
                };
                let effort = running
                    .as_deref()
                    .and_then(|model| model_effort(settings, model))
                    .or_else(|| first_setting(settings, |value| &value["effortLevel"]));
                HarnessDefaults { model, effort }
            }
            Self::Codex { model, effort } => HarnessDefaults {
                model: model.clone(),
                effort: effort.clone(),
            },
            Self::Unknown => HarnessDefaults::default(),
        }
    }
}

/// The effort level configured for one model under `modelSettings`. Those
/// keys are full model IDs (`claude-opus-5`), while the `model` setting is
/// often the alias the CLI accepts (`opus[1m]`), so an alias also matches the
/// ID that contains it. Only the effort hint depends on this, so a miss costs
/// nothing beyond falling back to the global level.
fn model_effort(settings: &[Value], model: &str) -> Option<String> {
    let alias = model.trim().trim_end_matches("[1m]").to_lowercase();
    if alias.is_empty() {
        return None;
    }
    settings
        .iter()
        .filter_map(|value| value["modelSettings"].as_object())
        .flat_map(|models| models.iter())
        .filter(|(key, _)| {
            let key = key.to_lowercase();
            key == alias || key.contains(&alias)
        })
        .filter_map(|(_, value)| value["effortLevel"].as_str())
        .map(str::trim)
        .find(|effort| !effort.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{HarnessConfig, HarnessDefaults};

    #[test]
    fn claude_defaults_come_from_settings_with_per_model_effort_first() {
        let config = HarnessConfig::Claude(vec![json!({
            "model": "claude-fable-5-1[1m]",
            "effortLevel": "xhigh",
            "modelSettings": {
                "claude-fable-5-1": { "effortLevel": "high" },
                "claude-opus-5": { "effortLevel": "medium" }
            }
        })]);

        // The default model's own setting wins, found without the [1m] tag.
        assert_eq!(
            config.defaults(""),
            HarnessDefaults {
                model: Some("claude-fable-5-1[1m]".into()),
                effort: Some("high".into()),
            }
        );
        // A chosen model looks up its own effort, by full ID or by the
        // alias the CLI takes, and otherwise falls back to the global level.
        assert_eq!(
            config.defaults("claude-opus-5").effort.as_deref(),
            Some("medium")
        );
        assert_eq!(
            config.defaults("opus[1m]").effort.as_deref(),
            Some("medium")
        );
        assert_eq!(config.defaults("sonnet").effort.as_deref(), Some("xhigh"));
    }

    #[test]
    fn the_more_specific_claude_settings_file_wins_per_key() {
        let config = HarnessConfig::Claude(vec![
            json!({ "effortLevel": "low" }),
            json!({ "model": "opus", "effortLevel": "max" }),
        ]);
        assert_eq!(
            config.defaults(""),
            HarnessDefaults {
                model: Some("opus".into()),
                effort: Some("low".into()),
            }
        );
        assert_eq!(
            HarnessConfig::Claude(Vec::new()).defaults(""),
            HarnessDefaults::default()
        );
    }

    #[test]
    fn codex_defaults_are_the_top_level_keys_of_its_config() {
        let config = HarnessConfig::codex_from_toml(
            "# Codex\nmodel = \"gpt-5.6-sol\" # the daily driver\nmodel_reasoning_effort = 'high'\n\n[profiles.fast]\nmodel = \"gpt-5-mini\"\nmodel_reasoning_effort = \"low\"\n",
        );
        assert_eq!(
            config.defaults("anything"),
            HarnessDefaults {
                model: Some("gpt-5.6-sol".into()),
                effort: Some("high".into()),
            }
        );
        assert_eq!(
            HarnessConfig::codex_from_toml("approval_policy = \"never\"\n").defaults(""),
            HarnessDefaults::default()
        );
        assert_eq!(
            HarnessConfig::Unknown.defaults(""),
            HarnessDefaults::default()
        );
    }
}
