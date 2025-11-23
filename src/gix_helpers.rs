use anyhow::Result;
use chrono::{DateTime, Datelike, Utc};
use gix::diff::blob::diff as blob_diff;
use gix::{Commit, Repository, bstr::BStr};
use std::{collections::BTreeMap, error::Error};

#[derive(Debug, Clone, Copy)]
pub enum Granularity {
    Weekly,
    Monthly,
    Yearly,
}

pub fn list_commits_with_granularity(
    repo: &Repository,
    granularity: Granularity,
    start: Option<DateTime<Utc>>,
    end: Option<DateTime<Utc>>,
) -> Result<Vec<Commit<'_>>, Box<dyn Error>> {
    let revwalk = repo
        .rev_walk(repo.head_id())
        .first_parent_only()
        .use_commit_graph(true)
        .all()?;

    let mut commits_by_period = BTreeMap::new();

    for info_result in revwalk {
        let info = info_result?;
        let commit = info.object().unwrap();
        let commit_time = commit.time()?;
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

        commits_by_period.entry(key).or_insert_with(|| commit);
    }

    let mut commits = commits_by_period.into_values().collect::<Vec<_>>();
    commits.sort_by_key(|c| c.time().unwrap());
    Ok(commits)
}

// Sets up the gix machinery to do a blob diff.
// Returns the line diffs as a vec of (delete_range, insert_range, commit_key)
pub fn get_blob_diff(
    platform_borrow: &mut gix::diff::blob::Platform,
    previous_id: gix::ObjectId,
    id: gix::ObjectId,
    location: &BStr,
    objects: &gix::odb::Handle,
    commit_key: usize,
) -> Result<Vec<(std::ops::Range<u32>, std::ops::Range<u32>, usize)>> {
    platform_borrow.set_resource(
        previous_id,
        gix::object::tree::EntryKind::Blob,
        location,
        gix::diff::blob::ResourceKind::OldOrSource,
        objects,
    )?;
    platform_borrow.set_resource(
        id,
        gix::object::tree::EntryKind::Blob,
        location,
        gix::diff::blob::ResourceKind::NewOrDestination,
        objects,
    )?;

    let outcome = platform_borrow.prepare_diff()?;
    let input = outcome.interned_input();
    let mut line_diffs = Vec::new();
    blob_diff(
        gix::diff::blob::Algorithm::Myers,
        &input,
        |before: std::ops::Range<u32>, after: std::ops::Range<u32>| {
            line_diffs.push((before, after, commit_key));
        },
    );
    Ok(line_diffs)
}
