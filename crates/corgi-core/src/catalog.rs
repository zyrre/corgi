//! The per-harness model catalogs behind the new-agent form's Model row, and
//! the harness, model and effort rows' choices.

use std::{
    collections::HashMap,
    env,
    time::{Duration, Instant},
};

use anyhow::Result;

use crate::{
    choices::Choice,
    defaults::{HarnessConfig, HarnessDefaults},
    harness::Harness,
    job::Job,
};

/// Shown in the model field while no model is selected, meaning the harness
/// starts on whatever model its own configuration selects.
pub const HARNESS_DEFAULT_MODEL: &str = "Harness default";
/// The reasoning levels Corgi offers when starting a Codex session. The empty
/// value leaves the harness/model default in charge.
pub const EFFORT_LEVELS: &[&str] = &["low", "medium", "high", "xhigh", "max"];
/// Shown in the thinking-effort field while no explicit level is selected;
/// the same words as the model placeholder, so both rows read alike.
pub const HARNESS_DEFAULT_EFFORT: &str = HARNESS_DEFAULT_MODEL;
/// How long a discovered model catalog is trusted before the form opening or
/// the harness changing triggers a background re-fetch. The dashboard is
/// typically left open for a long session, so a process-lifetime cache would
/// otherwise go stale the moment a harness ships new models.
pub const MODEL_CATALOG_TTL: Duration = Duration::from_secs(3600);

/// A completed refresh of one harness's model picker. Model discovery always
/// happens off the UI thread because Codex may contact its authenticated
/// app-server and OpenCode may read its provider cache.
pub struct ModelCatalogUpdate {
    pub harness: Harness,
    pub models: Result<Vec<String>, String>,
}

/// What the new-agent form knows about each harness's models: the catalogs
/// discovered in the background, when each was fetched, the fetches in
/// flight, and each harness's own configuration. Keyed by harness, which
/// the methods parse from the form's text.
#[derive(Default)]
pub struct ModelCatalogs {
    /// Live model picker entries. The curated table remains the fallback
    /// until a discovery request succeeds.
    pub models: HashMap<Harness, Vec<String>>,
    /// When each harness's catalog was last fetched, so a form left open for
    /// a while re-fetches instead of trusting a stale process-lifetime cache.
    pub fetched_at: HashMap<Harness, Instant>,
    /// Each harness's own configuration, read once per dashboard run when
    /// the form first needs its defaults.
    harness_configs: HashMap<Harness, HarnessConfig>,
    /// The fetches in flight.
    pub loading: HashMap<Harness, Job<Result<Vec<String>, String>>>,
}

impl ModelCatalogs {
    /// Starts one best-effort catalog refresh for a harness, unless a catalog
    /// fetched within `MODEL_CATALOG_TTL` is already cached or a fetch for
    /// this harness is already in flight. The dashboard is usually left open
    /// for a long time, so opening the composer or switching harness re-fetches
    /// a stale catalog in the background while the cached list stays visible;
    /// a failed refresh leaves that cached list in place.
    pub fn request(&mut self, harness: &Harness) {
        let is_fresh = self
            .fetched_at
            .get(harness)
            .is_some_and(|fetched_at| fetched_at.elapsed() < MODEL_CATALOG_TTL);
        if !harness.supports_model_discovery() || is_fresh || self.loading.contains_key(harness) {
            return;
        }
        let fetch = Job::spawn({
            let harness = harness.clone();
            move |_| {
                harness
                    .discover_models()
                    .map_err(|error| format!("{error:#}"))
            }
        });
        self.loading.insert(harness.clone(), fetch);
    }

    /// The fetches that have finished since the last call.
    pub fn finished(&mut self) -> Vec<ModelCatalogUpdate> {
        self.loading
            .iter_mut()
            .filter_map(|(harness, fetch)| {
                // A panicking fetch still reports, or its harness would stay
                // loading and never be fetched again.
                let models = fetch
                    .outcome()?
                    .unwrap_or_else(|_| Err("model discovery panicked".into()));
                Some(ModelCatalogUpdate {
                    harness: harness.clone(),
                    models,
                })
            })
            .collect()
    }

    /// Records one finished fetch: whether it brought a catalog, or why it
    /// failed. A failed or empty response leaves the last catalog in place.
    pub fn install(
        &mut self,
        harness: &Harness,
        models: Result<Vec<String>, String>,
    ) -> Result<bool, String> {
        self.loading.remove(harness);
        match models {
            Ok(models) if !models.is_empty() => {
                self.models.insert(harness.clone(), models);
                self.fetched_at.insert(harness.clone(), Instant::now());
                Ok(true)
            }
            Ok(_) => Ok(false),
            Err(error) => Err(error),
        }
    }

    /// The discovered account-specific catalog when available, otherwise the
    /// small offline fallback that keeps the composer usable without a CLI
    /// login or network access.
    pub fn options(&self, harness: &Harness) -> Vec<String> {
        self.models.get(harness).cloned().unwrap_or_else(|| {
            harness
                .fallback_models()
                .iter()
                .map(|model| (*model).to_string())
                .collect()
        })
    }

    /// What `harness`'s own configuration names for the rows left on "Harness
    /// default" with `model` selected, reading the configuration the first
    /// time.
    pub fn defaults(&mut self, harness: &Harness, model: &str) -> HarnessDefaults {
        self.harness_configs
            .entry(harness.clone())
            .or_insert_with_key(HarnessConfig::load)
            .defaults(model)
    }
}

/// The harness preselected in the new-agent form: `CORGI_DEFAULT_AGENT` if
/// set, otherwise the first installed harness in the form's order, otherwise
/// `codex`.
pub fn default_harness() -> Harness {
    default_harness_from(env::var("CORGI_DEFAULT_AGENT").ok(), &Harness::installed())
}

fn default_harness_from(override_kind: Option<String>, installed: &[Harness]) -> Harness {
    override_kind
        .map(|kind| Harness::from_typed(&kind))
        .filter(|harness| !harness.kind().is_empty())
        .or_else(|| {
            Harness::KNOWN
                .iter()
                .find(|harness| installed.contains(harness))
                .cloned()
        })
        .unwrap_or_else(|| Harness::KNOWN[0].clone())
}

/// Moves through [`Harness::KNOWN`]; an unknown current value jumps to the
/// start or end of the list depending on direction.
pub fn cycle_agent_kind(current: &str, delta: isize) -> String {
    let known = Harness::KNOWN;
    let len = known.len() as isize;
    let current = Harness::from_typed(current);
    let index = known
        .iter()
        .position(|harness| *harness == current)
        .map(|index| (index as isize + delta).rem_euclid(len))
        .unwrap_or(if delta < 0 { len - 1 } else { 0 });
    known[index as usize].kind().to_string()
}

/// The harness list: every kind Corgi knows, the installed CLIs first and
/// badged, and a kind typed by hand kept as a row of its own.
pub fn harness_choices(installed: &[Harness], current: &str) -> Vec<Choice> {
    let mut choices: Vec<Choice> = Harness::KNOWN
        .iter()
        .filter(|harness| installed.contains(harness))
        .map(|harness| Choice::new(harness.kind()).badge("installed"))
        .collect();
    choices.extend(
        Harness::KNOWN
            .iter()
            .filter(|harness| !installed.contains(harness))
            .map(|harness| Choice::new(harness.kind())),
    );
    let current = current.trim();
    if !current.is_empty()
        && !choices
            .iter()
            .any(|choice| choice.value.eq_ignore_ascii_case(current))
    {
        choices.push(Choice::new(current).badge("as typed"));
    }
    choices
}

/// "Harness default", followed by the configured value in parentheses when
/// the harness's own configuration names one.
pub fn default_label(placeholder: &str, configured: Option<&str>) -> String {
    match configured.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => format!("{placeholder} ({value})"),
        None => placeholder.to_string(),
    }
}

/// The model list: the harness default first, then the catalog, then a
/// model typed by hand that the catalog does not know.
pub fn model_choices(models: &[String], current: &str, configured: Option<&str>) -> Vec<Choice> {
    let mut choices = vec![Choice::labelled(
        "",
        default_label(HARNESS_DEFAULT_MODEL, configured),
    )];
    for model in models {
        if !model.trim().is_empty()
            && !choices
                .iter()
                .any(|choice| choice.value.eq_ignore_ascii_case(model.trim()))
        {
            choices.push(Choice::new(model.trim()));
        }
    }
    let current = current.trim();
    if !current.is_empty()
        && !choices
            .iter()
            .any(|choice| choice.value.eq_ignore_ascii_case(current))
    {
        choices.push(Choice::new(current).badge("as typed"));
    }
    choices
}

/// The reasoning levels Codex and Claude Code take, behind the harness
/// default.
pub fn effort_choices(configured: Option<&str>) -> Vec<Choice> {
    let mut choices = vec![Choice::labelled(
        "",
        default_label(HARNESS_DEFAULT_EFFORT, configured),
    )];
    choices.extend(EFFORT_LEVELS.iter().map(|effort| Choice::new(*effort)));
    choices
}

/// Moves through a concrete model catalog with the harness default as the
/// entry before the first one. A model typed by hand is treated as the
/// default's neighbour.
pub fn cycle_models(models: &[String], current: &str, delta: isize) -> String {
    if models.is_empty() {
        return String::new();
    }
    let current = current.trim();
    let index = if current.is_empty() {
        0
    } else {
        models
            .iter()
            .position(|model| model.eq_ignore_ascii_case(current))
            .map_or(0, |index| index as isize + 1)
    };
    let next = (index + delta).rem_euclid(models.len() as isize + 1);
    match next {
        0 => String::new(),
        index => models[index as usize - 1].to_string(),
    }
}

/// Keeps `model` only when the selected harness offers it. An empty catalog
/// keeps free text because Corgi has nothing to validate against.
pub fn supported_model(models: &[String], model: &str) -> String {
    let keep = model.trim().is_empty()
        || models.is_empty()
        || models
            .iter()
            .any(|known| known.eq_ignore_ascii_case(model.trim()));
    if keep {
        model.to_string()
    } else {
        String::new()
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn default_harness_prefers_override_then_installed_cli() {
        assert_eq!(
            default_harness_from(Some(" Claude ".into()), &[Harness::Codex]),
            Harness::Claude
        );
        assert_eq!(
            default_harness_from(Some(String::new()), &[Harness::Claude]),
            Harness::Claude
        );
        // The first installed harness in the form's order, whichever it is.
        assert_eq!(
            default_harness_from(None, &[Harness::OpenCode, Harness::Codex]),
            Harness::Codex
        );
        assert_eq!(
            default_harness_from(None, &[Harness::OpenCode]),
            Harness::OpenCode
        );
        assert_eq!(default_harness_from(None, &[]), Harness::Codex);
    }

    #[test]
    fn agent_kind_cycles_through_the_known_list() {
        assert_eq!(cycle_agent_kind("codex", 1), "claude");
        assert_eq!(
            cycle_agent_kind("codex", -1),
            Harness::KNOWN[Harness::KNOWN.len() - 1].kind()
        );
        assert_eq!(cycle_agent_kind("something-else", 1), "codex");
        assert_eq!(cycle_agent_kind("Claude", -1), "codex");
    }
}
