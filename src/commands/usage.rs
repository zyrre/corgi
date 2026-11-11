//! `corgi usage --json`: the plan usage of each installed harness, for the
//! CLAUDE and CODEX cards of the Herdr app's Projects list. It reads through the cache every Corgi process shares
//! (`crate::usage_cache`), so however often it runs, the provider is
//! asked at most once per [`USAGE_REFRESH_INTERVAL`] on the machine, and
//! only by whichever process finds the cache stale first.

use anyhow::{Result, bail};
use serde_json::{Value, json};

use crate::{
    usage::{PlanUsage, Provider, installed_providers},
    usage_cache::{CachedUsage, USAGE_REFRESH_INTERVAL, cached_plan_usage, shared_plan_usage},
};

use crate::time::unix_now;

/// The version of `corgi usage --json`'s shape: fields may be added under
/// it, and a rename or removal bumps it.
pub const USAGE_VERSION: u32 = 1;

pub const USAGE_USAGE: &str = "\
Usage: corgi usage --json [--cached]

Prints the plan usage of each installed harness with a plan to read (Codex,
Claude Code) as one JSON object: \"providers\", each with its \"provider\",
\"title\" (CODEX, CLAUDE), the last good reading in \"usage\" (\"plan_type\",
\"five_hour\" and \"week\" with \"used_percent\" and \"resets_at\" in Unix
seconds, 0 when unknown), when it was read (\"fetched_at\") and why the last
reading failed (\"error\"); and \"now\", the time it was read at. A reading
comes from the cache every Corgi process shares, which asks the provider at
most once a minute on this machine; --cached never asks, and gives null
until another process has read it. It changes nothing.";

pub fn usage_command(args: &[String]) -> Result<()> {
    let mut json_output = false;
    let mut cached = false;
    for arg in args {
        match arg.as_str() {
            "--json" => json_output = true,
            "--cached" => cached = true,
            other => bail!("unknown option {other:?}\n\n{USAGE_USAGE}"),
        }
    }
    anyhow::ensure!(
        json_output,
        "corgi usage prints JSON only for now: pass --json\n\n{USAGE_USAGE}"
    );
    let readings: Vec<(Provider, Option<CachedUsage>)> = installed_providers()
        .into_iter()
        .map(|provider| {
            let reading = if cached {
                cached_plan_usage(provider)
            } else {
                shared_plan_usage(provider, USAGE_REFRESH_INTERVAL)
            };
            (provider, reading)
        })
        .collect();
    println!(
        "{}",
        serde_json::to_string(&usage_value(readings, unix_now()))?
    );
    Ok(())
}

/// One provider's reading, as the command prints it.
#[derive(serde::Serialize)]
struct UsageCard {
    provider: Provider,
    /// `CODEX`, `CLAUDE`.
    title: String,
    /// The last good reading.
    usage: Option<PlanUsage>,
    /// When it was read, Unix seconds.
    fetched_at: Option<u64>,
    /// Why the last reading failed.
    error: Option<String>,
}

/// The command's object for each provider's reading at `now`: `None` while
/// nothing is cached yet (another process is fetching the first reading).
fn usage_value(readings: Vec<(Provider, Option<CachedUsage>)>, now: u64) -> Value {
    let providers: Vec<UsageCard> = readings
        .into_iter()
        .map(|(provider, cached)| {
            let cached = cached.unwrap_or(CachedUsage {
                checked_at: 0,
                usage: None,
                fetched_at: None,
                error: None,
            });
            UsageCard {
                provider,
                title: provider.card_title().to_string(),
                usage: cached.usage,
                fetched_at: cached.fetched_at,
                error: cached.error,
            }
        })
        .collect();
    json!({ "version": USAGE_VERSION, "now": now, "providers": providers })
}

#[cfg(test)]
mod tests {
    use crate::usage::UsageWindow;

    use super::*;

    #[test]
    fn each_provider_reads_as_its_card_with_the_plan_windows() {
        let claude = CachedUsage {
            checked_at: 1_000,
            usage: Some(PlanUsage {
                provider: Provider::Claude,
                plan_type: "Max".into(),
                five_hour: Some(UsageWindow {
                    used_percent: 31,
                    resets_at: 2_000,
                }),
                week: Some(UsageWindow {
                    used_percent: 86,
                    resets_at: 0,
                }),
                banked_resets: None,
            }),
            fetched_at: Some(900),
            error: Some("HTTP 429".into()),
        };
        let value = usage_value(
            vec![(Provider::Codex, None), (Provider::Claude, Some(claude))],
            1_005,
        );
        assert_eq!(
            value,
            json!({
                "version": 1,
                "now": 1_005,
                "providers": [
                    {
                        "provider": "codex",
                        "title": "CODEX",
                        "usage": null,
                        "fetched_at": null,
                        "error": null,
                    },
                    {
                        "provider": "claude",
                        "title": "CLAUDE",
                        "usage": {
                            "provider": "claude",
                            "plan_type": "Max",
                            "five_hour": { "used_percent": 31, "resets_at": 2_000 },
                            "week": { "used_percent": 86, "resets_at": 0 },
                            "banked_resets": null,
                        },
                        "fetched_at": 900,
                        "error": "HTTP 429",
                    },
                ],
            })
        );
    }

    #[test]
    fn no_installed_harness_is_an_empty_list() {
        assert_eq!(
            usage_value(Vec::new(), 7),
            json!({ "version": 1, "now": 7, "providers": [] })
        );
    }
}
