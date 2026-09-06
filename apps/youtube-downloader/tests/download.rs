#![cfg(unix)]

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};
use tempfile::TempDir;
use youtube_downloader::download::{Event, Job, Operation, Output, Progress, normalize_url};

const URL: &str = "https://www.youtube.com/watch?v=BaW_jenozKc";

fn fake(script: &str) -> (TempDir, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let executable = directory.path().join("fake-yt-dlp");
    fs::write(&executable, format!("#!/usr/bin/env python3\n{script}\n")).unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
    (directory, executable)
}

fn inspect(executable: PathBuf) -> Job {
    Job::with_executable(Operation::Inspect { url: URL.into() }, executable)
}

fn terminal(job: &Job) -> (Event, Vec<Event>) {
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut progress = Vec::new();
    loop {
        assert!(Instant::now() < deadline, "worker did not terminate");
        match job.events.try_recv() {
            Ok(event) if matches!(event, Event::Progress(_) | Event::Processing) => {
                progress.push(event)
            }
            Ok(event) => return (event, progress),
            Err(async_channel::TryRecvError::Empty) => thread::sleep(Duration::from_millis(10)),
            Err(error) => panic!("channel closed without terminal event: {error}"),
        }
    }
}

#[test]
fn only_single_video_urls_are_canonicalized() {
    for input in [
        URL,
        "https://youtu.be/BaW_jenozKc?t=3",
        "https://www.youtube.com/shorts/BaW_jenozKc",
        "https://m.youtube.com/watch?v=BaW_jenozKc&list=playlist",
    ] {
        assert_eq!(normalize_url(input).unwrap(), URL);
    }
    for input in [
        "--exec=whoami",
        "file:///etc/passwd",
        "https://youtube.com/playlist?list=x",
        "https://youtube.com/@channel",
        "https://youtube.com.evil.test/watch?v=BaW_jenozKc",
        "https://youtube.com/watch?v=abc",
        "https://evil@youtube.com/watch?v=BaW_jenozKc",
    ] {
        assert!(normalize_url(input).is_err(), "accepted {input}");
    }
}

#[test]
fn output_arguments_keep_paths_literal_and_select_real_codecs() {
    let args = Operation::Download {
        url: URL.into(),
        destination: PathBuf::from("/tmp/a $(echo nope) ' folder"),
        output: Output::Mp4,
        height: Some(720),
    }
    .args()
    .unwrap();
    assert_eq!(
        args.windows(2).find(|pair| pair[0] == "--paths").unwrap()[1],
        "."
    );
    let format = args.windows(2).find(|pair| pair[0] == "--format").unwrap()[1]
        .to_str()
        .unwrap();
    assert!(
        format.contains("vcodec^=avc1")
            && format.contains("height<=720")
            && format.contains("acodec^=mp4a")
    );
    assert_eq!(args[args.len() - 2], "--");
    assert_eq!(args[args.len() - 1], URL);
}

#[test]
fn inspection_drains_stderr_without_deadlock() {
    let (_directory, executable) = fake(
        "import sys, json\nfor _ in range(1500): print('warning' * 100, file=sys.stderr)\nprint(json.dumps({'title':'A real title', 'formats':[{'height':720,'vcodec':'avc1'},{'height':720,'vcodec':'vp9'},{'height':None,'vcodec':'none'}]}))",
    );
    let (event, _) = terminal(&inspect(executable));
    match event {
        Event::Inspected(media) => {
            assert_eq!(media.title, "A real title");
            assert_eq!(media.heights(), vec![720]);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn invalid_metadata_and_live_videos_fail_usefully() {
    for (script, expected) in [
        ("print('{broken')", "metadata"),
        ("print('{\"title\":\"Live\",\"is_live\":true}')", "Live"),
        (
            "print('{\"title\":\"Playlist\",\"_type\":\"playlist\"}')",
            "single video",
        ),
    ] {
        let (_directory, executable) = fake(script);
        assert!(
            matches!(terminal(&inspect(executable)).0, Event::Failed(message) if message.contains(expected))
        );
    }
}

#[test]
fn stream_completion_does_not_hide_a_later_failure() {
    let (_directory, executable) = fake(
        "import sys\nprint('PROGRESS:{\"status\":\"finished\",\"downloaded_bytes\":100,\"total_bytes\":100}')\nprint('ERROR: destination is full',file=sys.stderr)\nsys.exit(1)",
    );
    let (event, progress) = terminal(&inspect(executable));
    assert_eq!(progress.len(), 1);
    assert!(matches!(event, Event::Failed(message) if message.contains("destination is full")));
}

#[test]
fn final_path_survives_immediate_exit_and_unusual_characters() {
    let (directory, executable) = fake(
        "import pathlib, json\np = pathlib.Path.cwd() / 'quotes \\\" café\\nvideo.mkv'\np.write_bytes(b'video')\nprint('PROGRESS:{\"status\":\"finished\"}')\nprint('PROCESSING:{}')\nprint('FILE:' + json.dumps(str(p)))",
    );
    let job = Job::with_executable(
        Operation::Download {
            url: URL.into(),
            destination: directory.path().into(),
            output: Output::Video,
            height: None,
        },
        executable,
    );
    let (event, progress) = terminal(&job);
    assert_eq!(progress.len(), 2);
    assert!(
        matches!(event, Event::Completed(path) if path.is_file() && path.to_string_lossy().contains("café\n"))
    );
}

#[test]
fn successful_exit_without_an_existing_output_is_not_success() {
    for script in [
        "print('nothing')",
        "print('FILE:\"/no/such/output/file.mkv\"')",
    ] {
        let (directory, executable) = fake(script);
        let job = Job::with_executable(
            Operation::Download {
                url: URL.into(),
                destination: directory.path().into(),
                output: Output::Video,
                height: None,
            },
            executable,
        );
        assert!(matches!(terminal(&job).0, Event::Failed(_)));
    }
}

#[test]
fn cancellation_and_drop_kill_descendants() {
    for drop_job in [false, true] {
        let (directory, executable) = fake(
            "import subprocess,pathlib,time\nchild=subprocess.Popen(['sleep','30'])\npathlib.Path(__file__).with_name('child.pid').write_text(str(child.pid))\ntime.sleep(30)",
        );
        let job = inspect(executable);
        let pid_file = directory.path().join("child.pid");
        let deadline = Instant::now() + Duration::from_secs(5);
        while !pid_file.exists() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        }
        let pid = fs::read_to_string(pid_file).unwrap();
        if drop_job {
            drop(job);
        } else {
            job.cancel();
            assert!(matches!(terminal(&job).0, Event::Cancelled));
        }
        loop {
            let stat = fs::read_to_string(format!("/proc/{pid}/stat"));
            if stat.is_err() || stat.unwrap().split_whitespace().nth(2) == Some("Z") {
                break;
            }
            assert!(Instant::now() < deadline, "descendant still alive");
            thread::sleep(Duration::from_millis(10));
        }
    }
}

#[test]
fn unknown_totals_are_indeterminate_and_estimates_are_supported() {
    assert_eq!(Progress::default().fraction(), None);
    let mut progress = Progress {
        downloaded_bytes: Some(50.),
        total_bytes_estimate: Some(100.),
        ..Default::default()
    };
    assert_eq!(progress.fraction(), Some(0.5));
    progress.total_bytes = Some(0.);
    assert_eq!(progress.fraction(), None);
}

#[test]
fn parent_exit_cleans_up_descendants_holding_output_pipes() {
    let (directory, executable) = fake(
        "import subprocess,pathlib\nchild=subprocess.Popen(['sleep','30'])\npathlib.Path(__file__).with_name('child.pid').write_text(str(child.pid))\nprint('{\"title\":\"Complete\"}')",
    );
    let job = inspect(executable);
    assert!(matches!(terminal(&job).0, Event::Inspected(_)));
    let pid = fs::read_to_string(directory.path().join("child.pid")).unwrap();
    let stat = fs::read_to_string(format!("/proc/{pid}/stat"));
    assert!(stat.is_err() || stat.unwrap().split_whitespace().nth(2) == Some("Z"));
}

#[test]
fn destination_variables_and_trailing_spaces_are_preserved() {
    let (directory, executable) = fake(
        "import pathlib,json,sys\nassert sys.argv[sys.argv.index('--paths')+1] == '.'\np=pathlib.Path('video.mkv')\np.write_bytes(b'video')\nprint('FILE:'+json.dumps(str(p)))",
    );
    let destination = directory.path().join("literal $HOME folder ");
    fs::create_dir(&destination).unwrap();
    let job = Job::with_executable(
        Operation::Download {
            url: URL.into(),
            destination: destination.clone(),
            output: Output::Video,
            height: None,
        },
        executable,
    );
    assert!(
        matches!(terminal(&job).0, Event::Completed(path) if path == destination.join("video.mkv"))
    );
    assert!(destination.join("video.mkv").is_file());
}

#[test]
fn cancelled_conversion_stays_private_and_retry_discards_truncated_output() {
    let (directory, executable) = fake(
        "import pathlib,time\npathlib.Path('video.mp3').write_bytes(b'truncated')\npathlib.Path('video.webm.part').write_bytes(b'resumable')\npathlib.Path(__file__).with_name('started').touch()\ntime.sleep(30)",
    );
    let operation = Operation::Download {
        url: URL.into(),
        destination: directory.path().into(),
        output: Output::Mp3,
        height: None,
    };
    let job = Job::with_executable(operation.clone(), executable.clone());
    let deadline = Instant::now() + Duration::from_secs(5);
    while !directory.path().join("started").exists() {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(10));
    }
    job.cancel();
    assert!(matches!(terminal(&job).0, Event::Cancelled));
    assert!(!directory.path().join("video.mp3").exists());
    fs::write(&executable, "#!/usr/bin/env python3\nimport pathlib\nassert not pathlib.Path('video.mp3').exists()\nassert pathlib.Path('video.webm.part').read_bytes()==b'resumable'\npathlib.Path('video.mp3').write_bytes(b'complete')\nprint('FILE:\"video.mp3\"')\n").unwrap();
    // Simulate a crash leaving another apparently finished conversion behind.
    let stage = directory
        .path()
        .join(".misc-tools-partials/BaW_jenozKc-mp3-best");
    fs::write(stage.join("video.mp3"), b"crashed conversion").unwrap();
    let retry = Job::with_executable(operation, executable);
    assert!(matches!(terminal(&retry).0, Event::Completed(_)));
    assert_eq!(
        fs::read(directory.path().join("video.mp3")).unwrap(),
        b"complete"
    );
}

#[test]
fn publication_never_replaces_an_existing_destination() {
    let (directory, executable) = fake(
        "import pathlib\npathlib.Path('video.mkv').write_bytes(b'new download')\nprint('FILE:\"video.mkv\"')",
    );
    fs::write(directory.path().join("video.mkv"), b"keep original").unwrap();
    let job = Job::with_executable(
        Operation::Download {
            url: URL.into(),
            destination: directory.path().into(),
            output: Output::Video,
            height: None,
        },
        executable,
    );
    assert!(
        matches!(terminal(&job).0, Event::Failed(message) if message.contains("already exists"))
    );
    assert_eq!(
        fs::read(directory.path().join("video.mkv")).unwrap(),
        b"keep original"
    );
}

#[test]
fn concurrent_downloads_cannot_share_the_same_working_files() {
    let (directory, executable) = fake(
        "import pathlib,time\npathlib.Path(__file__).with_name('started').touch()\ntime.sleep(30)",
    );
    let operation = Operation::Download {
        url: URL.into(),
        destination: directory.path().into(),
        output: Output::Video,
        height: None,
    };
    let first = Job::with_executable(operation.clone(), executable.clone());
    let deadline = Instant::now() + Duration::from_secs(5);
    while !directory.path().join("started").exists() {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(10));
    }
    let second = Job::with_executable(operation, executable);
    assert!(
        matches!(terminal(&second).0, Event::Failed(message) if message.contains("another window"))
    );
    first.cancel();
    assert!(matches!(terminal(&first).0, Event::Cancelled));
}

// Runs only as a subprocess of the crash-recovery regression below.
#[test]
#[ignore = "subprocess fixture"]
fn crash_fixture_helper() {
    let destination = PathBuf::from(std::env::var_os("MISC_TEST_DESTINATION").unwrap());
    let executable = PathBuf::from(std::env::var_os("MISC_TEST_EXECUTABLE").unwrap());
    let _job = Job::with_executable(
        Operation::Download {
            url: URL.into(),
            destination: destination.clone(),
            output: Output::Video,
            height: None,
        },
        executable,
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while !destination.join("writer.pid").exists() {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(10));
    }
    // Simulate GUI death: no destructors and no process-group cleanup.
    unsafe {
        libc::_exit(0);
    }
}

#[test]
fn supervisor_crash_does_not_unlock_a_still_running_downloader() {
    let (directory, executable) = fake(
        "import pathlib,os,time\npathlib.Path(__file__).with_name('writer.pid').write_text(str(os.getpid()))\ntime.sleep(30)",
    );
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "crash_fixture_helper"])
        .env("MISC_TEST_DESTINATION", directory.path())
        .env("MISC_TEST_EXECUTABLE", &executable)
        .stdout(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(status.success());
    let pid: i32 = fs::read_to_string(directory.path().join("writer.pid"))
        .unwrap()
        .parse()
        .unwrap();
    let retry = Job::with_executable(
        Operation::Download {
            url: URL.into(),
            destination: directory.path().into(),
            output: Output::Video,
            height: None,
        },
        executable,
    );
    let (event, _) = terminal(&retry);
    unsafe {
        libc::kill(-pid, libc::SIGKILL);
    }
    assert!(matches!(event, Event::Failed(message) if message.contains("another window")));
}
