// SPDX-License-Identifier: MPL-2.0
// SPDX-FileCopyrightText: 2026 Hakoniwa
use efude_canvas::{Document, composite};
use efude_core::InkPoint;
use std::hint::black_box;
use std::time::{Duration, Instant};

fn median_time(mut run: impl FnMut(), samples: usize) -> Duration {
    let mut times = Vec::with_capacity(samples);
    for _ in 0..samples {
        let start = Instant::now();
        run();
        times.push(start.elapsed());
    }
    times.sort_unstable();
    times[times.len() / 2]
}

fn main() {
    let packets: Vec<_> = (0..2048)
        .map(|i| {
            let x = i as f32 * 1.7;
            efude_input::PenPacket {
                x,
                y: (x * 0.013).sin() * 80.0 + 200.0,
                pressure: 0.25 + (i % 75) as f32 / 100.0,
                tilt_x: 0.2,
                tilt_y: -0.1,
                rotation: 0.3,
                time_ms: i as u64 * 2,
                received_at: std::time::Instant::now(),
            }
        })
        .collect();
    let input_dab_time = median_time(
        || {
            let mut normalizer = efude_input::InputNormalizer::default();
            normalizer.begin();
            let points = packets
                .iter()
                .map(|packet| {
                    let mut point = normalizer.pen_point(
                        packet.x,
                        packet.y,
                        packet.pressure,
                        packet.tilt_x,
                        packet.tilt_y,
                        packet.rotation,
                    );
                    point.time_ms = packet.time_ms;
                    point
                })
                .collect::<Vec<_>>();
            let stroke = efude_stroke::process_with_options(&points, 8, 1.5, 12.0, 0.4);
            let dabs = stroke
                .iter()
                .map(|point| {
                    (
                        [point.position.x, point.position.y],
                        4.0 * point.pressure,
                        point.pressure,
                    )
                })
                .collect::<Vec<_>>();
            black_box(dabs);
        },
        11,
    );
    let input_queue_time = median_time(
        || {
            let queue = efude_input::PenInputQueue::new(4096);
            for packet in &packets {
                queue.push(*packet);
            }
            let mut drained = Vec::with_capacity(packets.len());
            black_box(queue.drain_into(&mut drained, packets.len()));
            black_box(drained);
        },
        11,
    );

    let points: Vec<_> = (0..2048)
        .map(|i| {
            let x = i as f32 * 1.7;
            InkPoint::new(x, (x * 0.013).sin() * 80.0 + 200.0, 0.6, i as u64 * 2)
        })
        .collect();
    let stroke_time = median_time(
        || {
            black_box(efude_stroke::process_with_options(
                black_box(&points),
                8,
                1.5,
                12.0,
                0.4,
            ));
        },
        11,
    );

    let mut document = Document::new(768, 768);
    document.layers.clear();
    for layer_index in 0..4 {
        let mut layer = efude_canvas::Layer::new(
            layer_index as u64 + 1,
            format!("Benchmark {}", layer_index + 1),
            document.width,
            document.height,
        );
        for y in (layer_index * 31..768).step_by(31) {
            for x in (layer_index * 17..768).step_by(29) {
                layer.pixels.set_pixel(
                    x,
                    y,
                    [((x + layer_index) % 255) as u8, (y % 255) as u8, 160, 210],
                );
            }
        }
        document.layers.push(layer);
    }
    let composite_time = median_time(
        || {
            black_box(composite(black_box(&document)));
        },
        11,
    );

    println!(
        "[{{\"name\":\"Pen input queue ingest + frame drain (2048 packets)\",\"unit\":\"ns\",\"value\":{}}},{{\"name\":\"Input normalization + stroke + dab preparation (2048 packets)\",\"unit\":\"ns\",\"value\":{}}},{{\"name\":\"Stroke processing (2048 points)\",\"unit\":\"ns\",\"value\":{}}},{{\"name\":\"Layer compositing (768x768, 4 layers)\",\"unit\":\"ns\",\"value\":{}}}]",
        input_queue_time.as_nanos(),
        input_dab_time.as_nanos(),
        stroke_time.as_nanos(),
        composite_time.as_nanos()
    );
}
