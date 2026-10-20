use std::collections::HashMap;
use std::time::Duration;

use e2e::config::Capacity as Limits;

use super::probe::{
    Process, System, parse_iostat, parse_meminfo, parse_pid_stat, parse_proc_stat, parse_ps, usage,
};
use super::{Advice, Capacity, Detector, Limit, Sample};
use crate::load::metrics::Mark;

/// Second `t` with the machine's CPU at `cpu`%.
fn cpu(t: u64, cpu: f64) -> Sample {
    Sample {
        t,
        users: u32::try_from(t).unwrap(),
        browsers: 1,
        cpu: Some(cpu),
        memory_free: Some(50.0),
        lag_p99: Some(1.0),
        ..Sample::default()
    }
}

/// The marks the detector gives `samples`.
fn marks(samples: &[Sample]) -> Vec<Mark> {
    let mut detector = Detector::new(Limits::default());
    samples.iter().map(|s| detector.see(s).mark).collect()
}

#[test]
fn five_seconds_in_a_row_past_a_limit_is_saturated() {
    // Four seconds over, one under: never saturated.
    let flicker: Vec<Sample> = [95.0, 96.0, 97.0, 95.0, 50.0, 95.0]
        .iter()
        .enumerate()
        .map(|(i, c)| cpu(u64::try_from(i).unwrap() + 1, *c))
        .collect();
    assert_eq!(
        marks(&flicker),
        [
            Mark::Over,
            Mark::Over,
            Mark::Over,
            Mark::Over,
            Mark::Clean,
            Mark::Over
        ]
    );
    // From second 3 on: saturated at its fifth second, since the third.
    let load: Vec<Sample> = [50.0, 60.0, 92.0, 95.0, 99.0, 97.0, 93.0, 98.0, 40.0]
        .iter()
        .enumerate()
        .map(|(i, c)| cpu(u64::try_from(i).unwrap() + 1, *c))
        .collect();
    let mut detector = Detector::new(Limits::default());
    let seen: Vec<Mark> = load.iter().map(|s| detector.see(s).mark).collect();
    assert_eq!(
        seen,
        [
            Mark::Clean,
            Mark::Clean,
            Mark::Over,
            Mark::Over,
            Mark::Over,
            Mark::Over,
            Mark::Began(3),
            Mark::Saturated,
            Mark::Clean
        ]
    );
    assert!(!detector.saturated());
    let span = &detector.spans()[0];
    assert_eq!((span.from_s, span.to_s, span.limit.as_str()), (3, 8, "cpu"));
    assert!((span.value - 98.0).abs() < 1e-9, "{}", span.value);
}

#[test]
fn memory_and_lag_are_limits_too() {
    let detector = Detector::new(Limits::default());
    let low = Sample {
        memory_free: Some(4.0),
        ..cpu(1, 10.0)
    };
    assert_eq!(detector.over(&low).unwrap().limit, Limit::Memory);
    let late = Sample {
        lag_p99: Some(80.0),
        ..cpu(1, 10.0)
    };
    let over = detector.over(&late).unwrap();
    assert_eq!(over.limit, Limit::Lag);
    assert_eq!(over.against(&Limits::default()), "lag 80 ms (limit 50 ms)");
    // What cannot be read crosses nothing.
    assert!(detector.over(&Sample::default()).is_none());
    // The CPU is said first.
    let all = Sample {
        cpu: Some(97.0),
        memory_free: Some(4.0),
        lag_p99: Some(80.0),
        ..Sample::default()
    };
    assert_eq!(
        detector.over(&all).unwrap().against(&Limits::default()),
        "cpu 97% (limit 90%)"
    );
    // A limit of the config's own.
    let strict = Detector::new(Limits {
        cpu: 50,
        ..Limits::default()
    });
    assert!(strict.over(&cpu(1, 60.0)).is_some());
}

#[test]
fn the_summary_says_what_the_machine_held() {
    let capacity = Capacity::new(Limits::default());
    for t in 1..=12u64 {
        capacity.late(Duration::from_millis(2));
        if t == 3 {
            capacity.page(Some(12.0));
        }
        let users = u32::try_from(t * 10).unwrap();
        let system = System {
            cpu: Some(if t >= 8 {
                97.0
            } else {
                40.0 + 5.0 * f64::from(users / 10)
            }),
            memory_free: Some(60.0),
            chrome_cpu: Some(f64::from(users) / 10.0),
            chrome_rss: Some(u64::from(users) * 100),
            own_cpu: Some(0.1),
            own_rss: Some(50),
            load: Some(3.0),
        };
        capacity.second(t, users, users.div_ceil(10), system);
    }
    let summary = capacity.summary(12, 120);
    assert_eq!(summary.saturations.len(), 1);
    assert_eq!((summary.saturations[0].from_s, summary.saturated_s), (8, 5));
    // Before second 8: 70 users in 7 Chromes.
    assert_eq!((summary.held_users, summary.held_per_browser), (70, 10));
    assert_eq!(summary.peak_users, 120);
    assert_eq!(summary.cpu_max, Some(97.0));
    assert_eq!(summary.chrome_rss_max, Some(12_000));
    assert_eq!(summary.page_lag_p99_ms, Some(12.0));
    assert!(
        (summary.lag_p99_ms - 2.0).abs() < 0.1,
        "{}",
        summary.lag_p99_ms
    );
    assert_eq!(capacity.samples()[0].lag_p99, Some(2.0));
    // A run that never saturated held its peak.
    let calm = Capacity::new(Limits::default());
    calm.second(1, 30, 3, System::default());
    let summary = calm.summary(1, 40);
    assert_eq!((summary.held_users, summary.saturated_s), (40, 0));
}

#[test]
fn the_advice_names_the_next_step() {
    let open = Advice {
        held: 120,
        peak: 200,
        rate: Some(20.0),
        third_party_blocked: false,
        browser: "chrome-headless-shell",
    };
    assert_eq!(
        open.text(),
        "lower the rate to about 10.8/s, make each user cheaper with [load] block = \
         [\"third-party\"], or run the profile on a bigger machine"
    );
    let closed = Advice {
        rate: None,
        third_party_blocked: true,
        browser: "chrome",
        ..open
    };
    assert_eq!(
        closed.text(),
        "run at most 108 users, run the users in chrome-headless-shell (chrome-cdp install \
         --headless-shell), or run the profile on a bigger machine"
    );
}

#[test]
fn the_machine_is_read_from_proc_iostat_and_ps() {
    assert_eq!(
        parse_proc_stat("cpu  100 0 50 800 50 0 0 0 0 0\ncpu0 1 2 3\n"),
        Some((150, 1000))
    );
    assert_eq!(
        parse_meminfo("MemTotal: 1000 kB\nMemFree: 10 kB\nMemAvailable: 250 kB\n"),
        Some(25.0)
    );
    // The command may hold spaces and parentheses.
    let stat = "42 (Web Content (x)) S 7 42 42 0 -1 0 0 0 0 0 150 50 0 0 20 0 1 0 1 1000 300 0";
    assert_eq!(parse_pid_stat(stat), Some((7, 200, 300)));
    assert_eq!(
        parse_iostat("  9  7 84  5.68 4.48 7.57"),
        Some((16.0, 5.68))
    );
    assert_eq!(parse_iostat(" us sy id   1m   5m   15m"), None);
    let ps = parse_ps("  1     0  26912 224:36.75\n 539  1 57520 1-01:02:03\nbad\n");
    assert_eq!(ps.len(), 2);
    assert!((ps[0].cpu - 13_476.75).abs() < 1e-6);
    assert!((ps[1].cpu - 90_123.0).abs() < 1e-6);
    assert_eq!(ps[1].rss, 57_520 * 1024);
}

#[test]
fn a_chromes_tree_is_its_processes_and_theirs() {
    let p = |pid, ppid, rss, cpu| Process {
        pid,
        ppid,
        rss,
        cpu,
    };
    let mut seen = HashMap::new();
    let first = [
        p(1, 0, 10, 100.0),
        p(10, 1, 50, 2.0),   // chrome-cdp
        p(20, 10, 100, 5.0), // a Chrome
        p(21, 20, 200, 1.0), // its renderer
        p(22, 21, 300, 1.0), // and below that
        p(30, 10, 999, 9.0), // a web server: not a Chrome
    ];
    let used = usage(&first, &[20], 10, &mut seen);
    assert_eq!(used.chrome_rss, 600);
    assert_eq!(used.own_rss, Some(50));
    // The first read counts all their time; the next only what grew.
    assert!((used.chrome_cpu - 7.0).abs() < 1e-9);
    let next = [
        p(10, 1, 50, 2.5),
        p(20, 10, 100, 5.5),
        p(21, 20, 200, 2.0),
        p(23, 20, 10, 0.25),
    ];
    let used = usage(&next, &[20], 10, &mut seen);
    assert!((used.chrome_cpu - 1.75).abs() < 1e-9, "{}", used.chrome_cpu);
    assert!((used.own_cpu - 0.5).abs() < 1e-9);
    assert!(!seen.contains_key(&22));
}
