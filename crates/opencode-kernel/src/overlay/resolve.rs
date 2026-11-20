//! Start-up validation and deterministic Kahn topological resolution for [`super::OverlayDispatcher`].

use std::cmp::Reverse;
use std::collections::{BTreeSet, HashMap, HashSet};

use super::dispatcher::{DispatchTable, EscapeEntry, KeyedEntry, RenderedEntry};

pub(super) fn resolve_table<S>(table: &mut DispatchTable<S>) {
    let key_band_rank = index_unique("key band", table.key_bands.iter().map(|b| b.name));
    let layer_rank = index_unique("overlay layer", table.layers.iter().map(|l| l.name));
    let escape_band_rank = index_unique("escape band", table.escape_bands.iter().map(|b| b.name));

    for layer in &table.layers {
        assert!(
            key_band_rank.contains_key(layer.default_key_band.name),
            "overlay dispatcher: layer '{}' default_key_band '{}' is not declared",
            layer.name,
            layer.default_key_band.name
        );
    }

    let mut overlay_by_id: HashMap<&'static str, usize> = HashMap::with_capacity(table.pending_overlays.len());
    for (idx, reg) in table.pending_overlays.iter().enumerate() {
        assert!(
            overlay_by_id.insert(reg.id, idx).is_none(),
            "overlay dispatcher: duplicate overlay id '{}'",
            reg.id
        );
        if reg.render.is_none() {
            assert!(
                reg.above.is_empty(),
                "overlay dispatcher: overlay '{}' declares `above` edges without a render callback",
                reg.id
            );
        }
        if reg.on_key.is_none() {
            assert!(
                reg.key_band.is_none() && reg.after_key.is_empty(),
                "overlay dispatcher: overlay '{}' declares key band or `after_key` edges without a key callback",
                reg.id
            );
        }
    }

    resolve_render(table, &layer_rank, &overlay_by_id);
    resolve_keys(table, &layer_rank, &key_band_rank, &overlay_by_id);
    resolve_escape(table, &escape_band_rank);

    table.dismiss.clear();
    for reg in &table.pending_overlays {
        if let Some(on_dismiss) = reg.on_dismiss {
            table.dismiss.push((reg.id, on_dismiss));
        }
    }
}

fn index_unique(kind: &str, names: impl Iterator<Item = &'static str>) -> HashMap<&'static str, usize> {
    let mut map = HashMap::new();
    for (rank, name) in names.enumerate() {
        assert!(map.insert(name, rank).is_none(), "overlay dispatcher: duplicate {kind} '{name}'");
    }
    map
}

fn resolve_render<S>(
    table: &mut DispatchTable<S>,
    layer_rank: &HashMap<&'static str, usize>,
    overlay_by_id: &HashMap<&'static str, usize>,
) {
    let n = table.pending_overlays.len();
    let mut ranks: Vec<Option<usize>> = vec![None; n];
    for (idx, reg) in table.pending_overlays.iter().enumerate() {
        if reg.render.is_some() {
            let layer = reg.layer.unwrap_or_else(|| {
                panic!("overlay dispatcher: renderable overlay '{}' has no declared OverlayLayer", reg.id)
            });
            let &rank = layer_rank.get(layer.name).unwrap_or_else(|| {
                panic!("overlay dispatcher: overlay '{}' targets undeclared layer '{}'", reg.id, layer.name)
            });
            ranks[idx] = Some(rank);
        }
    }

    let mut in_degree = vec![0usize; n];
    let mut outgoing: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (idx, reg) in table.pending_overlays.iter().enumerate() {
        let Some(rank) = ranks[idx] else { continue };
        let mut seen = HashSet::new();
        for &dep in &reg.above {
            assert!(dep != reg.id, "overlay dispatcher: render cycle: '{}' cannot draw above itself", reg.id);
            let &dep_idx = overlay_by_id.get(dep).unwrap_or_else(|| {
                panic!("overlay dispatcher: overlay '{}' draws above missing overlay '{dep}'", reg.id)
            });
            let dep_rank = ranks[dep_idx].unwrap_or_else(|| {
                panic!("overlay dispatcher: overlay '{}' draws above non-rendered overlay '{dep}'", reg.id)
            });
            if dep_rank > rank {
                panic!(
                    "overlay dispatcher: overlay '{}' ({}) cannot draw above '{dep}' in later layer ({})",
                    reg.id,
                    table.layers[rank].name,
                    table.layers[dep_rank].name
                );
            }
            if dep_rank == rank && seen.insert(dep) {
                outgoing[dep_idx].push(idx);
                in_degree[idx] += 1;
            }
        }
    }

    let mut buckets: Vec<Vec<usize>> = vec![Vec::new(); table.layers.len()];
    for (idx, rank) in ranks.iter().enumerate() {
        if let Some(rank) = *rank {
            buckets[rank].push(idx);
        }
    }

    table.rendered.clear();
    table.rendered_ranges.clear();
    for (rank, &layer) in table.layers.iter().enumerate() {
        let start = table.rendered.len();
        let bucket = &buckets[rank];
        let mut ready: BTreeSet<&'static str> = BTreeSet::new();
        for &idx in bucket {
            if in_degree[idx] == 0 {
                ready.insert(table.pending_overlays[idx].id);
            }
        }
        let mut emitted = 0usize;
        while let Some(id) = ready.pop_first() {
            let idx = overlay_by_id[id];
            if let Some(render) = table.pending_overlays[idx].render {
                table.rendered.push(RenderedEntry { id, layer: layer.name, render });
            }
            emitted += 1;
            for &next in &outgoing[idx] {
                in_degree[next] -= 1;
                if in_degree[next] == 0 {
                    ready.insert(table.pending_overlays[next].id);
                }
            }
        }
        if emitted < bucket.len() {
            let mut stuck: Vec<&'static str> = bucket
                .iter()
                .copied()
                .filter(|&idx| in_degree[idx] > 0)
                .map(|idx| table.pending_overlays[idx].id)
                .collect();
            stuck.sort_unstable();
            panic!(
                "overlay dispatcher: render cycle in layer '{}' involving: {}",
                layer.name,
                stuck.join(", ")
            );
        }
        table.rendered_ranges.push((layer, start..table.rendered.len()));
    }
}

fn resolve_keys<S>(
    table: &mut DispatchTable<S>,
    layer_rank: &HashMap<&'static str, usize>,
    key_band_rank: &HashMap<&'static str, usize>,
    overlay_by_id: &HashMap<&'static str, usize>,
) {
    let n = table.pending_overlays.len();
    let mut band_ranks: Vec<Option<usize>> = vec![None; n];
    let mut visual_ranks: Vec<Option<usize>> = vec![None; n];
    for (idx, reg) in table.pending_overlays.iter().enumerate() {
        visual_ranks[idx] = reg.layer.and_then(|l| layer_rank.get(l.name).copied());
        if reg.on_key.is_some() {
            let band = reg.key_band.or_else(|| reg.layer.map(|l| l.default_key_band)).unwrap_or_else(|| {
                panic!(
                    "overlay dispatcher: keyed overlay '{}' has no OverlayLayer or explicit KeyBand; use `with_key_in`",
                    reg.id
                )
            });
            let &rank = key_band_rank.get(band.name).unwrap_or_else(|| {
                panic!("overlay dispatcher: overlay '{}' targets undeclared key band '{}'", reg.id, band.name)
            });
            band_ranks[idx] = Some(rank);
        }
    }

    let mut in_degree = vec![0usize; n];
    let mut outgoing: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (idx, reg) in table.pending_overlays.iter().enumerate() {
        let Some(rank) = band_ranks[idx] else { continue };
        let mut seen = HashSet::new();
        for &dep in &reg.after_key {
            assert!(dep != reg.id, "overlay dispatcher: key cycle: '{}' cannot run after itself", reg.id);
            let &dep_idx = overlay_by_id.get(dep).unwrap_or_else(|| {
                panic!("overlay dispatcher: overlay '{}' runs key after missing overlay '{dep}'", reg.id)
            });
            let dep_rank = band_ranks[dep_idx].unwrap_or_else(|| {
                panic!("overlay dispatcher: overlay '{}' runs key after keyless overlay '{dep}'", reg.id)
            });
            if dep_rank > rank {
                panic!(
                    "overlay dispatcher: overlay '{}' ({}) cannot run key after '{dep}' in later band ({})",
                    reg.id,
                    table.key_bands[rank].name,
                    table.key_bands[dep_rank].name
                );
            }
            if dep_rank == rank && seen.insert(dep) {
                outgoing[dep_idx].push(idx);
                in_degree[idx] += 1;
            }
        }
    }

    let mut buckets: Vec<Vec<usize>> = vec![Vec::new(); table.key_bands.len()];
    for (idx, rank) in band_ranks.iter().enumerate() {
        if let Some(rank) = *rank {
            buckets[rank].push(idx);
        }
    }

    table.keyed.clear();
    table.keyed_ranges.clear();
    for (rank, &band) in table.key_bands.iter().enumerate() {
        let start = table.keyed.len();
        let bucket = &buckets[rank];
        // Higher visual layers handle keys first automatically; non-rendered overlays (`None`) run
        // after rendered overlays in the same band. Lexicographical id breaks ties deterministically.
        let mut ready: BTreeSet<(Reverse<Option<usize>>, &'static str)> = BTreeSet::new();
        for &idx in bucket {
            if in_degree[idx] == 0 {
                ready.insert((Reverse(visual_ranks[idx]), table.pending_overlays[idx].id));
            }
        }
        let mut emitted = 0usize;
        while let Some((_, id)) = ready.pop_first() {
            let idx = overlay_by_id[id];
            if let Some(on_key) = table.pending_overlays[idx].on_key {
                table.keyed.push(KeyedEntry { id, band: band.name, on_key });
            }
            emitted += 1;
            for &next in &outgoing[idx] {
                in_degree[next] -= 1;
                if in_degree[next] == 0 {
                    ready.insert((Reverse(visual_ranks[next]), table.pending_overlays[next].id));
                }
            }
        }
        if emitted < bucket.len() {
            let mut stuck: Vec<&'static str> = bucket
                .iter()
                .copied()
                .filter(|&idx| in_degree[idx] > 0)
                .map(|idx| table.pending_overlays[idx].id)
                .collect();
            stuck.sort_unstable();
            panic!(
                "overlay dispatcher: key cycle in band '{}' involving: {}",
                band.name,
                stuck.join(", ")
            );
        }
        table.keyed_ranges.push((band, start..table.keyed.len()));
    }
}

fn resolve_escape<S>(table: &mut DispatchTable<S>, band_rank: &HashMap<&'static str, usize>) {
    let n = table.pending_escape.len();
    let mut by_id: HashMap<&'static str, usize> = HashMap::with_capacity(n);
    let mut ranks: Vec<usize> = Vec::with_capacity(n);
    for (idx, layer) in table.pending_escape.iter().enumerate() {
        assert!(
            by_id.insert(layer.id, idx).is_none(),
            "overlay dispatcher: duplicate escape layer id '{}'",
            layer.id
        );
        let &rank = band_rank.get(layer.band.name).unwrap_or_else(|| {
            panic!("overlay dispatcher: escape layer '{}' targets undeclared band '{}'", layer.id, layer.band.name)
        });
        ranks.push(rank);
    }

    let mut in_degree = vec![0usize; n];
    let mut outgoing: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (idx, layer) in table.pending_escape.iter().enumerate() {
        let mut seen = HashSet::new();
        for &dep in &layer.after {
            assert!(dep != layer.id, "overlay dispatcher: escape cycle: '{}' cannot run after itself", layer.id);
            let &dep_idx = by_id.get(dep).unwrap_or_else(|| {
                panic!("overlay dispatcher: escape layer '{}' depends on missing layer '{dep}'", layer.id)
            });
            if ranks[dep_idx] > ranks[idx] {
                panic!(
                    "overlay dispatcher: escape layer '{}' ({}) cannot run after '{dep}' in later band ({})",
                    layer.id,
                    table.escape_bands[ranks[idx]].name,
                    table.escape_bands[ranks[dep_idx]].name
                );
            }
            if ranks[dep_idx] == ranks[idx] && seen.insert(dep) {
                outgoing[dep_idx].push(idx);
                in_degree[idx] += 1;
            }
        }
    }

    let mut buckets: Vec<Vec<usize>> = vec![Vec::new(); table.escape_bands.len()];
    for (idx, &rank) in ranks.iter().enumerate() {
        buckets[rank].push(idx);
    }

    table.escape_resolved.clear();
    for (rank, &band) in table.escape_bands.iter().enumerate() {
        let bucket = &buckets[rank];
        let mut ready: BTreeSet<&'static str> = BTreeSet::new();
        for &idx in bucket {
            if in_degree[idx] == 0 {
                ready.insert(table.pending_escape[idx].id);
            }
        }
        let mut emitted = 0usize;
        while let Some(id) = ready.pop_first() {
            let idx = by_id[id];
            let layer = &table.pending_escape[idx];
            table.escape_resolved.push(EscapeEntry {
                id,
                band: band.name,
                is_open: layer.is_open,
                on_escape: layer.on_escape,
            });
            emitted += 1;
            for &next in &outgoing[idx] {
                in_degree[next] -= 1;
                if in_degree[next] == 0 {
                    ready.insert(table.pending_escape[next].id);
                }
            }
        }
        if emitted < bucket.len() {
            let mut stuck: Vec<&'static str> = bucket
                .iter()
                .copied()
                .filter(|&idx| in_degree[idx] > 0)
                .map(|idx| table.pending_escape[idx].id)
                .collect();
            stuck.sort_unstable();
            panic!(
                "overlay dispatcher: escape cycle in band '{}' involving: {}",
                band.name,
                stuck.join(", ")
            );
        }
    }
}
