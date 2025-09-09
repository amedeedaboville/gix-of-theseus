use crate::blame::LineNumber;
use crate::repo_blame_snapshot::RepositoryBlameSnapshot;
use crate::gix_helpers::{get_blob_diff, list_commits_with_granularity, Granularity};
use anyhow::Result;
use gix::bstr::ByteSlice;
use gix::date::time::CustomFormat;
use gix::diff::object::TreeRefIter;
use gix::diff::tree_with_rewrites;
use gix::diff::tree_with_rewrites::{Action, Change, ChangeRef};
use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;
use std::cell::RefCell;
use thread_local::ThreadLocal;

pub struct CommitCohortInfo {
    pub id: gix::ObjectId,
    pub time_string: String,
    pub year: u32,
}
pub struct TheseusResult {
    //One entry per commit, with metadata about it and which cohort it belongs to
    pub commit_cohort_info: Vec<CommitCohortInfo>,
    // One entry per commit, with the child vec being key,value pairs of commit idx + number of lines
    pub cohort_data: Vec<Vec<(usize, i64)>>,
}

pub fn run_theseus(repo_path: &str) -> Result<TheseusResult, Box<dyn std::error::Error>> {
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
    let commit_trees_and_years: Vec<(gix::ObjectId, gix::date::Time, String, Vec<u8>, u32)> =
        weekly_commits
            .into_iter()
            .map(|commit| {
                let time = commit.time().unwrap();
                (
                    commit.id().to_owned().into(),
                    time,
                    time.format(CustomFormat::new("%Y-%m-%d %H:%M:%S")),
                    commit.tree().unwrap().detach().data,
                    time.format(CustomFormat::new("%Y")).parse().unwrap(),
                )
            })
            .collect();
    let commit_changes_and_cohorts: Vec<(Vec<Change>, usize)> = (0..commit_trees_and_years.len())
        .into_par_iter()
        .map(|i| {
            let (repo, platform_cell) = get_thread_local_vars();
            let mut platform = platform_cell.borrow_mut();
    
            let mut tree_diff_state = gix::diff::tree::State::default();
            let mut objects = &repo.objects;

            let (_id, _time, _ts, current_tree_data, _year) = &commit_trees_and_years[i];
            let previous_tree_data = if i > 0 {
                commit_trees_and_years[i - 1].3.as_slice()
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
    for (work_todo, commit_idx) in progress_bar.wrap_iter(commit_changes_and_cohorts.into_iter()) {
        work_todo
            .into_par_iter()
            .map(
                |change| -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
                    let (thread_repo, _) = get_thread_local_vars();

                    match change {
                        Change::Addition { location, id, .. } => {
                            let blob = thread_repo.find_blob(id)?;
                            current_snapshot.add_file(
                                &location,
                                blob.data.lines().count() as LineNumber,
                                commit_idx,
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
                                    current_snapshot.add_file(&location, new_lines, commit_idx);
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

                        let line_diffs = get_blob_diff(
                            &mut platform_borrow,
                            previous_id,
                            id,
                            location.as_ref(),
                            &thread_repo.objects,
                            commit_idx,
                        )?;
                        current_snapshot.modify_file(&location, line_diffs);
                    }
                        Change::Rewrite {
                            source_location,
                            location,
                            diff,
                            id,
                            source_id,
                            ..
                        } => {
                            let _ = current_snapshot
                                .rename_file(source_location.clone(), location.clone());

                            if diff.is_some() {
                                let (thread_repo, platform_cell) = get_thread_local_vars();
                                let mut platform_borrow = platform_cell.borrow_mut();
                                let line_diffs = get_blob_diff(
                                    &mut platform_borrow,
                                    source_id,
                                    id,
                                    location.as_ref(),
                                    &thread_repo.objects,
                                    commit_idx,
                                )?;
                                current_snapshot.modify_file(&location, line_diffs);
                            }
                        }
                    };
                    return Ok(());
                },
            )
            .for_each(|v| v.unwrap());

        // We need to clear the diff cache every so often.
        // Clearing it every 2, 10, 100 or 200 commits has nearly the same performance improvement:
        // a speedup of ~10s on torvalds/linux, but it consumes 60+ GB of RAM compared to capping out at 200MB
        // when clearing every commit. Clearing it less often than every commit is not worth it.
        rayon::broadcast(|_| {
            let (_, platform_cell) = get_thread_local_vars();
            platform_cell
                .borrow_mut()
                .clear_resource_cache_keep_allocation();
        });
        results.push(current_snapshot.repository_cohort_stats());
    }

    let commit_infos = commit_trees_and_years
        .iter()
        .map(|(id, _time, ts, _, year)| CommitCohortInfo {
            id: id.clone(),
            time_string: ts.clone(),
            year: *year,
        })
        .collect();
    Ok(TheseusResult {
        commit_cohort_info: commit_infos,
        cohort_data: results,
    })
}
