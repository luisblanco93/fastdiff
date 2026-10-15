use super::*;
use egui_kittest::Harness;
use serde_json::json;
use std::{fs, path::PathBuf, thread, time::Instant};

fn cpu_ticks() -> [u64; 4] {
    let stat = fs::read_to_string("/proc/self/stat").unwrap();
    let fields: Vec<&str> = stat
        .rsplit_once(") ")
        .unwrap()
        .1
        .split_whitespace()
        .collect();
    [11, 12, 13, 14].map(|index| fields[index].parse().unwrap())
}

fn git_calls() -> Vec<String> {
    fs::read_to_string(std::env::var("LIT1_GIT_LOG").unwrap())
        .expect("Git invocation log must be readable after successful startup")
        .lines()
        .map(str::to_owned)
        .collect()
}

fn frame_contains(harness: &Harness<Fastdiff>, marker: &str) -> bool {
    harness
        .output()
        .shapes
        .iter()
        .any(|shape| match &shape.shape {
            egui::Shape::Text(text) => text.galley.job.text.contains(marker),
            _ => false,
        })
}

fn step_until(harness: &mut Harness<Fastdiff>, marker: &str) -> f64 {
    let start = Instant::now();
    loop {
        harness.run_steps(1);
        assert!(
            harness.state().tabs[harness.state().selected_tab]
                .error
                .is_none(),
            "{:?}",
            harness.state().tabs[harness.state().selected_tab].error
        );
        if frame_contains(harness, marker) {
            return start.elapsed().as_secs_f64() * 1000.0;
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "Refresh timed out for {marker}"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
#[ignore = "LIT-1 60-second idle and 30 edit-to-headless-frame measurement"]
fn lit1_refresh_measurement() {
    let fixture = PathBuf::from(std::env::var("LIT1_FIXTURE").unwrap());
    let report = PathBuf::from(std::env::var("LIT1_REPORT").unwrap());
    let mut results = Vec::new();
    for tab_count in [1, 3] {
        let app = Fastdiff {
            tabs: Vec::new(),
            selected_tab: 0,
            preferences: Preferences::default(),
            server: None,
            benchmark: Benchmark::new(false),
            help_open: false,
            fallback_fonts: Default::default(),
            retired_workers: Vec::new(),
        };
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1100.0, 720.0))
            .build_state(
                |context, app| {
                    app.poll(context);
                    app.show(context);
                },
                app,
            );
        let context = harness.ctx.clone();
        for index in 0..tab_count {
            harness.state_mut().open(
                OpenRequest {
                    target: OpenTarget::Repository,
                    repository: fixture
                        .join(format!("repo-{index}"))
                        .to_str()
                        .unwrap()
                        .into(),
                    file: Some(GitPath(b"main.rs".to_vec())),
                    view: View::All,
                    wsl: None,
                    theme: false,
                },
                &context,
            );
            step_until(&mut harness, "edit-000");
        }
        eprintln!(
            "LIT1 {tab_count}-tab initial diff rendered; settling before 60-second idle sample"
        );
        let settle = Instant::now();
        while settle.elapsed() < Duration::from_secs(5) {
            harness.run_steps(1);
            thread::sleep(Duration::from_millis(50));
        }
        let call_start = git_calls().len();
        let cpu_start = cpu_ticks();
        let start = Instant::now();
        let mut frames = 0;
        while start.elapsed() < Duration::from_secs(60) {
            harness.run_steps(1);
            frames += 1;
            thread::sleep(Duration::from_millis(50));
        }
        let elapsed = start.elapsed().as_secs_f64();
        let cpu_end = cpu_ticks();
        let calls = git_calls().into_iter().skip(call_start).collect::<Vec<_>>();
        let ticks = (0..4)
            .map(|index| cpu_end[index] - cpu_start[index])
            .collect::<Vec<_>>();
        results.push(json!({"tabs":tab_count,"selected_tab":harness.state().selected_tab,"elapsed_seconds":elapsed,
            "headless_frames":frames,"git_subprocess_count":calls.len(),"git_calls":calls,
            "cpu_ticks_user_system_children_user_children_system":ticks}));
        drop(harness);
    }
    let app = Fastdiff {
        tabs: Vec::new(),
        selected_tab: 0,
        preferences: Preferences::default(),
        server: None,
        benchmark: Benchmark::new(false),
        help_open: false,
        fallback_fonts: Default::default(),
        retired_workers: Vec::new(),
    };
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1100.0, 720.0))
        .build_state(
            |context, app| {
                app.poll(context);
                app.show(context);
            },
            app,
        );
    let context = harness.ctx.clone();
    harness.state_mut().open(
        OpenRequest {
            target: OpenTarget::Repository,
            repository: fixture.join("repo-0").to_str().unwrap().into(),
            file: Some(GitPath(b"main.rs".to_vec())),
            view: View::All,
            wsl: None,
            theme: false,
        },
        &context,
    );
    step_until(&mut harness, "edit-000");
    let mut latencies = Vec::new();
    for index in 1..=30 {
        let marker = format!("edit-{index:03}");
        let path = fixture.join("repo-0/main.rs");
        let start = Instant::now();
        fs::write(&path, format!("fn main() {{ println!(\"{marker}\"); }}\n")).unwrap();
        step_until(&mut harness, &marker);
        latencies.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    let mut sorted = latencies.clone();
    sorted.sort_by(f64::total_cmp);
    let p95 = sorted[(sorted.len() as f64 * 0.95).ceil() as usize - 1];
    fs::write(report, serde_json::to_string_pretty(&json!({
        "platform":"Linux WSL2; headless egui_kittest app+native worker; no helper",
        "idle_frame_period_ms":50,"edit_poll_period_ms":10,
        "idle_samples":results,"edit_to_headless_frame_ms":latencies,"edit_to_headless_frame_p95_ms":p95,
        "limitations":["Headless CPU includes fixed-cadence harness work; not native GUI CPU", "Git shim adds identical before/after Bash process overhead", "No WSL helper is involved", "Frame output contains edited text; no compositor-present acknowledgement"]
    })).unwrap()).unwrap();
}
