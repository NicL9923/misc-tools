use file_converter::{Cancellation, Request, convert};
use std::path::PathBuf;
fn main() -> anyhow::Result<()> {
    let mut args = std::env::args_os().skip(1);
    let source = PathBuf::from(args.next().expect("Usage: convert SOURCE TARGET [FOLDER]"));
    let target = args
        .next()
        .expect("Missing target extension")
        .to_string_lossy()
        .into_owned();
    let destination = args.next().map(PathBuf::from);
    let saved = convert(
        &Request {
            source,
            target,
            destination,
        },
        &Cancellation::default(),
    )?;
    println!("{}", saved.display());
    Ok(())
}
