use anyhow::{Context, Result, bail};
use std::{
    ffi::CString,
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    os::unix::{ffi::OsStrExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Image,
    Audio,
    Video,
    Document,
    Sheet,
    Slides,
    Text,
    Pdf,
}

pub fn kind(path: &Path) -> Option<Kind> {
    Some(match extension(path).as_str() {
        "png" | "jpg" | "jpeg" | "webp" | "gif" | "tif" | "tiff" | "bmp" | "ico" | "heic"
        | "heif" | "avif" | "svg" | "jxl" => Kind::Image,
        "mp3" | "wav" | "flac" | "m4a" | "aac" | "ogg" | "opus" | "aiff" | "wma" => Kind::Audio,
        "mp4" | "mkv" | "mov" | "webm" | "avi" | "m4v" | "mpeg" | "mpg" | "wmv" | "ts" => {
            Kind::Video
        }
        "doc" | "docx" | "odt" | "rtf" => Kind::Document,
        "xls" | "xlsx" | "ods" | "csv" => Kind::Sheet,
        "ppt" | "pptx" | "odp" => Kind::Slides,
        "md" | "markdown" | "html" | "htm" | "txt" | "epub" => Kind::Text,
        "pdf" => Kind::Pdf,
        _ => return None,
    })
}

fn extension(path: &Path) -> String {
    path.extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase()
}

pub fn targets(kind: Kind) -> &'static [&'static str] {
    match kind {
        Kind::Image => &[
            "png", "jpg", "webp", "avif", "tiff", "bmp", "gif", "ico", "pdf",
        ],
        Kind::Audio => &["mp3", "m4a", "flac", "wav", "ogg", "opus"],
        Kind::Video => &[
            "mp4", "mkv", "webm", "mov", "mp3", "m4a", "flac", "wav", "ogg", "opus",
        ],
        Kind::Document => &["pdf", "docx", "odt", "rtf", "txt"],
        Kind::Sheet => &["xlsx", "ods", "pdf", "csv"],
        Kind::Slides => &["pdf", "pptx", "odp"],
        Kind::Text => &["html", "docx", "odt", "md", "txt", "epub"],
        Kind::Pdf => &["txt", "png", "jpg"],
    }
}

pub fn engine(kind: Kind, target: &str) -> &'static str {
    match kind {
        Kind::Image => "magick",
        Kind::Audio | Kind::Video => "ffmpeg",
        Kind::Document | Kind::Sheet | Kind::Slides => "libreoffice",
        Kind::Text => "pandoc",
        Kind::Pdf if target == "txt" => "pdftotext",
        Kind::Pdf => "pdftoppm",
    }
}

pub fn note(kind: Kind, target: &str) -> &'static str {
    match (kind, target) {
        (Kind::Image, "jpg") => "First frame/page only. Transparency becomes white. JPEG is lossy.",
        (Kind::Image, _) => {
            "First frame/page only. SVG is rasterized. Codec support depends on ImageMagick."
        }
        (Kind::Pdf, "txt") => {
            "Extracts existing text. Scanned pages need OCR, which is not included."
        }
        (Kind::Pdf, _) => "Renders the first page at 150 dpi.",
        (Kind::Sheet, "csv") => {
            "Exports the first sheet only; formatting and formulas are not preserved."
        }
        (Kind::Text, "md") => "Text and formatting only. Embedded images are not exported.",
        (Kind::Document | Kind::Sheet | Kind::Slides | Kind::Text, _) => {
            "Layout and formatting can change. Check the converted document."
        }
        (_, "mp3" | "m4a" | "ogg" | "opus") => {
            "Lossy audio conversion. Video inputs export their first audio track."
        }
        (Kind::Video, "wav" | "flac") => {
            "Exports the first audio track. Cannot restore quality lost in the source."
        }
        (Kind::Video, _) => {
            "Re-encodes the first video and audio tracks. Subtitles and extra tracks are omitted."
        }
        _ => "Lossless audio encoding. Cannot restore quality lost in the source.",
    }
}

pub fn available(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|dir| {
            let path = dir.join(program);
            use std::os::unix::fs::PermissionsExt;
            path.metadata()
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
    })
}

#[derive(Clone, Debug)]
pub struct Request {
    pub source: PathBuf,
    pub target: String,
    /// None saves beside the source.
    pub destination: Option<PathBuf>,
}

#[derive(Debug)]
pub enum Event {
    Started(usize),
    Finished(usize, std::result::Result<PathBuf, String>),
    Done,
}

#[derive(Default)]
pub struct Cancellation {
    cancelled: AtomicBool,
    pid: Mutex<Option<u32>>,
}
impl Cancellation {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        if let Some(pid) = *self.pid.lock().unwrap() {
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
    }
    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}
pub struct Job {
    cancel: Arc<Cancellation>,
}
impl Job {
    pub fn cancel(&self) {
        self.cancel.cancel();
    }
}
impl Drop for Job {
    fn drop(&mut self) {
        self.cancel();
    }
}

pub fn start(requests: Vec<(usize, Request)>) -> (Job, async_channel::Receiver<Event>) {
    let cancel = Arc::new(Cancellation::default());
    let worker_cancel = cancel.clone();
    let (send, receive) = async_channel::unbounded();
    thread::spawn(move || {
        for (index, request) in requests {
            if worker_cancel.is_cancelled() {
                break;
            }
            if send.send_blocking(Event::Started(index)).is_err() {
                break;
            }
            let result = convert(&request, &worker_cancel).map_err(|e| format!("{e:#}"));
            if send.send_blocking(Event::Finished(index, result)).is_err() {
                break;
            }
        }
        let _ = send.send_blocking(Event::Done);
    });
    (Job { cancel }, receive)
}

/// Convert in an isolated directory on the destination filesystem, then publish
/// atomically. Originals and existing results are never overwritten.
pub fn convert(request: &Request, cancel: &Cancellation) -> Result<PathBuf> {
    check_cancel(cancel)?;
    let source = request
        .source
        .canonicalize()
        .context("Source file is unavailable")?;
    if !source.is_file() {
        bail!("Choose a regular file");
    }
    let kind = kind(&source).context("Unsupported file extension")?;
    if !targets(kind).contains(&request.target.as_str()) {
        bail!("Unsupported conversion");
    }
    let destination = request
        .destination
        .as_deref()
        .unwrap_or(source.parent().unwrap());
    let destination = destination
        .canonicalize()
        .context("Destination folder is unavailable")?;
    let stage = tempfile::Builder::new()
        .prefix(".file-converter-")
        .tempdir_in(&destination)?;
    // A safe basename avoids ImageMagick's filename syntax and command options.
    let input = stage.path().join(format!("input.{}", extension(&source)));
    fs::copy(&source, &input).context("Could not stage source file")?;
    check_cancel(cancel)?;
    let output_dir = stage.path().join("output");
    fs::create_dir(&output_dir)?;
    let output = output_dir.join(format!("input.{}", request.target));
    let mut command = command_for(kind, &request.target, &input, &output, stage.path())?;
    command.current_dir(source.parent().unwrap());
    run(&mut command, stage.path(), cancel)?;
    check_cancel(cancel)?;
    let metadata = fs::symlink_metadata(&output)
        .context("The engine produced no output. Check format and codec support")?;
    if !metadata.is_file() || (metadata.len() == 0 && request.target != "txt") {
        bail!("The engine produced an empty or invalid output");
    }
    publish(
        &output,
        &destination,
        source.file_stem().unwrap(),
        &request.target,
    )
}

fn check_cancel(cancel: &Cancellation) -> Result<()> {
    if cancel.is_cancelled() {
        bail!("Cancelled");
    }
    Ok(())
}

fn command_for(
    kind: Kind,
    target: &str,
    input: &Path,
    output: &Path,
    stage: &Path,
) -> Result<Command> {
    let mut cmd = Command::new(engine(kind, target));
    match kind {
        Kind::Image => {
            cmd.arg(format!("{}[0]", input.display()))
                .arg("-auto-orient");
            if target == "jpg" {
                cmd.args(["-background", "white", "-alpha", "remove", "-alpha", "off"]);
            }
            if target == "ico" {
                cmd.args(["-resize", "256x256>"]);
            }
            cmd.args(["-quality", "90"]).arg(output);
        }
        Kind::Audio | Kind::Video => {
            cmd.args(["-hide_banner", "-loglevel", "error", "-nostdin", "-n", "-i"])
                .arg(input);
            if matches!(target, "mp4" | "mkv" | "mov" | "webm") {
                cmd.args(["-map", "0:v:0", "-map", "0:a:0?", "-sn"]);
                if target == "webm" {
                    cmd.args([
                        "-c:v",
                        "libvpx-vp9",
                        "-crf",
                        "32",
                        "-b:v",
                        "0",
                        "-c:a",
                        "libopus",
                    ]);
                } else {
                    cmd.args([
                        "-c:v", "libx264", "-crf", "20", "-preset", "medium", "-c:a", "aac",
                        "-b:a", "192k",
                    ]);
                    if target != "mkv" {
                        cmd.args(["-movflags", "+faststart"]);
                    }
                }
                cmd.args([
                    "-vf",
                    "pad=ceil(iw/2)*2:ceil(ih/2)*2",
                    "-pix_fmt",
                    "yuv420p",
                ]);
            } else {
                cmd.args(["-map", "0:a:0", "-vn"]);
                cmd.args(match target {
                    "mp3" => vec!["-c:a", "libmp3lame", "-q:a", "2"],
                    "m4a" => vec!["-c:a", "aac", "-b:a", "192k"],
                    "flac" => vec!["-c:a", "flac"],
                    "wav" => vec!["-c:a", "pcm_s16le"],
                    "ogg" => vec!["-c:a", "libvorbis", "-q:a", "5"],
                    "opus" => vec!["-c:a", "libopus", "-b:a", "160k"],
                    _ => unreachable!(),
                });
            }
            cmd.arg(output);
        }
        Kind::Document | Kind::Sheet | Kind::Slides => {
            let profile = url::Url::from_directory_path(stage.join("office-profile"))
                .map_err(|_| anyhow::anyhow!("Invalid profile path"))?;
            cmd.arg(format!("-env:UserInstallation={profile}"))
                .args(["--headless", "--norestore", "--convert-to"])
                .arg(if target == "csv" {
                    "csv:Text - txt - csv (StarCalc):44,34,76,1"
                } else {
                    target
                })
                .arg("--outdir")
                .arg(output.parent().unwrap())
                .arg(input);
        }
        Kind::Text => {
            let from = match extension(input).as_str() {
                "md" | "markdown" | "txt" => "markdown",
                "html" | "htm" => "html",
                "epub" => "epub",
                _ => unreachable!(),
            };
            let to = match target {
                "md" => "markdown",
                "txt" => "plain",
                other => other,
            };
            if target == "html" {
                cmd.arg("--embed-resources");
            }
            cmd.args(["--standalone", "--from", from, "--to", to])
                .arg("--output")
                .arg(output)
                .arg(input);
        }
        Kind::Pdf if target == "txt" => {
            cmd.arg("-layout").arg(input).arg(output);
        }
        Kind::Pdf => {
            cmd.args(["-f", "1", "-singlefile", "-r", "150"])
                .arg(if target == "jpg" { "-jpeg" } else { "-png" })
                .arg(input)
                .arg(output.with_extension(""));
        }
    }
    Ok(cmd)
}

fn run(command: &mut Command, stage: &Path, cancel: &Cancellation) -> Result<()> {
    // File-backed diagnostics avoid pipe deadlocks and unbounded memory use.
    let log_path = stage.join("engine.log");
    let log = File::create(&log_path)?;
    command
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .process_group(0);
    check_cancel(cancel)?;
    let mut child = {
        let mut pid = cancel.pid.lock().unwrap();
        check_cancel(cancel)?;
        let child = command.spawn().with_context(|| {
            format!(
                "Could not start {:?}. Install it and check PATH",
                command.get_program()
            )
        })?;
        *pid = Some(child.id());
        child
    };
    let status = loop {
        let mut pid = cancel.pid.lock().unwrap();
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let result = unsafe {
            libc::waitid(
                libc::P_PID,
                child.id(),
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        if result == -1 || cancel.is_cancelled() || unsafe { info.si_pid() } != 0 {
            // Kill descendants before reaping the leader, so its PID cannot be reused.
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            let status = child.wait();
            *pid = None;
            let status = status?;
            if result == -1 {
                bail!("Could not monitor conversion process");
            }
            break status;
        }
        drop(pid);
        thread::sleep(Duration::from_millis(40));
    };
    check_cancel(cancel)?;
    if !status.success() {
        let mut file = File::open(log_path)?;
        let len = file.metadata()?.len();
        file.seek(SeekFrom::Start(len.saturating_sub(6000)))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        bail!(
            "Engine exited with {status}: {}",
            String::from_utf8_lossy(&bytes).trim()
        );
    }
    Ok(())
}

fn publish(
    source: &Path,
    directory: &Path,
    stem: &std::ffi::OsStr,
    extension: &str,
) -> Result<PathBuf> {
    for number in 0..10_000 {
        let mut name = stem.to_os_string();
        if number > 0 {
            name.push(format!(" ({number})"));
        }
        name.push(format!(".{extension}"));
        let destination = directory.join(name);
        let from = CString::new(source.as_os_str().as_bytes())?;
        let to = CString::new(destination.as_os_str().as_bytes())?;
        let result = unsafe {
            libc::renameat2(
                libc::AT_FDCWD,
                from.as_ptr(),
                libc::AT_FDCWD,
                to.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        if result == 0 {
            return Ok(destination);
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::AlreadyExists {
            return Err(error).context("Could not safely save converted file");
        }
    }
    bail!("Too many files share this name")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn routes_are_bounded() {
        assert_eq!(kind(Path::new("PHOTO.HEIC")), Some(Kind::Image));
        assert_eq!(kind(Path::new("archive.zip")), None);
        assert!(!targets(Kind::Pdf).contains(&"docx"));
        assert!(targets(Kind::Video).contains(&"mp3"));
        assert!(!targets(Kind::Audio).contains(&"mp4"));
    }
    #[test]
    fn publish_preserves_existing_files_and_symlinks() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let source = dir.path().join("staged");
        fs::write(&source, "new")?;
        fs::write(dir.path().join("photo.png"), "original")?;
        std::os::unix::fs::symlink("missing", dir.path().join("photo (1).png"))?;
        let saved = publish(&source, dir.path(), std::ffi::OsStr::new("photo"), "png")?;
        assert_eq!(saved.file_name().unwrap(), "photo (2).png");
        assert_eq!(
            fs::read_to_string(dir.path().join("photo.png"))?,
            "original"
        );
        assert_eq!(fs::read_to_string(saved)?, "new");
        Ok(())
    }
    #[test]
    fn batch_continues_after_failure() {
        let requests = (0..2)
            .map(|index| {
                (
                    index,
                    Request {
                        source: PathBuf::from(format!("/missing-converter-test-{index}.png")),
                        target: "jpg".into(),
                        destination: None,
                    },
                )
            })
            .collect();
        let (_job, events) = start(requests);
        for index in 0..2 {
            assert!(matches!(events.recv_blocking().unwrap(), Event::Started(i) if i == index));
            assert!(
                matches!(events.recv_blocking().unwrap(), Event::Finished(i, Err(_)) if i == index)
            );
        }
        assert!(matches!(events.recv_blocking().unwrap(), Event::Done));
    }

    #[test]
    fn failed_engine_reports_diagnostics() -> Result<()> {
        let stage = tempfile::tempdir()?;
        let mut command = Command::new("sh");
        command.args(["-c", "echo 'bad input' >&2; exit 2"]);
        let error = run(&mut command, stage.path(), &Cancellation::default()).unwrap_err();
        assert!(error.to_string().contains("bad input"));
        Ok(())
    }

    #[test]
    fn cancellation_kills_descendants() -> Result<()> {
        let stage = tempfile::tempdir()?;
        let marker = stage.path().join("escaped");
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg("(sleep 1; touch \"$1\") & wait")
            .arg("test")
            .arg(&marker);
        let cancel = Cancellation::default();
        thread::scope(|scope| {
            scope.spawn(|| {
                thread::sleep(Duration::from_millis(100));
                cancel.cancel();
            });
            assert!(
                run(&mut command, stage.path(), &cancel)
                    .unwrap_err()
                    .to_string()
                    .contains("Cancelled")
            );
        });
        thread::sleep(Duration::from_millis(1100));
        assert!(!marker.exists());
        Ok(())
    }
}
