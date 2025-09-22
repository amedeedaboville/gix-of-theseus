use crate::actions::Action;
use crate::blame::{FileBlame, Keyable, LineDiffs, LineNumber};
use anyhow::Result;
use crossbeam_channel::{Sender, unbounded};
use gix::bstr::BString;
use std::collections::HashMap;
use std::thread::{JoinHandle, spawn};

/// Represents blame information for the entire repository at a specific commit
/// A CommitKey is a usize that is essentially a pointer into an array of commit info
#[derive(Debug, Clone)]
pub struct RepositoryBlameSnapshot<CommitKey>
where
    CommitKey: Keyable,
{
    pub commit_id: gix::ObjectId,
    pub file_blames: HashMap<BString, FileBlame<CommitKey>>,
    pub running_cohort_stats: HashMap<CommitKey, i64>,
    pub commit_results: Vec<Vec<(CommitKey, i64)>>,
}

impl<CommitKey> RepositoryBlameSnapshot<CommitKey>
where
    CommitKey: Keyable,
{
    pub fn new(commit_id: gix::ObjectId) -> Self {
        Self {
            commit_id,
            file_blames: HashMap::new(),
            running_cohort_stats: HashMap::new(),
            commit_results: Vec::new(),
        }
    }
    pub fn set_commit_id(&mut self, commit_id: gix::ObjectId) {
        self.commit_id = commit_id;
    }

    pub fn add_file(&mut self, path: &BString, total_lines: LineNumber, cohort: CommitKey) {
        let file_blame = FileBlame::new(total_lines, cohort);
        self.file_blames.insert(path.clone(), file_blame);
        self.running_cohort_stats
            .entry(cohort)
            .and_modify(|v| *v += total_lines as i64)
            .or_insert(total_lines as i64);
    }

    pub fn delete_file(&mut self, path: &BString) {
        if let Some(file_blame) = self.file_blames.remove(path) {
            for (cohort, line_count) in file_blame.cohort_stats() {
                self.running_cohort_stats
                    .entry(cohort)
                    .and_modify(|v| *v -= line_count as i64);
            }
        } else {
            panic!("File not found for delete: {:?}", path);
        }
    }

    pub fn rename_file(&mut self, old_path: BString, new_path: BString) -> Result<(), String> {
        let file_blame = self
            .file_blames
            .remove(&old_path)
            .ok_or_else(|| format!("File not found for rename: {:?}", old_path))?;
        self.file_blames.insert(new_path.clone(), file_blame);
        Ok(())
    }

    pub fn modify_file(&mut self, path: &BString, line_diffs: LineDiffs<CommitKey>) {
        if let Some(file_blame) = self.file_blames.get_mut(path) {
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

    pub fn handle_action(&mut self, action: Action<CommitKey>) {
        match action {
            Action::AddFile {
                path,
                total_lines,
                cohort,
            } => self.add_file(&path, total_lines, cohort),
            Action::DeleteFile { path } => self.delete_file(&path),
            Action::RenameFile { old_path, new_path } => {
                self.rename_file(old_path, new_path).unwrap();
            }
            Action::ModifyFile { path, line_diffs } => self.modify_file(&path, line_diffs),
            Action::FinishCommit => {
                self.commit_results.push(self.repository_cohort_stats());
            }
            Action::SetCommitId(id) => {
                self.set_commit_id(id);
            }
        }
    }
    pub fn repository_cohort_stats(&self) -> Vec<(CommitKey, i64)>
    where
        CommitKey: Keyable,
    {
        self.running_cohort_stats
            .iter()
            .map(|(k, v)| (*k, *v))
            .collect()
    }
}

pub struct BlameProcessor<CommitKey>
where
    CommitKey: Keyable,
{
    sender: Sender<Action<CommitKey>>,
    join_handle: Option<JoinHandle<RepositoryBlameSnapshot<CommitKey>>>,
}

impl<CommitKey> BlameProcessor<CommitKey>
where
    CommitKey: Keyable + Send + 'static,
{
    pub fn new(initial_commit_id: gix::ObjectId) -> Self {
        let (sender, receiver) = unbounded();
        let mut snapshot = RepositoryBlameSnapshot::new(initial_commit_id);

        let join_handle = spawn(move || {
            for action in receiver {
                snapshot.handle_action(action);
            }
            snapshot
        });

        Self {
            sender,
            join_handle: Some(join_handle),
        }
    }

    pub fn sender(&self) -> Sender<Action<CommitKey>> {
        self.sender.clone()
    }

    pub fn finish(mut self) -> Vec<Vec<(CommitKey, i64)>> {
        drop(self.sender);
        self.join_handle
            .take()
            .unwrap()
            .join()
            .unwrap()
            .commit_results
    }
}
