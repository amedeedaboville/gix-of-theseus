use crate::blame::{FileBlame, Keyable, LineDiffs, LineNumber};
use crate::collectors::list_in_range::Granularity;
use crate::collectors::list_in_range::list_commits_with_granularity;
use anyhow::Result;
use dashmap::DashMap;
use gix::bstr::{BString, ByteSlice};
use gix::date::time::CustomFormat;
use gix::diff::blob::diff as blob_diff;
use gix::diff::object::TreeRefIter;
use gix::diff::tree_with_rewrites;
use gix::diff::tree_with_rewrites::{Action, Change, ChangeRef};
use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::HashMap;
use thread_local::ThreadLocal;

/// Represents blame information for the entire repository at a specific commit
/// Uses Dashmap so we can update entries concurrently for a slight boost
#[derive(Debug, Clone)]
pub struct RepositoryBlameSnapshot<CommitKey>
where
    CommitKey: Keyable,
{
    pub commit_id: gix::ObjectId,
    pub file_blames: DashMap<BString, FileBlame<CommitKey>>,
    pub running_cohort_stats: DashMap<CommitKey, u64>,
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

    fn add_file(&self, path: &BString, total_lines: LineNumber, cohort: CommitKey) {
        let file_blame = FileBlame::new(total_lines, cohort);
        self.file_blames.insert(path.clone(), file_blame);
        self.running_cohort_stats
            .entry(cohort)
            .and_modify(|v| *v += total_lines as u64)
            .or_insert(total_lines as u64);
    }

    fn delete_file(&self, path: &BString) {
        if let Some((_, file_blame)) = self.file_blames.remove(path) {
            for (cohort, line_count) in file_blame.cohort_stats() {
                self.running_cohort_stats
                    .entry(cohort)
                    .and_modify(|v| *v -= line_count);
            }
        }
    }

    fn rename_file(&self, old_path: BString, new_path: BString) -> Result<(), String> {
        let (_old_path, file_blame) = self
            .file_blames
            .remove(&old_path)
            .ok_or_else(|| format!("File not found for rename: {:?}", old_path))?;
        self.file_blames.insert(new_path.clone(), file_blame);
        Ok(())
    }

    pub fn modify_file(&self, path: &BString, line_diffs: LineDiffs<CommitKey>) {
        self.file_blames
            .view(path, |_key, old_blame| {
                let new_blame = old_blame.apply_line_diffs(line_diffs);
                let mut cohort_diff: HashMap<CommitKey, i64> = HashMap::new();
                for (cohort, line_count) in old_blame.cohort_stats() {
                    *cohort_diff.entry(cohort).or_insert(0) -= line_count as i64;
                }
                for (cohort, line_count) in new_blame.cohort_stats() {
                    *cohort_diff.entry(cohort).or_insert(0) += line_count as i64;
                }

                for (cohort, delta) in cohort_diff {
                    self.running_cohort_stats
                        .entry(cohort)
                        .and_modify(|v| *v = (*v as i64 + delta) as u64)
                        .or_insert(delta as u64);
                }
                new_blame
            })
            .unwrap();
    }
    pub fn repository_cohort_stats(&self) -> Vec<(String, u64)>
    where
        CommitKey: Keyable + Send + Sync,
    {
        self.running_cohort_stats
            .iter()
            .map(|ref_multi| (ref_multi.key().to_string(), *ref_multi.value()))
            .collect()
    }
}
// The data in cohorts.json
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CohortData {
    pub y: Vec<Vec<u64>>,
    pub ts: Vec<String>,
    pub labels: Vec<String>,
}

pub fn run_theseus(repo_path: &str) -> Result<CohortData, Box<dyn std::error::Error>> {
    let repo = gix::open(repo_path)?;
    let safe_repo = repo.clone().into_sync();
    let weekly_commits = list_commits_with_granularity(&repo, Granularity::Weekly, None, None)?;
    let current_snapshot = RepositoryBlameSnapshot::<usize>::new(weekly_commits[0].id);

    //Each thread gets its own repo handle and its own diff cache
    let tl = ThreadLocal::new();
    let get_thread_local_vars = || {
        tl.get_or(|| {
            let repo = safe_repo.clone().to_thread_local();
            let platform = RefCell::new(repo.diff_resource_cache_for_tree_diff().unwrap());
            (repo, platform)
        })
    };
    let progress_bar = ProgressBar::new(weekly_commits.len() as u64);
    progress_bar.set_style(
        ProgressStyle::default_bar()
            .template("[{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta_precise}) {per_sec:0.1} {msg}")
            .unwrap()
            .progress_chars("=>-"),
    );
    let commit_trees_and_cohorts: Vec<(gix::ObjectId, gix::date::Time, String, Vec<u8>)> = weekly_commits
    .into_iter()
    .map(|commit| {
        let time = commit.time().unwrap();
        (
            commit.id().to_owned().into(),
            time,
            time.format(CustomFormat::new("%Y-%m-%d %H:%M:%S")),
            commit.tree().unwrap().detach().data,
        )
    })
    .collect();
    // We do this detaching serially, so that we can look up the commit data concurrently afterwards
    let all_cohort_labels: Vec<String> = commit_trees_and_cohorts
    .iter()
    .map(|(_, _, ts, _)| ts.clone())
    .collect();
    let commit_changes_and_cohorts: Vec<(Vec<Change>, usize)> = (0..commit_trees_and_cohorts.len())
        .into_par_iter()
        .map(|i| {
            let (repo, platform_cell) = get_thread_local_vars();
            let mut platform = platform_cell.borrow_mut();
    
            let mut tree_diff_state = gix::diff::tree::State::default();
            let mut objects = &repo.objects;

            let (_id, _cohort, _ts, current_tree_data) = &commit_trees_and_cohorts[i];
            let previous_tree_data = if i > 0 {
                commit_trees_and_cohorts[i - 1].3.as_slice()
            } else {
                &[]
            };

            let mut work_todo = Vec::new();
            tree_with_rewrites(
                TreeRefIter::from_bytes(previous_tree_data),
                TreeRefIter::from_bytes(current_tree_data.as_slice()),
                &mut platform,
                &mut tree_diff_state,
                &mut objects,
                |change: ChangeRef<'_>| -> Result<Action, Box<dyn std::error::Error + Send + Sync>> {
                    if change.entry_mode().is_blob() {
                        work_todo.push(change.into_owned());
                    }
                    Ok(Action::Continue)
                },
                gix::diff::tree_with_rewrites::Options {
                    location: Some(gix::diff::tree::recorder::Location::Path),
                    rewrites: Some(gix::diff::Rewrites::default()),
                },
            )
            .expect("tree diff failed");
            (work_todo, i)
        })
        .collect();

    let mut results = Vec::new();
    for (work_todo, cohort) in progress_bar.wrap_iter(commit_changes_and_cohorts.into_iter()) {
        work_todo
            .into_par_iter()
            .map(
                |change| -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
                    let (thread_repo, _) = get_thread_local_vars();

                    match change {
                        Change::Addition { location, id, .. } => {
                            let blob = thread_repo.find_blob(id)?;
                            let content = &blob.data;
                            let num_lines = content.lines().count();
                            current_snapshot.add_file(
                                &location.clone(),
                                num_lines as LineNumber,
                                cohort,
                            );
                        }
                        Change::Deletion { location, .. } => {
                            current_snapshot.delete_file(&location);
                        }
                        Change::Modification {
                            location,
                            previous_entry_mode,
                            previous_id,
                            entry_mode,
                            id,
                        } => {
                            let (thread_repo, platform_cell) = get_thread_local_vars();
                            let mut platform_borrow = platform_cell.borrow_mut();

                            if previous_entry_mode != entry_mode {
                                let prev_is_blob = previous_entry_mode.is_blob();
                                let new_is_blob = entry_mode.is_blob();
                                if !prev_is_blob && new_is_blob {
                                    // Treat as adding file
                                    let new_blob = thread_repo.find_blob(id)?;
                                    let new_lines = new_blob.data.lines().count() as LineNumber;
                                    current_snapshot.add_file(&location, new_lines, cohort);
                                    return Ok(());
                                } else if prev_is_blob && !new_is_blob {
                                    current_snapshot.delete_file(&location);
                                    return Ok(());
                                } else {
                                    // Non-blob → non-blob: ignore
                                    if !prev_is_blob && !new_is_blob {
                                        return Ok(());
                                    }
                                }
                            }

                            platform_borrow.set_resource(
                                previous_id,
                                gix::object::tree::EntryKind::Blob,
                                location.as_ref(),
                                gix::diff::blob::ResourceKind::OldOrSource,
                                &thread_repo.objects,
                            )?;
                            platform_borrow.set_resource(
                                id,
                                gix::object::tree::EntryKind::Blob,
                                location.as_ref(),
                                gix::diff::blob::ResourceKind::NewOrDestination,
                                &thread_repo.objects,
                            )?;

                            let outcome = platform_borrow.prepare_diff()?;
                            let input = outcome.interned_input();
                            let mut line_diffs = Vec::new();
                            blob_diff(
                                gix::diff::blob::Algorithm::Myers,
                                &input,
                                |before: std::ops::Range<u32>, after: std::ops::Range<u32>| {
                                    line_diffs.push((before, after, cohort));
                                },
                            );
                            current_snapshot.modify_file(&location, line_diffs);
                        }
                        Change::Rewrite {
                            source_location,
                            location,
                            ..
                        } => {
                            let _ = current_snapshot
                                .rename_file(source_location.clone(), location.clone());
                        }
                    };
                    return Ok(());
                },
            )
            .for_each(|v| v.unwrap());

        // We need to clear the diff cache every so often.
        // Clearing it every 2, 10, 100 or 200 commits has nearly the same performance improvement,
        // a bit less than 10s, but consumes 60+ GB of RAM compared to capping out at 200MB
        // when clearing every commit. Clearing it less often than every commit is not worth it.
        rayon::broadcast(|_| {
            let (_, platform_cell) = get_thread_local_vars();
            platform_cell
                .borrow_mut()
                .clear_resource_cache_keep_allocation();
        });
        results.push(current_snapshot.repository_cohort_stats());
    }

    let ts: Vec<String> = all_cohort_labels.clone();
    let mut all_labels_set = std::collections::BTreeSet::new();
    for snapshot in &results {
        for (label, _) in snapshot {
            all_labels_set.insert(label.clone());
        }
    }
    let labels: Vec<String> = all_labels_set.into_iter().collect();
    let label_to_index: std::collections::HashMap<_, _> = labels
        .iter()
        .enumerate()
        .map(|(i, label)| (label.clone(), i))
        .collect();

    let mut y = vec![vec![0; results.len()]; labels.len()];

    for (t_idx, snapshot) in results.iter().enumerate() {
        for (label, count) in snapshot {
            if let Some(&label_idx) = label_to_index.get(label) {
                y[label_idx][t_idx] = *count;
            }
        }
    }

    let cohort_data = CohortData { y, ts, labels };
    Ok(cohort_data)
}
