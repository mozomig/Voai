use super::{Turn, engine::Embedder};
use std::collections::{BTreeMap, BTreeSet};

type Descriptors = BTreeMap<u32, Option<Vec<f32>>>;

fn normalize(v: &mut [f32]) -> bool {
    let length = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if !length.is_finite() || length <= f32::EPSILON {
        return false;
    }
    for x in v {
        *x /= length;
    }
    true
}

/// Give each voice up to twelve seconds of its longest uncontested speech.
pub(super) fn describe(turns: &[Turn], samples: &[f32], embedder: &Embedder) -> Descriptors {
    let mut ranges = BTreeMap::<u32, Vec<(u64, u64)>>::new();
    let mut edges = Vec::with_capacity(turns.len() * 2);
    for t in turns {
        ranges.entry(t.speaker).or_default();
        edges.extend([t.start_ms, t.end_ms]);
    }
    edges.sort_unstable();
    edges.dedup();
    for edge in edges.windows(2) {
        let (from, to) = (edge[0], edge[1]);
        let mut active = turns.iter().filter(|t| t.start_ms < to && t.end_ms > from);
        if let Some(t) = active.next()
            && active.all(|other| other.speaker == t.speaker)
        {
            ranges.get_mut(&t.speaker).unwrap().push((from, to));
        }
    }
    ranges
        .into_iter()
        .map(|(id, mut spans)| {
            const MAX: usize = 12 * 16_000;
            spans.sort_unstable_by_key(|(a, b)| std::cmp::Reverse(b - a));
            let mut speech = Vec::new();
            for (from, to) in spans {
                let start = (from as usize * 16).min(samples.len());
                let end = (to as usize * 16)
                    .min(start + MAX - speech.len())
                    .min(samples.len());
                speech.extend_from_slice(&samples[start..end]);
                if speech.len() == MAX {
                    break;
                }
            }
            let vector = embedder
                .extract(&speech)
                .filter(|v| v.iter().all(|x| x.is_finite()))
                .and_then(|mut v| normalize(&mut v).then_some(v));
            (id, vector)
        })
        .collect()
}

#[derive(Default)]
struct Profile {
    centroid: Vec<f32>,
    count: usize,
    last_window: u64,
}
impl Profile {
    fn update(&mut self, vector: Option<&[f32]>, window: u64) {
        self.last_window = window;
        if let Some(vector) = vector {
            if self.centroid.is_empty() {
                self.centroid = vector.to_vec();
            } else {
                let weight = self.count.min(8) as f32;
                for (x, v) in self.centroid.iter_mut().zip(vector) {
                    *x = *x * weight + v;
                }
                normalize(&mut self.centroid);
            }
            self.count += 1;
        }
    }
}

#[derive(Default)]
pub(super) struct Registry {
    profiles: BTreeMap<u32, Profile>,
    next: u32,
    window: u64,
}

impl Registry {
    /// Match local identities one-to-one. Real speech on both sides of a window
    /// edge provides a second signal when an embedding is short or uncertain.
    pub(super) fn assign(&mut self, turns: &mut [Turn], voices: Descriptors, tail: &[Turn]) {
        let mut overlap = BTreeMap::<(u32, u32), u64>::new();
        for t in turns.iter() {
            for p in tail {
                let ms = t
                    .end_ms
                    .min(p.end_ms)
                    .saturating_sub(t.start_ms.max(p.start_ms));
                if ms > 0 {
                    *overlap.entry((t.speaker, p.speaker)).or_default() += ms;
                }
            }
        }
        let mut candidates = Vec::new();
        for (local, vector) in &voices {
            let support: u64 = overlap
                .range((*local, 0)..=(*local, u32::MAX))
                .map(|(_, ms)| ms)
                .sum();
            for (&global, p) in &self.profiles {
                let similarity = vector
                    .as_ref()
                    .filter(|_| !p.centroid.is_empty())
                    .map(|v| v.iter().zip(&p.centroid).map(|(a, b)| a * b).sum::<f32>());
                let shared = overlap.get(&(*local, global)).copied().unwrap_or_default();
                let anchored = shared >= 700
                    && shared * 4 >= support * 3
                    && similarity.is_none_or(|s| s >= 0.25);
                if anchored || similarity.is_some_and(|s| s >= 0.45) {
                    let rank = similarity.unwrap_or(0.0) + if anchored { 1.0 } else { 0.0 };
                    candidates.push((rank, *local, global));
                }
            }
        }
        candidates.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
        let mut mapping = BTreeMap::new();
        let mut used = BTreeSet::new();
        for (_, local, global) in candidates {
            if let std::collections::btree_map::Entry::Vacant(entry) = mapping.entry(local)
                && used.insert(global)
            {
                entry.insert(global);
            }
        }
        for (local, vector) in voices {
            let global = *mapping.entry(local).or_insert_with(|| {
                let id = self.next;
                self.next += 1;
                id
            });
            self.profiles
                .entry(global)
                .or_default()
                .update(vector.as_deref(), self.window);
        }
        for t in turns {
            t.speaker = mapping[&t.speaker];
        }
        self.window += 1;
        // Cap retained embeddings even on noisy day-long recordings. IDs are never
        // reused. Once over the cap, keep the most recently heard speakers.
        const CAPACITY: usize = 256;
        while self.profiles.len() > CAPACITY {
            let oldest = self
                .profiles
                .iter()
                .min_by_key(|(_, p)| p.last_window)
                .map(|(id, _)| *id)
                .unwrap();
            self.profiles.remove(&oldest);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn turns(ids: &[u32], start: u64) -> Vec<Turn> {
        ids.iter()
            .enumerate()
            .map(|(i, id)| Turn {
                start_ms: start + i as u64 * 1000,
                end_ms: start + (i as u64 + 1) * 1000,
                speaker: *id,
            })
            .collect()
    }

    #[test]
    fn return_and_new_speaker_after_a_long_gap_have_distinct_global_labels() {
        let mut registry = Registry::default();
        let mut first = turns(&[3, 1], 0);
        registry.assign(
            &mut first,
            BTreeMap::from([(3, Some(vec![1., 0., 0.])), (1, Some(vec![0., 1., 0.]))]),
            &[],
        );
        let mut later = turns(&[1, 3], 1_800_000);
        registry.assign(
            &mut later,
            BTreeMap::from([(1, Some(vec![1., 0., 0.])), (3, Some(vec![0., 0., 1.]))]),
            &[],
        );
        assert_eq!(first[0].speaker, later[0].speaker);
        assert!(first.iter().all(|s| s.speaker != later[1].speaker));
    }

    #[test]
    fn same_voice_without_an_embedding_can_bridge_an_overlap() {
        let mut registry = Registry::default();
        let mut tail = turns(&[7], 74_000);
        registry.assign(&mut tail, BTreeMap::from([(7, None)]), &[]);
        let mut next = turns(&[4], 74_000);
        registry.assign(&mut next, BTreeMap::from([(4, None)]), &tail);
        assert_eq!(tail[0].speaker, next[0].speaker);
    }
}
