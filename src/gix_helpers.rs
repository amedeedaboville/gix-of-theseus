use anyhow::Result;
use chrono::{DateTime, Datelike, Utc};
use gix_diff::blob::Platform;
use gix_diff::blob::ResourceKind;
use gix_diff::blob::diff as blob_diff;
use gix_diff::object::bstr::BStr;
use gix_hash::ObjectId;
use gix_odb::pack::FindExt;
use gix_ref::file::Store;
use gix_traverse::commit::{Parents, Simple, simple::Sorting};

use std::path::Path;
use std::{collections::BTreeMap, error::Error};

fn get_head_id_for_store(store: &Store) -> Result<ObjectId, Box<dyn Error>> {
    let head_ref = store.find("HEAD")?;
    let target = head_ref.target;
    match target {
        gix_ref::Target::Object(id) => Ok(id),
        gix_ref::Target::Symbolic(symbolic_ref) => {
            let target_ref = store.find(&symbolic_ref)?;
            Ok(target_ref.target.id().into())
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Granularity {
    Weekly,
    Monthly,
    Yearly,
}

pub fn list_commits_with_granularity(
    repo_path: &str,
    granularity: Granularity,
    start: Option<DateTime<Utc>>,
    end: Option<DateTime<Utc>>,
) -> Result<Vec<(ObjectId, gix_date::Time, Vec<u8>)>, Box<dyn Error>> {
    let git_dir = if repo_path.ends_with(".git") {
        repo_path.to_string()
    } else {
        format!("{}/.git", repo_path)
    };
    let git_dir_path = Path::new(&git_dir);
    let objects_dir = git_dir_path.join("objects");
    let store = Store::at(
        git_dir_path.to_path_buf(),
        gix_ref::store::init::Options::default(),
    );
    let odb = gix_odb::at(&objects_dir).unwrap().to_owned().into_inner();

    let head_id = get_head_id_for_store(&store)?;

    // Create a Simple commit walker with first parent only and reverse chronological order
    let walker = Simple::new([head_id], &odb)
        .parents(Parents::First) // first parent only
        .sorting(Sorting::BreadthFirst)?; // topological order

    let mut commits_by_period = BTreeMap::new();

    let mut buffer = Vec::new();

    for info_result in walker {
        let info = info_result?;
        let commit_id = info.id;
        let commit = odb.find_commit(commit_id.as_ref(), &mut buffer)?.0;
        let commit_time = commit.time();
        let mut tree_buffer = Vec::new();
        let commit_tree_data = odb
            .find(commit.tree().as_ref(), &mut tree_buffer)?
            .0
            .data
            .to_vec();
        let datetime = DateTime::from_timestamp(commit_time.seconds, 0).unwrap();

        // If the commit is before the start time, end the loop early
        if let Some(start) = start {
            if datetime < start {
                break;
            }
        }

        // If the commit is after the end time, skip this commit
        if let Some(end) = end {
            if datetime > end {
                continue;
            }
        }

        let key = match granularity {
            Granularity::Weekly => {
                let num_days = datetime.weekday().num_days_from_sunday();
                let start_of_week = datetime - chrono::Duration::days(num_days.into());
                start_of_week.format("%Y-%m-%d").to_string()
            }
            Granularity::Monthly => datetime.format("%Y-%m").to_string(),
            Granularity::Yearly => datetime.format("%Y").to_string(),
        };

        commits_by_period
            .entry(key)
            .or_insert_with(|| (commit_id, commit_time, commit_tree_data));
    }

    let mut commits = commits_by_period
        .into_iter()
        .map(|(_key, o)| o)
        .collect::<Vec<(ObjectId, gix_date::Time, Vec<u8>)>>();
    commits.sort_by_key(|c| c.1.clone());
    Ok(commits)
}

// Sets up the gix machinery to do a blob diff.
// Returns the line diffs as a vec of (delete_range, insert_range, commit_key)
pub fn get_blob_diff(
    platform_borrow: &mut Platform,
    previous_id: ObjectId,
    id: ObjectId,
    location: &BStr,
    objects: &gix_odb::HandleArc,
    commit_key: usize,
) -> Result<Vec<(std::ops::Range<u32>, std::ops::Range<u32>, usize)>> {
    platform_borrow.set_resource(
        previous_id,
        gix_object::tree::EntryKind::Blob,
        location,
        ResourceKind::OldOrSource,
        objects,
    )?;
    platform_borrow.set_resource(
        id,
        gix_object::tree::EntryKind::Blob,
        location,
        ResourceKind::NewOrDestination,
        objects,
    )?;

    let outcome = platform_borrow.prepare_diff()?;
    let input = outcome.interned_input();
    let mut line_diffs = Vec::new();
    blob_diff(
        gix_diff::blob::Algorithm::Myers,
        &input,
        |before: std::ops::Range<u32>, after: std::ops::Range<u32>| {
            line_diffs.push((before, after, commit_key));
        },
    );
    Ok(line_diffs)
}
