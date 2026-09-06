use anyhow::{Context, Result, bail};
use async_channel::{Receiver, Sender};
use serde::{Deserialize, Serialize};
use std::os::{fd::AsRawFd, unix::ffi::OsStrExt};
use std::{
    collections::VecDeque,
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Read},
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};
use url::Url;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Media {
    pub title: String,
    #[serde(default)]
    pub channel: Option<String>,
    #[serde(default)]
    pub duration: Option<f64>,
    #[serde(default)]
    pub formats: Vec<Format>,
    #[serde(default)]
    pub is_live: bool,
    #[serde(default)]
    pub live_status: Option<String>,
    #[serde(default, rename = "_type")]
    pub kind: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Format {
    pub height: Option<u32>,
    pub vcodec: Option<String>,
}

impl Media {
    pub fn heights(&self) -> Vec<u32> {
        let mut heights: Vec<_> = self
            .formats
            .iter()
            .filter(|f| f.vcodec.as_deref() != Some("none"))
            .filter_map(|f| f.height)
            .collect();
        heights.sort_unstable_by(|a, b| b.cmp(a));
        heights.dedup();
        heights
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Output {
    Video,
    Mp4,
    Audio,
    Mp3,
}

impl Output {
    pub fn is_video(self) -> bool {
        matches!(self, Self::Video | Self::Mp4)
    }
}

#[derive(Clone, Debug)]
pub enum Operation {
    Inspect {
        url: String,
    },
    Download {
        url: String,
        destination: PathBuf,
        output: Output,
        height: Option<u32>,
    },
}

/// Only single YouTube video URLs are accepted. Canonicalization drops playlist
/// parameters and prevents yt-dlp from expanding a channel or playlist.
pub fn normalize_url(input: &str) -> Result<String> {
    let url = Url::parse(input.trim()).context("Paste a complete YouTube video URL.")?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
    {
        bail!("Use an https:// YouTube video URL.");
    }
    let host = url.host_str().unwrap_or_default();
    let id = if matches!(host, "youtu.be" | "www.youtu.be") {
        url.path().strip_prefix('/').unwrap_or_default().to_owned()
    } else if matches!(
        host,
        "youtube.com" | "www.youtube.com" | "m.youtube.com" | "music.youtube.com"
    ) {
        if url.path() == "/watch" {
            url.query_pairs()
                .find(|(key, _)| key == "v")
                .map(|(_, value)| value.into_owned())
                .unwrap_or_default()
        } else {
            let parts: Vec<_> = url.path().trim_matches('/').split('/').collect();
            if parts.len() == 2 && matches!(parts[0], "shorts" | "embed" | "live") {
                parts[1].to_owned()
            } else {
                String::new()
            }
        }
    } else {
        bail!("Only YouTube video links are supported.");
    };
    if id.len() != 11
        || !id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
    {
        bail!("Use a single video link, not a playlist or channel.");
    }
    Ok(format!("https://www.youtube.com/watch?v={id}"))
}

impl Operation {
    pub fn args(&self) -> Result<Vec<OsString>> {
        let mut args: Vec<OsString> = [
            "--ignore-config",
            "--no-playlist",
            "--no-color",
            "--socket-timeout",
            "20",
            "--retries",
            "3",
        ]
        .into_iter()
        .map(Into::into)
        .collect();
        let url = match self {
            Self::Inspect { url } => {
                args.push("--dump-single-json".into());
                url
            }
            Self::Download {
                url,
                destination: _,
                output,
                height,
            } => {
                let capped = height.map(|h| format!("[height<={h}]")).unwrap_or_default();
                let selector = match output {
                    Output::Video => format!("bv*{capped}+ba/b{capped}"),
                    Output::Mp4 => format!(
                        "bv*[vcodec^=avc1]{capped}+ba[acodec^=mp4a]/b[ext=mp4][vcodec^=avc1]{capped}"
                    ),
                    Output::Audio | Output::Mp3 => "ba/b".into(),
                };
                args.extend(
                    [
                        "--no-overwrites",
                        "--no-simulate",
                        "--newline",
                        "--progress",
                        "--progress-delta",
                        "0.2",
                        "--progress-template",
                        "download:PROGRESS:%(progress)j",
                        "--progress-template",
                        "postprocess:PROCESSING:%(progress)j",
                        "--print",
                        "after_move:FILE:%(filepath)j",
                        "--output",
                        "%(title).160B [%(id)s].%(ext)s",
                        "--paths",
                    ]
                    .into_iter()
                    .map(Into::into),
                );
                // yt-dlp expands variables and trims whitespace in --paths.
                // Set the working directory instead, preserving literal names.
                args.push(".".into());
                args.extend([OsString::from("--format"), selector.into()]);
                match output {
                    Output::Video => args.extend(
                        ["--merge-output-format", "mkv", "--remux-video", "mkv"].map(Into::into),
                    ),
                    Output::Mp4 => args.extend(
                        ["--merge-output-format", "mp4", "--remux-video", "mp4"].map(Into::into),
                    ),
                    Output::Audio => args.push("--extract-audio".into()),
                    Output::Mp3 => args.extend(
                        [
                            "--extract-audio",
                            "--audio-format",
                            "mp3",
                            "--audio-quality",
                            "0",
                        ]
                        .map(Into::into),
                    ),
                }
                url
            }
        };
        args.extend([OsString::from("--"), normalize_url(url)?.into()]);
        Ok(args)
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Progress {
    pub downloaded_bytes: Option<f64>,
    pub total_bytes: Option<f64>,
    pub total_bytes_estimate: Option<f64>,
    pub speed: Option<f64>,
    pub eta: Option<f64>,
    #[serde(default)]
    pub status: String,
}

impl Progress {
    pub fn fraction(&self) -> Option<f32> {
        let total = self.total_bytes.or(self.total_bytes_estimate)?;
        if total <= 0.0 {
            return None;
        }
        Some((self.downloaded_bytes? / total).clamp(0.0, 1.0) as f32)
    }
}

#[derive(Clone, Debug)]
pub enum Event {
    Progress(Progress),
    Processing,
    Inspected(Media),
    Completed(PathBuf),
    Failed(String),
    Cancelled,
}

#[derive(Default)]
struct Control {
    cancelled: AtomicBool,
    pid: Mutex<Option<u32>>,
}

impl Control {
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        let pid = self.pid.lock().unwrap();
        if let Some(pid) = *pid {
            // The child starts in a dedicated group. Kill descendants as well,
            // including FFmpeg. The mutex prevents racing a reaped/reused PID.
            #[cfg(unix)]
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
    }
}

pub struct Job {
    pub events: Receiver<Event>,
    control: Arc<Control>,
}

impl Job {
    pub fn start(operation: Operation) -> Self {
        Self::with_executable(operation, PathBuf::from("yt-dlp"))
    }

    pub fn with_executable(operation: Operation, executable: PathBuf) -> Self {
        let (send, events) = async_channel::unbounded();
        let control = Arc::new(Control::default());
        let worker_control = control.clone();
        thread::spawn(move || {
            let result = run(operation, executable, &worker_control, &send);
            let terminal = if matches!(&result, Ok(Event::Completed(_))) {
                // Once atomically published, completion wins a concurrent cancel.
                result.unwrap()
            } else if worker_control.cancelled.load(Ordering::SeqCst) {
                Event::Cancelled
            } else {
                result.unwrap_or_else(|error| Event::Failed(format!("{error:#}")))
            };
            let _ = send.send_blocking(terminal);
        });
        Self { events, control }
    }

    pub fn cancel(&self) {
        self.control.cancel();
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[derive(Default)]
struct Collected {
    media: Option<Media>,
    path: Option<PathBuf>,
    diagnostics: VecDeque<String>,
    parse_error: Option<String>,
}

/// Keep conversion output private until it is complete. A crash or cancellation
/// may leave an apparently finished MP3 behind; remove it before retrying.
struct Staging {
    directory: PathBuf,
    destination: PathBuf,
    _lock: File,
}

impl Staging {
    fn new(operation: &Operation) -> Result<Option<Self>> {
        let Operation::Download {
            url,
            destination,
            output,
            height,
        } = operation
        else {
            return Ok(None);
        };
        if !destination.is_dir() {
            bail!("Choose an existing destination folder.");
        }
        let destination = destination
            .canonicalize()
            .context("Could not access the destination folder.")?;
        let url = normalize_url(url)?;
        let id = url.rsplit('=').next().unwrap();
        let format = match output {
            Output::Video => "mkv",
            Output::Mp4 => "mp4",
            Output::Audio => "audio",
            Output::Mp3 => "mp3",
        };
        let quality = height
            .map(|h| h.to_string())
            .unwrap_or_else(|| "best".into());
        let directory = destination
            .join(".misc-tools-partials")
            .join(format!("{id}-{format}-{quality}"));
        fs::create_dir_all(&directory)
            .context("Could not create a working folder in the destination.")?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(directory.join(".lock"))?;
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == -1 {
            bail!(
                "This video and format are already downloading into that folder in another window."
            );
        }
        let staging = Self {
            directory: directory.canonicalize()?,
            destination,
            _lock: lock,
        };
        staging.clean_incomplete_outputs()?;
        Ok(Some(staging))
    }

    fn clean_incomplete_outputs(&self) -> Result<()> {
        for entry in fs::read_dir(&self.directory)? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let fragment = name.rsplit_once(".part-Frag").is_some_and(|(_, suffix)| {
                !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_digit())
            });
            if name == ".lock" || name.ends_with(".part") || name.ends_with(".ytdl") || fragment {
                continue;
            }
            let kind = entry.file_type()?;
            if kind.is_file() || kind.is_symlink() {
                fs::remove_file(entry.path())?;
            }
        }
        Ok(())
    }

    fn publish(&self, path: PathBuf) -> Result<PathBuf> {
        let path = if path.is_absolute() {
            path
        } else {
            self.directory.join(path)
        };
        let path = path
            .canonicalize()
            .context("The reported output file does not exist.")?;
        if !path.is_file() || path.parent() != Some(self.directory.as_path()) {
            bail!("yt-dlp reported an output outside its working folder.");
        }
        let destination = self.destination.join(path.file_name().unwrap());
        let source_c = std::ffi::CString::new(path.as_os_str().as_bytes())?;
        let destination_c = std::ffi::CString::new(destination.as_os_str().as_bytes())?;
        // Same-filesystem atomic publication, including on filesystems without
        // hard links. Never replace an existing file, even across app instances.
        let result = unsafe {
            libc::renameat2(
                libc::AT_FDCWD,
                source_c.as_ptr(),
                libc::AT_FDCWD,
                destination_c.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        if result == -1 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                bail!(
                    "A file named {} already exists. Choose another folder or rename the existing file.",
                    destination.file_name().unwrap().to_string_lossy()
                );
            }
            return Err(error)
                .context("Could not move the completed file safely into the destination.");
        }
        Ok(destination)
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        if let Err(error) = self.clean_incomplete_outputs() {
            eprintln!("Could not clean working files: {error}");
        }
        // Keep the lock inode. Deleting it would let two processes acquire
        // different locks for the same directory.
    }
}

fn collect(reader: impl Read, inspect: bool, send: &Sender<Event>, collected: &Mutex<Collected>) {
    for line in BufReader::new(reader).lines() {
        let line = match line {
            Ok(line) => line,
            Err(_) => break,
        };
        if let Some(json) = line.strip_prefix("PROGRESS:") {
            if let Ok(progress) = serde_json::from_str(json) {
                let _ = send.send_blocking(Event::Progress(progress));
            }
        } else if line.starts_with("PROCESSING:") {
            let _ = send.send_blocking(Event::Processing);
        } else if let Some(json) = line.strip_prefix("FILE:") {
            let mut data = collected.lock().unwrap();
            match serde_json::from_str::<String>(json) {
                Ok(path) => data.path = Some(path.into()),
                Err(error) => {
                    data.parse_error = Some(format!("Invalid final path from yt-dlp: {error}"))
                }
            }
        } else if inspect && line.starts_with('{') {
            let mut data = collected.lock().unwrap();
            match serde_json::from_str(&line) {
                Ok(media) => data.media = Some(media),
                Err(error) => {
                    data.parse_error = Some(format!("Could not read yt-dlp metadata: {error}"))
                }
            }
        } else if !line.is_empty() {
            let mut data = collected.lock().unwrap();
            if data.diagnostics.len() == 12 {
                data.diagnostics.pop_front();
            }
            data.diagnostics.push_back(line.chars().take(600).collect());
        }
    }
}

fn run(
    operation: Operation,
    executable: PathBuf,
    control: &Control,
    send: &Sender<Event>,
) -> Result<Event> {
    let args = operation.args()?;
    let staging = Staging::new(&operation)?;
    let inspect = matches!(operation, Operation::Inspect { .. });
    let mut command = Command::new(executable);
    if let Some(staging) = &staging {
        command.current_dir(&staging.directory);
    }
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
        if let Some(staging) = &staging {
            let fd = staging._lock.as_raw_fd();
            // Keep the flock alive in yt-dlp if the GUI crashes. Clear only
            // this child's copy of CLOEXEC; never change the parent's flags.
            unsafe {
                command.pre_exec(move || {
                    if libc::fcntl(fd, libc::F_SETFD, 0) == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
        }
    }
    let mut child = {
        let mut pid = control.pid.lock().unwrap();
        if control.cancelled.load(Ordering::SeqCst) {
            return Ok(Event::Cancelled);
        }
        let child = command
            .spawn()
            .context("Could not start yt-dlp. Install it and ensure it is on PATH.")?;
        *pid = Some(child.id());
        child
    };
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let collected = Mutex::new(Collected::default());
    let status = thread::scope(|scope| -> Result<_> {
        scope.spawn(|| collect(stdout, inspect, send, &collected));
        scope.spawn(|| collect(stderr, false, send, &collected));
        loop {
            {
                let mut pid = control.pid.lock().unwrap();
                // Observe exit without reaping, preserving the group identity
                // until any descendants holding our pipes have been stopped.
                let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
                let waited = unsafe {
                    libc::waitid(
                        libc::P_PID,
                        child.id(),
                        &mut info,
                        libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                    )
                };
                if waited == -1 {
                    return Err(std::io::Error::last_os_error().into());
                }
                if unsafe { info.si_pid() } != 0 {
                    unsafe {
                        libc::kill(-(child.id() as i32), libc::SIGKILL);
                    }
                    let status = child.wait()?;
                    *pid = None;
                    break Ok(status);
                }
            }
            thread::sleep(Duration::from_millis(40));
        }
    })?;
    let data = collected.into_inner().unwrap();
    if !status.success() {
        let details = data.diagnostics.into_iter().collect::<Vec<_>>().join("\n");
        bail!("yt-dlp exited with {status}.\n{details}");
    }
    if let Some(error) = data.parse_error {
        bail!("{error}");
    }
    if inspect {
        let media = data.media.context("yt-dlp returned no video metadata.")?;
        if media.is_live
            || matches!(
                media.live_status.as_deref(),
                Some("is_live" | "is_upcoming")
            )
        {
            bail!("Live and upcoming streams are not supported. Choose a recorded video.");
        }
        if matches!(media.kind.as_deref(), Some("playlist" | "multi_video")) {
            bail!("Choose a single video, not a playlist.");
        }
        Ok(Event::Inspected(media))
    } else {
        let path = data
            .path
            .context("yt-dlp exited successfully but did not report an output file.")?;
        if control.cancelled.load(Ordering::SeqCst) {
            return Ok(Event::Cancelled);
        }
        Ok(Event::Completed(staging.as_ref().unwrap().publish(path)?))
    }
}

/// Run away from the UI thread. Version probes also verify executables launch.
pub fn dependencies() -> Vec<(String, bool)> {
    ["yt-dlp", "ffmpeg", "ffprobe", "deno"]
        .into_iter()
        .map(|name| {
            let flag = if name.starts_with("ff") {
                "-version"
            } else {
                "--version"
            };
            let ready = Command::new(name)
                .arg(flag)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success());
            (name.to_owned(), ready)
        })
        .collect()
}
