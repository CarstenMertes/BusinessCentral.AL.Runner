//! The position-keyed maps the checker fills: FxHash and the sharded `PosMap`.

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

/// FxHash (as in rustc): the per-expression maps below are keyed by three
/// small integers and written for every node, where SipHash dominated their
/// cost. Not DoS-resistant, which does not matter for compiler-internal keys;
/// it is also deterministic.
#[derive(Default, Clone, Copy)]
pub struct FxHasher(u64);

impl Hasher for FxHasher {
    fn write(&mut self, bytes: &[u8]) {
        let mut chunks = bytes.chunks_exact(8);
        for c in &mut chunks {
            self.write_u64(u64::from_le_bytes(c.try_into().expect("8 bytes")));
        }
        let mut tail = 0u64;
        for &b in chunks.remainder() {
            tail = tail << 8 | b as u64;
        }
        self.write_u64(tail);
    }
    fn write_u64(&mut self, i: u64) {
        self.0 = (self.0.rotate_left(5) ^ i).wrapping_mul(0x517c_c1b7_2722_0a95);
    }
    fn write_usize(&mut self, i: usize) {
        self.write_u64(i as u64);
    }
    fn finish(&self) -> u64 {
        self.0
    }
}

pub(crate) type PosKey = (usize, usize, usize);

/// A `HashMap` keyed by node position, hashed with [`FxHasher`].
/// A `HashMap` hashed with [`FxHasher`]: for the small string-keyed maps of the check and the lowering.
pub(crate) type FxMap<K, V> = HashMap<K, V, BuildHasherDefault<FxHasher>>;

/// A `HashSet` hashed with [`FxHasher`].
pub(crate) type FxSet<K> = std::collections::HashSet<K, BuildHasherDefault<FxHasher>>;

pub(crate) type RawPosMap<V> = HashMap<PosKey, V, BuildHasherDefault<FxHasher>>;

/// How many maps a [`PosMap`] is split into. The workers of a parallel check each fill their own
/// `PosMap`; with the same split everywhere, the union is built one shard per thread instead of
/// by one thread inserting every entry.
pub(crate) const POS_SHARDS: usize = 16;

/// Node position (file index, start offset, end offset) to a value, split into [`POS_SHARDS`]
/// hash maps by position.
pub struct PosMap<V> {
    pub(crate) shards: Vec<RawPosMap<V>>,
}

impl<V> Default for PosMap<V> {
    fn default() -> Self {
        PosMap {
            shards: (0..POS_SHARDS).map(|_| RawPosMap::default()).collect(),
        }
    }
}

impl<V> PosMap<V> {
    fn shard_of(key: &PosKey) -> usize {
        let h = (key.0 as u64)
            .wrapping_mul(0x9e37_79b9_7f4a_7c15)
            .wrapping_add((key.1 as u64).wrapping_mul(0x517c_c1b7_2722_0a95));
        (h >> 40) as usize % POS_SHARDS
    }

    pub fn get(&self, key: &PosKey) -> Option<&V> {
        self.shards[Self::shard_of(key)].get(key)
    }

    pub fn insert(&mut self, key: PosKey, value: V) -> Option<V> {
        self.shards[Self::shard_of(&key)].insert(key, value)
    }

    pub fn len(&self) -> usize {
        self.shards.iter().map(RawPosMap::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.shards.iter().all(RawPosMap::is_empty)
    }

    pub fn keys(&self) -> impl Iterator<Item = &PosKey> {
        self.shards.iter().flat_map(RawPosMap::keys)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&PosKey, &V)> {
        self.shards.iter().flat_map(RawPosMap::iter)
    }

    /// The union of `parts`, built with one thread per shard. A key does not repeat across parts
    /// (a routine is checked once); if one did, one of its values would be kept.
    pub(crate) fn union(mut parts: Vec<PosMap<V>>) -> PosMap<V>
    where
        V: Send,
    {
        if parts.len() <= 1 {
            return parts.pop().unwrap_or_default();
        }
        let mut per_shard: Vec<Vec<RawPosMap<V>>> = (0..POS_SHARDS).map(|_| Vec::new()).collect();
        for part in parts {
            for (shard, map) in part.shards.into_iter().enumerate() {
                per_shard[shard].push(map);
            }
        }
        let shards = std::thread::scope(|scope| {
            let handles: Vec<_> = per_shard
                .into_iter()
                .map(|mut maps| {
                    scope.spawn(move || {
                        // Start from the biggest map so the fewest entries move.
                        let biggest = (0..maps.len()).max_by_key(|&i| maps[i].len()).unwrap_or(0);
                        let mut union = maps.swap_remove(biggest);
                        for map in maps {
                            union.extend(map);
                        }
                        union
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().expect("merging a shard"))
                .collect()
        });
        PosMap { shards }
    }
}
