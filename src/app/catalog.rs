//! The new-agent form's side of the model catalogs of
//! [`corgi_core::catalog`]: fetching them as the form asks, and handing one
//! that arrives to an open model list.

use std::time::Duration;

pub(crate) use corgi_core::catalog::*;

use crate::harness::Harness;

use super::{App, form::NewField};

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
    fn codex_fallback_offers_the_locally_configured_sol_6_1_model() {
        let app = test_app();

        let options = app.model_options(&Harness::Codex);
        assert!(options.iter().any(|model| model == "gpt-6.1-sol"));
        assert!(
            model_choices(&options, "", None)
                .iter()
                .any(|choice| choice.value == "gpt-6.1-sol")
        );
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
}
