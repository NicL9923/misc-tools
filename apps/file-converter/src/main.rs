use file_converter::{self as engine, Event, Job, Kind, Request};
use gpui_kit::component::{
    button::{Button, ButtonVariants},
    input::{Input, InputState},
    select::{Select, SelectEvent, SelectState},
    *,
};
use gpui_kit::*;
use std::path::PathBuf;

struct Row {
    source: PathBuf,
    kind: Option<Kind>,
    select: Entity<SelectState<Vec<SharedString>>>,
    target: String,
    status: String,
    saved: Option<PathBuf>,
    _subscription: Subscription,
}

struct Converter {
    rows: Vec<Row>,
    destination: Entity<InputState>,
    job: Option<Job>,
    status: String,
    tools: Vec<(&'static str, bool)>,
    _quit: Subscription,
}

impl Converter {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let destination = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Leave blank to save beside each original")
        });
        let mut app = Self {
            rows: vec![],
            destination,
            job: None,
            status: "Add files, choose their formats, then convert.".into(),
            tools: [
                "magick",
                "ffmpeg",
                "libreoffice",
                "pandoc",
                "pdftotext",
                "pdftoppm",
            ]
            .into_iter()
            .map(|tool| (tool, engine::available(tool)))
            .collect(),
            _quit: cx.on_app_quit(|this, _| {
                this.job.take();
                async {}
            }),
        };
        app.add(
            std::env::args_os().skip(1).map(PathBuf::from).collect(),
            window,
            cx,
        );
        app
    }

    fn add(&mut self, paths: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        if self.job.is_some() {
            return;
        }
        for source in paths {
            let source = source.canonicalize().unwrap_or(source);
            if self.rows.iter().any(|row| row.source == source) {
                continue;
            }
            let kind = engine::kind(&source);
            let choices: Vec<SharedString> = kind
                .map(engine::targets)
                .unwrap_or(&[])
                .iter()
                .map(|s| SharedString::from(*s))
                .collect();
            let target = choices.first().map(|s| s.to_string()).unwrap_or_default();
            let select =
                cx.new(|cx| SelectState::new(choices, Some(IndexPath::default()), window, cx));
            let path = source.clone();
            let subscription = cx.subscribe(&select, move |this, _, event, cx| {
                if let SelectEvent::Confirm(Some(value)) = event
                    && let Some(row) = this.rows.iter_mut().find(|row| row.source == path)
                {
                    row.target = value.to_string();
                    row.saved = None;
                    row.status = "Ready".into();
                    cx.notify();
                }
            });
            self.rows.push(Row {
                source,
                kind,
                select,
                target,
                status: if kind.is_some() {
                    "Ready"
                } else {
                    "Unsupported file type"
                }
                .into(),
                saved: None,
                _subscription: subscription,
            });
        }
        cx.notify();
    }

    fn pick(&mut self, folder: bool, window: &mut Window, cx: &mut Context<Self>) {
        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: !folder,
            directories: folder,
            multiple: !folder,
            prompt: Some(
                if folder {
                    "Save conversions here"
                } else {
                    "Add files to convert"
                }
                .into(),
            ),
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = prompt.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.job.is_some() {
                    return;
                }
                match result {
                    Ok(Ok(Some(paths))) if folder => {
                        if let Some(path) = paths.first() {
                            this.destination.update(cx, |input, cx| {
                                input.set_value(path.to_string_lossy().into_owned(), window, cx)
                            });
                        }
                    }
                    Ok(Ok(Some(paths))) => this.add(paths, window, cx),
                    Ok(Ok(None)) => {}
                    _ => this.status =
                        "Could not open the picker. You can also launch the app with file paths."
                            .into(),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn convert(&mut self, cx: &mut Context<Self>) {
        if self.job.is_some() {
            return;
        }
        let value = self.destination.read(cx).value().to_string();
        let destination = if value.trim().is_empty() {
            None
        } else {
            Some(PathBuf::from(value.trim()))
        };
        if destination.as_ref().is_some_and(|path| !path.is_dir()) {
            self.status = "Choose an existing destination folder.".into();
            cx.notify();
            return;
        }
        let mut requests = vec![];
        for (index, row) in self.rows.iter_mut().enumerate() {
            if row.saved.is_some() {
                continue;
            }
            if let Some(kind) = row.kind {
                let tool = engine::engine(kind, &row.target);
                if !engine::available(tool) {
                    row.status = format!("Install {tool}, then retry");
                    continue;
                }
                row.status = "Queued".into();
                requests.push((
                    index,
                    Request {
                        source: row.source.clone(),
                        target: row.target.clone(),
                        destination: destination.clone(),
                    },
                ));
            }
        }
        if requests.is_empty() {
            self.status =
                "No files are ready. Check the formats and required engines below.".into();
            cx.notify();
            return;
        }
        let (job, receive) = engine::start(requests);
        self.job = Some(job);
        self.status = "Converting locally…".into();
        cx.spawn(async move |this, cx| {
            while let Ok(event) = receive.recv().await {
                let done = matches!(event, Event::Done);
                if this.update(cx, |this, cx| {
                    match event {
                        Event::Started(index) => {
                            this.rows[index].status = "Converting…".into();
                            this.status = format!("Converting file {} of {}…", index + 1, this.rows.len());
                        },
                        Event::Finished(index, result) => {
                            let row = &mut this.rows[index];
                            match result {
                                Ok(path) => {
                                    row.status = format!("Saved: {}", path.display());
                                    row.saved = Some(path);
                                }
                                Err(error) => row.status = error,
                            }
                        }
                        Event::Done => {
                            this.job.take();
                            for row in &mut this.rows {
                                if row.status == "Queued" { row.status = "Cancelled".into(); }
                            }
                            let saved = this.rows.iter().filter(|row| row.saved.is_some()).count();
                            this.status = format!("{saved} of {} files saved. Failed or cancelled files can be retried.", this.rows.len());
                        }
                    }
                    cx.notify();
                }).is_err() { break; }
                if done { break; }
            }
        }).detach();
        cx.notify();
    }
}

impl Render for Converter {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = self.job.is_some();
        let muted = cx.theme().muted_foreground;
        let border = cx.theme().border;
        let mut content =
            v_flex()
                .gap_5()
                .w_full()
                .max_w(px(1000.))
                .mx_auto()
                .p_6()
                .child(
                    div()
                        .text_3xl()
                        .font_weight(FontWeight::BOLD)
                        .child("File converter"),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(muted)
                        .child("Images, media & documents. Converted on your machine."),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .flex_wrap()
                        .child(
                            Button::new("add")
                                .label("Add files")
                                .primary()
                                .disabled(busy)
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.pick(false, window, cx)),
                                ),
                        )
                        .child(
                            Button::new("clear")
                                .label("Clear queue")
                                .disabled(busy || self.rows.is_empty())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.rows.clear();
                                    this.status = "Add files to get started.".into();
                                    cx.notify();
                                })),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(muted)
                                .child(format!("{} files", self.rows.len())),
                        ),
                )
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
                                            .aria_label(
                                                "Destination folder, blank saves beside originals",
                                            )
                                            .disabled(busy),
                                    ),
                                )
                                .child(
                                    Button::new("folder")
                                        .label("Browse")
                                        .disabled(busy)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.pick(true, window, cx)
                                        })),
                                ),
                        )
                        .child(div().text_xs().text_color(muted).child(
                            "Existing files are kept. Duplicate names get a numbered suffix.",
                        )),
                );
        content = content
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("convert")
                            .label(if busy {
                                "Converting…"
                            } else {
                                "Convert / retry"
                            })
                            .primary()
                            .disabled(
                                busy || self
                                    .rows
                                    .iter()
                                    .all(|row| row.kind.is_none() || row.saved.is_some()),
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.convert(cx))),
                    )
                    .child(
                        Button::new("cancel")
                            .label("Cancel")
                            .disabled(!busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(job) = &this.job {
                                    job.cancel();
                                    this.status = "Cancelling… Completed files are kept.".into();
                                    cx.notify();
                                }
                            })),
                    ),
            )
            .child(div().text_sm().child(self.status.clone()));
        if self.rows.is_empty() {
            content = content.child(
                v_flex()
                    .gap_3()
                    .p_6()
                    .border_1()
                    .border_color(border)
                    .rounded_md()
                    .child(div().text_lg().child("What would you like to convert?"))
                    .child(
                        div()
                            .text_sm()
                            .text_color(muted)
                            .child("Images: PNG, JPEG, WebP, HEIC, AVIF, SVG and more"),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(muted)
                            .child("Media: MP4, MOV, MKV, WebM, MP3, WAV, FLAC and more"),
                    )
                    .child(div().text_sm().text_color(muted).child(
                        "Documents: Word, Excel, PowerPoint, OpenDocument, PDF, Markdown and EPUB",
                    ))
                    .child(
                        div()
                            .text_sm()
                            .child("Choose multiple files. Each file gets its own output format."),
                    ),
            );
        }
        for (index, row) in self.rows.iter().enumerate() {
            let mut card = v_flex()
                .gap_2()
                .p_4()
                .border_1()
                .border_color(border)
                .rounded_md()
                .child(
                    div().font_weight(FontWeight::SEMIBOLD).child(
                        row.source
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned(),
                    ),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child(row.source.to_string_lossy().into_owned()),
                );
            let mut controls = h_flex().gap_2().flex_wrap();
            if let Some(kind) = row.kind {
                controls = controls
                    .child(div().text_sm().child("Convert to"))
                    .child(Select::new(&row.select).w(px(145.)).disabled(busy));
                card = card.child(controls).child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child(engine::note(kind, &row.target)),
                );
                let tool = engine::engine(kind, &row.target);
                if !self
                    .tools
                    .iter()
                    .any(|(name, found)| *name == tool && *found)
                {
                    card = card.child(div().text_sm().child(format!(
                        "Requires {tool}. Install it, then click Check engines."
                    )));
                }
            }
            card = card.child(div().text_sm().child(row.status.clone()));
            let mut actions = h_flex().gap_2();
            if let Some(path) = &row.saved {
                let path = path.clone();
                actions = actions.child(
                    Button::new(("open", index))
                        .label("Show in folder")
                        .on_click(move |_, _, cx| {
                            if let Some(parent) = path.parent() {
                                cx.open_url(
                                    url::Url::from_directory_path(parent).unwrap().as_str(),
                                );
                            }
                        }),
                );
            }
            actions = actions.child(
                Button::new(("remove", index))
                    .label("Remove")
                    .disabled(busy)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.rows.remove(index);
                        cx.notify();
                    })),
            );
            content = content.child(card.child(actions));
        }
        content = content
            .child(div().h(px(1.)).bg(border))
            .child(
                div().text_xs().text_color(muted).child(
                    self.tools
                        .iter()
                        .map(|(tool, found)| {
                            format!("{tool}: {}", if *found { "found" } else { "missing" })
                        })
                        .collect::<Vec<_>>()
                        .join("  ·  "),
                ),
            )
            .child(
                Button::new("check")
                    .label("Check engines")
                    .disabled(busy)
                    .on_click(cx.listener(|this, _, _, cx| {
                        for (tool, found) in &mut this.tools {
                            *found = engine::available(tool);
                        }
                        cx.notify();
                    })),
            );
        div()
            .id("converter-scroll")
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
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            cx.spawn(async move |cx| {
                cx.open_window(
                    WindowOptions {
                        app_id: Some("misc-tools-file-converter".into()),
                        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                            point(px(120.), px(100.)),
                            size(px(860.), px(800.)),
                        ))),
                        titlebar: Some(TitlebarOptions {
                            title: Some("File converter · misc-tools".into()),
                            ..Default::default()
                        }),
                        window_min_size: Some(size(px(560.), px(480.))),
                        ..Default::default()
                    },
                    |window, cx| {
                        let view = cx.new(|cx| Converter::new(window, cx));
                        cx.new(|cx| Root::new(view, window, cx))
                    },
                )
                .expect("Could not open File converter");
            })
            .detach();
        });
}
