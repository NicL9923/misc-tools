use crate::{Job, Model, ModelStatus};
use serde_json::{Value, json};

pub fn files(model: Model) -> Vec<Value> {
    let manifest: Value =
        serde_json::from_str(include_str!("../models.json")).expect("bundled model manifest");
    let names = manifest["models"][model.recipe()]
        .as_array()
        .expect("model files");
    manifest["files"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|file| names.contains(&file["filename"]))
        .cloned()
        .collect()
}
fn recipe(model: Model) -> Value {
    serde_json::from_str(match model {
        Model::Klein4B => include_str!("../workflows/klein.json"),
        Model::ZImageTurbo => include_str!("../workflows/z-image.json"),
        Model::Ideogram4Quality => include_str!("../workflows/ideogram.json"),
        Model::Flux2Dev => include_str!("../workflows/flux-dev.json"),
    })
    .expect("bundled workflow")
}
pub fn availability(info: &Value) -> Vec<ModelStatus> {
    Model::ALL
        .into_iter()
        .map(|model| {
            let mut missing = Vec::new();
            for file in files(model) {
                let (node, input) = match file["directory"].as_str().unwrap() {
                    "diffusion_models" => ("UNETLoader", "unet_name"),
                    "text_encoders" => ("CLIPLoader", "clip_name"),
                    "vae" => ("VAELoader", "vae_name"),
                    _ => unreachable!("bundled model directory"),
                };
                if !info[node]["input"]["required"][input][0]
                    .as_array()
                    .is_some_and(|values| values.contains(&file["filename"]))
                {
                    missing.push(file["filename"].as_str().unwrap().to_owned());
                }
            }
            ModelStatus {
                model,
                available: missing.is_empty(),
                detail: if missing.is_empty() {
                    format!("Installed · {}", model.license())
                } else {
                    format!("Missing {}", missing.join(", "))
                },
            }
        })
        .collect()
}
pub fn build(job: &Job) -> Value {
    let mut graph = recipe(job.model);
    // Preserve structured captions. Plain prompts use a minimal local JSON caption;
    // never call a hosted prompt-expansion service or add invented scene details.
    let prompt = if job.model == Model::Ideogram4Quality {
        match serde_json::from_str::<Value>(&job.prompt) {
            Ok(value) if value.is_object() && value["high_level_description"].is_string() => {
                job.prompt.clone()
            }
            _ => json!({"high_level_description": job.prompt,
                "compositional_deconstruction": {"background": "", "elements": []}})
            .to_string(),
        }
    } else {
        job.prompt.clone()
    };
    for node in graph.as_object_mut().unwrap().values_mut() {
        let class = node["class_type"].as_str().unwrap_or("").to_owned();
        let inputs = &mut node["inputs"];
        match class.as_str() {
            "CLIPTextEncode" => inputs["text"] = json!(prompt),
            "RandomNoise" => inputs["noise_seed"] = json!(job.seed),
            "KSampler" => inputs["seed"] = json!(job.seed),
            "EmptyFlux2LatentImage"
            | "EmptySD3LatentImage"
            | "Flux2Scheduler"
            | "Ideogram4Scheduler" => {
                inputs["width"] = json!(job.width);
                inputs["height"] = json!(job.height);
            }
            "SaveImage" => inputs["filename_prefix"] = json!(format!("ImageStudio/{}", job.id)),
            _ => {}
        }
    }
    graph
}
