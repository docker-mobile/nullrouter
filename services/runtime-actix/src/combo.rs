//! Combo model selection: strategy, rotation, and capability ordering.
//!
//! Ports the selection half of `open-sse/services/combo.js`. A combo is one name
//! fronting several models; this module decides *which order* to try them in. The
//! execution half — actually calling each model and deciding whether a failure
//! warrants advancing — lives in [`crate::pipeline`].
//!
//! Before this existed a combo resolved to `models.first()` and stopped there. A
//! user who built a round-robin combo got neither rotation nor fallback, and was
//! told nothing: the first model answered every request, and if it was down the
//! whole combo was down. That is the failure this module removes.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// How a combo picks among its models.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ComboStrategy {
    /// Try models in their configured order.
    Fallback,
    /// Advance the starting model every `sticky_limit` requests.
    RoundRobin,
    /// Ask every model in parallel, then have a judge synthesize one answer.
    Fusion,
}

impl ComboStrategy {
    /// Read a settings string. Unrecognised values read as [`Self::Fallback`],
    /// which is upstream's default and the safest reading of a typo: trying the
    /// models in order is never worse than refusing the request.
    pub(crate) fn from_settings(raw: Option<&str>) -> Self {
        match raw {
            Some("round-robin") => Self::RoundRobin,
            Some("fusion") => Self::Fusion,
            _ => Self::Fallback,
        }
    }
}

/// Where a combo's rotation currently sits.
#[derive(Debug, Clone, Default)]
struct Rotation {
    /// Index of the model this combo currently starts from.
    index: usize,
    /// Requests already served from that index.
    consecutive: u32,
    /// The model list this cursor was advanced against.
    ///
    /// Held so an edited combo starts over on its own. Upstream resets rotation
    /// explicitly when combos or settings change; the runtime cannot observe an
    /// edit in the state service, so the cursor notices for itself instead of
    /// carrying a position into a list it no longer describes.
    models: Vec<String>,
}

/// Per-combo rotation cursors, shared across requests.
///
/// Round-robin is stateful by definition: "next model" only means something
/// relative to the last request. Upstream keeps this in a module-level `Map`;
/// here it is owned by the runtime so tests can hold an isolated one.
///
/// A poisoned lock is treated as an empty cursor rather than a panic — losing a
/// rotation position degrades to "start from the first model", which is the
/// fallback strategy and still answers the request.
#[derive(Debug, Clone, Default)]
pub(crate) struct RotationState {
    cursors: Arc<Mutex<HashMap<String, Rotation>>>,
}

impl RotationState {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Take the current rotation for `combo` and advance it if the sticky limit
    /// is reached.
    ///
    /// Returns the index to start from. A cursor recorded against a different
    /// model list is discarded, so an edited combo cannot inherit a position that
    /// no longer means what it meant.
    fn take_index(&self, combo: &str, models: &[String], sticky_limit: u32) -> usize {
        let model_count = models.len();
        if model_count == 0 {
            return 0;
        }
        let Ok(mut cursors) = self.cursors.lock() else {
            return 0;
        };
        let current = cursors
            .get(combo)
            .filter(|rotation| rotation.models == models)
            .cloned()
            .unwrap_or_default();
        let index = current.index % model_count;
        let next_count = current.consecutive.saturating_add(1);
        // `max(1)` because a zero limit would rotate on every request *and*
        // never record progress, which reads as "sticky forever" to a caller
        // inspecting the state.
        let updated = if next_count >= sticky_limit.max(1) {
            Rotation {
                index: (index + 1) % model_count,
                consecutive: 0,
                models: models.to_vec(),
            }
        } else {
            Rotation {
                index,
                consecutive: next_count,
                models: models.to_vec(),
            }
        };
        cursors.insert(combo.to_owned(), updated);
        index
    }
}

/// Rotate `models` left by `index`, preserving relative order.
fn rotate_from(models: &[String], index: usize) -> Vec<String> {
    let mut rotated = Vec::with_capacity(models.len());
    rotated.extend_from_slice(models.get(index..).unwrap_or_default());
    rotated.extend_from_slice(models.get(..index).unwrap_or_default());
    rotated
}

/// The order to try a combo's models in.
///
/// `Fallback` and `Fusion` keep the configured order — fusion asks all of them
/// anyway, so rotating would only shuffle which one judges. `RoundRobin` starts
/// from the rotating cursor and wraps, so every model remains a fallback for the
/// others rather than only the ones after it.
pub(crate) fn ordered_models(
    models: &[String],
    combo: &str,
    strategy: ComboStrategy,
    sticky_limit: u32,
    rotation: &RotationState,
) -> Vec<String> {
    if models.len() <= 1 || strategy != ComboStrategy::RoundRobin {
        return models.to_vec();
    }
    let index = rotation.take_index(combo, models, sticky_limit);
    rotate_from(models, index)
}

/// Rotation work for one combo, for `benches/combo.rs`.
///
/// A purpose-built entry point rather than making [`RotationState`] and
/// [`ordered_models`] public: combo rotation takes a lock on shared state, so its cost
/// under contention is worth measuring, but widening the real API to make a benchmark
/// compile would be the benchmark changing the program it measures.
///
/// Owns its own [`RotationState`] so the bench cannot accidentally share a cursor
/// between iterations and measure lock-free repeats.
#[cfg(feature = "bench-internals")]
pub struct RotationBench {
    rotation: RotationState,
    models: Vec<String>,
}

#[cfg(feature = "bench-internals")]
impl RotationBench {
    /// A bench over `count` synthetic models.
    #[must_use]
    pub fn new(count: usize) -> Self {
        Self {
            rotation: RotationState::new(),
            models: (0..count)
                .map(|index| format!("openai-compatible-{index}/model-{index}"))
                .collect(),
        }
    }

    /// One `fill-first` ordering. Should not take the lock at all.
    #[must_use]
    pub fn fill_first(&self) -> usize {
        ordered_models(
            &self.models,
            "panel",
            ComboStrategy::Fallback,
            3,
            &self.rotation,
        )
        .len()
    }

    /// One `fusion` ordering. Also should not take the lock: fusion asks every model,
    /// so rotating would only shuffle which one judges.
    #[must_use]
    pub fn fusion(&self) -> usize {
        ordered_models(
            &self.models,
            "panel",
            ComboStrategy::Fusion,
            3,
            &self.rotation,
        )
        .len()
    }

    /// One `round-robin` ordering, which takes the lock and advances the cursor.
    #[must_use]
    pub fn round_robin(&self, sticky_limit: u32) -> usize {
        ordered_models(
            &self.models,
            "panel",
            ComboStrategy::RoundRobin,
            sticky_limit,
            &self.rotation,
        )
        .len()
    }
}

#[cfg(test)]
mod tests {
    use super::{ComboStrategy, RotationState, ordered_models, rotate_from};

    fn models(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn an_unknown_strategy_reads_as_fallback() {
        // A typo in settings must not refuse the request or invent rotation.
        assert_eq!(
            ComboStrategy::from_settings(Some("round-robbin")),
            ComboStrategy::Fallback
        );
        assert_eq!(ComboStrategy::from_settings(None), ComboStrategy::Fallback);
        assert_eq!(
            ComboStrategy::from_settings(Some("round-robin")),
            ComboStrategy::RoundRobin
        );
        assert_eq!(
            ComboStrategy::from_settings(Some("fusion")),
            ComboStrategy::Fusion
        );
    }

    #[test]
    fn fallback_keeps_the_configured_order() {
        let list = models(&["a", "b", "c"]);
        let rotation = RotationState::new();
        for _ in 0..3 {
            assert_eq!(
                ordered_models(&list, "combo", ComboStrategy::Fallback, 1, &rotation),
                list,
                "fallback must not rotate"
            );
        }
    }

    #[test]
    fn round_robin_advances_one_model_per_request() {
        let list = models(&["a", "b", "c"]);
        let rotation = RotationState::new();
        let seen: Vec<String> = (0..4)
            .map(|_| {
                ordered_models(&list, "combo", ComboStrategy::RoundRobin, 1, &rotation)
                    .first()
                    .cloned()
                    .unwrap_or_default()
            })
            .collect();
        // Wraps back to the first model rather than running off the end.
        assert_eq!(seen, models(&["a", "b", "c", "a"]));
    }

    #[test]
    fn round_robin_keeps_every_model_as_a_fallback() {
        // Rotation reorders; it must never shorten the list, or a rotated-to
        // position would have fewer models to fall back through than position 0.
        let list = models(&["a", "b", "c"]);
        let rotation = RotationState::new();
        let second = ordered_models(&list, "combo", ComboStrategy::RoundRobin, 1, &rotation);
        assert_eq!(second, models(&["a", "b", "c"]));
        let third = ordered_models(&list, "combo", ComboStrategy::RoundRobin, 1, &rotation);
        assert_eq!(third, models(&["b", "c", "a"]), "must wrap, not truncate");
        assert_eq!(third.len(), list.len());
    }

    #[test]
    fn a_sticky_limit_holds_a_model_for_that_many_requests() {
        let list = models(&["a", "b"]);
        let rotation = RotationState::new();
        let firsts: Vec<String> = (0..6)
            .map(|_| {
                ordered_models(&list, "combo", ComboStrategy::RoundRobin, 3, &rotation)
                    .first()
                    .cloned()
                    .unwrap_or_default()
            })
            .collect();
        assert_eq!(firsts, models(&["a", "a", "a", "b", "b", "b"]));
    }

    #[test]
    fn a_zero_sticky_limit_still_rotates() {
        // Upstream normalises a non-positive limit to 1. A literal 0 would both
        // rotate every request and never record progress.
        let list = models(&["a", "b"]);
        let rotation = RotationState::new();
        let firsts: Vec<String> = (0..3)
            .map(|_| {
                ordered_models(&list, "combo", ComboStrategy::RoundRobin, 0, &rotation)
                    .first()
                    .cloned()
                    .unwrap_or_default()
            })
            .collect();
        assert_eq!(firsts, models(&["a", "b", "a"]));
    }

    #[test]
    fn rotation_is_tracked_per_combo() {
        // Two combos sharing one cursor would advance each other's position.
        let list = models(&["a", "b", "c"]);
        let rotation = RotationState::new();
        let _ = ordered_models(&list, "first", ComboStrategy::RoundRobin, 1, &rotation);
        let second_combo = ordered_models(&list, "second", ComboStrategy::RoundRobin, 1, &rotation);
        assert_eq!(
            second_combo,
            models(&["a", "b", "c"]),
            "a second combo starts from its own position"
        );
    }

    #[test]
    fn a_single_model_combo_never_rotates() {
        let list = models(&["only"]);
        let rotation = RotationState::new();
        for _ in 0..3 {
            assert_eq!(
                ordered_models(&list, "combo", ComboStrategy::RoundRobin, 1, &rotation),
                list
            );
        }
    }

    #[test]
    fn an_edited_combo_starts_its_rotation_over() {
        // A cursor recorded against one model list means nothing against another:
        // position 2 of [a,b,c] is not position 2 of [x,y,z]. Upstream resets
        // rotation explicitly on edit; the runtime cannot see that edit, so the
        // cursor has to notice for itself.
        let before = models(&["a", "b", "c"]);
        let rotation = RotationState::new();
        for _ in 0..2 {
            let _ = ordered_models(&before, "combo", ComboStrategy::RoundRobin, 1, &rotation);
        }
        let after = models(&["x", "y", "z"]);
        assert_eq!(
            ordered_models(&after, "combo", ComboStrategy::RoundRobin, 1, &rotation),
            models(&["x", "y", "z"]),
            "an edited combo starts from its own first model"
        );
    }

    #[test]
    fn a_shrunk_combo_cannot_point_past_its_models() {
        // A combo edited down between requests must not index out of range, even
        // before the list-change check catches it.
        let three = models(&["a", "b", "c"]);
        let rotation = RotationState::new();
        for _ in 0..3 {
            let _ = ordered_models(&three, "combo", ComboStrategy::RoundRobin, 1, &rotation);
        }
        let one = models(&["a"]);
        assert_eq!(
            ordered_models(&one, "combo", ComboStrategy::RoundRobin, 1, &rotation),
            one
        );
        let two = models(&["a", "b"]);
        let after = ordered_models(&two, "combo", ComboStrategy::RoundRobin, 1, &rotation);
        assert_eq!(after.len(), 2, "must stay in range: {after:?}");
    }

    #[test]
    fn rotating_preserves_relative_order() {
        let list = models(&["a", "b", "c", "d"]);
        assert_eq!(rotate_from(&list, 0), list);
        assert_eq!(rotate_from(&list, 2), models(&["c", "d", "a", "b"]));
        // An out-of-range index yields the tail-then-head split, never a panic.
        assert_eq!(rotate_from(&list, 4), list);
    }
}
