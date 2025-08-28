use std::process::Command;

pub fn stack_plot(
    input_fn: &str,
    outfile: &str,
    max_n: usize,
    normalize: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = Command::new("src/stackplot.py");
    cmd.arg(input_fn)
        .arg("--outfile")
        .arg(outfile)
        .arg("--max-n")
        .arg(max_n.to_string());

    if normalize {
        cmd.arg("--normalize");
    }

    let status = cmd.status()?;

    if status.success() {
        println!("Writing output to {}", outfile);
        Ok(())
    } else {
        let output = cmd.output()?;
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!(
            "Python script failed with status: {}. Stderr:\n{}",
            status, stderr
        )
        .into())
    }
}
