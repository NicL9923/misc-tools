use crate::{Job, Model, ModelStatus};
use serde_json::{Value, json};

pub fn files(model: Model) -> Vec<Value> {
    let manifest: Value =
        serde_json::from_str(include_str!("../models.json")).expect("bundled model manifest");
    let names = names(model);
    manifest["files"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|file| names.contains(&file["filename"].as_str().unwrap_or("")))
        .cloned()
        .collect()
}
fn names(model: Model) -> [&'static str; 3] {
    match model {
        Model::Klein4B => [
            "flux-2-klein-4b-fp8.safetensors",
            "qwen_3_4b.safetensors",
            "flux2-vae.safetensors",
        ],
        Model::ZImageTurbo => [
            "z_image_turbo_bf16.safetensors",
            "qwen_3_4b.safetensors",
            "ae.safetensors",
        ],
    }
}
pub fn availability(info: &Value) -> Vec<ModelStatus> {
    [Model::Klein4B, Model::ZImageTurbo]
        .into_iter()
        .map(|model| {
            let missing: Vec<_> = names(model)
                .into_iter()
                .zip([
                    ("UNETLoader", "unet_name"),
                    ("CLIPLoader", "clip_name"),
                    ("VAELoader", "vae_name"),
                ])
                .filter_map(|(name, (node, input))| {
                    (!info[node]["input"]["required"][input][0]
                        .as_array()
                        .is_some_and(|v| v.iter().any(|n| n == name)))
                    .then_some(name)
                })
                .collect();
            ModelStatus {
                model,
                available: missing.is_empty(),
                detail: if missing.is_empty() {
                    "Installed · Apache 2.0".into()
                } else {
                    format!("Missing {}", missing.join(", "))
                },
            }
        })
        .collect()
}
pub fn build(job: &Job) -> Value {
    let recipe = match job.model {
        Model::Klein4B => include_str!("../workflows/klein.json"),
        Model::ZImageTurbo => include_str!("../workflows/z-image.json"),
    };
    let mut graph: Value = serde_json::from_str(recipe).expect("bundled workflow");
    for node in graph.as_object_mut().unwrap().values_mut() {
        let class = node["class_type"].as_str().unwrap_or("").to_owned();
        let inputs = &mut node["inputs"];
        match class.as_str() {
            "CLIPTextEncode" => inputs["text"] = json!(job.prompt),
            "RandomNoise" => inputs["noise_seed"] = json!(job.seed),
            "KSampler" => inputs["seed"] = json!(job.seed),
            "EmptyFlux2LatentImage" | "EmptySD3LatentImage" | "Flux2Scheduler" => {
                inputs["width"] = json!(job.width);
                inputs["height"] = json!(job.height);
            }
            "SaveImage" => inputs["filename_prefix"] = json!(format!("ImageStudio/{}", job.id)),
            _ => {}
        }
    }
    graph
}
