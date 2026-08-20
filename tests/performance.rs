use std::{
    io::Read,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use editur::agent::controller::{AgentController, Command as AgentCommand, Event};
use serde::Serialize;

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
enum Decision {
    Green,
    Watch,
    Red,
}

#[derive(Serialize)]
struct TimingRecord {
    scenario: &'static str,
    workload: String,
    scale: usize,
    samples: usize,
    median_ms: f64,
    p95_ms: f64,
    max_ms: f64,
    decision: Decision,
    trigger: &'static str,
}

#[derive(Serialize)]
struct MetricRecord {
    scenario: &'static str,
    workload: String,
    scale: usize,
    metric: &'static str,
    value: f64,
    unit: &'static str,
    decision: Decision,
    trigger: &'static str,
}

struct ChildProbe {
    elapsed: Duration,
    peak_rss_kib: Option<u64>,
    timed_out: bool,
}

fn summarize(mut samples: Vec<Duration>) -> (Duration, Duration, Duration) {
    assert!(!samples.is_empty(), "a benchmark needs a sample");
    samples.sort_unstable();
    (
        samples[samples.len() / 2],
        samples[samples.len() * 95 / 100],
        samples[samples.len() - 1],
    )
}

fn emit(record: impl Serialize) {
    println!(
        "BENCH {}",
        serde_json::to_string(&record).expect("benchmark result serializes")
    );
}

fn emit_timing(
    scenario: &'static str,
    workload: impl Into<String>,
    scale: usize,
    samples: Vec<Duration>,
    green_ms: f64,
    red_ms: f64,
    trigger: &'static str,
) {
    let sample_count = samples.len();
    let (median, p95, max) = summarize(samples);
    let p95_ms = p95.as_secs_f64() * 1_000.0;
    emit(TimingRecord {
        scenario,
        workload: workload.into(),
        scale,
        samples: sample_count,
        median_ms: median.as_secs_f64() * 1_000.0,
        p95_ms,
        max_ms: max.as_secs_f64() * 1_000.0,
        decision: if p95_ms <= green_ms {
            Decision::Green
        } else if p95_ms <= red_ms {
            Decision::Watch
        } else {
            Decision::Red
        },
        trigger,
    });
}

fn receive_until(
    controller: &AgentController,
    timeout: Duration,
    mut done: impl FnMut(&Event) -> bool,
) -> Vec<(Instant, Event)> {
    let deadline = Instant::now() + timeout;
    let mut events = Vec::new();
    while Instant::now() < deadline {
        if let Ok(event) = controller.events().recv_timeout(Duration::from_millis(100)) {
            let finished = done(&event);
            events.push((Instant::now(), event));
            if finished {
                return events;
            }
        }
    }
    panic!("timed out waiting for benchmark controller event");
}

fn current_rss_kib(process: u32) -> Option<u64> {
    #[cfg(unix)]
    {
        let output = Command::new("ps")
            .args(["-o", "rss=", "-p", &process.to_string()])
            .output()
            .ok()?;
        output.status.success().then(|| {
            String::from_utf8_lossy(&output.stdout)
                .trim()
                .parse::<u64>()
                .ok()
        })?
    }
    #[cfg(not(unix))]
    {
        let _ = process;
        None
    }
}

fn run_test_child(test: &str, environment: &[(&str, String)], timeout: Duration) -> ChildProbe {
    let mut command = Command::new(std::env::current_exe().expect("benchmark test executable"));
    command
        .args(["--exact", test, "--nocapture"])
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for (name, value) in environment {
        command.env(name, value);
    }
    let started = Instant::now();
    let mut child = command.spawn().expect("benchmark child process");
    let mut peak_rss_kib = None;
    let timed_out = loop {
        if child.try_wait().expect("benchmark child status").is_some() {
            break false;
        }
        peak_rss_kib = peak_rss_kib.max(current_rss_kib(child.id()));
        if started.elapsed() >= timeout {
            child.kill().expect("stop timed-out benchmark child");
            let _ = child.wait();
            break true;
        }
        thread::sleep(Duration::from_millis(5));
    };
    ChildProbe {
        elapsed: started.elapsed(),
        peak_rss_kib,
        timed_out,
    }
}

#[test]
fn fake_agent_can_emit_a_requested_raw_frame_fixture() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_editur-fake-agent"))
        .args(["--raw-frame-bytes", "1024"])
        .stdout(Stdio::piped())
        .spawn()
        .expect("raw-frame fixture");
    let deadline = Instant::now() + Duration::from_secs(2);
    while child
        .try_wait()
        .expect("raw-frame fixture status")
        .is_none()
    {
        if Instant::now() >= deadline {
            child.kill().expect("stop stalled fixture");
            panic!("raw-frame fixture did not exit");
        }
        thread::sleep(Duration::from_millis(5));
    }
    let mut stdout = Vec::new();
    child
        .stdout
        .take()
        .expect("captured stdout")
        .read_to_end(&mut stdout)
        .expect("raw frame output");

    assert!(stdout.len() >= 1024 && stdout.ends_with(b"\n"));
}

#[cfg(unix)]
#[test]
fn benchmark_runner_documents_quick_and_full_profiles() {
    let output = Command::new("bash")
        .arg(format!(
            "{}/scripts/benchmark_hotspots.sh",
            env!("CARGO_MANIFEST_DIR")
        ))
        .arg("--help")
        .output()
        .expect("benchmark runner help");
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(output.status.success() && stdout.contains("--quick") && stdout.contains("--full"));
}

#[test]
fn performance_child_acp_frame() {
    let Some(bytes) = std::env::var("EDITUR_PERF_FRAME_BYTES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
    else {
        return;
    };
    let unterminated = std::env::var_os("EDITUR_PERF_FRAME_UNTERMINATED").is_some();
    let project = tempfile::tempdir().expect("frame benchmark project");
    let controller = AgentController::start_process(
        project.path().to_path_buf(),
        env!("CARGO_BIN_EXE_editur-fake-agent").into(),
        vec![
            if unterminated {
                "--raw-frame-unterminated-bytes"
            } else {
                "--raw-frame-bytes"
            }
            .into(),
            bytes.to_string(),
        ],
    );
    receive_until(&controller, Duration::from_secs(10), |event| {
        matches!(event, Event::ProcessExited { .. })
    });
}

#[test]
fn oversized_acp_frame_is_rejected_at_the_transport_boundary() {
    let project = tempfile::tempdir().expect("frame limit project");
    let controller = AgentController::start_process(
        project.path().to_path_buf(),
        env!("CARGO_BIN_EXE_editur-fake-agent").into(),
        vec!["--raw-frame-bytes".into(), (32 * 1024 * 1024).to_string()],
    );
    let events = receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::ProcessExited { .. })
    });

    assert!(
        events.iter().any(|(_, event)| matches!(
            event,
            Event::ProcessExited { error, .. }
                if error.contains("ACP frame exceeds 25165824 bytes")
        )),
        "unexpected events: {events:?}"
    );
}

#[test]
fn performance_child_controller_drop() {
    let Some(milliseconds) = std::env::var("EDITUR_PERF_STALL_MS").ok() else {
        return;
    };
    let project = tempfile::tempdir().expect("drop benchmark project");
    let controller = AgentController::start_process(
        project.path().to_path_buf(),
        env!("CARGO_BIN_EXE_editur-fake-agent").into(),
        vec!["--stall-before-protocol-ms".into(), milliseconds],
    );
    thread::sleep(Duration::from_millis(50));
    drop(controller);
}

#[test]
fn controller_drop_does_not_wait_for_stalled_initialize() {
    let project = tempfile::tempdir().expect("drop test project");
    let controller = AgentController::start_process(
        project.path().to_path_buf(),
        env!("CARGO_BIN_EXE_editur-fake-agent").into(),
        vec!["--stall-before-protocol-ms".into(), "1000".into()],
    );
    thread::sleep(Duration::from_millis(50));

    let started = Instant::now();
    drop(controller);

    assert!(
        started.elapsed() < Duration::from_millis(250),
        "controller drop blocked for {:?}",
        started.elapsed()
    );
}

#[cfg(unix)]
#[test]
fn dropping_controller_terminates_stalled_agent_process() {
    let project = tempfile::tempdir().expect("shutdown test project");
    let pid_file = project.path().join("agent.pid");
    let controller = AgentController::start_process(
        project.path().to_path_buf(),
        env!("CARGO_BIN_EXE_editur-fake-agent").into(),
        vec![
            "--performance-pid-file".into(),
            pid_file.display().to_string(),
            "--stall-before-protocol-ms".into(),
            "10000".into(),
        ],
    );
    let deadline = Instant::now() + Duration::from_secs(2);
    while !pid_file.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    let pid = std::fs::read_to_string(&pid_file)
        .expect("agent wrote pid")
        .parse::<u32>()
        .expect("valid agent pid");

    drop(controller);

    let deadline = Instant::now() + Duration::from_secs(1);
    while current_rss_kib(pid).is_some() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        current_rss_kib(pid).is_none(),
        "stalled ACP process {pid} survived controller shutdown"
    );
}

#[test]
#[ignore = "manual release-mode performance benchmark"]
fn benchmark_acp_frame_growth() {
    let quick = std::env::var_os("EDITUR_BENCH_QUICK").is_some();
    let scales: &[usize] = if quick {
        &[1024 * 1024, 32 * 1024 * 1024]
    } else {
        &[
            1024 * 1024,
            8 * 1024 * 1024,
            16 * 1024 * 1024,
            32 * 1024 * 1024,
            64 * 1024 * 1024,
        ]
    };
    let mut peaks = Vec::new();
    for &scale in scales {
        let probe = run_test_child(
            "performance_child_acp_frame",
            &[("EDITUR_PERF_FRAME_BYTES", scale.to_string())],
            Duration::from_secs(15),
        );
        let peak = probe.peak_rss_kib.unwrap_or_default();
        peaks.push(peak);
        emit(MetricRecord {
            scenario: "protocol",
            workload: "terminated-acp-frame".into(),
            scale,
            metric: "peak_rss",
            value: peak as f64,
            unit: "KiB",
            decision: if probe.timed_out || peak > 96 * 1024 {
                Decision::Red
            } else if peak > 64 * 1024 {
                Decision::Watch
            } else {
                Decision::Green
            },
            trigger: "oversized frames must disconnect below a 64 MiB process peak",
        });
    }
    let largest = *scales.last().expect("frame scale");
    let rss_growth_kib = peaks
        .last()
        .zip(peaks.first())
        .map_or(0, |(largest, baseline)| largest.saturating_sub(*baseline));
    emit(MetricRecord {
        scenario: "protocol",
        workload: "acp-frame-rss-growth".into(),
        scale: largest,
        metric: "peak_rss_growth",
        value: rss_growth_kib as f64,
        unit: "KiB",
        decision: if rss_growth_kib <= 32 * 1024 {
            Decision::Green
        } else if rss_growth_kib <= 40 * 1024 {
            Decision::Watch
        } else {
            Decision::Red
        },
        trigger: "frame memory growth should stop at a fixed transport cap",
    });
    if !quick {
        let tail_growth_kib = peaks[peaks.len() - 1].saturating_sub(peaks[peaks.len() - 2]);
        emit(MetricRecord {
            scenario: "protocol",
            workload: "acp-frame-post-cap-growth".into(),
            scale: largest,
            metric: "peak_rss_growth_from_32_to_64_mib",
            value: tail_growth_kib as f64,
            unit: "KiB",
            decision: if tail_growth_kib <= 4 * 1024 {
                Decision::Green
            } else if tail_growth_kib <= 8 * 1024 {
                Decision::Watch
            } else {
                Decision::Red
            },
            trigger: "RSS must plateau once frames exceed the transport cap",
        });
    }
    let probe = run_test_child(
        "performance_child_acp_frame",
        &[
            ("EDITUR_PERF_FRAME_BYTES", largest.to_string()),
            ("EDITUR_PERF_FRAME_UNTERMINATED", "1".into()),
        ],
        Duration::from_secs(15),
    );
    emit_timing(
        "protocol",
        "unterminated-acp-frame",
        largest,
        vec![probe.elapsed],
        750.0,
        1_000.0,
        "an unterminated frame must fail within a bounded deadline",
    );
}

#[test]
#[ignore = "manual release-mode performance benchmark"]
fn benchmark_controller_drop() {
    let probe = run_test_child(
        "performance_child_controller_drop",
        &[("EDITUR_PERF_STALL_MS", "2000".into())],
        Duration::from_secs(5),
    );
    let elapsed = if probe.timed_out {
        Duration::from_secs(5)
    } else {
        probe.elapsed
    };
    emit_timing(
        "shutdown",
        "controller-drop-during-stalled-initialize",
        2_000,
        vec![elapsed],
        500.0,
        1_000.0,
        "closing or switching integrations must not block the UI for one second",
    );
}

#[test]
#[ignore = "manual release-mode performance benchmark"]
fn benchmark_agent_event_stream() {
    let project = tempfile::tempdir().expect("stream benchmark project");
    let controller = AgentController::start_process(
        project.path().to_path_buf(),
        env!("CARGO_BIN_EXE_editur-fake-agent").into(),
        Vec::new(),
    );
    receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::SessionReady { .. })
    });
    controller
        .send(AgentCommand::Prompt("memory-stress".into()))
        .expect("start stream benchmark");
    thread::sleep(Duration::from_millis(100));
    let drain_started = Instant::now();
    let events = receive_until(&controller, Duration::from_secs(30), |event| {
        matches!(event, Event::TurnFinished { .. })
    });
    let mut previous = drain_started;
    let gaps = events
        .iter()
        .map(|(received, _)| {
            let gap = received.saturating_duration_since(previous);
            previous = *received;
            gap
        })
        .collect::<Vec<_>>();
    emit_timing(
        "persistence",
        "agent-event-gap-after-saturated-queue",
        events.len(),
        gaps,
        8.0,
        16.7,
        "event persistence and backpressure should not create frame-sized gaps",
    );
    emit_timing(
        "persistence",
        "agent-backlog-drain",
        events.len(),
        vec![drain_started.elapsed()],
        500.0,
        2_000.0,
        "a saturated event backlog should recover promptly",
    );
}
