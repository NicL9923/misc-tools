//! Durable, sequential local image generation. The UI never performs network requests.
mod workflow;
use anyhow::{Context, Result, bail};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs::{self, File, OpenOptions},
    os::fd::AsRawFd,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};
use uuid::Uuid;

#[derive(Debug)]
struct BackendHttpError {
    status: reqwest::StatusCode,
    detail: String,
}
impl std::fmt::Display for BackendHttpError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "ComfyUI HTTP {}: {}", self.status, self.detail)
    }
}
impl std::error::Error for BackendHttpError {}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Model {
    Klein4B,
    ZImageTurbo,
}
impl Model {
    pub fn id(self) -> &'static str {
        match self {
            Self::Klein4B => "klein-4b",
            Self::ZImageTurbo => "z-image-turbo",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Klein4B => "FLUX.2 Klein 4B",
            Self::ZImageTurbo => "Z-Image-Turbo",
        }
    }
}
#[derive(Clone, Debug)]
pub struct Config {
    pub state_dir: PathBuf,
    pub output_dir: PathBuf,
    pub backend_url: String,
}
impl Default for Config {
    fn default() -> Self {
        let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| ".".into()));
        let state = std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/state"));
        Self {
            state_dir: state.join("misc-tools/image-studio"),
            output_dir: home.join("Pictures/Image Studio"),
            backend_url: "http://127.0.0.1:8190".into(),
        }
    }
}
#[derive(Clone, Debug)]
pub struct Batch {
    pub prompt: String,
    pub models: Vec<Model>,
    pub images_per_model: u32,
    pub width: u32,
    pub height: u32,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum JobStatus {
    Pending,
    Generating,
    Completed,
    Failed,
    Cancelled,
}
impl JobStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Pending => "Waiting",
            Self::Generating => "Generating",
            Self::Completed => "Completed",
            Self::Failed => "Failed",
            Self::Cancelled => "Cancelled",
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub prompt: String,
    pub model: Model,
    pub seed: u64,
    pub width: u32,
    pub height: u32,
    pub status: JobStatus,
    pub output: Option<PathBuf>,
    pub error: Option<String>,
    pub favorite: bool,
    #[serde(default)]
    pub prompt_id: Option<String>,
    #[serde(default)]
    pub cancel_requested: bool,
    #[serde(default)]
    pub workflow: Option<Value>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelStatus {
    pub model: Model,
    pub available: bool,
    pub detail: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub jobs: Vec<Job>,
    pub models: Vec<ModelStatus>,
    pub paused: bool,
    pub backend_status: String,
}
impl Default for Snapshot {
    fn default() -> Self {
        Self {
            jobs: vec![],
            models: vec![],
            paused: false,
            backend_status: "Connecting to local ComfyUI…".into(),
        }
    }
}
struct Shared {
    snapshot: Mutex<Snapshot>,
    config: Config,
    client: Client,
    stop: AtomicBool,
    refresh: AtomicBool,
    memory_loaded: AtomicBool,
    _lock: File,
}
pub struct Engine {
    shared: Arc<Shared>,
}
impl Engine {
    pub fn open(config: Config) -> Result<Self> {
        let url = url::Url::parse(&config.backend_url)?;
        if url.scheme() != "http"
            || !matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))
            || url.path() != "/"
            || url.query().is_some()
            || !url.username().is_empty()
        {
            bail!("ComfyUI must use a local HTTP address");
        }
        fs::create_dir_all(&config.state_dir)?;
        fs::create_dir_all(&config.output_dir)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(config.state_dir.join("queue.lock"))?;
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            bail!("Image Studio is already running");
        }
        let state_path = config.state_dir.join("queue.json");
        let snapshot = if state_path.exists() {
            serde_json::from_slice(&fs::read(&state_path)?)
                .context("Cannot read saved queue; it has been preserved")?
        } else {
            Snapshot::default()
        };
        let shared = Arc::new(Shared {
            snapshot: Mutex::new(snapshot),
            config,
            client: Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(3))
                .timeout(Duration::from_secs(15))
                .build()?,
            stop: AtomicBool::new(false),
            refresh: AtomicBool::new(true),
            memory_loaded: AtomicBool::new(true),
            _lock: lock,
        });
        let worker = shared.clone();
        thread::Builder::new()
            .name("image-queue".into())
            .spawn(move || worker.run())?;
        Ok(Self { shared })
    }
    pub fn snapshot(&self) -> Snapshot {
        self.shared.snapshot.lock().unwrap().clone()
    }
    fn update(&self, edit: impl FnOnce(&mut Snapshot) -> Result<()>) -> Result<()> {
        self.shared.update(edit)
    }
    pub fn enqueue(&self, batch: Batch) -> Result<()> {
        if batch.prompt.trim().is_empty() || batch.prompt.len() > 16000 {
            bail!("Enter a prompt of 1–16000 characters");
        }
        if batch.models.is_empty() || batch.images_per_model == 0 || batch.images_per_model > 32 {
            bail!("Select models and 1–32 images per model");
        }
        if [batch.width, batch.height]
            .iter()
            .any(|n| *n < 256 || *n > 2048 || n % 64 != 0)
        {
            bail!("Image dimensions must be 256–2048, in multiples of 64");
        }
        self.update(|state| {
            let mut models = vec![];
            for model in batch.models {
                if !models.contains(&model) {
                    models.push(model);
                }
            }
            for model in models {
                for _ in 0..batch.images_per_model {
                    state.jobs.push(new_job(
                        batch.prompt.clone(),
                        model,
                        batch.width,
                        batch.height,
                    ));
                }
            }
            Ok(())
        })
    }
    pub fn cancel(&self, id: &str) -> Result<()> {
        self.update(|s| {
            let j = find_job(s, id)?;
            cancel_job(j);
            Ok(())
        })
    }
    pub fn cancel_all(&self) -> Result<()> {
        self.update(|s| {
            for j in &mut s.jobs {
                cancel_job(j);
            }
            Ok(())
        })
    }
    pub fn retry(&self, id: &str) -> Result<()> {
        self.update(|s| {
            let old = find_job(s, id)?.clone();
            let mut new = new_job(old.prompt, old.model, old.width, old.height);
            new.seed = old.seed;
            s.jobs.push(new);
            Ok(())
        })
    }
    pub fn regenerate(&self, id: &str) -> Result<()> {
        self.update(|s| {
            let old = find_job(s, id)?.clone();
            s.jobs
                .push(new_job(old.prompt, old.model, old.width, old.height));
            Ok(())
        })
    }
    pub fn favorite(&self, id: &str) -> Result<()> {
        self.update(|s| {
            let j = find_job(s, id)?;
            j.favorite = !j.favorite;
            Ok(())
        })
    }
    pub fn set_paused(&self, paused: bool) -> Result<()> {
        self.update(|s| {
            s.paused = paused;
            Ok(())
        })
    }
    pub fn refresh_models(&self) -> Result<()> {
        self.shared.refresh.store(true, Ordering::Relaxed);
        Ok(())
    }
}
impl Drop for Engine {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
    }
}
fn find_job<'a>(s: &'a mut Snapshot, id: &str) -> Result<&'a mut Job> {
    s.jobs
        .iter_mut()
        .find(|j| j.id == id)
        .context("Job no longer exists")
}
fn cancel_job(j: &mut Job) {
    match j.status {
        JobStatus::Pending => j.status = JobStatus::Cancelled,
        JobStatus::Generating => j.cancel_requested = true,
        _ => {}
    }
}
fn new_job(prompt: String, model: Model, width: u32, height: u32) -> Job {
    let seed = u64::from_le_bytes(Uuid::new_v4().as_bytes()[..8].try_into().unwrap())
        & 0x7fff_ffff_ffff_ffff;
    Job {
        id: Uuid::new_v4().to_string(),
        prompt,
        model,
        seed,
        width,
        height,
        status: JobStatus::Pending,
        output: None,
        error: None,
        favorite: false,
        prompt_id: None,
        cancel_requested: false,
        workflow: None,
    }
}
fn atomic_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let temp = path.with_extension("json.tmp");
    let file = File::create(&temp)?;
    serde_json::to_writer_pretty(&file, value)?;
    file.sync_all()?;
    fs::rename(temp, path)?;
    File::open(path.parent().unwrap())?.sync_all()?;
    Ok(())
}
impl Shared {
    fn update(&self, edit: impl FnOnce(&mut Snapshot) -> Result<()>) -> Result<()> {
        let mut state = self.snapshot.lock().unwrap();
        let mut next = state.clone();
        edit(&mut next)?;
        atomic_json(&self.config.state_dir.join("queue.json"), &next)?;
        *state = next;
        Ok(())
    }
    fn get(&self, path: &str) -> Result<Value> {
        Ok(self
            .client
            .get(format!(
                "{}{path}",
                self.config.backend_url.trim_end_matches('/')
            ))
            .send()?
            .error_for_status()?
            .json()?)
    }
    fn post(&self, path: &str, value: Value) -> Result<Value> {
        let r = self
            .client
            .post(format!(
                "{}{path}",
                self.config.backend_url.trim_end_matches('/')
            ))
            .json(&value)
            .send()?;
        let status = r.status();
        if !status.is_success() {
            use std::io::Read;
            const LIMIT: usize = 16 * 1024;
            let mut bytes = Vec::new();
            let read_error = r.take((LIMIT + 1) as u64).read_to_end(&mut bytes).err();
            let truncated = bytes.len() > LIMIT;
            bytes.truncate(LIMIT);
            let mut detail = String::from_utf8_lossy(&bytes).into_owned();
            if truncated {
                detail.push_str(" [response truncated]");
            }
            if let Some(error) = read_error {
                detail.push_str(&format!(" [response read failed: {error}]"));
            }
            return Err(BackendHttpError { status, detail }.into());
        }
        let bytes = r.bytes()?;
        if bytes.is_empty() {
            Ok(Value::Null)
        } else {
            Ok(serde_json::from_slice(&bytes)?)
        }
    }
    fn run(&self) {
        let mut ticks = 0u32;
        while !self.stop.load(Ordering::Relaxed) {
            if self.refresh.swap(false, Ordering::Relaxed) || ticks.is_multiple_of(40) {
                match self.get("/object_info") {
                    Ok(info) => {
                        let models = workflow::availability(&info);
                        let _ = self.update(|s| {
                            s.models = models;
                            s.backend_status = "ComfyUI connected".into();
                            Ok(())
                        });
                    }
                    Err(e) => {
                        let _ = self.update(|s| {
                            s.backend_status = format!("ComfyUI offline: {e}");
                            s.models.clear();
                            Ok(())
                        });
                    }
                }
            }
            if let Err(e) = self.tick() {
                let _ = self.update(|s| {
                    s.backend_status = format!("Backend: {e:#}");
                    Ok(())
                });
            }
            ticks = ticks.wrapping_add(1);
            thread::sleep(Duration::from_millis(250));
        }
    }
    fn tick(&self) -> Result<()> {
        let state = self.snapshot.lock().unwrap().clone();
        if let Some(job) = state
            .jobs
            .iter()
            .find(|j| j.status == JobStatus::Generating)
        {
            return self.poll(job);
        }
        if state.paused {
            return self.release_idle_cache();
        }
        let Some(job) = state.jobs.iter().find(|j| {
            j.status == JobStatus::Pending
                && state
                    .models
                    .iter()
                    .any(|m| m.model == j.model && m.available)
        }) else {
            return self.release_idle_cache();
        };
        // Never put a second workload behind an unrelated ComfyUI user.
        let queue = self.get("/queue")?;
        if ["queue_running", "queue_pending"]
            .iter()
            .any(|key| queue[*key].as_array().is_some_and(|v| !v.is_empty()))
        {
            return Ok(());
        }
        if state
            .jobs
            .iter()
            .rev()
            .find(|j| j.prompt_id.is_some())
            .is_some_and(|j| j.model != job.model)
        {
            self.post("/free", json!({"unload_models":true,"free_memory":true}))?;
        }
        let workflow = workflow::build(job);
        let prompt_id = Uuid::new_v4().to_string();
        let mut claimed = false;
        self.update(|s| {
            if s.paused {
                return Ok(());
            }
            let j = find_job(s, &job.id)?;
            if j.status != JobStatus::Pending {
                return Ok(());
            }
            claimed = true;
            j.status = JobStatus::Generating;
            j.prompt_id = Some(prompt_id.clone());
            j.workflow = Some(workflow.clone());
            Ok(())
        })?;
        if !claimed {
            return Ok(());
        }
        self.memory_loaded.store(true, Ordering::Relaxed);
        // Caller-selected ID makes a submitted job recoverable after a GUI crash.
        let response=self.post("/prompt",json!({"prompt":workflow,"prompt_id":prompt_id,"client_id":"misc-tools-image-studio","extra_data":{"image_studio_job":job.id}}));
        match response {
            Ok(value) => {
                if let Some(actual) = value["prompt_id"].as_str() {
                    self.update(|s| {
                        find_job(s, &job.id)?.prompt_id = Some(actual.into());
                        Ok(())
                    })?;
                } else {
                    self.fail(&job.id, "ComfyUI did not return a prompt ID")?;
                }
            }
            Err(e)
                if e.downcast_ref::<BackendHttpError>()
                    .is_some_and(|error| error.status.is_client_error()) =>
            {
                self.fail(&job.id, &format!("Workflow rejected: {e}"))?;
            }
            Err(e) => self.update(|s| {
                find_job(s, &job.id)?.error = Some(format!(
                    "Submission response lost: {e}. Reconciling saved prompt ID…"
                ));
                Ok(())
            })?,
        }
        Ok(())
    }
    fn release_idle_cache(&self) -> Result<()> {
        if !self.memory_loaded.load(Ordering::Relaxed) {
            return Ok(());
        }
        let queue = self.get("/queue")?;
        if ["queue_running", "queue_pending"]
            .iter()
            .any(|key| queue[*key].as_array().is_some_and(|v| !v.is_empty()))
        {
            return Ok(());
        }
        self.post("/free", json!({"unload_models":true,"free_memory":true}))?;
        self.memory_loaded.store(false, Ordering::Relaxed);
        Ok(())
    }
    fn fail(&self, id: &str, error: &str) -> Result<()> {
        self.update(|s| {
            let j = find_job(s, id)?;
            j.status = if j.cancel_requested {
                JobStatus::Cancelled
            } else {
                JobStatus::Failed
            };
            j.error = Some(error.into());
            Ok(())
        })
    }
    fn poll(&self, job: &Job) -> Result<()> {
        let Some(id) = job.prompt_id.as_deref() else {
            return self.fail(
                &job.id,
                "Interrupted before submission; regenerate to try again",
            );
        };
        let history = self.get(&format!("/history/{id}"))?;
        if let Some(entry) = history.get(id) {
            return self.finish_entry(job, entry);
        }
        let queue = self.get("/queue")?;
        let present = |key: &str| {
            queue[key]
                .as_array()
                .is_some_and(|items| items.iter().any(|row| row[1].as_str() == Some(id)))
        };
        if present("queue_pending") && job.cancel_requested {
            self.post("/queue", json!({"delete":[id]}))?;
            return self.update(|s| {
                find_job(s, &job.id)?.status = JobStatus::Cancelled;
                Ok(())
            });
        }
        if !present("queue_running") && !present("queue_pending") {
            // Completion may occur between the first history read and queue read.
            let history = self.get(&format!("/history/{id}"))?;
            if let Some(entry) = history.get(id) {
                return self.finish_entry(job, entry);
            }
            return self.fail(
                &job.id,
                "Backend lost this job, possibly after a restart. Regenerate to try again.",
            );
        }
        // Even the pinned server's prompt_id-scoped /interrupt checks a queue snapshot
        // before setting a global interrupt flag. Drain this image rather than risk
        // cancelling another client's job between that check and the interrupt.
        Ok(())
    }
    fn finish_entry(&self, job: &Job, entry: &Value) -> Result<()> {
        if job.cancel_requested {
            return self.update(|s| {
                find_job(s, &job.id)?.status = JobStatus::Cancelled;
                Ok(())
            });
        }
        if entry["status"]["status_str"] == "error" {
            return self.fail(
                &job.id,
                &format!("Generation failed: {}", entry["status"]["messages"]),
            );
        }
        if let Some(images) = entry["outputs"]
            .as_object()
            .and_then(|nodes| nodes.values().find_map(|n| n["images"].as_array()))
            && let Some(image) = images.first()
        {
            return self.save_result(job, image);
        }
        self.fail(&job.id, "ComfyUI completed without an output image")
    }
    fn save_result(&self, job: &Job, image: &Value) -> Result<()> {
        let name = image["filename"]
            .as_str()
            .context("Missing image filename")?;
        let bytes = self
            .client
            .get(format!(
                "{}/view",
                self.config.backend_url.trim_end_matches('/')
            ))
            .query(&[
                ("filename", name),
                ("subfolder", image["subfolder"].as_str().unwrap_or("")),
                ("type", image["type"].as_str().unwrap_or("output")),
            ])
            .send()?
            .error_for_status()?
            .bytes()?;
        if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            return self.fail(&job.id, "Backend returned an invalid PNG image");
        }
        let path = self
            .config
            .output_dir
            .join(format!("{}-{}.png", job.model.id(), job.id));
        let temp = path.with_extension("png.tmp");
        fs::write(&temp, &bytes)?;
        File::open(&temp)?.sync_all()?;
        fs::rename(&temp, &path)?;
        let mut metadata = serde_json::to_value(job)?;
        metadata["output"] = json!(path);
        metadata["status"] = json!("Completed");
        metadata["model_files"] = json!(workflow::files(job.model));
        atomic_json(&path.with_extension("json"), &metadata)?;
        self.update(|s| {
            let j = find_job(s, &job.id)?;
            j.output = Some(path);
            j.status = JobStatus::Completed;
            j.error = None;
            Ok(())
        })
    }
}
