use gpui_kit::component::{
    button::{Button, ButtonVariants},
    input::{Input, InputEvent, InputState},
    progress::Progress as ProgressBar,
    select::{SearchableVec, Select, SelectEvent, SelectGroup, SelectItem, SelectState},
    *,
};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use std::path::PathBuf;
use youtube_downloader::{
    download::{self, Event, Job, Media, Operation, Output, Progress},
    settings::Settings,
};

#[derive(Clone)]
struct FormatOption {
    output: Output,
    label: &'static str,
    description: &'static str,
}

impl SelectItem for FormatOption {
    type Value = Output;
    fn title(&self) -> SharedString {
        self.label.into()
    }
    fn value(&self) -> &Output {
        &self.output
    }
    fn render(&self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        v_flex().min_w_0().py_1().child(self.label).child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(self.description),
        )
    }
}

type FormatChoices = SearchableVec<SelectGroup<FormatOption>>;

#[derive(Clone)]
struct QualityOption(Option<u32>);
impl SelectItem for QualityOption {
    type Value = Option<u32>;
    fn title(&self) -> SharedString {
        self.0
            .map(|height| format!("Up to {height}p"))
            .unwrap_or_else(|| "Best available".into())
            .into()
    }
    fn value(&self) -> &Option<u32> {
        &self.0
    }
}

struct Downloader {
    url: Entity<InputState>,
    destination: Entity<InputState>,
    media: Option<Media>,
    inspected_url: Option<String>,
    format_select: Entity<SelectState<FormatChoices>>,
    quality_select: Entity<SelectState<Vec<QualityOption>>>,
    quality_dirty: bool,
    output: Output,
    height: Option<u32>,
    job: Option<Job>,
    generation: u64,
    progress: Progress,
    status: String,
    error: Option<String>,
    completed: Option<PathBuf>,
    dependencies: Option<Vec<(String, bool)>>,
    _subscriptions: Vec<Subscription>,
}

impl Downloader {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let settings = Settings::load();
        let url =
            cx.new(|cx| InputState::new(window, cx).placeholder("Paste a YouTube video link"));
        let destination = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("Destination folder");
            input.set_value(
                settings.destination.to_string_lossy().into_owned(),
                window,
                cx,
            );
            input
        });
        let format_select = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(vec![
                    SelectGroup::new("Video").items([
                        FormatOption {
                            output: Output::Video,
                            label: "MKV",
                            description: "Best available codecs · no conversion",
                        },
                        FormatOption {
                            output: Output::Mp4,
                            label: "MP4",
                            description: "H.264 + AAC · widely compatible video",
                        },
                    ]),
                    SelectGroup::new("Audio").items([
                        FormatOption {
                            output: Output::Audio,
                            label: "Original audio",
                            description: "Preserve source quality · file type varies",
                        },
                        FormatOption {
                            output: Output::Mp3,
                            label: "MP3",
                            description: "Widely compatible audio · converted from source",
                        },
                    ]),
                ]),
                Some(IndexPath::default()),
                window,
                cx,
            )
        });
        let quality_select = cx.new(|cx| {
            SelectState::new(
                vec![QualityOption(None)],
                Some(IndexPath::default()),
                window,
                cx,
            )
        });
        let subscriptions = vec![
            cx.subscribe(&format_select, |this, _, event, cx| {
                if let SelectEvent::Confirm(Some(output)) = event {
                    this.output = *output;
                    cx.notify();
                }
            }),
            cx.subscribe(&quality_select, |this, _, event, cx| {
                if let SelectEvent::Confirm(Some(height)) = event {
                    this.height = *height;
                    cx.notify();
                }
            }),
            cx.subscribe_in(&url, window, |this, _, event, _, cx| match event {
                InputEvent::Change if this.job.is_none() => {
                    let current = download::normalize_url(&this.url.read(cx).value()).ok();
                    if current != this.inspected_url {
                        this.media = None;
                        this.inspected_url = None;
                        this.completed = None;
                        this.error = None;
                        this.status = "Ready for a link".into();
                    }
                    cx.notify();
                }
                InputEvent::PressEnter { .. } if this.job.is_none() => this.inspect(cx),
                _ => {}
            }),
            cx.on_app_quit(|this, _| {
                this.job.take(); // Drop cancels the whole child process group.
                async {}
            }),
        ];
        let mut this = Self {
            url,
            destination,
            media: None,
            inspected_url: None,
            format_select,
            quality_select,
            quality_dirty: false,
            output: Output::Video,
            height: None,
            job: None,
            generation: 0,
            progress: Progress::default(),
            status: "Ready for a link".into(),
            error: None,
            completed: None,
            dependencies: None,
            _subscriptions: subscriptions,
        };
        this.check_dependencies(cx);
        this
    }

    fn check_dependencies(&mut self, cx: &mut Context<Self>) {
        self.dependencies = None;
        let task = cx
            .background_executor()
            .spawn(async { download::dependencies() });
        cx.spawn(async move |this, cx| {
            let dependencies = task.await;
            let _ = this.update(cx, |this, cx| {
                this.dependencies = Some(dependencies);
                cx.notify();
            });
        })
        .detach();
    }

    fn inspect(&mut self, cx: &mut Context<Self>) {
        if self.job.is_some() {
            return;
        }
        match download::normalize_url(&self.url.read(cx).value()) {
            Ok(url) => {
                self.media = None;
                self.inspected_url = Some(url.clone());
                self.status = "Reading video details…".into();
                self.start(Operation::Inspect { url }, cx);
            }
            Err(error) => {
                self.error = Some(error.to_string());
                cx.notify();
            }
        }
    }

    fn download(&mut self, cx: &mut Context<Self>) {
        if self.job.is_some() || self.media.is_none() {
            return;
        }
        let Some(url) = self.inspected_url.clone() else {
            return;
        };
        let destination = PathBuf::from(self.destination.read(cx).value().as_str());
        if !destination.is_dir() {
            self.error = Some("Choose an existing folder before downloading.".into());
            cx.notify();
            return;
        }
        if let Err(error) = (Settings {
            destination: destination.clone(),
        })
        .save()
        {
            // Saving a preference must not stop a download.
            eprintln!("Could not save destination preference: {error:#}");
        }
        self.status = "Starting download…".into();
        self.start(
            Operation::Download {
                url,
                destination,
                output: self.output,
                height: self.height,
            },
            cx,
        );
    }

    fn start(&mut self, operation: Operation, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        let job = Job::start(operation);
        let events = job.events.clone();
        self.job = Some(job);
        self.error = None;
        self.completed = None;
        self.progress = Progress::default();
        cx.spawn(async move |this, cx| {
            while let Ok(event) = events.recv().await {
                let terminal = !matches!(event, Event::Progress(_) | Event::Processing);
                if this
                    .update(cx, |this, cx| {
                        if this.generation != generation {
                            return;
                        }
                        this.apply_event(event);
                        if terminal {
                            this.job.take();
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
                if terminal {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }

    fn apply_event(&mut self, event: Event) {
        match event {
            Event::Progress(progress) => {
                self.status = if progress.status == "finished" {
                    "Stream received · preparing output…".into()
                } else {
                    "Downloading…".into()
                };
                self.progress = progress;
            }
            Event::Processing => {
                self.status = "Processing audio and video…".into();
            }
            Event::Inspected(media) => {
                self.media = Some(media);
                self.height = None;
                self.quality_dirty = true;
                self.status = "Choose a format and download".into();
            }
            Event::Completed(path) => {
                self.status = "Saved to your folder".into();
                self.completed = Some(path);
            }
            Event::Failed(error) => {
                self.status = "Could not finish".into();
                self.error = Some(error);
            }
            Event::Cancelled => {
                self.status = "Cancelled · partial downloads are kept for retry".into();
            }
        }
    }

    fn choose_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Save downloads here".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = prompt.await;
            let _ = this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(Ok(Some(paths))) => {
                        if let Some(path) = paths.first() {
                            this.destination.update(cx, |input, cx| {
                                input.set_value(path.to_string_lossy().into_owned(), window, cx);
                            });
                        }
                    }
                    Ok(Ok(None)) => {}
                    _ => {
                        this.error = Some(
                            "The folder picker could not open. Type a folder path instead.".into(),
                        )
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}

impl Render for Downloader {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.quality_dirty {
            let mut items = vec![QualityOption(None)];
            if let Some(media) = &self.media {
                items.extend(
                    media
                        .heights()
                        .into_iter()
                        .map(|height| QualityOption(Some(height))),
                );
            }
            self.quality_select.update(cx, |select, cx| {
                select.set_items(items, window, cx);
                select.set_selected_value(&None, window, cx);
            });
            self.quality_dirty = false;
        }
        let busy = self.job.is_some();
        let muted = cx.theme().muted_foreground;
        let border = cx.theme().border;
        let dependencies_ready = self
            .dependencies
            .as_ref()
            .is_some_and(|tools| tools.iter().all(|(_, ready)| *ready));
        let inspector_ready = self.dependencies.as_ref().is_some_and(|tools| {
            tools
                .iter()
                .filter(|(name, _)| name == "yt-dlp" || name == "deno")
                .all(|(_, ready)| *ready)
        });
        let mut content = v_flex()
            .gap_5()
            .w_full()
            .max_w(px(880.))
            .mx_auto()
            .p_6()
            .child(
                div()
                    .text_3xl()
                    .font_weight(FontWeight::BOLD)
                    .child("YouTube downloader"),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(div().text_sm().child("Video link"))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                div().flex_1().min_w_0().child(
                                    Input::new(&self.url)
                                        .aria_label("YouTube video link")
                                        .disabled(busy),
                                ),
                            )
                            .child(
                                Button::new("inspect")
                                    .label("Inspect")
                                    .primary()
                                    .disabled(busy || !inspector_ready)
                                    .on_click(cx.listener(|this, _, _, cx| this.inspect(cx))),
                            ),
                    ),
            );

        if let Some(tools) = &self.dependencies {
            let missing: Vec<_> = tools
                .iter()
                .filter(|(_, ready)| !ready)
                .map(|(name, _)| name.as_str())
                .collect();
            if !missing.is_empty() {
                content =
                    content.child(
                        v_flex()
                            .gap_2()
                            .p_3()
                            .border_1()
                            .border_color(border)
                            .rounded_md()
                            .child(format!(
                                "Install these tools to continue: {}",
                                missing.join(", ")
                            ))
                            .child(div().text_sm().text_color(muted).child(
                                "Follow the setup instructions in README.md, then check again.",
                            ))
                            .child(
                                Button::new("check-tools")
                                    .label("Check again")
                                    .disabled(busy)
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.check_dependencies(cx)),
                                    ),
                            ),
                    );
            }
        } else {
            content = content.child(
                div()
                    .text_sm()
                    .text_color(muted)
                    .child("Checking download tools…"),
            );
        }

        if let Some(media) = &self.media {
            let duration = media
                .duration
                .map(format_time)
                .unwrap_or_else(|| "Duration unknown".into());
            let description = format!(
                "{}  ·  {}",
                media.channel.as_deref().unwrap_or("YouTube"),
                duration
            );
            content = content.child(
                v_flex()
                    .gap_1()
                    .p_4()
                    .rounded_md()
                    .bg(cx.theme().muted)
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(media.title.clone()),
                    )
                    .child(div().text_sm().text_color(muted).child(description)),
            );
        }

        let description = match self.output {
            Output::Video => "Best available codecs, combined into MKV without re-encoding.",
            Output::Mp4 => {
                "H.264 video + AAC audio for compatibility. Some resolutions may be unavailable."
            }
            Output::Audio => {
                "Keep the source audio quality. The file type depends on the available stream."
            }
            Output::Mp3 => "Convert to MP3 at high variable quality. Conversion may take a moment.",
        };
        let mut options = v_flex()
            .gap_2()
            .child(div().text_sm().child("Save as"))
            .child(
                Select::new(&self.format_select)
                    .accessibility_label("Save as")
                    .w_full()
                    .menu_max_h(px(340.))
                    .disabled(busy),
            )
            .child(div().text_sm().text_color(muted).child(description));
        if self.output.is_video() && self.media.is_some() {
            options = options
                .child(div().pt_2().text_sm().child("Video quality"))
                .child(
                    Select::new(&self.quality_select)
                        .accessibility_label("Video quality")
                        .w_full()
                        .disabled(busy),
                );
        }
        content = content
            .child(options)
            .child(
                v_flex()
                    .gap_2()
                    .child(div().text_sm().child("Save to"))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                div().flex_1().min_w_0().child(
                                    Input::new(&self.destination)
                                        .aria_label("Destination folder")
                                        .disabled(busy),
                                ),
                            )
                            .child(
                                Button::new("browse")
                                    .label("Browse…")
                                    .disabled(busy)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.choose_folder(window, cx)
                                    })),
                            ),
                    ),
            )
            .child(
                h_flex()
                    .gap_3()
                    .child(
                        Button::new("download")
                            .label(if self.completed.is_some() {
                                "Download again"
                            } else {
                                "Download"
                            })
                            .primary()
                            .disabled(busy || self.media.is_none() || !dependencies_ready)
                            .on_click(cx.listener(|this, _, _, cx| this.download(cx))),
                    )
                    .when(busy, |row| {
                        row.child(Button::new("cancel").label("Cancel").on_click(cx.listener(
                            |this, _, _, cx| {
                                if let Some(job) = &this.job {
                                    job.cancel();
                                }
                                this.status = "Cancelling…".into();
                                cx.notify();
                            },
                        )))
                    }),
            );

        let mut status = v_flex()
            .gap_2()
            .pt_4()
            .border_t_1()
            .border_color(border)
            .child(div().text_sm().child(self.status.clone()));
        if busy && self.media.is_some() {
            if let Some(fraction) = self.progress.fraction() {
                status = status.child(
                    ProgressBar::new("download-progress")
                        .accessibility_label("Download progress")
                        .value(fraction * 100.),
                );
            } else {
                status = status.child(
                    ProgressBar::new("download-progress")
                        .accessibility_label("Waiting for download progress")
                        .loading(true),
                );
            }
            let received = self
                .progress
                .downloaded_bytes
                .map(format_bytes)
                .unwrap_or_else(|| "Waiting for data".into());
            let speed = self
                .progress
                .speed
                .map(|speed| format!("{} / s", format_bytes(speed)))
                .unwrap_or_default();
            let eta = self
                .progress
                .eta
                .map(|eta| format!("{} remaining", format_time(eta)))
                .unwrap_or_default();
            status = status.child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(format!("{received}    {speed}    {eta}")),
            );
        }
        if let Some(path) = &self.completed {
            let file = path.clone();
            let folder = path.parent().unwrap_or(path).to_owned();
            status = status
                .child(div().text_sm().child(path.to_string_lossy().into_owned()))
                .child(
                    h_flex()
                        .gap_2()
                        .child(Button::new("open-file").label("Open file").on_click(
                            move |_, _, cx| {
                                if let Ok(url) = url::Url::from_file_path(&file) {
                                    cx.open_url(url.as_str());
                                }
                            },
                        ))
                        .child(Button::new("open-folder").label("Open folder").on_click(
                            move |_, _, cx| {
                                if let Ok(url) = url::Url::from_file_path(&folder) {
                                    cx.open_url(url.as_str());
                                }
                            },
                        )),
                );
        }
        if let Some(error) = &self.error {
            status = status.child(v_flex().gap_2().p_3().rounded_md().bg(cx.theme().muted)
                .child(div().text_sm().text_color(cx.theme().danger).child(error.clone()))
                .child(div().text_sm().text_color(muted)
                    .child("If YouTube extraction failed, update yt-dlp and retry. Sign-in, private videos, and live recording are not supported yet.")));
        }
        content = content.child(status);
        div()
            .id("downloader-scroll")
            .size_full()
            .overflow_y_scroll()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(content)
    }
}

fn format_time(seconds: f64) -> String {
    let seconds = seconds.max(0.) as u64;
    if seconds >= 3600 {
        format!(
            "{}:{:02}:{:02}",
            seconds / 3600,
            seconds / 60 % 60,
            seconds % 60
        )
    } else {
        format!("{}:{:02}", seconds / 60, seconds % 60)
    }
}

fn format_bytes(bytes: f64) -> String {
    if bytes >= 1_048_576. {
        format!("{:.1} MiB", bytes / 1_048_576.)
    } else {
        format!("{:.0} KiB", bytes / 1024.)
    }
}

fn main() {
    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(|cx| {
            gpui_kit::init(cx);
            Theme::change(ThemeMode::Dark, None, cx);
            let theme = Theme::global_mut(cx);
            theme.colors.primary = rgb(0x2563eb).into();
            theme.colors.primary_hover = rgb(0x1d4ed8).into();
            theme.colors.primary_active = rgb(0x1e40af).into();
            theme.colors.primary_foreground = rgb(0xffffff).into();
            theme.colors.button_primary = theme.colors.primary;
            theme.colors.button_primary_hover = theme.colors.primary_hover;
            theme.colors.button_primary_active = theme.colors.primary_active;
            theme.colors.button_primary_foreground = theme.colors.primary_foreground;
            theme.colors.progress_bar = rgb(0x60a5fa).into();
            theme.colors.ring = rgb(0x60a5fa).into();
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
                        app_id: Some("misc-tools-youtube-downloader".into()),
                        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                            point(px(120.), px(100.)),
                            size(px(850.), px(780.)),
                        ))),
                        titlebar: Some(TitlebarOptions {
                            title: Some("YouTube downloader · misc-tools".into()),
                            ..Default::default()
                        }),
                        window_min_size: Some(size(px(560.), px(520.))),
                        ..Default::default()
                    },
                    |window, cx| {
                        let view = cx.new(|cx| Downloader::new(window, cx));
                        cx.new(|cx| Root::new(view, window, cx))
                    },
                )
                .expect("Could not open the downloader window");
            })
            .detach();
        });
}
