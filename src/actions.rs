use crate::blame::{Keyable, LineDiffs, LineNumber};
use gix_diff::object::bstr::BString;
use gix_hash::ObjectId;

#[derive(Debug)]
pub enum Action<CommitKey>
where
    CommitKey: Keyable,
{
    AddFile {
        path: BString,
        total_lines: LineNumber,
        cohort: CommitKey,
    },
    DeleteFile {
        path: BString,
    },
    RenameFile {
        old_path: BString,
        new_path: BString,
    },
    ModifyFile {
        path: BString,
        line_diffs: LineDiffs<CommitKey>,
    },
    FinishCommit,
    SetCommitId(ObjectId),
}
