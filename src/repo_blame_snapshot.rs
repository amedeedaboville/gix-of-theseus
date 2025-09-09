use crate::blame::{FileBlame, Keyable, LineDiffs, LineNumber};
use anyhow::Result;
use dashmap::DashMap;
use gix::bstr::BString;

/// Represents blame information for the entire repository at a specific commit
/// Uses Dashmap so we can update entries concurrently for a slight boost
/// Later I'd like to switch to a crossbeam consumer that updates a single threaded map,
/// I think that would be faster than the coarse locking in the Dashmap.
/// A CommitKey is a usize that is essentially a pointer into an array of commit info
#[derive(Debug, Clone)]
pub struct RepositoryBlameSnapshot<CommitKey>
where
    CommitKey: Keyable,
{
    pub commit_id: gix::ObjectId,
    pub file_blames: DashMap<BString, FileBlame<CommitKey>>,
    pub running_cohort_stats: DashMap<CommitKey, i64>,
}

impl<CommitKey> RepositoryBlameSnapshot<CommitKey>
where
    CommitKey: Keyable,
{
    pub fn new(commit_id: gix::ObjectId) -> Self {
        Self {
            commit_id,
            file_blames: DashMap::new(),
            running_cohort_stats: DashMap::new(),
        }
    }
    pub fn set_commit_id(&mut self, commit_id: gix::ObjectId) {
        self.commit_id = commit_id;
    }

    pub fn add_file(&self, path: &BString, total_lines: LineNumber, cohort: CommitKey) {
        let file_blame = FileBlame::new(total_lines, cohort);
        self.file_blames.insert(path.clone(), file_blame);
        self.running_cohort_stats
            .entry(cohort)
            .and_modify(|v| *v += total_lines as i64)
            .or_insert(total_lines as i64);
    }

    pub fn delete_file(&self, path: &BString) {
        if let Some((_, file_blame)) = self.file_blames.remove(path) {
            for (cohort, line_count) in file_blame.cohort_stats() {
                self.running_cohort_stats
                    .entry(cohort)
                    .and_modify(|v| *v -= line_count as i64);
            }
        } else {
            panic!("File not found for delete: {:?}", path);
        }
    }

    pub fn rename_file(&self, old_path: BString, new_path: BString) -> Result<(), String> {
        let (_old_path, file_blame) = self
            .file_blames
            .remove(&old_path)
            .ok_or_else(|| format!("File not found for rename: {:?}", old_path))?;
        self.file_blames.insert(new_path.clone(), file_blame);
        Ok(())
    }

    pub fn modify_file(&self, path: &BString, line_diffs: LineDiffs<CommitKey>) {
        if let Some(mut file_blame) = self.file_blames.get_mut(path) {
            let old_blame = file_blame.clone();
            let new_blame = old_blame.apply_line_diffs(line_diffs.clone());
            let mut cohort_diff: std::collections::HashMap<CommitKey, i64> =
                std::collections::HashMap::new();
            for (cohort, line_count) in old_blame.cohort_stats() {
                *cohort_diff.entry(cohort).or_insert(0) -= line_count as i64;
            }
            for (cohort, line_count) in new_blame.cohort_stats() {
                *cohort_diff.entry(cohort).or_insert(0) += line_count as i64;
            }

            for (cohort, delta) in cohort_diff {
                self.running_cohort_stats
                    .entry(cohort)
                    .and_modify(|v| *v += delta)
                    .or_insert(delta);
            }
            *file_blame = new_blame;
        } else {
            panic!("File not found for modify: {:?}", path);
        }
    }
    pub fn repository_cohort_stats(&self) -> Vec<(CommitKey, i64)>
    where
        CommitKey: Keyable,
    {
        self.running_cohort_stats
            .iter()
            .map(|ref_multi| (*ref_multi.key(), *ref_multi.value()))
            .collect()
    }
}
