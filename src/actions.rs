use crate::blame::{Keyable, LineDiffs, LineNumber};
use gix::bstr::BString;

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
    SetCommitId(gix::ObjectId),
}
