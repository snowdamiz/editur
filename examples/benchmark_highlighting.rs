use std::{path::Path, time::Instant};

use editur::syntax::{Highlighter, IncrementalHighlightCache, SyntaxManager};

fn main() -> Result<(), String> {
    let syntaxes = SyntaxManager::built_in()?;
    let syntax = syntaxes.detect(Path::new("fixture.rs"), false);
    let highlighter = Highlighter::new()?;
    let line = "pub fn value(input: usize) -> usize { input + 1 } // fixture\n";

    for (name, text) in [
        ("small", line.repeat(20)),
        ("10k-lines", line.repeat(10_000)),
        ("1-mib", line.repeat(1_048_576 / line.len() + 1)),
    ] {
        let mut text = text;
        let mut cache = IncrementalHighlightCache::default();
        let started = Instant::now();
        let job = highlighter.highlight_job_incremental(
            &text,
            syntax,
            syntaxes.set(),
            900.0,
            &mut cache,
        )?;
        if job.text.len() != text.len() {
            return Err(format!("{name}: highlighted output changed the text"));
        }
        let initial = started.elapsed();
        let mut edits = Vec::with_capacity(20);
        for iteration in 0..20 {
            if iteration % 2 == 0 {
                text.insert(10, 'x');
            } else {
                text.remove(10);
            }
            let started = Instant::now();
            highlighter.highlight_job_incremental(
                &text,
                syntax,
                syntaxes.set(),
                900.0,
                &mut cache,
            )?;
            edits.push(started.elapsed());
        }
        edits.sort_unstable();
        println!(
            "{name}: {} bytes, {initial:.2?} initial, {:.2?} median edit, {:.2?} p95 edit",
            text.len(),
            edits[edits.len() / 2],
            edits[edits.len() * 95 / 100],
        );
    }
    Ok(())
}
