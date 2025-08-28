use anyhow::Result;
use std::io::Write;
use std::process::Command;
use std::{env, fs};

const STACKPLOT_SCRIPT: &str = include_str!("stackplot.py");

pub fn run_stackplot(input_file: String, output_file: String) -> Result<()> {
    let mut path = env::temp_dir();
    path.push("stackplot.py");

    let mut file = fs::File::create(&path)?;
    file.write_all(STACKPLOT_SCRIPT.as_bytes())?;

    let status = Command::new("uv")
        .arg("run")
        .arg(&path)
        .arg("--outfile")
        .arg(output_file)
        .arg(input_file)
        .status()?;

    if !status.success() {
        anyhow::bail!("Failed to execute 'uv run {}'", path.display());
    }

    fs::remove_file(path)?;
    Ok(())
}
