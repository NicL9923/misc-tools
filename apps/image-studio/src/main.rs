use gpui_kit::component::{
    button::{Button, ButtonVariants},
    checkbox::Checkbox,
    input::{Textarea, TextareaState},
    select::{Select, SelectEvent, SelectState},
    *,
};
use gpui_kit::*;
use image_studio::{Batch, Config, Engine, Job, JobStatus, Model, Snapshot};
use std::{path::PathBuf, sync::Arc, time::Duration};

struct Studio {
    engine: Option<Arc<Engine>>,
    prompt: Entity<TextareaState>,
    count: Entity<SelectState<Vec<&'static str>>>,
    shape: Entity<SelectState<Vec<&'static str>>>,
    selected: Vec<Model>,
    images_per_model: u32,
    dimensions: (u32, u32),
    favorites_only: bool,
    details: Option<String>,
    error: Option<String>,
    notice: Option<String>,
    backend_starting: bool,
    show_setup: bool,
    _subscriptions: Vec<Subscription>,
}

impl Studio {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let prompt = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Describe the image you want to make…")
                .rows(5)
        });
        let count = cx.new(|cx| {
            SelectState::new(
                vec!["1", "2", "4", "8", "16"],
                Some(IndexPath::new(1)),
                window,
                cx,
            )
        });
        let shape = cx.new(|cx| {
            SelectState::new(
                vec![
                    "Square · 1024 × 1024",
                    "Landscape · 1216 × 832",
                    "Portrait · 832 × 1216",
                ],
                Some(IndexPath::default()),
                window,
                cx,
            )
        });
        let subscriptions = vec![
            cx.subscribe(&count, |this, _, event, cx| {
                if let SelectEvent::Confirm(Some(value)) = event {
                    this.images_per_model = value.parse().unwrap_or(1);
                    cx.notify();
                }
            }),
            cx.subscribe(&shape, |this, _, event, cx| {
                if let SelectEvent::Confirm(Some(value)) = event {
                    this.dimensions = if value.starts_with("Landscape") {
                        (1216, 832)
                    } else if value.starts_with("Portrait") {
                        (832, 1216)
                    } else {
                        (1024, 1024)
                    };
                    cx.notify();
                }
            }),
        ];
        let (engine, error) = match Engine::open(Config::default()) {
            Ok(engine) => (Some(Arc::new(engine)), None),
            Err(error) => (None, Some(format!("Could not open the queue: {error:#}"))),
        };
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(500))
                    .await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        })
        .detach();
        Self {
            engine,
            prompt,
            count,
            shape,
            selected: vec![Model::Klein4B, Model::ZImageTurbo],
            images_per_model: 2,
            dimensions: (1024, 1024),
            favorites_only: false,
            details: None,
            error,
            notice: None,
            backend_starting: false,
            show_setup: false,
            _subscriptions: subscriptions,
        }
    }

    fn result(&mut self, result: anyhow::Result<()>, cx: &mut Context<Self>) {
        self.error = result.err().map(|error| format!("{error:#}"));
        cx.notify();
    }

    fn enqueue(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.engine.clone() else {
            return;
        };
        let batch = Batch {
            prompt: self.prompt.read(cx).value().to_string(),
            models: self.selected.clone(),
            images_per_model: self.images_per_model,
            width: self.dimensions.0,
            height: self.dimensions.1,
        };
        let total = batch.models.len() as u32 * batch.images_per_model;
        let result = engine.enqueue(batch);
        if result.is_ok() {
            self.notice = Some(format!("Added {total} images to the queue"));
        }
        self.result(result, cx);
    }

    fn start_backend(&mut self, cx: &mut Context<Self>) {
        self.backend_starting = true;
        let task = cx.background_executor().spawn(async {
            std::process::Command::new("systemctl")
                .args(["--user", "start", "misc-tools-comfyui.service"])
                .output()
                .map_err(|error| error.to_string())
                .and_then(|output| {
                    if output.status.success() {
                        Ok(())
                    } else {
                        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
                    }
                })
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.backend_starting = false;
                match result {
                    Ok(()) => {
                        this.notice = Some(
                            "Starting the local engine. Model availability will appear shortly."
                                .into(),
                        );
                        if let Some(engine) = &this.engine {
                            let _ = engine.refresh_models();
                        }
                    }
                    Err(error) => {
                        this.error = Some(format!("Could not start ComfyUI: {error}"));
                        this.show_setup = true;
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn save_copy(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        let prompt = cx.prompt_for_new_path(
            &home.join("Pictures"),
            path.file_name().and_then(|name| name.to_str()),
        );
        cx.spawn(async move |this, cx| match prompt.await {
            Ok(Ok(Some(destination))) => {
                let result = cx
                    .background_executor()
                    .spawn(async move {
                        use std::io::Write;
                        let bytes = std::fs::read(&path)?;
                        let parent = destination
                            .parent()
                            .ok_or_else(|| std::io::Error::other("No destination folder"))?;
                        let mut output = tempfile::NamedTempFile::new_in(parent)?;
                        output.write_all(&bytes)?;
                        output.as_file().sync_all()?;
                        output
                            .persist_noclobber(&destination)
                            .map_err(|error| error.error)?;
                        Ok::<_, std::io::Error>(())
                    })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    match result {
                        Ok(()) => this.notice = Some("Saved a copy".into()),
                        Err(error) => this.error = Some(format!("Could not save copy: {error}")),
                    }
                    cx.notify();
                });
            }
            Ok(Ok(None)) => {}
            _ => {
                let _ = this.update(cx, |this, cx| {
                    this.error = Some(
                        "The save dialog could not open. Use Show in folder to copy the image."
                            .into(),
                    );
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn composer(
        &self,
        snapshot: Option<&Snapshot>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let muted = cx.theme().muted_foreground;
        let mut models = v_flex().gap_2();
        for model in Model::ALL {
            let selected = self.selected.contains(&model);
            let status = snapshot.and_then(|s| s.models.iter().find(|m| m.model == model));
            let available = status.is_some_and(|status| status.available);
            let subtitle = match model {
                Model::Klein4B => "Fast, general-purpose generation",
                Model::ZImageTurbo => "Photorealism and prompt exploration",
                Model::Ideogram4Quality => "48-step Quality · NVFP4 · Blackwell GPU",
                Model::Flux2Dev => "Full 32B · NVFP4 · slower, uses disk offload",
            };
            models = models.child(
                v_flex()
                    .gap_0()
                    .p_2()
                    .rounded_lg()
                    .border_1()
                    .border_color(if selected {
                        rgb(0x3b82f6).into()
                    } else {
                        cx.theme().border
                    })
                    .child(
                        Checkbox::new(model.id())
                            .label(model.label())
                            .checked(selected)
                            .on_click(cx.listener(move |this, checked, _, cx| {
                                if *checked {
                                    if !this.selected.contains(&model) {
                                        this.selected.push(model);
                                    }
                                } else {
                                    this.selected.retain(|value| *value != model);
                                }
                                cx.notify();
                            })),
                    )
                    .child(div().text_xs().text_color(muted).child(subtitle))
                    .child(
                        div()
                            .text_xs()
                            .text_color(if available {
                                rgb(0x86efac).into()
                            } else {
                                muted
                            })
                            .child(if available {
                                format!("Installed · {}", model.license())
                            } else {
                                "Needs setup · open Engine & models".to_owned()
                            }),
                    ),
            );
        }
        let total = self.selected.len() as u32 * self.images_per_model;
        let mut composer = v_flex()
            .gap_4()
            .child(
                div()
                    .text_lg()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Create images"),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(div().text_sm().child("Prompt"))
                    .child(
                        Textarea::new(&self.prompt)
                            .aria_label("Image prompt")
                            .h(px(145.)),
                    ),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(div().text_sm().child("Models"))
                    .child(models),
            )
            .child(
                h_flex()
                    .gap_3()
                    .child(
                        v_flex()
                            .w(px(104.))
                            .flex_shrink_0()
                            .min_w_0()
                            .gap_2()
                            .child(div().text_sm().child("Per model"))
                            .child(
                                Select::new(&self.count)
                                    .accessibility_label("Images per model")
                                    .w_full(),
                            ),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_2()
                            .child(div().text_sm().child("Canvas"))
                            .child(
                                Select::new(&self.shape)
                                    .accessibility_label("Canvas dimensions")
                                    .w_full(),
                            ),
                    ),
            )
            .child(
                Button::new("generate")
                    .primary()
                    .label(format!(
                        "Queue {total} {}",
                        if total == 1 { "image" } else { "images" }
                    ))
                    .disabled(total == 0 || self.engine.is_none())
                    .on_click(cx.listener(|this, _, _, cx| this.enqueue(cx))),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child("One image at a time. Jobs run together by model."),
            );
        if let Some(snapshot) = snapshot {
            let pending = snapshot
                .jobs
                .iter()
                .filter(|j| matches!(j.status, JobStatus::Pending | JobStatus::Generating))
                .count();
            composer = composer.child(
                v_flex()
                    .gap_2()
                    .pt_3()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex().justify_between().child("Queue").child(
                            div()
                                .text_sm()
                                .text_color(muted)
                                .child(format!("{pending} remaining")),
                        ),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .flex_wrap()
                            .child(
                                Button::new("pause")
                                    .label(if snapshot.paused {
                                        "Resume queue"
                                    } else {
                                        "Pause queue"
                                    })
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if let Some(engine) = &this.engine {
                                            let result =
                                                engine.set_paused(!engine.snapshot().paused);
                                            this.result(result, cx);
                                        }
                                    })),
                            )
                            .child(
                                Button::new("cancel-all")
                                    .label("Cancel remaining")
                                    .disabled(pending == 0)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if let Some(engine) = &this.engine {
                                            let result = engine.cancel_all();
                                            this.result(result, cx);
                                        }
                                    })),
                            ),
                    ),
            );
        }
        composer
    }

    fn job_card(
        &self,
        job: &Job,
        width: Pixels,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let muted = cx.theme().muted_foreground;
        let id = job.id.clone();
        let mut card = v_flex()
            .w(width)
            .gap_2()
            .rounded_lg()
            .border_1()
            .border_color(cx.theme().border)
            .overflow_hidden();
        if let Some(path) = &job.output {
            let path = path.clone();
            card = card.child(
                div()
                    .id(SharedString::from(format!("preview-{id}")))
                    .relative()
                    .w_full()
                    .h(px(220.))
                    .flex_shrink_0()
                    .overflow_hidden()
                    .bg(rgb(0x101218))
                    .cursor_pointer()
                    .child(
                        img(path.clone())
                            .absolute()
                            .size_full()
                            .aspect_ratio(f32::from(width) / 220.)
                            .object_fit(ObjectFit::Contain)
                            .bg(rgb(0x101218)),
                    )
                    .on_click(move |_, _, cx| {
                        if let Ok(url) = url::Url::from_file_path(&path) {
                            cx.open_url(url.as_str());
                        }
                    }),
            );
        } else {
            card = card.child(
                v_flex()
                    .h(px(160.))
                    .justify_center()
                    .items_center()
                    .gap_2()
                    .bg(rgb(0x101218))
                    .child(div().text_lg().child(if job.cancel_requested {
                        "Cancelling after this image"
                    } else {
                        status_label(&job.status)
                    }))
                    .child(div().text_xs().text_color(muted).child(job.model.label())),
            );
        }
        let favorite_id = id.clone();
        let details_id = id.clone();
        let mut actions = h_flex().gap_2().flex_wrap();
        if job.output.is_some() {
            actions = actions.child(
                Button::new(SharedString::from(format!("favorite-{id}")))
                    .label(if job.favorite {
                        "★ Favorite"
                    } else {
                        "☆ Favorite"
                    })
                    .selected(job.favorite)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(engine) = &this.engine {
                            let result = engine.favorite(&favorite_id);
                            this.result(result, cx);
                        }
                    })),
            );
            let regen_id = id.clone();
            actions = actions.child(
                Button::new(SharedString::from(format!("regenerate-{id}")))
                    .label("Regenerate")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(engine) = &this.engine {
                            let result = engine.regenerate(&regen_id);
                            this.result(result, cx);
                        }
                    })),
            );
        } else if matches!(job.status, JobStatus::Pending | JobStatus::Generating) {
            let cancel_id = id.clone();
            actions = actions.child(
                Button::new(SharedString::from(format!("cancel-{id}")))
                    .label(if job.cancel_requested {
                        "Cancelling…"
                    } else {
                        "Cancel"
                    })
                    .disabled(job.cancel_requested)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(engine) = &this.engine {
                            let result = engine.cancel(&cancel_id);
                            this.result(result, cx);
                        }
                    })),
            );
        } else {
            let retry_id = id.clone();
            actions = actions.child(
                Button::new(SharedString::from(format!("retry-{id}")))
                    .label("Retry")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(engine) = &this.engine {
                            let result = engine.retry(&retry_id);
                            this.result(result, cx);
                        }
                    })),
            );
        }
        actions = actions.child(
            Button::new(SharedString::from(format!("details-{id}")))
                .label("Details")
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.details = if this.details.as_ref() == Some(&details_id) {
                        None
                    } else {
                        Some(details_id.clone())
                    };
                    cx.notify();
                })),
        );
        let mut body = v_flex()
            .gap_2()
            .p_3()
            .child(div().text_sm().line_clamp(2).child(job.prompt.clone()))
            .child(actions);
        if let Some(error) = &job.error {
            body = body.child(
                div()
                    .text_xs()
                    .text_color(rgb(0xfca5a5))
                    .child(error.clone()),
            );
        }
        if self.details.as_ref() == Some(&id) {
            body = body
                .child(div().text_xs().text_color(muted).child(format!(
                    "{} · {} × {} · seed {}",
                    status_label(&job.status),
                    job.width,
                    job.height,
                    job.seed
                )))
                .child(div().text_sm().child(job.prompt.clone()));
            let prompt = job.prompt.clone();
            let model = job.model;
            body = body.child(
                Button::new(SharedString::from(format!("reuse-{id}")))
                    .label("Use prompt")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.prompt
                            .update(cx, |input, cx| input.set_value(prompt.clone(), window, cx));
                        this.selected = vec![model];
                        cx.notify();
                    })),
            );
            if let Some(path) = &job.output {
                let save_path = path.clone();
                let show_path = path.clone();
                body = body.child(
                    h_flex()
                        .gap_2()
                        .flex_wrap()
                        .child(
                            Button::new(SharedString::from(format!("save-{id}")))
                                .label("Save a copy")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.save_copy(save_path.clone(), cx)
                                })),
                        )
                        .child(
                            Button::new(SharedString::from(format!("folder-{id}")))
                                .label("Show in folder")
                                .on_click(move |_, _, cx| cx.reveal_path(&show_path)),
                        ),
                );
            }
        }
        card.child(body)
    }

    fn gallery(
        &self,
        snapshot: Option<&Snapshot>,
        width: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let muted = cx.theme().muted_foreground;
        let mut gallery = v_flex().gap_4().w_full().min_w_0().child(
            h_flex()
                .justify_between()
                .gap_3()
                .flex_wrap()
                .child(
                    div()
                        .text_lg()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Gallery"),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("all-images")
                                .label("All images")
                                .selected(!self.favorites_only)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.favorites_only = false;
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("favorites")
                                .label("Favorites")
                                .selected(self.favorites_only)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.favorites_only = true;
                                    cx.notify();
                                })),
                        ),
                ),
        );
        let jobs: Vec<_> = snapshot
            .map(|s| {
                s.jobs
                    .iter()
                    .rev()
                    .filter(|job| !self.favorites_only || job.favorite)
                    .collect()
            })
            .unwrap_or_default();
        if jobs.is_empty() {
            return gallery.child(v_flex().w_full().min_h(px(300.)).justify_center().items_center().gap_3()
                .child(div().text_lg().child(if self.favorites_only { "No favorites yet" } else { "Your next idea starts here" }))
                .child(div().max_w(px(350.)).text_center().text_sm().text_color(muted).child(if self.favorites_only { "Favorite an image to keep it here." } else { "Write a prompt and choose your models. Images appear here as they finish." })));
        }
        let columns = if width >= 620. { 2. } else { 1. };
        let card_width = px(((width - (columns - 1.) * 16.) / columns).max(240.));
        for model in Model::ALL {
            let group: Vec<_> = jobs.iter().filter(|job| job.model == model).collect();
            if group.is_empty() {
                continue;
            }
            let mut cards = h_flex().items_start().flex_wrap().gap_4();
            for job in &group {
                cards = cards.child(self.job_card(job, card_width, cx));
            }
            gallery = gallery.child(
                v_flex()
                    .gap_3()
                    .child(
                        h_flex()
                            .gap_3()
                            .items_center()
                            .child(div().font_weight(FontWeight::SEMIBOLD).child(model.label()))
                            .child(div().text_xs().text_color(muted).child(format!(
                                "{} {}",
                                group.len(),
                                if group.len() == 1 { "image" } else { "images" }
                            ))),
                    )
                    .child(cards),
            );
        }
        gallery
    }
}

fn status_label(status: &JobStatus) -> &'static str {
    match status {
        JobStatus::Pending => "Waiting",
        JobStatus::Generating => "Loading / generating…",
        JobStatus::Completed => "Completed",
        JobStatus::Failed => "Failed",
        JobStatus::Cancelled => "Cancelled",
    }
}

impl Render for Studio {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let snapshot = self.engine.as_ref().map(|engine| engine.snapshot());
        let width = f32::from(window.viewport_size().width);
        let wide = width >= 980.;
        let available = (width - 48.).min(1500.);
        let gallery_width = if wide { available - 384. } else { available };
        let mut content = v_flex()
            .gap_5()
            .p_6()
            .w_full()
            .max_w(px(1548.))
            .mx_auto()
            .child(
                h_flex()
                    .justify_between()
                    .gap_3()
                    .flex_wrap()
                    .child(
                        div()
                            .text_3xl()
                            .font_weight(FontWeight::BOLD)
                            .child("Image Studio"),
                    )
                    .child(
                        Button::new("setup-toggle")
                            .label("Engine & models")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.show_setup = !this.show_setup;
                                cx.notify();
                            })),
                    ),
            );
        if self.show_setup {
            content = content.child(v_flex().gap_2().p_4().rounded_lg().bg(rgb(0x181b23))
                .child(div().text_sm().child(snapshot.as_ref().map(|s| s.backend_status.clone()).unwrap_or_else(|| "Queue unavailable".into())))
                .child(div().text_sm().child("ComfyUI runs locally. Models are loaded only when their jobs run."))
                .child(h_flex().gap_2().flex_wrap()
                    .child(Button::new("start-engine").label("Start engine").disabled(self.backend_starting)
                        .on_click(cx.listener(|this, _, _, cx| this.start_backend(cx))))
                    .child(Button::new("refresh-models").label("Refresh models")
                        .on_click(cx.listener(|this, _, _, cx| { if let Some(engine) = &this.engine { let result = engine.refresh_models(); this.result(result, cx); } }))))
                .child(div().text_xs().text_color(cx.theme().muted_foreground).child("Setup from the misc-tools checkout: .agents/tools/setup-image-runtime.py. Add --models ideogram flux-dev for the larger NVFP4 models. These require a supported Blackwell GPU and have non-commercial model licenses.")));
        }
        if let Some(error) = &self.error {
            content = content.child(
                div()
                    .rounded_lg()
                    .p_3()
                    .bg(rgb(0x391e25))
                    .text_color(rgb(0xfca5a5))
                    .child(error.clone()),
            );
        }
        if let Some(notice) = &self.notice {
            content = content.child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(notice.clone()),
            );
        }
        let composer = self.composer(snapshot.as_ref(), cx);
        let gallery = self.gallery(snapshot.as_ref(), gallery_width, cx);
        content = if wide {
            content.child(
                h_flex()
                    .gap_6()
                    .items_start()
                    .child(div().w(px(360.)).flex_shrink_0().child(composer))
                    .child(div().flex_1().min_w_0().child(gallery)),
            )
        } else {
            content.child(composer).child(gallery)
        };
        div()
            .id("studio-scroll")
            .size_full()
            .overflow_y_scroll()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(content)
    }
}

fn main() {
    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(|cx| {
            gpui_kit::init(cx);
            Theme::change(ThemeMode::Dark, None, cx);
            let theme = Theme::global_mut(cx);
            theme.colors.background = rgb(0x101216).into();
            theme.colors.primary = rgb(0x2563eb).into();
            theme.colors.primary_hover = rgb(0x1d4ed8).into();
            theme.colors.primary_active = rgb(0x1e40af).into();
            theme.colors.primary_foreground = rgb(0xffffff).into();
            theme.colors.button_primary = theme.colors.primary;
            theme.colors.button_primary_hover = theme.colors.primary_hover;
            theme.colors.button_primary_active = theme.colors.primary_active;
            theme.colors.button_primary_foreground = theme.colors.primary_foreground;
            theme.colors.ring = rgb(0x60a5fa).into();
            theme.tokens.primary = theme.colors.primary.into();
            theme.tokens.button_primary = theme.colors.primary.into();
            theme.tokens.button_primary_hover = theme.colors.primary_hover.into();
            theme.tokens.button_primary_active = theme.colors.primary_active.into();
            theme.tokens.button_primary_foreground = theme.colors.primary_foreground.into();
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            cx.spawn(async move |cx| {
                cx.open_window(
                    WindowOptions {
                        app_id: Some("misc-tools-image-studio".into()),
                        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                            point(px(80.), px(60.)),
                            size(px(1200.), px(850.)),
                        ))),
                        window_min_size: Some(size(px(600.), px(600.))),
                        titlebar: Some(TitlebarOptions {
                            title: Some("Image Studio".into()),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                    |window, cx| {
                        let view = cx.new(|cx| Studio::new(window, cx));
                        cx.new(|cx| Root::new(view, window, cx))
                    },
                )
                .expect("Could not open Image Studio");
            })
            .detach();
        });
}
