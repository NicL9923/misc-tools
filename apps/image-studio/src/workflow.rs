use crate::{Job, Model, ModelStatus};
use anyhow::{Context, Result, bail};
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
    if job.model == Model::Ideogram4Quality {
        // Reuse the local image encoder as a caption-writing LLM. Structured
        // captions bypass generation, preserving the user's exact instructions.
        if serde_json::from_str::<Value>(&job.prompt)
            .is_ok_and(|value| value.is_object() && value["high_level_description"].is_string())
        {
            graph.as_object_mut().unwrap().remove("18");
            graph["19"]["inputs"]["source"] = json!(job.prompt);
        } else {
            graph["18"]["inputs"]["prompt"] = json!(
                graph["18"]["inputs"]["prompt"]
                    .as_str()
                    .unwrap()
                    .replace("{aspect_ratio}", &format!("{}:{}", job.width, job.height))
                    .replace("{image_request}", &job.prompt)
            );
        }
    }
    for node in graph.as_object_mut().unwrap().values_mut() {
        let class = node["class_type"].as_str().unwrap_or("").to_owned();
        let inputs = &mut node["inputs"];
        match class.as_str() {
            "CLIPTextEncode" if job.model != Model::Ideogram4Quality => {
                inputs["text"] = json!(job.prompt);
            }
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

pub fn resolved_caption(job: &Job, entry: &Value) -> Result<Option<String>> {
    if job.model != Model::Ideogram4Quality {
        return Ok(None);
    }
    const ERROR: &str = "Ideogram caption was missing, malformed, or truncated. Retry or supply a complete structured JSON caption.";
    let text = entry["outputs"]["19"]["text"][0]
        .as_str()
        .or_else(|| job.workflow.as_ref()?["19"]["inputs"]["source"].as_str())
        .or_else(|| job.workflow.as_ref()?["4"]["inputs"]["text"].as_str())
        .context(ERROR)?;
    let caption: Value = serde_json::from_str(text).context(ERROR)?;
    if !caption.is_object()
        || !caption["high_level_description"]
            .as_str()
            .is_some_and(|value| !value.trim().is_empty())
        || !caption["compositional_deconstruction"]["background"].is_string()
        || caption
            .get("style_description")
            .is_some_and(|value| !value.is_object())
    {
        bail!(ERROR);
    }
    Ok(Some(text.to_owned()))
}
