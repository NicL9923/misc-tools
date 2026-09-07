use image_studio::{Batch, Config, Engine, JobStatus, Model};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use tempfile::TempDir;
#[derive(Default)]
struct Backend {
    pending: Vec<String>,
    history: HashMap<String, Value>,
    submissions: Vec<Value>,
    deleted: Vec<String>,
    free: usize,
    unrelated: bool,
    offline: bool,
    reject_prompt: bool,
    extra_models: bool,
    generated_caption: Option<String>,
    hold_view: bool,
    view_started: bool,
}
struct Server {
    url: String,
    state: Arc<Mutex<Backend>>,
    stop: Arc<AtomicBool>,
}
impl Server {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let state = Arc::new(Mutex::new(Backend::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let s = state.clone();
        let end = stop.clone();
        thread::spawn(move || {
            while !end.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => serve(stream, &s),
                    Err(_) => thread::sleep(Duration::from_millis(5)),
                }
            }
        });
        Self { url, state, stop }
    }
    fn complete(&self, id: &str) {
        let mut b = self.state.lock().unwrap();
        b.pending.retain(|p| p != id);
        let mut outputs =
            json!({"13":{"images":[{"filename":"image.png","type":"output","subfolder":""}]}});
        if let Some(caption) = &b.generated_caption {
            outputs["19"] = json!({"text": [caption]});
        }
        b.history.insert(
            id.into(),
            json!({"status":{"status_str":"success"},"outputs":outputs}),
        );
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}
fn serve(mut stream: TcpStream, state: &Mutex<Backend>) {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut bytes = vec![];
    let mut buf = [0; 4096];
    let end;
    loop {
        let n = stream.read(&mut buf).unwrap();
        if n == 0 {
            return;
        }
        bytes.extend_from_slice(&buf[..n]);
        if let Some(i) = bytes.windows(4).position(|s| s == b"\r\n\r\n") {
            end = i + 4;
            break;
        }
    }
    let header = String::from_utf8_lossy(&bytes[..end]).into_owned();
    let len = header
        .lines()
        .find_map(|s| {
            s.to_lowercase()
                .strip_prefix("content-length:")
                .map(|s| s.trim().parse::<usize>().unwrap())
        })
        .unwrap_or(0);
    while bytes.len() < end + len {
        let n = stream.read(&mut buf).unwrap();
        bytes.extend_from_slice(&buf[..n]);
    }
    let line = header.lines().next().unwrap();
    let path = line.split_whitespace().nth(1).unwrap();
    let body: Value = serde_json::from_slice(&bytes[end..]).unwrap_or(Value::Null);
    let mut b = state.lock().unwrap();
    if b.offline {
        stream
            .write_all(
                b"HTTP/1.1 503 Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .unwrap();
        return;
    }
    let response = match path {
        "/object_info" => {
            let mut unets = vec![
                "flux-2-klein-4b-fp8.safetensors",
                "z_image_turbo_bf16.safetensors",
            ];
            let mut encoders = vec!["qwen_3_4b.safetensors"];
            if b.extra_models {
                unets.extend([
                    "ideogram4_nvfp4_mixed.safetensors",
                    "ideogram4_unconditional_nvfp4_mixed.safetensors",
                    "flux2-dev-nvfp4.safetensors",
                ]);
                encoders.extend([
                    "qwen3vl_8b_nvfp4.safetensors",
                    "mistral_3_small_flux2_fp4_mixed.safetensors",
                ]);
            }
            json!({"UNETLoader":{"input":{"required":{"unet_name":[unets]}}},"CLIPLoader":{"input":{"required":{"clip_name":[encoders]}}},"VAELoader":{"input":{"required":{"vae_name":[["ae.safetensors","flux2-vae.safetensors"]]}}}})
        }
        "/queue" if line.starts_with("POST") => {
            for id in body["delete"].as_array().unwrap() {
                let id = id.as_str().unwrap();
                b.deleted.push(id.into());
                b.pending.retain(|p| p != id);
            }
            json!({})
        }
        "/queue" => {
            json!({"queue_running":if b.unrelated {vec![json!([0,"other-client",{}])]}else{b.pending.iter().map(|id|json!([0,id,{}])).collect::<Vec<_>>()},"queue_pending":[]})
        }
        "/prompt" if b.reject_prompt => {
            b.submissions.push(body);
            let rejection=json!({"error":{"type":"prompt_outputs_failed_validation","message":"CLIPLoader: encoder not found"},"node_errors":{"2":{"errors":[{"message":"qwen_3_4b.safetensors is missing"}]}},"padding":"x".repeat(20000)}).to_string();
            write!(stream,"HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{rejection}",rejection.len()).unwrap();
            return;
        }
        "/prompt" => {
            let id = body["prompt_id"].as_str().unwrap().to_string();
            b.pending.push(id.clone());
            b.submissions.push(body);
            json!({"prompt_id":id})
        }
        "/free" => {
            b.free += 1;
            json!({})
        }
        p if p.starts_with("/history/") => {
            let id = p.trim_start_matches("/history/");
            b.history
                .get(id)
                .map(|h| json!({id:h}))
                .unwrap_or(json!({}))
        }
        p if p.starts_with("/view?") => {
            b.view_started = true;
            drop(b);
            wait(|| !state.lock().unwrap().hold_view);
            let png = b"\x89PNG\r\n\x1a\nfixture";
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                png.len()
            )
            .unwrap();
            stream.write_all(png).unwrap();
            return;
        }
        _ => panic!("Unexpected request {line}"),
    };
    let text = response.to_string();
    write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",text.len()).unwrap();
}
fn config(dir: &TempDir, server: &Server) -> Config {
    Config {
        state_dir: dir.path().join("state"),
        output_dir: dir.path().join("outputs"),
        backend_url: server.url.clone(),
    }
}
fn batch(models: Vec<Model>, count: u32) -> Batch {
    Batch {
        prompt: "A Texas barn".into(),
        models,
        images_per_model: count,
        width: 512,
        height: 512,
    }
}
fn wait(check: impl Fn() -> bool) {
    let start = Instant::now();
    while !check() {
        assert!(start.elapsed() < Duration::from_secs(8), "timed out");
        thread::sleep(Duration::from_millis(20));
    }
}
#[test]
fn cancellation_during_image_download_discards_the_result() {
    let server = Server::start();
    server.state.lock().unwrap().hold_view = true;
    let dir = TempDir::new().unwrap();
    let engine = Engine::open(config(&dir, &server)).unwrap();
    engine.enqueue(batch(vec![Model::Klein4B], 1)).unwrap();
    wait(|| server.state.lock().unwrap().submissions.len() == 1);
    let job = engine.snapshot().jobs[0].clone();
    server.complete(job.prompt_id.as_ref().unwrap());
    wait(|| server.state.lock().unwrap().view_started);
    engine.cancel(&job.id).unwrap();
    server.state.lock().unwrap().hold_view = false;
    wait(|| engine.snapshot().jobs[0].status != JobStatus::Generating);
    let finished = engine.snapshot().jobs[0].clone();
    assert_eq!(finished.status, JobStatus::Cancelled);
    assert!(finished.output.is_none());
    assert_eq!(
        std::fs::read_dir(dir.path().join("outputs"))
            .unwrap()
            .count(),
        0
    );
    let saved: Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("state/queue.json")).unwrap())
            .unwrap();
    assert_eq!(saved["jobs"][0]["status"], "Cancelled");
}
#[test]
fn batches_are_sequential_grouped_and_publish_metadata() {
    let server = Server::start();
    let dir = TempDir::new().unwrap();
    let engine = Engine::open(config(&dir, &server)).unwrap();
    engine
        .enqueue(batch(vec![Model::Klein4B, Model::ZImageTurbo], 2))
        .unwrap();
    for n in 0..4 {
        wait(|| server.state.lock().unwrap().submissions.len() == n + 1);
        thread::sleep(Duration::from_millis(100));
        assert_eq!(server.state.lock().unwrap().submissions.len(), n + 1);
        let id = server.state.lock().unwrap().pending[0].clone();
        server.complete(&id);
        wait(|| engine.snapshot().jobs[n].status == JobStatus::Completed);
    }
    let snapshot = engine.snapshot();
    assert_eq!(
        snapshot.jobs.iter().map(|j| j.model).collect::<Vec<_>>(),
        vec![
            Model::Klein4B,
            Model::Klein4B,
            Model::ZImageTurbo,
            Model::ZImageTurbo
        ]
    );
    wait(|| server.state.lock().unwrap().free >= 2);
    let frees = server.state.lock().unwrap().free;
    thread::sleep(Duration::from_millis(350));
    assert_eq!(server.state.lock().unwrap().free, frees);
    for job in snapshot.jobs {
        let path = job.output.unwrap();
        assert!(path.exists());
        let metadata: Value =
            serde_json::from_slice(&std::fs::read(path.with_extension("json")).unwrap()).unwrap();
        assert_eq!(metadata["seed"], job.seed);
        assert_eq!(metadata["prompt"], job.prompt);
        assert!(metadata["workflow"].is_object());
        assert_eq!(metadata["model_files"].as_array().unwrap().len(), 3);
    }
}
#[test]
fn recovery_reconciles_without_duplicate_submission_and_locks_state() {
    let server = Server::start();
    let dir = TempDir::new().unwrap();
    let cfg = config(&dir, &server);
    let engine = Engine::open(cfg.clone()).unwrap();
    engine.enqueue(batch(vec![Model::Klein4B], 1)).unwrap();
    wait(|| server.state.lock().unwrap().submissions.len() == 1);
    assert!(Engine::open(cfg.clone()).is_err());
    let job = engine.snapshot().jobs[0].clone();
    drop(engine);
    thread::sleep(Duration::from_millis(500));
    server.complete(job.prompt_id.as_ref().unwrap());
    let engine = Engine::open(cfg).unwrap();
    wait(|| engine.snapshot().jobs[0].status == JobStatus::Completed);
    assert_eq!(server.state.lock().unwrap().submissions.len(), 1);
    engine.favorite(&job.id).unwrap();
    engine.set_paused(true).unwrap();
    engine.retry(&job.id).unwrap();
    engine.regenerate(&job.id).unwrap();
    let state = engine.snapshot();
    assert!(state.jobs[0].favorite);
    assert_eq!(state.jobs[1].seed, job.seed);
    assert_ne!(state.jobs[2].seed, job.seed);
}
#[test]
fn cancellation_preserves_completed_and_does_not_interrupt_other_clients() {
    let server = Server::start();
    let dir = TempDir::new().unwrap();
    let engine = Engine::open(config(&dir, &server)).unwrap();
    server.state.lock().unwrap().unrelated = true;
    engine.enqueue(batch(vec![Model::Klein4B], 3)).unwrap();
    thread::sleep(Duration::from_millis(400));
    assert!(server.state.lock().unwrap().submissions.is_empty());
    server.state.lock().unwrap().unrelated = false;
    wait(|| server.state.lock().unwrap().submissions.len() == 1);
    let id = server.state.lock().unwrap().pending[0].clone();
    server.complete(&id);
    wait(|| engine.snapshot().jobs[0].status == JobStatus::Completed);
    wait(|| server.state.lock().unwrap().submissions.len() == 2);
    engine.cancel_all().unwrap();
    let state = engine.snapshot();
    assert_eq!(state.jobs[0].status, JobStatus::Completed);
    assert!(state.jobs[1].cancel_requested);
    assert_eq!(state.jobs[2].status, JobStatus::Cancelled);
    let id = server.state.lock().unwrap().pending[0].clone();
    server.complete(&id);
    wait(|| engine.snapshot().jobs[1].status == JobStatus::Cancelled);
    assert_eq!(server.state.lock().unwrap().submissions.len(), 2);
}
#[test]
fn validates_input_and_preserves_corrupt_saved_queue() {
    let server = Server::start();
    let dir = TempDir::new().unwrap();
    let cfg = config(&dir, &server);
    let engine = Engine::open(cfg.clone()).unwrap();
    assert!(engine.enqueue(batch(vec![], 1)).is_err());
    assert!(engine.enqueue(batch(vec![Model::Klein4B], 0)).is_err());
    let mut invalid = batch(vec![Model::Klein4B], 1);
    invalid.width = 513;
    assert!(engine.enqueue(invalid).is_err());
    drop(engine);
    thread::sleep(Duration::from_millis(500));
    std::fs::write(cfg.state_dir.join("queue.json"), "broken").unwrap();
    assert!(Engine::open(cfg.clone()).is_err());
    assert_eq!(
        std::fs::read_to_string(cfg.state_dir.join("queue.json")).unwrap(),
        "broken"
    );
}

#[test]
fn backend_disconnect_keeps_active_job_until_reconciled() {
    let server = Server::start();
    let dir = TempDir::new().unwrap();
    let engine = Engine::open(config(&dir, &server)).unwrap();
    engine.enqueue(batch(vec![Model::Klein4B], 1)).unwrap();
    wait(|| server.state.lock().unwrap().submissions.len() == 1);
    let id = server.state.lock().unwrap().pending[0].clone();
    server.state.lock().unwrap().offline = true;
    wait(|| engine.snapshot().backend_status.contains("503"));
    assert_eq!(engine.snapshot().jobs[0].status, JobStatus::Generating);
    server.complete(&id);
    server.state.lock().unwrap().offline = false;
    engine.refresh_models().unwrap();
    wait(|| engine.snapshot().jobs[0].status == JobStatus::Completed);
    assert_eq!(server.state.lock().unwrap().submissions.len(), 1);
}
#[test]
fn failed_persistence_does_not_accept_or_submit_jobs() {
    let server = Server::start();
    let dir = TempDir::new().unwrap();
    let cfg = config(&dir, &server);
    let engine = Engine::open(cfg.clone()).unwrap();
    engine.set_paused(true).unwrap();
    // Initial model discovery also writes the queue. Let that transaction finish
    // before replacing its temporary file path with the failure fixture.
    wait(|| engine.snapshot().models.len() == Model::ALL.len());
    let saved = std::fs::read(cfg.state_dir.join("queue.json")).unwrap();
    std::fs::create_dir(cfg.state_dir.join("queue.json.tmp")).unwrap();
    assert!(engine.enqueue(batch(vec![Model::Klein4B], 1)).is_err());
    assert!(engine.snapshot().jobs.is_empty());
    assert_eq!(
        std::fs::read(cfg.state_dir.join("queue.json")).unwrap(),
        saved
    );
    assert!(server.state.lock().unwrap().submissions.is_empty());
    std::fs::remove_dir(cfg.state_dir.join("queue.json.tmp")).unwrap();
    engine.enqueue(batch(vec![Model::Klein4B], 1)).unwrap();
    assert_eq!(engine.snapshot().jobs.len(), 1);
}

#[test]
fn rejected_workflow_keeps_bounded_actionable_error_without_resubmission() {
    let server = Server::start();
    server.state.lock().unwrap().reject_prompt = true;
    let dir = TempDir::new().unwrap();
    let engine = Engine::open(config(&dir, &server)).unwrap();
    engine.enqueue(batch(vec![Model::Klein4B], 1)).unwrap();
    wait(|| engine.snapshot().jobs[0].status == JobStatus::Failed);
    let error = engine.snapshot().jobs[0].error.clone().unwrap();
    assert!(error.contains("400 Bad Request"));
    assert!(error.contains("CLIPLoader: encoder not found"));
    assert!(error.contains("qwen_3_4b.safetensors is missing"));
    assert!(error.contains("response truncated"));
    assert!(error.len() < 17000);
    thread::sleep(Duration::from_millis(600));
    assert_eq!(
        engine.snapshot().jobs[0].error.as_deref(),
        Some(error.as_str())
    );
    let state = server.state.lock().unwrap();
    assert_eq!(state.submissions.len(), 1);
    assert!(state.pending.is_empty());
}

#[test]
fn large_models_wait_for_install_then_generate_with_reproducible_recipes() {
    let server = Server::start();
    let dir = TempDir::new().unwrap();
    let engine = Engine::open(config(&dir, &server)).unwrap();
    let mut request = batch(vec![Model::Ideogram4Quality, Model::Flux2Dev], 1);
    request.prompt = "A sign reading \"Howdy\"\nunder an oak tree".into();
    request.width = 832;
    request.height = 1216;
    engine.enqueue(request.clone()).unwrap();
    wait(|| engine.snapshot().models.len() == 4);
    assert!(
        !engine
            .snapshot()
            .models
            .iter()
            .find(|m| m.model == Model::Ideogram4Quality)
            .unwrap()
            .available
    );
    assert!(server.state.lock().unwrap().submissions.is_empty());
    server.state.lock().unwrap().extra_models = true;
    engine.refresh_models().unwrap();
    let caption = format!(
        r#"{{"high_level_description":{},"compositional_deconstruction":{{"background":"A meadow","elements":[]}}}}"#,
        json!(request.prompt)
    );
    server.state.lock().unwrap().generated_caption = Some(caption.clone());
    for index in 0..2 {
        wait(|| server.state.lock().unwrap().submissions.len() == index + 1);
        let job = engine.snapshot().jobs[index].clone();
        let graph = job.workflow.as_ref().unwrap();
        let nodes: Vec<_> = graph.as_object().unwrap().values().collect();
        let text = nodes
            .iter()
            .find(|n| n["class_type"] == "CLIPTextEncode")
            .unwrap()["inputs"]["text"]
            .clone();
        if index == 0 {
            assert_eq!(text, json!(["19", 0]));
            assert!(nodes.iter().any(|n| {
                n["class_type"] == "TextGenerate"
                    && n["inputs"]["prompt"]
                        .as_str()
                        .unwrap()
                        .contains(&request.prompt)
            }));
            let scheduler = nodes
                .iter()
                .find(|n| n["class_type"] == "Ideogram4Scheduler")
                .unwrap();
            assert_eq!(scheduler["inputs"]["steps"], 48);
            assert_eq!(scheduler["inputs"]["width"], request.width);
            assert_eq!(scheduler["inputs"]["height"], request.height);
            assert!(
                nodes
                    .iter()
                    .any(|n| n["class_type"] == "SplitSigmas" && n["inputs"]["step"] == 45)
            );
        } else {
            assert_eq!(text, request.prompt);
            assert!(
                nodes
                    .iter()
                    .any(|n| n["inputs"]["unet_name"] == "flux2-dev-nvfp4.safetensors")
            );
        }
        assert!(
            nodes
                .iter()
                .any(|n| n["class_type"] == "RandomNoise" && n["inputs"]["noise_seed"] == job.seed)
        );
        server.complete(job.prompt_id.as_ref().unwrap());
        wait(|| engine.snapshot().jobs[index].status == JobStatus::Completed);
        let output = engine.snapshot().jobs[index].output.clone().unwrap();
        let metadata: Value =
            serde_json::from_slice(&std::fs::read(output.with_extension("json")).unwrap()).unwrap();
        assert_eq!(metadata["workflow"], *graph);
        if index == 0 {
            assert_eq!(metadata["resolved_caption"], caption);
        }
        assert_eq!(
            metadata["model_files"].as_array().unwrap().len(),
            if index == 0 { 4 } else { 3 }
        );
    }
}

#[test]
fn malformed_generated_captions_fail_and_structured_captions_keep_key_order() {
    let server = Server::start();
    server.state.lock().unwrap().extra_models = true;
    server.state.lock().unwrap().generated_caption = Some("{truncated".into());
    let dir = TempDir::new().unwrap();
    let engine = Engine::open(config(&dir, &server)).unwrap();
    engine
        .enqueue(batch(vec![Model::Ideogram4Quality], 1))
        .unwrap();
    wait(|| server.state.lock().unwrap().submissions.len() == 1);
    let id = server.state.lock().unwrap().pending[0].clone();
    server.complete(&id);
    wait(|| engine.snapshot().jobs[0].status == JobStatus::Failed);
    assert!(
        engine.snapshot().jobs[0]
            .error
            .as_ref()
            .unwrap()
            .contains("caption")
    );
    assert!(engine.snapshot().jobs[0].output.is_none());
    assert_eq!(
        std::fs::read_dir(dir.path().join("outputs"))
            .unwrap()
            .count(),
        0
    );

    let caption = r#"{"high_level_description":"A barn","style_description":{"aesthetics":"rustic","lighting":"sunset","medium":"illustration","art_style":"screen print"},"compositional_deconstruction":{"background":"A meadow","elements":[]}}"#;
    let mut request = batch(vec![Model::Ideogram4Quality], 1);
    request.prompt = caption.into();
    server.state.lock().unwrap().generated_caption = None;
    engine.enqueue(request).unwrap();
    wait(|| server.state.lock().unwrap().submissions.len() == 2);
    let job = engine.snapshot().jobs[1].clone();
    let graph = job.workflow.as_ref().unwrap();
    assert!(
        !graph
            .as_object()
            .unwrap()
            .values()
            .any(|n| n["class_type"] == "TextGenerate")
    );
    assert_eq!(graph["19"]["inputs"]["source"], caption);
    server.complete(job.prompt_id.as_ref().unwrap());
    wait(|| engine.snapshot().jobs[1].status == JobStatus::Completed);
    let output = engine.snapshot().jobs[1].output.clone().unwrap();
    let metadata: Value =
        serde_json::from_slice(&std::fs::read(output.with_extension("json")).unwrap()).unwrap();
    assert_eq!(metadata["resolved_caption"], caption);
}
