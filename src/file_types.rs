use gix::bstr::BStr;
use gix::path::from_bstr;
use globset::{Glob, GlobSet, GlobSetBuilder};
use std::sync::OnceLock;

static ALLOWLIST: OnceLock<GlobSet> = OnceLock::new();

fn get_allowlist() -> &'static GlobSet {
    ALLOWLIST.get_or_init(|| {
        let mut builder = GlobSetBuilder::new();
        include_str!("allowed_filetypes.txt")
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .for_each(|p| {
                builder.add(Glob::new(p).expect("invalid glob pattern"));
            });
        builder.build().expect("failed to build globset")
    })
}

pub fn is_allowed_filetype(path: &BStr) -> bool {
    let path = from_bstr(path);
    let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    get_allowlist().is_match(filename)
}
