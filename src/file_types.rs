const allowed_filetypes: [&str] = !include_str!("allowed_filetypes.txt").split("\n").collect();

pub fn is_allowed_filetype(filetype: &str) -> bool {
    allowed_filetypes.contains(&filetype)
}
