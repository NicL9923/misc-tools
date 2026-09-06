//! Manual live check through the same engine interface used by the GUI.
use youtube_downloader::download::{Event, Job, Operation, Output};

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let url = args
        .next()
        .ok_or_else(|| anyhow::anyhow!("Usage: probe URL [DIRECTORY] [mkv|mp4|audio|mp3]"))?;
    let operation = if let Some(destination) = args.next() {
        let output = match args.next().as_deref().unwrap_or("mkv") {
            "mkv" => Output::Video,
            "mp4" => Output::Mp4,
            "audio" => Output::Audio,
            "mp3" => Output::Mp3,
            other => anyhow::bail!("Unknown output: {other}"),
        };
        Operation::Download {
            url,
            destination: destination.into(),
            output,
            height: Some(720),
        }
    } else {
        Operation::Inspect { url }
    };
    let job = Job::start(operation);
    while let Ok(event) = job.events.recv_blocking() {
        match event {
            Event::Inspected(media) => {
                println!("{}\nHeights: {:?}", media.title, media.heights());
                return Ok(());
            }
            Event::Completed(path) => {
                println!("Saved: {}", path.display());
                return Ok(());
            }
            Event::Failed(message) => anyhow::bail!("{message}"),
            Event::Cancelled => anyhow::bail!("Cancelled"),
            other => println!("{other:?}"),
        }
    }
    anyhow::bail!("Worker closed without reporting a result")
}
