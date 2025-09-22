use crate::actions::Action;
use crate::blame::LineNumber;
use crate::gix_helpers::{Granularity, get_blob_diff, list_commits_with_granularity};
use crate::repo_blame_snapshot::BlameProcessor;
use anyhow::Result;
use gix::bstr::ByteSlice;
use gix::date::time::CustomFormat;
use gix::diff::object::TreeRefIter;
use gix::diff::tree_with_rewrites;
use gix::diff::tree_with_rewrites::{Action as DiffAction, Change, ChangeRef};
use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;
use std::cell::RefCell;
use thread_local::ThreadLocal;

// Information about a commit that we use to make the graphs.
// For now we only care about the year, though the time_string could
// be used to plot weeks.
pub struct CommitCohortInfo {
    pub id: gix::ObjectId,
    pub time_string: String,
    pub year: u32,
}
pub struct TheseusResult {
    //A table listing metadata for each commit
    //Mentions to "commit_idx" elsewhere refer to the index in this Vec
    pub commit_cohort_info: Vec<CommitCohortInfo>,
    // One entry per commit, with the child vec being key,value pairs of commit idx + number of lines
    pub cohort_data: Vec<Vec<(usize, i64)>>,
}

pub fn run_theseus(repo_path: &str) -> Result<TheseusResult, Box<dyn std::error::Error>> {
    let repo = gix::open(repo_path)?;
    let safe_repo = repo.clone().into_sync();
    let weekly_commits = list_commits_with_granularity(&repo, Granularity::Weekly, None, None)?;
    let processor = BlameProcessor::<usize>::new(weekly_commits[0].id);
    let sender = processor.sender();

    //Each thread gets its own repo handle and its own diff cache
    let tl = ThreadLocal::new();
    let get_thread_local_vars = || {
        tl.get_or(|| {
            let repo = safe_repo.clone().to_thread_local();
            let platform = RefCell::new(repo.diff_resource_cache_for_tree_diff().unwrap());
            (repo, platform)
        })
    };
    let progress_bar = ProgressBar::new(weekly_commits.len() as u64).with_style(
        ProgressStyle::default_bar()
            .template("[{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta_precise}) {per_sec:0.1} {msg}")
            .unwrap()
            .progress_chars("=>-"),
    );
    let commit_trees_and_years: Vec<(gix::ObjectId, String, Vec<u8>, u32)> = weekly_commits
        .into_iter()
        .map(|commit| {
            let time = commit.time().unwrap();
            (
                commit.id().to_owned().into(),
                time.format(CustomFormat::new("%Y-%m-%d %H:%M:%S")),
                commit.tree().unwrap().detach().data,
                time.format(CustomFormat::new("%Y")).parse().unwrap(),
            )
        })
        .collect();
    // First we compute the tree-diffs between each weekly commit and its preceding commit.
    // We can actually do this in parallel, which is nice.
    let commit_changes_and_cohorts: Vec<(Vec<Change>, usize)> = (0..commit_trees_and_years.len())
        .into_par_iter()
        .map(|i| {
            let (repo, platform_cell) = get_thread_local_vars();
            let mut platform = platform_cell.borrow_mut();

            let mut tree_diff_state = gix::diff::tree::State::default();
            let mut objects = &repo.objects;

            let (_id, _ts, current_tree_data, _year) = &commit_trees_and_years[i];
            let previous_tree_data = if i > 0 {
                commit_trees_and_years[i - 1].2.as_slice()
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
                |change: ChangeRef<'_>| -> Result<DiffAction, Box<dyn std::error::Error + Send + Sync>> {
                    if change.entry_mode().is_blob() || change.source_entry_mode_and_id().0.is_blob() {
                        work_todo.push(change.into_owned());
                    }
                    Ok(DiffAction::Continue)
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

    // Now work_todo is a vec of changes per commit that we need to accumulate to build our incremental blame.
    // We go through it serially, but we can process each commit's changes in parallel.
    for (work_todo, commit_idx) in progress_bar.wrap_iter(commit_changes_and_cohorts.into_iter()) {
        sender
            .send(Action::SetCommitId(
                commit_trees_and_years[commit_idx].0.clone(),
            ))
            .unwrap();

        // For any one commit, we process the changes that commit makes to the tree in parallel:
        work_todo
            .into_par_iter()
            .try_for_each(
                |change| -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
                    let (thread_repo, platform_cell) = get_thread_local_vars();
                    match change {
                        Change::Addition { location, id, .. } => {
                            handle_file_addition(&sender, thread_repo, id, &location, commit_idx)?;
                        }
                        Change::Deletion { location, .. } => {
                            handle_file_deletion(&sender, location)?;
                        }
                        Change::Modification {
                            location,
                            previous_entry_mode,
                            previous_id,
                            entry_mode,
                            id,
                        } => {
                            if handle_entry_mode_change(
                                &sender,
                                thread_repo,
                                previous_entry_mode,
                                entry_mode,
                                id,
                                &location,
                                commit_idx,
                            )? {
                                return Ok(());
                            }
                            handle_file_modification(
                                &sender,
                                thread_repo,
                                platform_cell,
                                previous_id,
                                id,
                                &location,
                                commit_idx,
                            )?;
                        }
                        Change::Rewrite {
                            source_location,
                            location,
                            diff,
                            id,
                            source_id,
                            entry_mode,
                            source_entry_mode,
                            ..
                        } => {
                            sender
                                .send(Action::RenameFile {
                                    old_path: source_location,
                                    new_path: location.clone(),
                                })
                                .unwrap();
                            if handle_entry_mode_change(
                                &sender,
                                thread_repo,
                                source_entry_mode,
                                entry_mode,
                                id,
                                &location,
                                commit_idx,
                            )? {
                                return Ok(());
                            }

                            if diff.is_some() {
                                handle_file_modification(
                                    &sender,
                                    thread_repo,
                                    platform_cell,
                                    source_id,
                                    id,
                                    &location,
                                    commit_idx,
                                )?;
                            }
                        }
                    };
                    Ok(())
                },
            )
            .unwrap();
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
        sender.send(Action::FinishCommit).unwrap();
    }
    drop(sender);
    let results = processor.finish();

    let commit_infos = commit_trees_and_years
        .iter()
        .map(|(id, ts, _, year)| CommitCohortInfo {
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

fn handle_file_modification(
    sender: &crossbeam_channel::Sender<Action<usize>>,
    thread_repo: &gix::Repository,
    platform_cell: &std::cell::RefCell<gix::diff::blob::Platform>,
    previous_id: gix::ObjectId,
    id: gix::ObjectId,
    location: &gix::bstr::BString,
    commit_idx: usize,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut platform_borrow = platform_cell.borrow_mut();
    let line_diffs = get_blob_diff(
        &mut platform_borrow,
        previous_id,
        id,
        location.as_ref(),
        &thread_repo.objects,
        commit_idx,
    )?;
    sender
        .send(Action::ModifyFile {
            path: location.clone(),
            line_diffs,
        })
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
    Ok(())
}

fn handle_file_addition(
    sender: &crossbeam_channel::Sender<Action<usize>>,
    thread_repo: &gix::Repository,
    id: gix::ObjectId,
    location: &gix::bstr::BString,
    commit_idx: usize,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let blob = thread_repo.find_blob(id)?;
    sender
        .send(Action::AddFile {
            path: location.clone(),
            total_lines: blob.data.lines().count() as LineNumber,
            cohort: commit_idx,
        })
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
    Ok(())
}

fn handle_file_deletion(
    sender: &crossbeam_channel::Sender<Action<usize>>,
    location: gix::bstr::BString,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    sender
        .send(Action::DeleteFile { path: location })
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
    Ok(())
}

// Returns true if the entry mode change was handled and no more processing is needed
fn handle_entry_mode_change(
    sender: &crossbeam_channel::Sender<Action<usize>>,
    thread_repo: &gix::Repository,
    previous_entry_mode: gix::object::tree::EntryMode,
    entry_mode: gix::object::tree::EntryMode,
    id: gix::ObjectId,
    location: &gix::bstr::BString,
    commit_idx: usize,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    if previous_entry_mode != entry_mode {
        let prev_is_blob = previous_entry_mode.is_blob();
        let new_is_blob = entry_mode.is_blob();
        if !prev_is_blob && new_is_blob {
            handle_file_addition(sender, thread_repo, id, location, commit_idx)?;
            return Ok(true);
        } else if prev_is_blob && !new_is_blob {
            handle_file_deletion(sender, location.clone())?;
            return Ok(true);
        } else if !prev_is_blob && !new_is_blob {
            return Ok(true);
        }
    }
    Ok(false)
}
