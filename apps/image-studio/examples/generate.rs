//! Runs the same persistent queue as the GUI against local ComfyUI.
use anyhow::{Context, Result, bail};
use image_studio::{Batch, Config, Engine, JobStatus, Model};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let directory = PathBuf::from(
        args.next()
            .context("Usage: generate DIRECTORY klein|z-image|both COUNT PROMPT")?,
    );
    let models = match args.next().as_deref() {
        Some("klein") => vec![Model::Klein4B],
        Some("z-image") => vec![Model::ZImageTurbo],
        Some("both") => vec![Model::Klein4B, Model::ZImageTurbo],
        _ => bail!("Choose klein, z-image, or both"),
    };
    let images_per_model = args.next().context("Missing count")?.parse()?;
    let prompt = args.collect::<Vec<_>>().join(" ");
    let engine = Engine::open(Config {
        state_dir: directory.join("state"),
        output_dir: directory.join("images"),
        ..Config::default()
    })?;
    engine.enqueue(Batch {
        prompt,
        models,
        images_per_model,
        width: 1024,
        height: 1024,
    })?;
    let started = Instant::now();
    let mut last = String::new();
    loop {
        let state = engine.snapshot();
        let current = serde_json::to_string_pretty(&state)?;
        if current != last {
            println!("{current}");
            last = current;
        }
        if state.jobs.iter().all(|j| {
            matches!(
                j.status,
                JobStatus::Completed | JobStatus::Failed | JobStatus::Cancelled
            )
        }) {
            println!("Elapsed: {:.1}s", started.elapsed().as_secs_f64());
            if state.jobs.iter().any(|j| j.status != JobStatus::Completed) {
                bail!("Some jobs did not complete; see their errors above");
            }
            break;
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    Ok(())
}
