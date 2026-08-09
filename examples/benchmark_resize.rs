use std::time::{Duration, Instant};

use editur::editor_surface::EditorSurface;
use egui::{Color32, FontId, RawInput, Rect, Vec2, epaint::text::LayoutJob, pos2};

const RESIZE_FRAMES: usize = 60;

fn percentile(samples: &mut [Duration], percentile: usize) -> Duration {
    samples.sort_unstable();
    samples[samples.len() * percentile / 100]
}

fn main() {
    let tokens = [
        "pub fn ",
        "value",
        "(input: usize) -> usize { input + 1 } ",
        "// resize fixture\n",
    ];
    let line_len = tokens.iter().map(|token| token.len()).sum::<usize>();
    let mut job = LayoutJob::default();
    for _ in 0..(1_048_576 / line_len + 1) {
        for (index, token) in tokens.into_iter().enumerate() {
            job.append(
                token,
                0.0,
                egui::TextFormat {
                    font_id: FontId::monospace(14.0),
                    color: Color32::from_gray(170 + index as u8 * 20),
                    ..egui::TextFormat::default()
                },
            );
        }
    }
    let mut text = job.text.clone();
    let context = egui::Context::default();
    let mut editor = EditorSurface::default();

    let mut frame_times = Vec::with_capacity(RESIZE_FRAMES);
    let total_started = Instant::now();
    for frame in 0..RESIZE_FRAMES {
        let width = 800.0 + frame as f32 * 8.0;
        let started = Instant::now();
        let _ = context.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(width, 760.0))),
                ..RawInput::default()
            },
            |ui| {
                editor.show(ui, &mut text, &job, 1, false, None);
            },
        );
        frame_times.push(started.elapsed());
    }
    let total = total_started.elapsed();
    let first = frame_times[0];
    let warm = frame_times[RESIZE_FRAMES - 1];
    let median = percentile(&mut frame_times, 50);
    let p95 = percentile(&mut frame_times, 95);

    println!(
        "resize: {} bytes, {RESIZE_FRAMES} frames, {total:.2?} total, {first:.2?} first, {warm:.2?} warm, {median:.2?} median, {p95:.2?} p95",
        text.len()
    );
}
