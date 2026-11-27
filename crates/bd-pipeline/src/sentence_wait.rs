//! How long the listener waits, after an utterance stops, before it ends the
//! sentence there ([`crate::PipelineConfig::sentence_wait`]), and a wait
//! suggested from the speaker's own pauses ([`suggest`]).
//!
//! The suggestion learns only from pauses the app can label on its own:
//! the speaker went on before the wait ran out, after a word a sentence can
//! end on, and the recognizer, hearing the next words, didn't end the
//! sentence there ([`Pause`]). A longer pause gets the listener's full stop,
//! so whether its sentence went on can't be told; what's seen is cut off at
//! the wait in force. So the suggestion is a log-normal fitted to the pauses
//! seen, truncated at each one's own wait, and its 95th percentile: the wait
//! that would have kept 19 in 20 of those sentences whole. On two Windows
//! testers' telemetry (2026-11-26) it came to 1.9 and 2.0 s; today's 1.5 s
//! was their 90th percentile. A heavy tail of long thinking pauses (2–8 s)
//! is beyond what it can see, so it errs short.

use std::time::Duration;

/// The wait before 0.1.24, and still the default.
pub const DEFAULT: Duration = Duration::from_millis(1500);
/// The shortest wait that can be set or suggested...
pub const MIN: Duration = Duration::from_millis(1000);
/// ...and the longest.
pub const MAX: Duration = Duration::from_millis(3000);
/// Pauses kept to suggest from: the most recent this many.
pub const KEEP: usize = 400;
/// Fewer pauses than this suggest nothing: on 100, the 95th percentile
/// still swung between 1.45 and 3.05 s for one speaker.
pub const ENOUGH: usize = 150;

/// The wait after a word a sentence doesn't end on ("and", "the", a
/// comma): the same speaker thinking, for longer. 4 s at the default.
pub fn unfinished(wait: Duration) -> Duration {
    wait * 8 / 3
}

/// A wait as set: within [`MIN`]..=[`MAX`].
pub fn clamp(wait: Duration) -> Duration {
    wait.clamp(MIN, MAX)
}

/// A pause the speaker went on from mid-sentence before the listener's wait
/// ran out, after a word a sentence can end on ([`crate::Event::Paused`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pause {
    /// From the utterance stopping (when the VAD said so) to the next
    /// starting: on the same clock as `wait`, so always shorter.
    pub pause: Duration,
    /// The wait in force then, past which no pause could be seen.
    pub wait: Duration,
}

/// The suggested wait from `pauses`, rounded to a tenth of a second, within
/// [`MIN`]..=[`MAX`]; `None` with fewer than [`ENOUGH`].
pub fn suggest(pauses: &[Pause]) -> Option<Duration> {
    let pauses: Vec<(f64, f64)> = pauses
        .iter()
        .filter(|p| p.pause < p.wait)
        // A pause of next to nothing is the VAD splitting a word: floor it
        // so its log stays finite.
        .map(|p| (p.pause.as_secs_f64().max(0.01).ln(), p.wait.as_secs_f64().ln()))
        .collect();
    if pauses.len() < ENOUGH {
        return None;
    }
    let (mu, sigma) = fit(&pauses);
    let q95 = (mu + sigma * Z95).exp();
    let tenths = (q95 * 10.0).round() / 10.0;
    Some(clamp(Duration::from_secs_f64(tenths)))
}

/// The standard normal's 95th percentile.
const Z95: f64 = 1.644_853_6;

/// The maximum-likelihood log-normal (μ, σ of the logs) of pauses given as
/// (ln pause, ln wait), each truncated at its wait: a grid over μ and σ,
/// then a finer one around the best.
fn fit(pauses: &[(f64, f64)]) -> (f64, f64) {
    let n = pauses.len() as f64;
    let s1: f64 = pauses.iter().map(|p| p.0).sum();
    let s2: f64 = pauses.iter().map(|p| p.0 * p.0).sum();
    // The truncation term only depends on the wait: few distinct ones.
    let mut waits: Vec<(f64, f64)> = Vec::new();
    for &(_, w) in pauses {
        match waits.iter_mut().find(|(lw, _)| (*lw - w).abs() < 1e-9) {
            Some(entry) => entry.1 += 1.0,
            None => waits.push((w, 1.0)),
        }
    }
    let ll = |mu: f64, sigma: f64| {
        let squares = s2 - 2.0 * mu * s1 + n * mu * mu;
        let kept: f64 = waits.iter().map(|&(w, count)| count * phi((w - mu) / sigma).max(1e-300).ln()).sum();
        -n * sigma.ln() - squares / (2.0 * sigma * sigma) - kept
    };
    let mut best = (f64::NEG_INFINITY, 0.0, 1.0);
    let search = |mus: (f64, f64, f64), sigmas: (f64, f64, f64), best: &mut (f64, f64, f64)| {
        let mut mu = mus.0;
        while mu <= mus.1 {
            let mut sigma = sigmas.0;
            while sigma <= sigmas.1 {
                let l = ll(mu, sigma);
                if l > best.0 {
                    *best = (l, mu, sigma);
                }
                sigma += sigmas.2;
            }
            mu += mus.2;
        }
    };
    // Medians of 0.05 to 5 s, spreads of 0.1 to 2.5.
    search(((0.05f64).ln(), (5.0f64).ln(), 0.05), (0.1, 2.5, 0.02), &mut best);
    let (_, mu, sigma) = best;
    search((mu - 0.05, mu + 0.05, 0.005), ((sigma - 0.02).max(0.05), sigma + 0.02, 0.002), &mut best);
    (best.1, best.2)
}

/// The standard normal's cumulative distribution.
fn phi(z: f64) -> f64 {
    0.5 * erfc(-z / std::f64::consts::SQRT_2)
}

/// The complementary error function (Numerical Recipes' `erfcc`, within
/// 1.2e-7 everywhere).
fn erfc(x: f64) -> f64 {
    let z = x.abs();
    let t = 1.0 / (1.0 + 0.5 * z);
    let poly = -z * z - 1.265_512_23
        + t * (1.000_023_68
            + t * (0.374_091_96
                + t * (0.096_784_18
                    + t * (-0.186_288_06
                        + t * (0.278_868_07 + t * (-1.135_203_98 + t * (1.488_515_87 + t * (-0.822_152_23 + t * 0.170_872_77))))))));
    let r = t * poly.exp();
    if x >= 0.0 { r } else { 2.0 - r }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The standard normal's quantile, by bisection.
    fn quantile(p: f64) -> f64 {
        let (mut lo, mut hi) = (-10.0, 10.0);
        for _ in 0..100 {
            let mid = (lo + hi) / 2.0;
            if phi(mid) < p { lo = mid } else { hi = mid }
        }
        lo
    }

    /// `n` evenly spread draws of a log-normal with this median and spread,
    /// keeping those under `wait`, as a speaker's seen pauses would be.
    fn seen(median: f64, sigma: f64, n: usize, wait: f64) -> Vec<Pause> {
        (0..n)
            .map(|i| median * (sigma * quantile((i as f64 + 0.5) / n as f64)).exp())
            .filter(|&secs| secs < wait)
            .map(|secs| Pause { pause: Duration::from_secs_f64(secs), wait: Duration::from_secs_f64(wait) })
            .collect()
    }

    #[test]
    fn erfc_is_accurate() {
        assert!((erfc(0.0) - 1.0).abs() < 1e-7);
        assert!((erfc(1.0) - 0.157_299_207).abs() < 1e-7);
        assert!((erfc(-1.0) - 1.842_700_793).abs() < 1e-7);
        assert!((phi(Z95) - 0.95).abs() < 1e-6);
    }

    #[test]
    fn finds_the_95th_percentile_past_the_wait() {
        // Like the Windows tester: median 0.6 s, spread 0.7, so the 95th
        // percentile, 1.90 s, is past the 1.5 s wait that cut them off.
        let pauses = seen(0.6, 0.7, 500, 1.5);
        assert!(pauses.len() < 500 && pauses.len() >= ENOUGH);
        assert_eq!(suggest(&pauses), Some(Duration::from_millis(1900)));
    }

    #[test]
    fn pauses_seen_under_different_waits_agree() {
        let mut pauses = seen(0.6, 0.7, 300, 1.5);
        pauses.extend(seen(0.6, 0.7, 300, 2.5));
        assert_eq!(suggest(&pauses), Some(Duration::from_millis(1900)));
    }

    #[test]
    fn a_quick_speaker_gets_the_shortest_wait_and_a_slow_one_the_longest() {
        assert_eq!(suggest(&seen(0.25, 0.5, 400, 1.5)), Some(MIN));
        assert_eq!(suggest(&seen(1.4, 0.8, 1000, 3.0)), Some(MAX));
    }

    #[test]
    fn too_few_pauses_suggest_nothing() {
        let pauses = seen(0.6, 0.7, 500, 1.5);
        assert_eq!(suggest(&pauses[..ENOUGH - 1]), None);
        // A pause as long as its wait can't have been seen going on.
        let mut odd = pauses[..ENOUGH - 1].to_vec();
        odd.push(Pause { pause: Duration::from_secs(2), wait: Duration::from_millis(1500) });
        assert_eq!(suggest(&odd), None);
    }

    #[test]
    fn the_unfinished_wait_keeps_its_ratio() {
        assert_eq!(unfinished(DEFAULT), Duration::from_secs(4));
        assert_eq!(unfinished(MAX), Duration::from_secs(8));
        assert_eq!(clamp(Duration::from_millis(200)), MIN);
    }
}
