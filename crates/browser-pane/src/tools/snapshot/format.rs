//! Second phase of snapshot generation: landmark/reading budgeting, fail-closed field values,
//! ref assignment, and truncation guidance footer (`snapshot.ts:280-440`).

use std::collections::{BTreeMap, HashMap};

use super::{Area, Line, PreparedSnapshot, ReadingKind, SnapshotOutput};

impl PreparedSnapshot {
    /// Formats the final bounded snapshot (`snapshot.ts:280-440`), printing `value=<hidden>` for any
    /// field whose ID is absent or `false` in `readable`.
    pub fn finish(
        mut self,
        readable: &HashMap<i64, bool>,
        boxes: &HashMap<i64, String>,
        previous: &HashMap<i64, String>,
        picked: &HashMap<i64, String>,
        next_ref: &mut impl FnMut() -> String,
    ) -> SnapshotOutput {
        let mut refs: BTreeMap<String, i64> = BTreeMap::new();
        let mut issued: HashMap<i64, String> = HashMap::new();
        let mut resolve_ref = |id: i64| -> String {
            if let Some(p) = picked.get(&id) {
                return p.clone();
            }
            if let Some(p) = previous.get(&id) {
                return p.clone();
            }
            if let Some(p) = issued.get(&id) {
                return p.clone();
            }
            let r = next_ref();
            issued.insert(id, r.clone());
            r
        };

        let mut ordered: Vec<Line> = Vec::new();
        let mut bucket_ranges: [(usize, usize); 6] = [(0, 0); 6];
        for (bi, bucket) in self.buckets.into_iter().enumerate() {
            let start = ordered.len();
            ordered.extend(bucket);
            bucket_ranges[bi] = (start, ordered.len());
        }

        let reading_indices: Vec<usize> = if self.opts.interactive {
            Vec::new()
        } else {
            ordered
                .iter()
                .enumerate()
                .filter(|(_, l)| {
                    !l.deep
                        && (l.reading == Some(ReadingKind::Heading)
                            || (l.reading == Some(ReadingKind::Prose) && l.entry_idx.is_none()))
                })
                .map(|(idx, _)| idx)
                .collect()
        };

        let max_chars = self.opts.max_chars;
        let max_lines = self.opts.max_lines.unwrap_or(usize::MAX);
        let mut placed = vec![false; ordered.len()];
        let mut output: Vec<(usize, String)> = Vec::new();
        let mut omitted_by_area: Vec<(Option<usize>, usize)> = Vec::new();
        let mut omitted = 0usize;
        let mut omitted_interactive = 0usize;
        let mut size = 0usize;
        let mut full = false;

        for pass in 0..7usize {
            let chars_budget = if pass == 0 { max_chars / 4 } else { max_chars };
            let lines_budget = if pass == 0 {
                if max_lines == usize::MAX { usize::MAX } else { max_lines / 4 }
            } else {
                max_lines
            };
            let candidates: Vec<usize> = if pass == 0 {
                reading_indices.clone()
            } else {
                let (s, e) = bucket_ranges[pass - 1];
                let mut g: [Vec<usize>; 4] = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
                for idx in s..e {
                    if placed[idx] {
                        continue;
                    }
                    let l = &ordered[idx];
                    let entry_keep = l.entry_idx.is_some_and(|ei| self.items[ei].keep);
                    let is_area_root = l.id.is_some()
                        && l.area_idx.is_some_and(|ai| Some(self.areas[ai].id) == l.id);
                    let rank = if l.important || entry_keep {
                        0
                    } else if l.interactive || l.reading == Some(ReadingKind::Heading) {
                        1
                    } else if is_area_root {
                        2
                    } else {
                        3
                    };
                    g[rank].push(idx);
                }
                g.into_iter().flatten().collect()
            };

            for idx in candidates {
                if pass == 0 && output.len() >= lines_budget {
                    break;
                }
                let line = &ordered[idx];
                if line.deep || full || output.len() >= max_lines {
                    bump_omit(line, &mut omitted, &mut omitted_interactive, &mut omitted_by_area);
                    continue;
                }
                let key = line.id.map(&mut resolve_ref);
                let mut text = if let Some(k) = &key {
                    let pad_len = line.label.bytes().take_while(|b| *b == b' ').count();
                    format!("{}@{k} {}", &line.label[..pad_len], &line.label[pad_len..])
                } else {
                    line.label.clone()
                };
                if let (Some(id), Some((val_json, _))) = (line.id, &line.value) {
                    if readable.get(&id).copied().unwrap_or(false) {
                        text.push_str(" value=");
                        text.push_str(val_json);
                    } else {
                        text.push_str(" value=<hidden>");
                    }
                }
                if let Some(id) = line.id
                    && let Some(bx) = boxes.get(&id)
                {
                    text.push_str(bx);
                }
                let next_len = size + usize::from(!output.is_empty()) + text.len();
                if next_len > chars_budget && pass == 0 {
                    break;
                }
                if next_len > chars_budget {
                    full = true;
                    bump_omit(line, &mut omitted, &mut omitted_interactive, &mut omitted_by_area);
                    continue;
                }
                size = next_len;
                placed[idx] = true;
                output.push((idx, text));
            }
        }

        if omitted == 0 {
            let content = build_content(&mut output, &ordered, &mut refs, &mut resolve_ref);
            return SnapshotOutput { content, truncated: false, refs };
        }

        for &(idx, _) in &output {
            let mut cur = ordered[idx].entry_idx;
            while let Some(ei) = cur {
                self.items[ei].shown += 1;
                cur = self.items[ei].parent;
            }
        }
        for item in &self.items {
            if item.meaningful && item.shown == 0 {
                self.areas[item.area_idx].skipped += 1;
            }
        }

        let mut hint = make_footer(
            omitted,
            omitted_interactive,
            &omitted_by_area,
            &self.areas,
            &mut refs,
            &mut resolve_ref,
        );
        while !output.is_empty() && size + 1 + hint.len() > max_chars {
            let Some((idx, text)) = output.pop() else { break };
            let mut cur = ordered[idx].entry_idx;
            while let Some(ei) = cur {
                self.items[ei].shown = self.items[ei].shown.saturating_sub(1);
                if self.items[ei].shown == 0 && self.items[ei].meaningful {
                    self.areas[self.items[ei].area_idx].skipped += 1;
                }
                cur = self.items[ei].parent;
            }
            bump_omit(&ordered[idx], &mut omitted, &mut omitted_interactive, &mut omitted_by_area);
            size = size.saturating_sub(text.len() + usize::from(!output.is_empty()));
            hint = make_footer(
                omitted,
                omitted_interactive,
                &omitted_by_area,
                &self.areas,
                &mut refs,
                &mut resolve_ref,
            );
        }

        let sep = if output.is_empty() {
            ""
        } else if output.len() < max_lines {
            "\n"
        } else {
            " "
        };
        let mut content = build_content(&mut output, &ordered, &mut refs, &mut resolve_ref);
        content.push_str(sep);
        content.push_str(&hint);
        SnapshotOutput { content, truncated: true, refs }
    }
}

fn bump_omit(
    line: &Line,
    omitted: &mut usize,
    omitted_interactive: &mut usize,
    omitted_by_area: &mut Vec<(Option<usize>, usize)>,
) {
    *omitted += 1;
    if line.interactive {
        *omitted_interactive += 1;
    }
    if let Some((_, count)) = omitted_by_area.iter_mut().find(|(a, _)| *a == line.area_idx) {
        *count += 1;
    } else {
        omitted_by_area.push((line.area_idx, 1));
    }
}

fn build_content(
    rows: &mut [(usize, String)],
    ordered: &[Line],
    refs: &mut BTreeMap<String, i64>,
    resolve_ref: &mut dyn FnMut(i64) -> String,
) -> String {
    rows.sort_by_key(|(idx, _)| *idx);
    let mut out = String::new();
    for (i, (idx, text)) in rows.iter().enumerate() {
        if let Some(id) = ordered[*idx].id {
            let k = resolve_ref(id);
            refs.insert(k, id);
        }
        if i > 0 {
            out.push('\n');
        }
        out.push_str(text);
    }
    out
}

fn make_footer(
    omitted: usize,
    omitted_interactive: usize,
    omitted_by_area: &[(Option<usize>, usize)],
    areas: &[Area],
    refs: &mut BTreeMap<String, i64>,
    resolve_ref: &mut dyn FnMut(i64) -> String,
) -> String {
    let places: Vec<String> = omitted_by_area
        .iter()
        .take(5)
        .map(|&(area_opt, count)| match area_opt {
            None => format!("document ({count} nodes)"),
            Some(ai) => {
                let a = &areas[ai];
                let k = resolve_ref(a.id);
                refs.insert(k.clone(), a.id);
                if a.skipped > 0 {
                    format!("… +{} more in @{k} [{}] ({count} nodes)", a.skipped, a.role)
                } else {
                    format!("@{k} [{}] ({count} nodes)", a.role)
                }
            }
        })
        .collect();
    let extra = if omitted_by_area.len() > 5 {
        format!("; {} other containers", omitted_by_area.len() - 5)
    } else {
        String::new()
    };
    format!(
        "… {omitted} nodes omitted ({omitted_interactive} interactive): {}{extra}. Use find, snapshot({{tabID, ref}}), or raise maxChars/maxLines.",
        places.join(", ")
    )
}
