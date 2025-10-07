use chrono::{DateTime, Datelike, Utc};
use gix::{Commit, Repository};
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
) -> Result<Vec<Commit>, Box<dyn Error>> {
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
