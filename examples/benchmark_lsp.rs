use std::time::Instant;

fn main() {
    let directory = tempfile::tempdir().unwrap();
    let missing = directory.path().join("settings.json");
    let mut startup = Vec::with_capacity(1_000);
    for _ in 0..1_000 {
        let started = Instant::now();
        assert_eq!(
            editur::settings::load(&missing).unwrap(),
            editur::settings::Settings::default()
        );
        startup.push(started.elapsed());
    }
    startup.sort_unstable();
    println!(
        "missing settings load: {:.2?} median, {:.2?} p95",
        startup[startup.len() / 2],
        startup[startup.len() * 95 / 100],
    );

    let line = "pub fn value(input: usize) -> usize { input + 1 } // fixture\n";
    let mut current = line.repeat(1_048_576 / line.len() + 1);
    let mut sent = current.clone();
    let mut timings = Vec::with_capacity(40);
    for iteration in 0..40 {
        if iteration % 2 == 0 {
            current.insert(10, 'x');
        } else {
            current.remove(10);
        }
        let started = Instant::now();
        let change = editur::lsp::incremental_change(&sent, &current);
        timings.push(started.elapsed());
        let range = editur::lsp::byte_range_for_lsp_range(&sent, change.range.unwrap()).unwrap();
        let rebuilt = format!(
            "{}{}{}",
            &sent[..range.start],
            change.text,
            &sent[range.end..]
        );
        assert_eq!(rebuilt, current);
        sent.clone_from(&current);
    }
    timings.sort_unstable();
    println!(
        "lsp diff: {} bytes, {:.2?} median, {:.2?} p95",
        current.len(),
        timings[timings.len() / 2],
        timings[timings.len() * 95 / 100],
    );
}
