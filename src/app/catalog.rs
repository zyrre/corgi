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

use super::{App, form::NewField};

/// Shown in the model field while no model is selected, meaning the harness
/// starts on whatever model its own configuration selects.
pub(crate) const HARNESS_DEFAULT_MODEL: &str = "Harness default";
/// The reasoning levels Corgi offers when starting a Codex session. The empty
/// value leaves the harness/model default in charge.
pub(super) const EFFORT_LEVELS: &[&str] = &["low", "medium", "high", "xhigh", "max"];
/// Shown in the thinking-effort field while no explicit level is selected;
/// the same words as the model placeholder, so both rows read alike.
pub(super) const HARNESS_DEFAULT_EFFORT: &str = HARNESS_DEFAULT_MODEL;
/// How long a discovered model catalog is trusted before the form opening or
/// the harness changing triggers a background re-fetch. The dashboard is
/// typically left open for a long session, so a process-lifetime cache would
/// otherwise go stale the moment a harness ships new models.
const MODEL_CATALOG_TTL: Duration = Duration::from_secs(3600);

/// A completed refresh of one harness's model picker. Model discovery always
/// happens off the UI thread because Codex may contact its authenticated
/// app-server and OpenCode may read its provider cache.
pub(super) struct ModelCatalogUpdate {
    pub(super) harness: Harness,
    pub(super) models: Result<Vec<String>, String>,
}

/// What the new-agent form knows about each harness's models: the catalogs
/// discovered in the background, when each was fetched, the fetches in
/// flight, and each harness's own configuration. Keyed by harness, which
/// the methods parse from the form's text.
#[derive(Default)]
pub(super) struct ModelCatalogs {
    /// Live model picker entries. The curated table remains the fallback
    /// until a discovery request succeeds.
    models: HashMap<Harness, Vec<String>>,
    /// When each harness's catalog was last fetched, so a form left open for
    /// a while re-fetches instead of trusting a stale process-lifetime cache.
    fetched_at: HashMap<Harness, Instant>,
    /// Each harness's own configuration, read once per dashboard run when
    /// the form first needs its defaults.
    harness_configs: HashMap<Harness, HarnessConfig>,
    loading: HashMap<Harness, Job<Result<Vec<String>, String>>>,
}

impl ModelCatalogs {
    /// Starts one best-effort catalog refresh for a harness, unless a catalog
    /// fetched within `MODEL_CATALOG_TTL` is already cached or a fetch for
    /// this harness is already in flight. The dashboard is usually left open
    /// for a long time, so opening the composer or switching harness re-fetches
    /// a stale catalog in the background while the cached list stays visible;
    /// a failed refresh leaves that cached list in place.
    fn request(&mut self, harness: &Harness) {
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
    fn finished(&mut self) -> Vec<ModelCatalogUpdate> {
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
    fn install(
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
    fn options(&self, harness: &Harness) -> Vec<String> {
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
    pub(super) fn defaults(&mut self, harness: &Harness, model: &str) -> HarnessDefaults {
        self.harness_configs
            .entry(harness.clone())
            .or_insert_with_key(HarnessConfig::load)
            .defaults(model)
    }
}

impl App {
    pub(super) fn request_model_catalog(&mut self, harness: &Harness) {
        self.model_catalogs.request(harness);
    }

    /// Applies completed model discovery without disturbing the composer. A
    /// failed or empty response deliberately leaves the curated fallback in
    /// place, and a model the user typed remains valid free text.
    pub(super) fn poll_model_catalogs(&mut self) {
        for update in self.model_catalogs.finished() {
            self.install_model_catalog(update);
        }
    }

    /// Takes one finished catalog fetch into the model picker.
    pub(super) fn install_model_catalog(&mut self, update: ModelCatalogUpdate) {
        let ModelCatalogUpdate { harness, models } = update;
        match self.model_catalogs.install(&harness, models) {
            Ok(true) => self.refresh_open_model_list(&harness),
            Ok(false) => {}
            Err(error) => {
                if self
                    .overlay
                    .new_agent_form()
                    .is_some_and(|form| form.harness() == harness)
                {
                    self.set_status(
                        format!("{harness} model list unavailable: {error}; using the fallback"),
                        Some(Duration::from_secs(8)),
                    );
                }
            }
        }
    }

    /// Hands a catalog that just arrived to a model list that is already open
    /// for that harness, keeping whatever filter has been typed.
    fn refresh_open_model_list(&mut self, harness: &Harness) {
        let options = self.model_options(harness);
        if let Some(form) = self.overlay.new_agent_form_mut()
            && form.field == NewField::Model
            && form.harness() == *harness
            && let Some(list) = &mut form.list
        {
            list.replace_choices(model_choices(
                &options,
                &form.model,
                form.defaults.model.as_deref(),
            ));
        }
    }

    pub(super) fn model_options(&self, harness: &Harness) -> Vec<String> {
        self.model_catalogs.options(harness)
    }
}

/// The harness preselected in the new-agent form: `CORGI_DEFAULT_AGENT` if
/// set, otherwise the first installed harness in the form's order, otherwise
/// `codex`.
pub(super) fn default_harness() -> Harness {
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
pub(super) fn cycle_agent_kind(current: &str, delta: isize) -> String {
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
pub(super) fn harness_choices(installed: &[Harness], current: &str) -> Vec<Choice> {
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
pub(super) fn default_label(placeholder: &str, configured: Option<&str>) -> String {
    match configured.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => format!("{placeholder} ({value})"),
        None => placeholder.to_string(),
    }
}

/// The model list: the harness default first, then the catalog, then a
/// model typed by hand that the catalog does not know.
pub(super) fn model_choices(
    models: &[String],
    current: &str,
    configured: Option<&str>,
) -> Vec<Choice> {
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
pub(super) fn effort_choices(configured: Option<&str>) -> Vec<Choice> {
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
pub(super) fn cycle_models(models: &[String], current: &str, delta: isize) -> String {
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
pub(super) fn supported_model(models: &[String], model: &str) -> String {
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
    use std::{thread, time::Instant};

    use crossterm::event::{KeyCode, KeyModifiers};

    use crate::{
        app::{Checkout, NewAgentForm, form::checkout_choices, test_helpers::press},
        defaults::HarnessDefaults,
        test_support::test_app,
    };

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
    fn a_model_cycles_through_its_harness_with_the_default_in_the_ring() {
        let app = test_app();
        let cycle = |kind: &str, current: &str, delta: isize| {
            cycle_models(
                &app.model_options(&Harness::from_kind(kind)),
                current,
                delta,
            )
        };
        assert_eq!(cycle("claude", "", 1), "opus");
        assert_eq!(cycle("claude", "opus", 1), "opus[1m]");
        assert_eq!(cycle("claude", "opus", -1), "");
        assert_eq!(cycle("claude", "fable", 1), "");
        assert_eq!(cycle("codex", "", -1), "gpt-6-astra");
        // A harness with no known models stays on its own default.
        assert_eq!(cycle("opencode", "", 1), "");
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

    #[test]
    fn a_discovered_catalog_replaces_only_its_harness_fallback() {
        let mut app = test_app();
        app.install_model_catalog(ModelCatalogUpdate {
            harness: Harness::Codex,
            models: Ok(vec!["gpt-5.6-sol".into(), "gpt-5.6-terra".into()]),
        });

        app.poll_model_catalogs();

        assert_eq!(
            app.model_options(&Harness::Codex),
            ["gpt-5.6-sol", "gpt-5.6-terra"]
        );
        assert_eq!(app.model_options(&Harness::Claude)[0], "opus");
    }

    #[test]
    fn a_fresh_catalog_is_not_refetched() {
        let mut app = test_app();
        app.model_catalogs
            .models
            .insert(Harness::Codex, vec!["gpt-5.6-sol".into()]);
        app.model_catalogs
            .fetched_at
            .insert(Harness::Codex, Instant::now());

        app.request_model_catalog(&Harness::Codex);

        assert!(!app.model_catalogs.loading.contains_key(&Harness::Codex));
        assert_eq!(app.model_options(&Harness::Codex), ["gpt-5.6-sol"]);
    }

    /// A fetch time just past the catalog's time to live.
    fn stale_catalog_time() -> Instant {
        Instant::now()
            .checked_sub(MODEL_CATALOG_TTL + Duration::from_secs(1))
            .expect("the monotonic clock is older than the catalog TTL")
    }

    #[test]
    fn a_stale_catalog_is_refetched_while_the_old_list_stays_available() {
        let mut app = test_app();
        app.model_catalogs
            .models
            .insert(Harness::Codex, vec!["gpt-5.6-sol".into()]);
        app.model_catalogs
            .fetched_at
            .insert(Harness::Codex, stale_catalog_time());

        app.request_model_catalog(&Harness::Codex);

        assert!(app.model_catalogs.loading.contains_key(&Harness::Codex));
        assert_eq!(app.model_options(&Harness::Codex), ["gpt-5.6-sol"]);
    }

    #[test]
    fn a_failed_refresh_keeps_the_last_good_catalog() {
        let mut app = test_app();
        app.model_catalogs
            .models
            .insert(Harness::Codex, vec!["gpt-5.6-sol".into()]);
        app.model_catalogs
            .fetched_at
            .insert(Harness::Codex, stale_catalog_time());
        app.request_model_catalog(&Harness::Codex);
        app.install_model_catalog(ModelCatalogUpdate {
            harness: Harness::Codex,
            models: Err("app-server unreachable".into()),
        });

        app.poll_model_catalogs();

        assert!(!app.model_catalogs.loading.contains_key(&Harness::Codex));
        assert_eq!(app.model_options(&Harness::Codex), ["gpt-5.6-sol"]);
    }

    #[test]
    fn a_catalog_that_arrives_refreshes_an_open_model_list() {
        let mut app = test_app();
        app.begin_new_agent_in("/repos/corgi".into());
        {
            let form = app.overlay.new_agent_form_mut().expect("form");
            form.kind = "codex".into();
            form.field = NewField::Model;
        }
        press(&mut app, KeyCode::Char(' '), KeyModifiers::NONE);
        let rows = |app: &App| -> Vec<String> {
            app.overlay
                .new_agent_form()
                .and_then(|form| form.list.as_ref())
                .expect("model list open")
                .rows()
                .into_iter()
                .map(|choice| choice.value)
                .collect()
        };
        assert!(rows(&app).iter().any(|value| value == "gpt-6-luna"));

        app.install_model_catalog(ModelCatalogUpdate {
            harness: Harness::Codex,
            models: Ok(vec!["gpt-5.6-sol".into()]),
        });
        app.poll_model_catalogs();

        let rows = rows(&app);
        assert_eq!(rows, ["", "gpt-5.6-sol"]);
    }

    #[test]
    fn selector_lists_lead_with_the_default_and_keep_typed_values() {
        let harness = harness_choices(&[Harness::Claude], "aider");
        assert_eq!(harness[0].value, "claude");
        assert_eq!(harness[0].badge, "installed");
        assert_eq!(harness.len(), Harness::KNOWN.len() + 1);
        assert_eq!(
            harness.last().map(|choice| choice.badge.as_str()),
            Some("as typed")
        );

        let model = model_choices(&["gpt-5".into(), "gpt-5-mini".into()], "gpt-5-mini", None);
        assert_eq!(model[0].label, HARNESS_DEFAULT_MODEL);
        assert_eq!(model[0].value, "");
        assert_eq!(model.len(), 3);
        let typed = model_choices(&["gpt-5".into()], "custom", Some("gpt-5.6-sol"));
        assert_eq!(typed[0].label, "Harness default (gpt-5.6-sol)");
        assert_eq!(
            typed.last().map(|choice| choice.value.as_str()),
            Some("custom")
        );

        let effort = effort_choices(Some("xhigh"));
        assert_eq!(effort[0].label, "Harness default (xhigh)");
        assert_eq!(effort[0].value, "");
        assert_eq!(effort[1].value, "low");

        // The rows say what the default is when the configuration names it.
        let mut form = NewAgentForm {
            field: NewField::Task,
            list: None,
            error: None,
            kind: "claude".into(),
            model: String::new(),
            effort: String::new(),
            defaults: HarnessDefaults {
                model: Some("opus[1m]".into()),
                effort: Some("high".into()),
            },
            project: "/repos/corgi".into(),
            new_project: false,
            checkout: Checkout::Worktree,
            prompt: String::new(),
            prompt_caret: 0,
            prompt_width: 0,
        };
        assert_eq!(form.model_label(), "Harness default (opus[1m])");
        assert_eq!(form.effort_label(), "Harness default (high)");
        form.model = "sonnet".into();
        form.defaults = HarnessDefaults::default();
        assert_eq!(form.model_label(), "sonnet");
        assert_eq!(form.effort_label(), HARNESS_DEFAULT_EFFORT);

        let checkout = checkout_choices();
        assert_eq!(checkout[0].value, Checkout::Worktree.value());
        assert_eq!(Checkout::from_value("directory"), Some(Checkout::Directory));
        assert_eq!(Checkout::from_value("elsewhere"), None);
    }

    #[test]
    #[ignore = "requires a locally authenticated Codex CLI"]
    fn live_codex_catalog_reaches_the_new_agent_cache() {
        let mut app = test_app();
        app.request_model_catalog(&Harness::Codex);
        let deadline = Instant::now() + Duration::from_secs(20);
        while !app.model_catalogs.models.contains_key(&Harness::Codex) && Instant::now() < deadline
        {
            app.poll_model_catalogs();
            thread::sleep(Duration::from_millis(50));
        }

        let models = app
            .model_catalogs
            .models
            .get(&Harness::Codex)
            .expect("Codex model catalog reached the picker cache");
        assert!(models.iter().any(|model| model.starts_with("gpt-")));
    }
}
