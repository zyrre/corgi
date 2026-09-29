//! One plan-usage reading per provider, shared by every Corgi process on the
//! machine through a file under `$XDG_STATE_HOME/corgi/usage/`.
//!
//! Each dashboard and Omarchy bar stream used to ask the provider on its own,
//! so reloads and extra dashboards multiplied the requests until Anthropic
//! answered HTTP 429. Now a process reads the cache first and uses it while it
//! is younger than the refresh interval. Once it is stale, whichever process
//! takes the provider's lock without waiting fetches and writes the cache;
//! the others keep what they have and find the new reading on a later check.
//! A failed fetch is written too, with its time, so the others do not all
//! retry at once, and the last good reading stays beside it.

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::{
    paths::corgi_state_dir,
    time::unix_now,
    usage::{PlanUsage, Provider, read_plan_usage},
};

/// What the cache knows about one provider's plan usage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CachedUsage {
    /// When a process last asked the provider, whether or not it answered
    /// (Unix seconds). The cache is fresh while this is recent.
    pub checked_at: u64,
    /// The last good reading, kept through failed fetches.
    pub usage: Option<PlanUsage>,
    /// When `usage` was read (Unix seconds).
    pub fetched_at: Option<u64>,
    /// Why the fetch at `checked_at` failed; `None` when it succeeded.
    pub error: Option<String>,
}

impl CachedUsage {
    /// Whether a fetch at `checked_at` still counts at `now`. A time ahead of
    /// the clock is not trusted, so a skewed file cannot silence fetching.
    fn is_fresh(&self, now: u64, interval: Duration) -> bool {
        self.checked_at <= now && now - self.checked_at < interval.as_secs()
    }
}

/// The provider's usage as every Corgi process shares it: the cached reading
/// while it is fresh, else a new one if this process gets to fetch it. `None`
/// when there is nothing yet, because another process is fetching the first
/// reading. Blocks on the fetch, so call it off the UI thread.
pub fn shared_plan_usage(provider: Provider, interval: Duration) -> Option<CachedUsage> {
    let fetch = || read_plan_usage(provider);
    match corgi_state_dir() {
        Some(state) => shared_reading(&state.join("usage"), provider, interval, unix_now, fetch),
        // Without a state directory there is nothing to share through.
        None => Some(record(None, fetch(), unix_now())),
    }
}

fn shared_reading(
    dir: &Path,
    provider: Provider,
    interval: Duration,
    now: impl Fn() -> u64,
    fetch: impl FnOnce() -> Result<PlanUsage>,
) -> Option<CachedUsage> {
    let path = cache_path(dir, provider);
    let cached = read_cache(&path);
    if cached
        .as_ref()
        .is_some_and(|cached| cached.is_fresh(now(), interval))
    {
        return cached;
    }
    let Some(lock) = try_lock(dir, provider) else {
        // Another process is fetching; its reading shows up on a later check.
        return cached;
    };
    // The holder before us may have just written a new reading.
    let cached = read_cache(&path);
    if cached
        .as_ref()
        .is_some_and(|cached| cached.is_fresh(now(), interval))
    {
        return cached;
    }
    let entry = record(cached, fetch(), now());
    // A cache that cannot be written only costs the other processes a fetch.
    let _ = write_cache(&path, &entry);
    let _ = lock.unlock();
    Some(entry)
}

/// The cache entry after a fetch at `now`: the new reading, or the failure
/// beside the last good one.
fn record(previous: Option<CachedUsage>, fetched: Result<PlanUsage>, now: u64) -> CachedUsage {
    match fetched {
        Ok(usage) => CachedUsage {
            checked_at: now,
            usage: Some(usage),
            fetched_at: Some(now),
            error: None,
        },
        Err(error) => {
            let (usage, fetched_at) = previous.map_or((None, None), |previous| {
                (previous.usage, previous.fetched_at)
            });
            CachedUsage {
                checked_at: now,
                usage,
                fetched_at,
                error: Some(format!("{error:#}")),
            }
        }
    }
}

fn provider_key(provider: Provider) -> &'static str {
    match provider {
        Provider::Codex => "codex",
        Provider::Claude => "claude",
    }
}

fn cache_path(dir: &Path, provider: Provider) -> PathBuf {
    dir.join(format!("{}.json", provider_key(provider)))
}

/// The cached entry, or `None` when there is none or it cannot be read, in
/// which case the next fetch replaces it.
fn read_cache(path: &Path) -> Option<CachedUsage> {
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

/// Replaces the cache in one step: the entry goes to a temporary file of this
/// process's own, which is then renamed over the cache, so a reader sees the
/// old entry or the new one and never half of either.
fn write_cache(path: &Path, entry: &CachedUsage) -> Result<()> {
    let temporary = path.with_extension(format!("json.{}.tmp", std::process::id()));
    let written = fs::File::create(&temporary).and_then(|mut file| {
        file.write_all(&serde_json::to_vec(entry)?)?;
        file.sync_all()
    });
    if let Err(error) = written.and_then(|()| fs::rename(&temporary, path)) {
        let _ = fs::remove_file(&temporary);
        return Err(error.into());
    }
    Ok(())
}

/// The provider's fetch lock, if no other process holds it.
fn try_lock(dir: &Path, provider: Provider) -> Option<fs::File> {
    fs::create_dir_all(dir).ok()?;
    let file = fs::File::create(dir.join(format!("{}.lock", provider_key(provider)))).ok()?;
    file.try_lock().is_ok().then_some(file)
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Arc, Barrier,
            atomic::{AtomicUsize, Ordering},
            mpsc,
        },
        thread,
    };

    use anyhow::anyhow;

    use super::*;
    use crate::{test_support::ScratchDir, usage::UsageWindow};

    const INTERVAL: Duration = Duration::from_secs(60);

    fn reading(used_percent: u8) -> PlanUsage {
        PlanUsage {
            provider: Provider::Claude,
            plan_type: "Max".into(),
            five_hour: Some(UsageWindow {
                used_percent,
                resets_at: 1_800_000_000,
            }),
            week: None,
            banked_resets: None,
        }
    }

    fn cached_at(checked_at: u64, used_percent: u8) -> CachedUsage {
        CachedUsage {
            checked_at,
            usage: Some(reading(used_percent)),
            fetched_at: Some(checked_at),
            error: None,
        }
    }

    fn seed(dir: &Path, entry: &CachedUsage) {
        fs::create_dir_all(dir).expect("create cache dir");
        write_cache(&cache_path(dir, Provider::Claude), entry).expect("seed cache");
    }

    fn no_fetch() -> Result<PlanUsage> {
        panic!("a fresh cache must not be fetched")
    }

    #[test]
    fn a_fresh_cache_is_used_without_fetching_even_by_a_new_process() {
        let scratch = ScratchDir::new("usage-cache-fresh");
        let dir = scratch.join("usage");
        seed(&dir, &cached_at(1_000, 12));

        // A process that has only just started has no reading of its own and
        // still takes the cache's, up to the last second of the interval.
        for now in [1_000, 1_059] {
            let entry = shared_reading(&dir, Provider::Claude, INTERVAL, || now, no_fetch);
            assert_eq!(entry, Some(cached_at(1_000, 12)));
        }
    }

    #[test]
    fn a_stale_cache_is_fetched_and_rewritten() {
        let scratch = ScratchDir::new("usage-cache-stale");
        let dir = scratch.join("usage");
        seed(&dir, &cached_at(1_000, 12));

        let entry = shared_reading(
            &dir,
            Provider::Claude,
            INTERVAL,
            || 1_060,
            || Ok(reading(40)),
        );
        assert_eq!(entry, Some(cached_at(1_060, 40)));
        assert_eq!(
            read_cache(&cache_path(&dir, Provider::Claude)),
            Some(cached_at(1_060, 40))
        );
    }

    #[test]
    fn a_first_reading_is_fetched_when_there_is_no_cache() {
        let scratch = ScratchDir::new("usage-cache-empty");
        let dir = scratch.join("usage");
        let entry = shared_reading(&dir, Provider::Claude, INTERVAL, || 5, || Ok(reading(3)));
        assert_eq!(entry, Some(cached_at(5, 3)));
    }

    #[test]
    fn a_checked_at_ahead_of_the_clock_is_stale() {
        assert!(!cached_at(2_000, 1).is_fresh(1_000, INTERVAL));
    }

    #[test]
    fn a_failed_fetch_is_recorded_with_its_time_and_keeps_the_last_good_reading() {
        let scratch = ScratchDir::new("usage-cache-error");
        let dir = scratch.join("usage");
        seed(&dir, &cached_at(1_000, 12));

        let failed = || Err(anyhow!("Claude usage endpoint answered HTTP 429"));
        let entry = shared_reading(&dir, Provider::Claude, INTERVAL, || 1_100, failed);
        let expected = CachedUsage {
            checked_at: 1_100,
            usage: Some(reading(12)),
            fetched_at: Some(1_000),
            error: Some("Claude usage endpoint answered HTTP 429".into()),
        };
        assert_eq!(entry.as_ref(), Some(&expected));
        assert_eq!(
            read_cache(&cache_path(&dir, Provider::Claude)).as_ref(),
            Some(&expected)
        );

        // The failure is as fresh as a reading: nobody retries until it ages.
        let entry = shared_reading(&dir, Provider::Claude, INTERVAL, || 1_159, no_fetch);
        assert_eq!(entry, Some(expected));
    }

    #[test]
    fn a_reader_that_finds_the_lock_taken_keeps_the_stale_reading_and_does_not_fetch() {
        let scratch = ScratchDir::new("usage-cache-locked");
        let dir = scratch.join("usage");
        seed(&dir, &cached_at(1_000, 12));

        let (locked_tx, locked_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let fetcher = {
            let dir = dir.clone();
            thread::spawn(move || {
                shared_reading(
                    &dir,
                    Provider::Claude,
                    INTERVAL,
                    || 2_000,
                    || {
                        locked_tx.send(()).expect("report the lock taken");
                        release_rx.recv().expect("wait to finish fetching");
                        Ok(reading(50))
                    },
                )
            })
        };
        locked_rx.recv().expect("the fetcher takes the lock");

        let entry = shared_reading(&dir, Provider::Claude, INTERVAL, || 2_000, no_fetch);
        assert_eq!(entry, Some(cached_at(1_000, 12)));

        release_tx.send(()).expect("let the fetcher finish");
        assert_eq!(
            fetcher.join().expect("fetcher panicked"),
            Some(cached_at(2_000, 50))
        );
        // The next check finds the fetcher's reading, fresh, and the lock free.
        let entry = shared_reading(&dir, Provider::Claude, INTERVAL, || 2_001, no_fetch);
        assert_eq!(entry, Some(cached_at(2_000, 50)));
    }

    #[test]
    fn two_readers_racing_on_a_stale_cache_fetch_exactly_once() {
        let scratch = ScratchDir::new("usage-cache-race");
        let dir = scratch.join("usage");
        seed(&dir, &cached_at(1_000, 12));
        let fetches = Arc::new(AtomicUsize::new(0));
        let start = Arc::new(Barrier::new(2));

        let readers: Vec<_> = (0..2)
            .map(|_| {
                let (dir, fetches, start) = (dir.clone(), fetches.clone(), start.clone());
                thread::spawn(move || {
                    start.wait();
                    shared_reading(
                        &dir,
                        Provider::Claude,
                        INTERVAL,
                        || 2_000,
                        || {
                            fetches.fetch_add(1, Ordering::SeqCst);
                            // Long enough for the other reader to arrive while
                            // the lock is held, or to find the new reading
                            // when it takes the lock after.
                            thread::sleep(Duration::from_millis(100));
                            Ok(reading(50))
                        },
                    )
                })
            })
            .collect();
        for reader in readers {
            reader.join().expect("reader panicked");
        }
        assert_eq!(fetches.load(Ordering::SeqCst), 1);
        assert_eq!(
            read_cache(&cache_path(&dir, Provider::Claude)),
            Some(cached_at(2_000, 50))
        );
    }

    #[test]
    fn the_cache_is_replaced_whole_through_a_temporary_file() {
        let scratch = ScratchDir::new("usage-cache-atomic");
        let path = scratch.join("claude.json");
        fs::write(&path, "{ half an old ent").expect("write a torn cache");
        // A torn file reads as no cache rather than as an error.
        assert_eq!(read_cache(&path), None);

        write_cache(&path, &cached_at(7, 9)).expect("write cache");
        assert_eq!(read_cache(&path), Some(cached_at(7, 9)));
        let names: Vec<_> = fs::read_dir(&*scratch)
            .expect("list cache dir")
            .map(|entry| entry.expect("dir entry").file_name())
            .collect();
        assert_eq!(names, ["claude.json"], "no temporary file is left behind");

        // The rename is what publishes the entry: a reader holding the old
        // file keeps reading the old entry whole.
        let before = fs::File::open(&path).expect("open the current cache");
        write_cache(&path, &cached_at(8, 10)).expect("rewrite cache");
        let old: CachedUsage = serde_json::from_reader(before).expect("old entry reads whole");
        assert_eq!(old, cached_at(7, 9));
        assert_eq!(read_cache(&path), Some(cached_at(8, 10)));
    }
}
