//! How Corgi's popups move: the clock their effects run on, the easing and
//! timing of each effect, and the state of the one popup the dashboard shows
//! or is closing. The dashboard watches its overlay once a frame and tells
//! [`Motion`] what it sees; the renderer reads back how far each effect has
//! got, and the main loop asks how soon the next frame is due.
//!
//! Nothing here draws. Every effect is a function of the clock, so a test
//! that sets a [`Clock::Manual`] time sees one fixed frame.

use std::time::{Duration, Instant};

use ratatui::layout::Rect;
use ratatui::style::Color;

/// How long a popup's frame takes to grow to its size, or shrink away.
pub(crate) const OPEN: Duration = Duration::from_millis(180);
/// How long the dashboard behind a popup takes to fade to muted, or back.
pub(crate) const DIM: Duration = Duration::from_millis(180);
/// How long a popup's content takes to fade in once its frame has its size.
pub(crate) const CONTENT: Duration = Duration::from_millis(120);
/// How long a failure's pulse of the frame takes to settle into red.
pub(crate) const FLASH: Duration = Duration::from_millis(550);
/// A success's check stamp: how long its big ✓ takes to be drawn, how long
/// it holds, and how long it takes to fade while the content comes back.
/// The content fades almost out in the first `STAMP_HIDE` of it.
pub(crate) const STAMP_DRAW: Duration = Duration::from_millis(280);
pub(crate) const STAMP_HOLD: Duration = Duration::from_millis(300);
pub(crate) const STAMP_OUT: Duration = Duration::from_millis(300);
pub(crate) const STAMP_HIDE: Duration = Duration::from_millis(120);
/// The whole of a success's effect.
pub(crate) const STAMP_LENGTH: Duration = STAMP_DRAW
    .saturating_add(STAMP_HOLD)
    .saturating_add(STAMP_OUT);
/// How long a failure shakes the popup.
pub(crate) const SHAKE: Duration = Duration::from_millis(400);
/// The widest a failure shakes the popup, in columns either way.
pub(crate) const SHAKE_COLUMNS: f32 = 2.0;
/// One frame of the shared spinner: 12.5 frames a second.
pub(crate) const SPINNER_FRAME: Duration = Duration::from_millis(80);
/// How long a text field's caret stays on, and then off.
pub(crate) const CARET_BLINK: Duration = Duration::from_millis(530);
/// Frame time while something moves: about 60 frames a second.
pub(crate) const ANIMATION_FRAME: Duration = Duration::from_millis(16);
/// Frame time while nothing moves, the dashboard's cadence without popups.
pub(crate) const IDLE_FRAME: Duration = Duration::from_millis(80);

/// Where effects read the time from.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Clock {
    /// Wall time since the dashboard started.
    Real(Instant),
    /// A time a test sets, which moves only when the test moves it.
    Manual(Duration),
}

impl Clock {
    pub(crate) fn real() -> Self {
        Self::Real(Instant::now())
    }

    /// The time on this clock, from its start.
    pub(crate) fn now(&self) -> Duration {
        match self {
            Self::Real(start) => start.elapsed(),
            Self::Manual(now) => *now,
        }
    }
}

/// A test clock stopped at its start, so a dashboard built for a test draws
/// the same frame every time.
impl Default for Clock {
    fn default() -> Self {
        Self::Manual(Duration::ZERO)
    }
}

/// What an outcome did, which decides the effect it plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// A launch or merge that finished: the frame flashes green.
    Success,
    /// A launch or merge that failed: the frame pulses into red and shakes.
    Failure,
}

/// Where the one popup is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
enum Popup {
    /// No popup is open or closing.
    #[default]
    Hidden,
    /// A popup is open, growing since `since` until it has its size.
    Shown { since: Duration },
    /// The popup closed at `since` and its frame, last drawn at `area` in
    /// `color`, shrinks away.
    Closing {
        since: Duration,
        area: Rect,
        color: Color,
    },
}

/// The frame of a popup that has closed and is shrinking away.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Ghost {
    pub(crate) area: Rect,
    pub(crate) color: Color,
    /// How much of its size is left, from 1 down to 0.
    pub(crate) openness: f32,
}

/// A success's check stamp: when it started, and the popup area it is
/// centred on once that is known. It keeps its own time, so it plays on
/// over the popup's close and the dashboard after it.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Stamp {
    since: Duration,
    area: Option<Rect>,
}

/// The state behind every popup effect.
#[derive(Debug, Default)]
pub(crate) struct Motion {
    clock: Clock,
    /// Whether effects play. Without them every effect is at its end, which
    /// is what tests that only look at content want; the spinner and caret
    /// still follow the clock.
    animate: bool,
    popup: Popup,
    /// Where and in which color the open popup's frame was last drawn, for
    /// the frame that shrinks away when it closes.
    last_frame: Option<(Rect, Color)>,
    /// The outcome the popup shows now, and the one whose effect plays with
    /// the time it arrived.
    outcome: Option<Outcome>,
    effect: Option<(Outcome, Duration)>,
    /// The check stamp of the latest success, while it plays.
    stamp: Option<Stamp>,
    /// When the caret was last put back on, by a key press.
    caret_since: Duration,
}

impl Motion {
    /// Effects that play on the wall clock, for the interactive dashboard.
    pub(crate) fn animated() -> Self {
        Self {
            clock: Clock::real(),
            animate: true,
            ..Self::default()
        }
    }

    /// Effects that play on a clock the caller sets, for tests of the
    /// effects themselves.
    #[cfg(test)]
    pub(crate) fn manual() -> Self {
        Self {
            animate: true,
            ..Self::default()
        }
    }

    /// Moves a manual clock to `now`.
    #[cfg(test)]
    pub(crate) fn set_time(&mut self, now: Duration) {
        self.clock = Clock::Manual(now);
    }

    pub(crate) fn now(&self) -> Duration {
        self.clock.now()
    }

    /// Whether effects play, so there is something to wait for.
    pub(crate) fn animates(&self) -> bool {
        self.animate
    }

    /// Tells the motion what the dashboard shows this frame: whether a popup
    /// is open, and the outcome it shows, if any. A popup that opens starts
    /// to grow, one that closes starts to shrink, and an outcome that was not
    /// there last frame plays its effect.
    pub(crate) fn observe(&mut self, open: bool, outcome: Option<Outcome>) {
        let now = self.now();
        // A new popup cuts off a stamp still playing from the last one.
        if open && !matches!(self.popup, Popup::Shown { .. }) {
            self.stamp = None;
        }
        self.popup = match (self.popup, open) {
            (Popup::Shown { since }, true) => Popup::Shown { since },
            (_, true) => Popup::Shown { since: now },
            (Popup::Shown { .. }, false) => match self.last_frame.take() {
                Some((area, color)) if self.animate => Popup::Closing {
                    since: now,
                    area,
                    color,
                },
                _ => Popup::Hidden,
            },
            (Popup::Closing { since, .. }, false) if now.saturating_sub(since) >= OPEN => {
                Popup::Hidden
            }
            (other, false) => other,
        };
        if outcome != self.outcome {
            self.outcome = outcome;
            // A failure plays on the popup; a success stamps its check over
            // wherever the popup is drawn next.
            self.effect = outcome
                .filter(|outcome| self.animate && *outcome == Outcome::Failure)
                .map(|outcome| (outcome, now));
            if outcome == Some(Outcome::Success) && self.animate {
                self.stamp = Some(Stamp {
                    since: now,
                    area: None,
                });
            }
        }
    }

    /// A popup's confirm action succeeded and closed it: its check stamp
    /// plays centred on where it was last drawn, over its close.
    pub(crate) fn succeeded(&mut self) {
        if self.animate {
            self.stamp = Some(Stamp {
                since: self.now(),
                area: self.last_frame.map(|(area, _)| area),
            });
        }
    }

    /// Centres a stamp that does not know its place yet on `area`, the popup
    /// drawn as its success arrived.
    pub(crate) fn anchor_stamp(&mut self, area: Rect) {
        if let Some(stamp) = &mut self.stamp
            && stamp.area.is_none()
        {
            stamp.area = Some(area);
        }
    }

    /// The check stamp playing now: the area it is centred on, and how far
    /// into it the clock is.
    pub(crate) fn stamp(&self) -> Option<(Rect, Duration)> {
        let stamp = self.stamp?;
        let elapsed = self.stamp_elapsed()?;
        Some((stamp.area?, elapsed))
    }

    /// How far into the stamp the clock is, while it plays, placed or not.
    pub(crate) fn stamp_time(&self) -> Option<Duration> {
        self.stamp_elapsed()
    }

    /// How far into the stamp the clock is, while it plays.
    fn stamp_elapsed(&self) -> Option<Duration> {
        let elapsed = self.now().saturating_sub(self.stamp?.since);
        (elapsed < STAMP_LENGTH).then_some(elapsed)
    }

    /// Notes where the open popup's frame was drawn this frame, and in which
    /// color, so it can shrink from there when the popup closes.
    pub(crate) fn record_frame(&mut self, area: Rect, color: Color) {
        self.last_frame = Some((area, color));
    }

    /// A key was pressed: the caret shows at once, so it never blinks out
    /// under the user's typing.
    pub(crate) fn key_pressed(&mut self) {
        self.caret_since = self.now();
    }

    /// How far the open popup's frame has grown, from 0 to 1.
    pub(crate) fn openness(&self) -> f32 {
        if !self.animate {
            return 1.0;
        }
        match self.popup {
            Popup::Shown { since } if self.animate => ease_out_cubic(self.progress(since, OPEN)),
            Popup::Shown { .. } => 1.0,
            Popup::Hidden | Popup::Closing { .. } => 0.0,
        }
    }

    /// How far the open popup's content has faded in, from 0 to 1. It starts
    /// once the frame has its size.
    pub(crate) fn content_fade(&self) -> f32 {
        match self.popup {
            Popup::Shown { since } if self.animate => {
                ease_out_cubic(self.progress(since + OPEN, CONTENT))
            }
            _ => 1.0,
        }
    }

    /// The frame of a popup that closed and is still shrinking away.
    pub(crate) fn ghost(&self) -> Option<Ghost> {
        match self.popup {
            Popup::Closing { since, area, color } => {
                let openness = 1.0 - ease_out_cubic(self.progress(since, OPEN));
                (openness > 0.0).then_some(Ghost {
                    area,
                    color,
                    openness,
                })
            }
            _ => None,
        }
    }

    /// How far the dashboard has faded behind a popup, from 0 to 1.
    pub(crate) fn dim(&self) -> f32 {
        match self.popup {
            Popup::Hidden => 0.0,
            Popup::Shown { .. } if !self.animate => 1.0,
            Popup::Shown { since } => ease_out_cubic(self.progress(since, DIM)),
            Popup::Closing { since, .. } => 1.0 - ease_out_cubic(self.progress(since, DIM)),
        }
    }

    /// The outcome effect playing now, and how far into it the clock is.
    pub(crate) fn outcome_effect(&self) -> Option<(Outcome, Duration)> {
        let (outcome, since) = self.effect?;
        let elapsed = self.now().saturating_sub(since);
        (elapsed < FLASH.max(SHAKE)).then_some((outcome, elapsed))
    }

    /// Columns the popup is pushed sideways this frame by a failure's shake.
    pub(crate) fn shake(&self) -> i16 {
        match self.outcome_effect() {
            Some((Outcome::Failure, elapsed)) => shake_offset(elapsed),
            _ => 0,
        }
    }

    /// The shared spinner's frame at this time.
    pub(crate) fn spinner(&self) -> &'static str {
        spinner_at(self.now())
    }

    /// Whether a focused text field's caret is on at this time.
    pub(crate) fn caret_on(&self) -> bool {
        caret_on_at(self.now().saturating_sub(self.caret_since))
    }

    /// Whether any effect is still moving, which needs the fast frame rate.
    pub(crate) fn is_moving(&self) -> bool {
        if !self.animate {
            return false;
        }
        let now = self.now();
        let popup_moving = match self.popup {
            Popup::Hidden => false,
            Popup::Shown { since } => now.saturating_sub(since) < OPEN.max(DIM) + CONTENT,
            Popup::Closing { since, .. } => now.saturating_sub(since) < OPEN.max(DIM),
        };
        popup_moving || self.outcome_effect().is_some() || self.stamp_elapsed().is_some()
    }

    /// How long the main loop may wait for a key before it draws again: a
    /// sixtieth of a second while something moves; otherwise the dashboard's
    /// usual cadence, cut short to land on the next frame of a spinning
    /// spinner or the next turn of a blinking caret.
    pub(crate) fn frame_interval(&self, spinning: bool, blinking: bool) -> Duration {
        if self.is_moving() {
            return ANIMATION_FRAME;
        }
        let now = self.now();
        let mut wait = IDLE_FRAME;
        if spinning {
            wait = wait.min(until_next(now, SPINNER_FRAME));
        }
        if blinking {
            wait = wait.min(until_next(
                now.saturating_sub(self.caret_since),
                CARET_BLINK,
            ));
        }
        wait.max(Duration::from_millis(1))
    }

    /// How far `now` is into an effect that started at `since` and lasts
    /// `total`, from 0 to 1.
    fn progress(&self, since: Duration, total: Duration) -> f32 {
        progress(self.now().saturating_sub(since), total)
    }
}

/// How much of `total` has passed after `elapsed`, from 0 to 1.
pub(crate) fn progress(elapsed: Duration, total: Duration) -> f32 {
    if total.is_zero() {
        return 1.0;
    }
    (elapsed.as_secs_f32() / total.as_secs_f32()).clamp(0.0, 1.0)
}

/// Fast at first and slowing into its end: the curve every effect that
/// grows, shrinks or fades follows.
pub(crate) fn ease_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// How much of a success's stamp is drawn `elapsed` into it, from 0 to 1,
/// as the share of its columns shown from the left.
pub(crate) fn stamp_drawn(elapsed: Duration) -> f32 {
    ease_out_cubic(progress(elapsed, STAMP_DRAW))
}

/// How far a success's stamp has faded out `elapsed` into it, from 0 to 1.
/// The frame eases from green back to its color on the same curve.
pub(crate) fn stamp_faded(elapsed: Duration) -> f32 {
    let shown = STAMP_DRAW + STAMP_HOLD;
    if elapsed < shown {
        return 0.0;
    }
    ease_out_cubic(progress(elapsed - shown, STAMP_OUT))
}

/// How visible the content under a success's stamp is `elapsed` into it,
/// from 1 down to 0 and back: it fades almost out as the stamp is drawn,
/// and back in as the stamp fades.
pub(crate) fn stamp_content(elapsed: Duration) -> f32 {
    if elapsed < STAMP_DRAW + STAMP_HOLD {
        1.0 - ease_out_cubic(progress(elapsed, STAMP_HIDE))
    } else {
        stamp_faded(elapsed)
    }
}

/// Columns a failure's shake pushes the popup, `elapsed` into it: three
/// swings either way, dying away to nothing.
pub(crate) fn shake_offset(elapsed: Duration) -> i16 {
    let t = progress(elapsed, SHAKE);
    if t >= 1.0 {
        return 0;
    }
    let swing = (t * std::f32::consts::PI * 6.0).sin() * (1.0 - t);
    (SHAKE_COLUMNS * swing).round() as i16
}

/// One spinner for everything that waits, so a launch and a merge tick alike.
pub(crate) const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// The spinner's frame at `now`.
pub(crate) fn spinner_at(now: Duration) -> &'static str {
    let frame = now.as_millis() / SPINNER_FRAME.as_millis();
    SPINNER[(frame % SPINNER.len() as u128) as usize]
}

/// Whether a caret put on `elapsed` ago is on: on first, then off, in turns.
pub(crate) fn caret_on_at(elapsed: Duration) -> bool {
    (elapsed.as_millis() / CARET_BLINK.as_millis()).is_multiple_of(2)
}

/// Time from `now` to the next multiple of `period`.
fn until_next(now: Duration, period: Duration) -> Duration {
    let period_ms = period.as_millis().max(1);
    let into = now.as_millis() % period_ms;
    Duration::from_millis((period_ms - into) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(millis: u64) -> Duration {
        Duration::from_millis(millis)
    }

    #[test]
    fn easing_starts_fast_and_lands_exactly_on_its_ends() {
        assert_eq!(ease_out_cubic(0.0), 0.0);
        assert_eq!(ease_out_cubic(1.0), 1.0);
        assert_eq!(ease_out_cubic(-1.0), 0.0);
        assert_eq!(ease_out_cubic(2.0), 1.0);
        // Ease-out: well past half way at half the time, and always rising.
        assert!((ease_out_cubic(0.5) - 0.875).abs() < 1e-6);
        let mut last = 0.0;
        for step in 1..=20 {
            let value = ease_out_cubic(step as f32 / 20.0);
            assert!(value > last);
            last = value;
        }
        assert_eq!(progress(ms(90), OPEN), 0.5);
        assert_eq!(progress(ms(900), OPEN), 1.0);
        assert_eq!(progress(ms(5), Duration::ZERO), 1.0);
    }

    #[test]
    fn a_popup_grows_then_fades_in_and_shrinks_away_when_it_closes() {
        let mut motion = Motion::manual();
        motion.set_time(ms(1_000));
        motion.observe(true, None);
        assert_eq!(motion.openness(), 0.0);
        assert_eq!(motion.content_fade(), 0.0);
        assert_eq!(motion.dim(), 0.0);

        motion.set_time(ms(1_090));
        motion.observe(true, None);
        assert!((motion.openness() - 0.875).abs() < 1e-6);
        assert_eq!(motion.content_fade(), 0.0, "content waits for the frame");

        motion.set_time(ms(1_180));
        assert_eq!(motion.openness(), 1.0);
        assert_eq!(motion.dim(), 1.0);
        motion.set_time(ms(1_240));
        assert!(motion.content_fade() > 0.5 && motion.content_fade() < 1.0);
        motion.set_time(ms(1_300));
        assert_eq!(motion.content_fade(), 1.0);

        // Closed, the frame last drawn shrinks from where it was.
        let area = Rect::new(10, 5, 40, 12);
        motion.record_frame(area, Color::Cyan);
        motion.observe(false, None);
        let ghost = motion.ghost().expect("a closing frame");
        assert_eq!(
            (ghost.area, ghost.color, ghost.openness),
            (area, Color::Cyan, 1.0)
        );
        assert_eq!(motion.openness(), 0.0);
        motion.set_time(ms(1_390));
        assert!(motion.ghost().expect("still closing").openness < 0.2);
        assert!(motion.dim() < 0.2 && motion.dim() > 0.0);
        motion.set_time(ms(1_480));
        motion.observe(false, None);
        assert_eq!(motion.ghost(), None);
        assert_eq!(motion.dim(), 0.0);
    }

    #[test]
    fn an_outcome_plays_once_when_it_arrives() {
        let mut motion = Motion::manual();
        motion.observe(true, None);
        motion.set_time(ms(2_000));
        motion.observe(true, Some(Outcome::Failure));
        assert_eq!(motion.outcome_effect(), Some((Outcome::Failure, ms(0))));
        motion.set_time(ms(2_050));
        motion.observe(true, Some(Outcome::Failure));
        assert_eq!(motion.outcome_effect(), Some((Outcome::Failure, ms(50))));
        assert_ne!(motion.shake(), 0);
        motion.set_time(ms(2_600));
        motion.observe(true, Some(Outcome::Failure));
        assert_eq!(motion.outcome_effect(), None);
        assert_eq!(motion.shake(), 0);

        // A retry that fails again is a new outcome, and plays again.
        motion.observe(true, None);
        motion.observe(true, Some(Outcome::Failure));
        assert_eq!(motion.outcome_effect(), Some((Outcome::Failure, ms(0))));
    }

    #[test]
    fn a_success_stamp_is_drawn_holds_and_fades_as_the_content_returns() {
        assert_eq!(stamp_drawn(ms(0)), 0.0);
        assert!(stamp_drawn(ms(100)) > 0.5, "drawn fast, then slowing");
        assert_eq!(stamp_drawn(STAMP_DRAW), 1.0);

        // The content is almost gone by the time the stroke is under way.
        assert_eq!(stamp_content(ms(0)), 1.0);
        assert_eq!(stamp_content(STAMP_HIDE), 0.0);
        assert_eq!(stamp_content(ms(500)), 0.0);

        // Holding, nothing fades; then stamp, frame and content turn together.
        assert_eq!(stamp_faded(ms(579)), 0.0);
        assert!(stamp_faded(ms(700)) > 0.0 && stamp_faded(ms(700)) < 1.0);
        assert_eq!(stamp_content(ms(700)), stamp_faded(ms(700)));
        assert_eq!(stamp_faded(STAMP_LENGTH), 1.0);
        assert_eq!(stamp_content(STAMP_LENGTH), 1.0);
        assert_eq!(STAMP_LENGTH, ms(880));
    }

    #[test]
    fn a_stamp_plays_its_own_time_over_the_close_and_a_new_popup_cuts_it_off() {
        let area = Rect::new(10, 5, 40, 12);
        // A success shown in its popup is centred on where it is drawn next.
        let mut motion = Motion::manual();
        motion.observe(true, None);
        motion.set_time(ms(1_000));
        motion.observe(true, Some(Outcome::Success));
        assert_eq!(motion.outcome_effect(), None, "a success does not shake");
        assert_eq!(motion.stamp(), None, "not placed yet");
        motion.anchor_stamp(area);
        assert_eq!(motion.stamp(), Some((area, ms(0))));
        assert!(motion.is_moving());

        // It plays on after the popup has closed, and the loop keeps its
        // fast rate until it ends.
        motion.record_frame(area, Color::Cyan);
        motion.set_time(ms(1_300));
        motion.observe(false, None);
        assert_eq!(motion.stamp(), Some((area, ms(300))));
        motion.set_time(ms(1_700));
        motion.observe(false, None);
        assert_eq!(motion.stamp(), Some((area, ms(700))));
        assert!(motion.is_moving());
        motion.set_time(ms(1_880));
        assert_eq!(motion.stamp(), None);
        assert!(!motion.is_moving());

        // A confirm that closes its popup stamps where the popup was, and a
        // popup opening while it plays cuts it off.
        motion.observe(true, None);
        motion.record_frame(area, Color::Cyan);
        motion.set_time(ms(3_000));
        motion.succeeded();
        motion.observe(false, None);
        assert_eq!(motion.stamp(), Some((area, ms(0))));
        motion.set_time(ms(3_100));
        motion.observe(true, None);
        assert_eq!(motion.stamp(), None);

        // Without effects there is no stamp to wait for.
        let mut still = Motion::default();
        still.record_frame(area, Color::Cyan);
        still.succeeded();
        still.observe(true, Some(Outcome::Success));
        still.anchor_stamp(area);
        assert_eq!(still.stamp(), None);
    }

    #[test]
    fn the_shake_swings_both_ways_within_two_columns_and_dies_away() {
        let offsets: Vec<i16> = (0..=40).map(|step| shake_offset(ms(step * 10))).collect();
        assert!(
            offsets.iter().all(|offset| offset.abs() <= 2),
            "{offsets:?}"
        );
        assert!(offsets.contains(&2) || offsets.contains(&1));
        assert!(offsets.iter().any(|offset| *offset < 0), "{offsets:?}");
        assert_eq!(shake_offset(SHAKE), 0);
        assert_eq!(shake_offset(ms(10_000)), 0);
    }

    #[test]
    fn the_spinner_and_caret_follow_the_clock() {
        assert_eq!(spinner_at(ms(0)), "⠋");
        assert_eq!(spinner_at(ms(79)), "⠋");
        assert_eq!(spinner_at(ms(160)), "⠹");
        assert_eq!(spinner_at(ms(800)), "⠋");
        assert!(caret_on_at(ms(0)));
        assert!(caret_on_at(ms(529)));
        assert!(!caret_on_at(ms(530)));
        assert!(caret_on_at(ms(1_060)));

        // A key press puts the caret back on at once.
        let mut motion = Motion::manual();
        motion.set_time(ms(600));
        assert!(!motion.caret_on());
        motion.key_pressed();
        assert!(motion.caret_on());
    }

    #[test]
    fn nothing_moving_means_the_usual_cadence() {
        let mut motion = Motion::manual();
        assert!(!motion.is_moving());
        assert_eq!(motion.frame_interval(false, false), IDLE_FRAME);

        // Opening, the loop runs at the animation rate until everything
        // has settled, then drops back.
        motion.set_time(ms(1_000));
        motion.observe(true, None);
        assert_eq!(motion.frame_interval(false, false), ANIMATION_FRAME);
        motion.set_time(ms(1_400));
        motion.observe(true, None);
        assert_eq!(motion.frame_interval(false, false), IDLE_FRAME);

        // A spinner or caret only lands the wait on its next turn, never
        // making it longer than the usual cadence.
        motion.set_time(ms(1_450));
        assert_eq!(motion.frame_interval(true, false), ms(70));
        assert!(motion.frame_interval(false, true) <= IDLE_FRAME);

        // Closing is animated too, and settles the same way.
        motion.record_frame(Rect::new(0, 0, 10, 5), Color::Cyan);
        motion.observe(false, None);
        assert_eq!(motion.frame_interval(false, false), ANIMATION_FRAME);
        motion.set_time(ms(2_000));
        motion.observe(false, None);
        assert_eq!(motion.frame_interval(false, false), IDLE_FRAME);

        // A dashboard without effects never asks for the fast rate.
        let mut still = Motion::default();
        still.observe(true, Some(Outcome::Success));
        assert!(!still.is_moving());
        assert_eq!(still.openness(), 1.0);
        assert_eq!(still.content_fade(), 1.0);
        assert_eq!(still.outcome_effect(), None);
        assert_eq!(still.frame_interval(false, false), IDLE_FRAME);
    }
}
