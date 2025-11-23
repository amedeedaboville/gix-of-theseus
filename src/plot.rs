use anyhow::Result;
use std::io::Write;
use std::process::Command;
use std::sync::OnceLock;
use std::{env, fs};

const STACKPLOT_SCRIPT: &str = include_str!("stackplot.py");

static PYTHON_SCRIPT_RUNNER: OnceLock<Option<String>> = OnceLock::new();

pub fn get_python_runner() -> Option<String> {
    PYTHON_SCRIPT_RUNNER
        .get_or_init(|| {
            let runners = ["uv", "pipx"];

            for runner in runners {
                if Command::new(runner)
                    .arg("--version")
                    .output()
                    .map(|output| output.status.success())
                    .unwrap_or(false)
                {
                    return Some(runner.to_string());
                }
            }

            None
        })
        .clone()
}
pub fn run_stackplot(input_file: String, output_file: String, title: Option<String>) -> Result<()> {
    let runner = get_python_runner().ok_or_else(|| anyhow::anyhow!("No Python runner found"))?;

    let mut path = env::temp_dir();
    path.push("stackplot.py");

    let mut file = fs::File::create(&path)?;
    file.write_all(STACKPLOT_SCRIPT.as_bytes())?;

    let status = if ["uv", "pipx"].contains(&runner.as_str()) {
        Command::new(&runner)
            .arg("run")
            .arg(&path)
            .arg("--outfile")
            .arg(output_file)
            .arg("--title")
            .arg(title.unwrap_or_default())
            .arg(input_file)
            .status()?
    } else {
        anyhow::bail!("Unsupported runner: {}", runner);
    };

    if !status.success() {
        anyhow::bail!("Failed to execute '{} run {}'", runner, path.display());
    }

    fs::remove_file(path)?;
    Ok(())
}
