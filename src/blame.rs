use std::{
    collections::BTreeMap,
    fmt::{Debug, Display},
    hash::Hash,
};

// Has to be u32 bc gix returns Ranges of u32s in its diff output
pub type LineNumber = u32;
pub type LineDelta = i64;
pub type LineDiffs<CohortKey> = Vec<(
    std::ops::Range<LineNumber>,
    std::ops::Range<LineNumber>,
    CohortKey,
)>;

pub trait Keyable: Copy + PartialEq + Display + Debug + Eq + Hash + Send + Sync {}
impl<T: Copy + PartialEq + Display + Debug + Eq + Hash + Send + Sync> Keyable for T {}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlameRange<CohortKey>
where
    CohortKey: Keyable,
{
    pub start_line: LineNumber,
    pub line_count: LineNumber,
    pub cohort: CohortKey,
}

impl<CohortKey: Keyable> BlameRange<CohortKey> {
    pub fn new(start_line: LineNumber, line_count: LineNumber, cohort: CohortKey) -> Self {
        Self {
            start_line,
            line_count,
            cohort,
        }
    }
}

/// FileBlame stores the data for a "git blame" for a file.
/// This means intervals of line numbers pointing to which commit introduced those lines.
/// Internally the line numbers are stored as "change points" mapping the start of
/// the interval to a "cohort" (any information you want to associate with the line).
/// The end of each interval is implicit: the next key, or `total_lines` for the last one.
#[derive(Debug, Clone)]
pub struct FileBlame<CohortKey: Keyable> {
    change_points: BTreeMap<LineNumber, CohortKey>,
    total_lines: LineNumber,
    cohort_stats: std::collections::HashMap<CohortKey, u64>,
}

impl<CohortKey: Keyable> FileBlame<CohortKey> {
    pub fn new(total_lines: LineNumber, cohort: CohortKey) -> Self {
        let mut change_points = BTreeMap::new();
        let mut cohort_stats = std::collections::HashMap::new();
        if total_lines > 0 {
            change_points.insert(0, cohort);
            cohort_stats.insert(cohort, total_lines as u64);
        }
        Self {
            change_points,
            total_lines,
            cohort_stats,
        }
    }

    pub fn total_lines(&self) -> LineNumber {
        self.total_lines
    }

    pub fn range_count(&self) -> usize {
        self.change_points.len()
    }

    fn cohort_at_index(&self, index: LineNumber) -> Option<CohortKey> {
        if index >= self.total_lines {
            return None;
        }
        self.change_points
            .range(..=index)
            .next_back()
            .map(|(_, cohort)| *cohort)
    }

    pub fn merge_adjacent_ranges(&mut self) {
        if self.change_points.is_empty() {
            return;
        }
        let mut prev_value: Option<CohortKey> = None;
        let mut keys_to_remove: Vec<LineNumber> = Vec::new();
        for (&k, &v) in self.change_points.iter() {
            if let Some(prev) = prev_value {
                if prev == v {
                    keys_to_remove.push(k);
                }
            }
            prev_value = Some(v);
        }
        for k in keys_to_remove {
            self.change_points.remove(&k);
        }
    }

    /// Get an iterator over ranges as (start_line, end_line, cohort)
    pub fn ranges(&self) -> impl Iterator<Item = (LineNumber, LineNumber, CohortKey)> + '_ {
        let mut iter = self.change_points.iter().peekable();
        std::iter::from_fn(move || {
            if let Some((&start, &cohort)) = iter.next() {
                let end = if let Some((next_start, _)) = iter.peek() {
                    **next_start
                } else {
                    self.total_lines
                };
                debug_assert!(
                    end >= start,
                    "start is after end for range: {}, end: {}. next start: {:?}, total lines: {}",
                    start,
                    end,
                    iter.peek().map(|(k, _)| *k),
                    self.total_lines
                );
                Some((start, end, cohort))
            } else {
                None
            }
        })
    }

    fn compute_cohort_stats(&self) -> std::collections::HashMap<CohortKey, u64> {
        let mut stats = std::collections::HashMap::new();
        for (start, end, cohort) in self.ranges() {
            *stats.entry(cohort).or_insert(0) += (end - start) as u64;
        }
        stats
    }

    pub fn cohort_stats(&self) -> std::collections::HashMap<CohortKey, u64> {
        self.cohort_stats.clone()
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.total_lines == 0 {
            if !self.change_points.is_empty() {
                return Err("Change-points must be empty when total_lines == 0".to_string());
            }
            return Ok(());
        }
        // First key must be 0
        match self.change_points.keys().next() {
            Some(&key) if key == 0 => {}
            _ => return Err("The first change-point must be at line 0".to_string()),
        }
        // Keys must be strictly increasing and strictly less than total_lines
        let mut prev_key: Option<LineNumber> = None;
        for &key in self.change_points.keys() {
            if key >= self.total_lines {
                return Err("Change-point key must be < total_lines".to_string());
            }
            if let Some(prev) = prev_key {
                if key <= prev {
                    return Err("Change-point keys must be strictly increasing".to_string());
                }
            }
            prev_key = Some(key);
        }

        if let Some(bad_cps) = self.change_points.range(self.total_lines..).next() {
            return Err(format!(
                "Found these change points beyond total_lines ({}): {:?}",
                self.total_lines, bad_cps
            ));
        }

        Ok(())
    }

    // The main method of this struct: it applies a vector of line diffs to the file blame.
    // The diffs are given as a vector of tuples, where each tuple contains:
    // - A range of lines to delete
    // - A range of lines to insert
    // - The cohort to apply to the lines in the range after the diff
    // The diffs are applied in order, from top to bottom, so we only go through
    // them once. As we go through them, we keep track of where we are in the file
    // so we also go through the whole file once. We keep a running offset
    // to track the delta we need to update the line numbers by.
    //
    // We have property tests against a reference implementation to validate correctness.
    pub fn apply_line_diffs(&self, line_diffs: LineDiffs<CohortKey>) -> Self {
        if line_diffs.is_empty() {
            return self.clone();
        }
        // Order the diffs by the start of the delete range, in case they aren't.
        // Shouldn't be needed, but typically sorting a presorted list is not
        // very expensive.
        let mut diffs = line_diffs.clone();
        diffs.sort_by_key(|(before, _, _)| before.start);

        // This algorithm works by building a new version of the change points,
        // copying them over. Instead of mutating the blame in place, we apply
        // modifications as we copy. For example, to delete lines, we simply don't
        // copy the change points that lie within the delete range into the new blame.
        // As we copy change points into the new blame, we also apply the offset to the line numbers.
        let mut new_change_points: BTreeMap<LineNumber, CohortKey> = BTreeMap::new();
        let mut cp_iter = self.change_points.iter().peekable();
        let old_total = self.total_lines;
        let mut offset: LineDelta = 0;

        // Helper to append a change point to the new blame only if it's different than the current last one
        let push_cp =
            |pos: LineNumber, cohort: CohortKey, map: &mut BTreeMap<LineNumber, CohortKey>| {
                if let Some((_, &last_cohort)) = map.last_key_value() {
                    if last_cohort == cohort {
                        return;
                    }
                }
                map.insert(pos, cohort);
            };

        // This is the hot loop. IME performance improvements are more likely to be found
        // by optimizing code in this loop than outside of it.
        for (delete, insert, cohort) in diffs.into_iter() {
            let delete_start = delete.start;
            let delete_end = delete.end;
            let delete_len = delete_end - delete_start;
            let insert_len = insert.len() as LineNumber;
            let delta = insert_len as LineDelta - delete_len as LineDelta;

            // Go through change points that are before the delete, and simply apply the
            // offset to them.
            while let Some((&line, &line_cohort)) = cp_iter.peek().copied() {
                if line < delete_start {
                    push_cp(
                        (line as LineDelta + offset) as LineNumber,
                        line_cohort,
                        &mut new_change_points,
                    );
                    cp_iter.next();
                } else {
                    break;
                }
            }

            // Insert the new lines' cohort at delete_start
            if insert_len > 0 {
                push_cp(
                    (delete_start as LineDelta + offset) as LineNumber,
                    cohort,
                    &mut new_change_points,
                );
            }

            // Skip change points that lie within [delete_start, delete_end)
            // This deletes them from the new blame.
            while let Some((&line, _)) = cp_iter.peek().copied() {
                if line < delete_end {
                    cp_iter.next();
                } else {
                    break;
                }
            }

            // Copy over the part of the current block that lies after the delete
            if delete_end < old_total {
                if let Some(resume_cohort) = self.cohort_at_index(delete_end) {
                    push_cp(
                        (delete_start as LineDelta + insert_len as LineDelta + offset)
                            as LineNumber,
                        resume_cohort,
                        &mut new_change_points,
                    );
                }
            }

            offset += delta;
        }

        // Copy over the remaining change points after the last delete
        while let Some((&line, &line_cohort)) = cp_iter.next() {
            push_cp(
                (line as LineDelta + offset) as LineNumber,
                line_cohort,
                &mut new_change_points,
            );
        }

        let new_total = (old_total as LineDelta + offset) as LineNumber;
        let mut new_blame = Self {
            change_points: new_change_points,
            total_lines: new_total,
            cohort_stats: std::collections::HashMap::new(),
        };
        //"Compact" the change points by removing adjacent ones with the same cohort
        // This used to be needed all the time, now cp_helper mostly handles it, but
        // I think there are some edge cases where it's not enough. I don't think it
        // fully breaks anything to not have it, but it's nice to have the invariant
        // of no adjacent change ranges.
        new_blame.merge_adjacent_ranges();
        // Drop any change-points that landed at or beyond new total
        // (can happen if a resume position coincides with the final end after deletions)
        // I'm not sure exactly the cases causing this and would like to remove it one day.
        if new_blame.total_lines > 0 {
            new_blame
                .change_points
                .retain(|&k, _| k < new_blame.total_lines);
        } else {
            new_blame.change_points.clear();
        }
        new_blame.cohort_stats = new_blame.compute_cohort_stats();
        debug_assert!(
            new_blame.validate().is_ok(),
            "invalid blame after applying line diffs: {:?}.\n old total: {:?}.\n offset: {:?}",
            new_blame.total_lines(),
            old_total,
            offset.abs(),
        );
        new_blame
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    // A few hand-written tests. There are not very principled, I trust the PBT
    // implemented against the reference implementation to be more thorough.
    #[test]
    fn test_cohort_stats() {
        let blame = FileBlame::new(0, 2022);
        let blame = blame.apply_line_diffs(vec![(0..0, 0..10, 2022), (0..0, 5..10, 2023)]);

        let stats = blame.cohort_stats();
        assert_eq!(stats.get(&2022), Some(&10));
        assert_eq!(stats.get(&2023), Some(&5));
    }

    #[test]
    fn test_apply_line_diffs_equal_length_hunks_near_end() {
        let blame = FileBlame::new(160, 2000);

        // Multiple replacements near the end; total_lines must stay unchanged
        let diffs = vec![
            (46..49, 46..49, 2006),
            (65..66, 65..66, 2006),
            (92..95, 92..95, 2006),
            (99..100, 99..100, 2006),
            (104..105, 104..105, 2006),
            (106..107, 106..107, 2006),
            (109..110, 109..110, 2006),
            (111..113, 111..113, 2006),
            (138..145, 138..145, 2006),
            (146..149, 146..149, 2006),
            (153..154, 153..154, 2006),
        ];

        let old_total = blame.total_lines();
        let blame = blame.apply_line_diffs(diffs);
        assert_eq!(blame.total_lines(), old_total);
        blame.validate().unwrap();
    }

    #[test]
    fn test_apply_line_diffs_insertion_then_deletion_tail() {
        let blame = FileBlame::new(200, 1999);
        // Insert 5 lines at position 50
        let blame = blame.apply_line_diffs(vec![(50..50, 50..55, 2001)]);
        assert_eq!(blame.total_lines(), 205);
        blame.validate().unwrap();
        // Now delete last 10 lines (from position 195..205 -> 195..195)
        let blame = blame.apply_line_diffs(vec![(195..205, 195..195, 2002)]);
        assert_eq!(blame.total_lines(), 195);
        blame.validate().unwrap();
    }

    #[test]
    fn test_apply_line_diffs_insertion_and_followup_replacements() {
        let blame = FileBlame::new(150, 2015);
        // Insert 4 lines at 20
        let blame = blame.apply_line_diffs(vec![(20..20, 20..24, 2016)]);
        assert_eq!(blame.total_lines(), 154);
        // Multiple replacements later in the file
        let old_total = blame.total_lines();
        let blame =
            blame.apply_line_diffs(vec![(100..102, 100..102, 2017), (150..151, 150..151, 2017)]);
        assert_eq!(blame.total_lines(), old_total);
        blame.validate().unwrap();
    }

    #[test]
    fn test_apply_line_diffs_resume_would_land_at_final_end() {
        let blame = FileBlame::new(100, 1);
        // Hunk 1: simple replacement 90..95 -> 90..95 (delta 0)
        // Hunk 2: delete tail 95..100 -> 95..95 (delta -5)
        let diffs = vec![(90..95, 90..95, 2), (95..100, 95..95, 3)];
        let blame = blame.apply_line_diffs(diffs);
        assert_eq!(blame.total_lines(), 95);
        blame.validate().unwrap();
    }

    // Reference implementation that stores each line as an entry in a Vec.
    // So 3 lines of 2022 -> [2022, 2022, 2022]. Deleting 2 lines at pos X means
    // literally going to pos X and removing 2 items. No fancy bookkeeping.
    #[derive(Clone, Debug)]
    struct NaiveBlame<CohortKey: Keyable> {
        lines: Vec<CohortKey>,
    }

    impl<CohortKey: Keyable> NaiveBlame<CohortKey> {
        fn new(total_lines: LineNumber, cohort: CohortKey) -> Self {
            Self {
                lines: vec![cohort; total_lines as usize],
            }
        }

        fn total_lines(&self) -> LineNumber {
            self.lines.len() as LineNumber
        }

        fn range_count(&self) -> usize {
            if self.lines.is_empty() {
                return 0;
            }
            let mut prev_cohort = self.lines[0];
            let mut count = 1;
            for &cohort in self.lines.iter().skip(1) {
                if cohort != prev_cohort {
                    count += 1;
                    prev_cohort = cohort;
                }
            }
            count
        }

        fn apply_line_diffs(
            &mut self,
            mut line_diffs: Vec<(
                std::ops::Range<LineNumber>,
                std::ops::Range<LineNumber>,
                CohortKey,
            )>,
        ) {
            // Apply from bottom to top so indices of earlier hunks are unaffected
            line_diffs.sort_by_key(|(before, _, _)| before.start);
            line_diffs.reverse();
            for (before, after, cohort) in line_diffs.into_iter() {
                //TODO we should just throw here if given a range that is out of bounds
                let start = before.start.min(self.lines.len() as LineNumber) as usize;
                let before_len = (before.end - before.start) as usize;

                // delete
                if before_len > 0 {
                    let available = self.lines.len() - start;
                    let to_delete = before_len.min(available);
                    if to_delete > 0 {
                        self.lines.drain(start..start + to_delete);
                    }
                }
                // insert
                let after_len = (after.end - after.start) as usize;
                if after_len > 0 {
                    self.lines.splice(start..start, vec![cohort; after_len]);
                }
            }
        }

        fn expand(&self) -> Vec<CohortKey> {
            self.lines.clone()
        }
        fn cohort_stats(&self) -> std::collections::HashMap<CohortKey, u64> {
            let mut stats = std::collections::HashMap::new();
            for &cohort in self.lines.iter() {
                *stats.entry(cohort).or_insert(0) += 1;
            }
            stats
        }
    }

    fn expand_file_blame<C: Keyable + Eq>(fb: &FileBlame<C>) -> Vec<C> {
        let mut out: Vec<C> = Vec::with_capacity(fb.total_lines() as usize);
        for (start, end, cohort) in fb.ranges() {
            let count = end - start;
            for _ in 0..count {
                out.push(cohort);
            }
        }
        out
    }

    const FILE_START_LEN: LineNumber = 500;
    const NUM_COHORTS: u32 = 20;
    const TOTAL_GENERATED_HUNKS: usize = 1000;
    const BATCH_AVG_LEN: u32 = 5;

    proptest! {
        #[test]
        fn pbt_apply_line_diffs_matches_naive(
            initial_len in 0u32..FILE_START_LEN,
            initial_cohort in 0u32..NUM_COHORTS,
            ops in proptest::collection::vec((any::<u32>(), any::<u32>(), any::<u32>(), any::<u32>()), 0..TOTAL_GENERATED_HUNKS)
        ) {
            let mut fb: FileBlame<u32> = FileBlame::new(initial_len, initial_cohort);
            let mut naive: NaiveBlame<u32> = NaiveBlame::new(initial_len, initial_cohort);

            let mut pending: Vec<(std::ops::Range<u32>, std::ops::Range<u32>, u32)> = Vec::new();
            let mut current_len: u32;
            let mut batch_old_len: u32 = initial_len;
            let mut batch_last_end: u32 = 0;

            for (pos_seed, del_seed, ins_seed, cohort_seed) in ops.into_iter() {
                // Choose non-overlapping start positions within the snapshot (batch_old_len)
                let remaining_space = batch_old_len - batch_last_end;
                let gap = if remaining_space == 0 { 0 } else { pos_seed % (remaining_space + 1) };
                let position = batch_last_end + gap;
                // Deletion up to 7 lines, bounded by the snapshot length from position
                let max_del = batch_old_len - position;
                let before_len = if max_del == 0 { 0 } else { del_seed % (max_del + 1) };
                // Insertion up to 7 lines
                let after_len = ins_seed % 8;
                let cohort = (cohort_seed % 11) + 1000; // avoid 0 to reduce accidental equality with prev cohort

                let before = position..(position + before_len);
                let after = position..(position + after_len);
                pending.push((before, after, cohort));
                batch_last_end = position + before_len; // enforce non-overlap and ascending order

                if pos_seed % BATCH_AVG_LEN == 0 {
                    fb = fb.apply_line_diffs(pending.clone());
                    naive.apply_line_diffs(pending.clone());
                    pending.clear();

                    current_len = naive.total_lines();
                    prop_assert_eq!(fb.total_lines(), current_len);

                    let fb_lines = expand_file_blame(&fb);
                    let naive_lines = naive.expand();
                    prop_assert_eq!(fb_lines, naive_lines);

                    prop_assert_eq!(fb.cohort_stats(), naive.cohort_stats());

                    batch_old_len = current_len;
                    batch_last_end = 0;
                }
            }

            if !pending.is_empty() {
                fb = fb.apply_line_diffs(pending.clone());
                naive.apply_line_diffs(pending.clone());
            }

            prop_assert_eq!(fb.total_lines(), naive.total_lines());
            prop_assert_eq!(fb.range_count(), naive.range_count());
            let fb_lines = expand_file_blame(&fb);
            let naive_lines = naive.expand();
            prop_assert_eq!(fb_lines, naive_lines);
            prop_assert!(fb.validate().is_ok());
            prop_assert_eq!(fb.cohort_stats(), naive.cohort_stats());
            prop_assert_eq!(fb.range_count(), naive.range_count());
        }
    }
}
