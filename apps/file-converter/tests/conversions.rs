// Real-engine checks are explicit so the default suite needs no external codecs.
use file_converter::{Cancellation, Request, convert};
use std::{fs, path::Path, process::Command};
fn convert_to(source: &Path, target: &str) -> anyhow::Result<std::path::PathBuf> {
    convert(
        &Request {
            source: source.into(),
            target: target.into(),
            destination: None,
        },
        &Cancellation::default(),
    )
}
fn run(program: &str, args: &[&str]) {
    let output = Command::new(program).args(args).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
#[test]
#[ignore = "Requires ImageMagick with image codecs"]
fn image_formats_transparency_and_existing_names() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let source = dir.path().join("-photo [1].png");
    run(
        "magick",
        &["-size", "32x24", "xc:none", source.to_str().unwrap()],
    );
    let jpg = convert_to(&source, "jpg")?;
    let pixel = Command::new("magick")
        .arg(&jpg)
        .args(["-format", "%[fx:mean]", "info:"])
        .output()?;
    assert!(pixel.status.success());
    assert_eq!(String::from_utf8_lossy(&pixel.stdout).trim(), "1");
    assert_ne!(convert_to(&source, "jpg")?, jpg);
    assert_ne!(convert_to(&source, "png")?, source);
    for target in ["webp", "avif", "tiff", "bmp", "gif", "ico", "pdf"] {
        assert!(convert_to(&source, target)?.is_file(), "{target}");
    }
    Ok(())
}
#[test]
#[ignore = "Requires FFmpeg with common audio and video encoders"]
fn audio_video_and_missing_audio() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let audio = dir.path().join("tone.wav");
    run(
        "ffmpeg",
        &[
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=0.2",
            audio.to_str().unwrap(),
        ],
    );
    for target in ["mp3", "m4a", "flac", "wav", "ogg", "opus"] {
        assert!(convert_to(&audio, target)?.is_file(), "{target}");
    }
    let video = dir.path().join("silent.mkv");
    run(
        "ffmpeg",
        &[
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=blue:size=32x24:duration=0.2",
            "-c:v",
            "ffv1",
            video.to_str().unwrap(),
        ],
    );
    for target in ["mp4", "mkv", "webm", "mov"] {
        assert!(convert_to(&video, target)?.is_file(), "{target}");
    }
    assert!(convert_to(&video, "mp3").is_err());
    assert!(!dir.path().join("silent.mp3").exists());
    Ok(())
}
#[test]
#[ignore = "Requires LibreOffice and Poppler"]
fn office_and_pdf() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let source = dir.path().join("letter.rtf");
    fs::write(&source, r"{\rtf1\ansi A conversion test.}")?;
    for target in ["docx", "odt", "rtf", "txt"] {
        assert!(convert_to(&source, target)?.is_file(), "{target}");
    }
    let pdf = convert_to(&source, "pdf")?;
    assert!(fs::read_to_string(convert_to(&pdf, "txt")?)?.contains("conversion test"));
    for target in ["png", "jpg"] {
        assert!(convert_to(&pdf, target)?.is_file());
    }
    let sheet = dir.path().join("sheet.csv");
    fs::write(&sheet, "Name,Count\nHello,42\n")?;
    for target in ["xlsx", "ods", "pdf", "csv"] {
        assert!(convert_to(&sheet, target)?.is_file());
    }
    Ok(())
}

#[test]
#[ignore = "Requires Pandoc"]
fn text_and_ebook_formats() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let source = dir.path().join("notes.md");
    fs::write(
        &source,
        "# Conversion test\n\nA paragraph with **bold** text.\n",
    )?;
    for target in ["html", "docx", "odt", "md", "txt", "epub"] {
        assert!(convert_to(&source, target)?.is_file(), "{target}");
    }
    let epub = convert_to(&source, "epub")?;
    let html = convert_to(&epub, "html")?;
    assert!(fs::read_to_string(html)?.contains("Conversion test"));
    fs::write(
        dir.path().join("picture.svg"),
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="10" height="10" fill="red"/></svg>"#,
    )?;
    fs::write(&source, "# Picture\n\n![Sample](picture.svg)\n")?;
    let html = convert_to(&source, "html")?;
    assert!(fs::read_to_string(html)?.contains("data:image/svg+xml"));
    Ok(())
}
