// Keyframe track sampling. Declares the track buffer (binding 9 of the simulation layout).

import package::gpu::shaders::types::Particle;

// Keyframe tracks: a header record, then one record per key. Must match `GpuTrackRecord`
// in src/track.rs.
//   header: data.x = period, info = (key count, clock + 2 * repeat, slot, width)
//   key:    data = value, info = (bitcast time, ease, 0, 0)
struct TrackRecord {
    data: vec4<f32>,
    info: vec4<u32>,
}

@group(0) @binding(9) var<storage, read> tracks: array<TrackRecord>;

const CLOCK_SECONDS: u32 = 1u;
const REPEAT_LOOP: u32 = 1u;
const REPEAT_PING_PONG: u32 = 2u;
const EASE_SMOOTH: u32 = 1u;
const EASE_STEP: u32 = 2u;

// Fraction of life used; 0 for immortal particles.
fn life_fraction(p: Particle) -> f32 {
    if p.lifetime > 0.0 {
        return saturate(p.age / p.lifetime);
    }
    return 0.0;
}

// Must match `Track::sample` in src/track.rs.
fn sample_track(header: u32, p: Particle) -> vec4<f32> {
    let period = tracks[header].data.x;
    let count = tracks[header].info.x;
    let clock = tracks[header].info.y & 1u;
    let repeat = tracks[header].info.y >> 1u;

    var t = life_fraction(p);
    if clock == CLOCK_SECONDS {
        t = p.age;
    }
    if period > 0.0 {
        if repeat == REPEAT_LOOP {
            t -= period * floor(t / period);
        } else if repeat == REPEAT_PING_PONG {
            let cycle = 2.0 * period;
            t = period - abs(t - cycle * floor(t / cycle) - period);
        }
    }

    // `next` = the first key after t.
    let first = header + 1u;
    var next = 0u;
    while next < count && bitcast<f32>(tracks[first + next].info.x) <= t {
        next++;
    }
    if next == 0u {
        return tracks[first].data;
    }
    let from_key = tracks[first + next - 1u];
    if next == count {
        return from_key.data;
    }
    let to_key = tracks[first + next];
    let from_time = bitcast<f32>(from_key.info.x);
    var x = (t - from_time) / (bitcast<f32>(to_key.info.x) - from_time);
    if from_key.info.y == EASE_SMOOTH {
        x = x * x * (3.0 - 2.0 * x);
    } else if from_key.info.y == EASE_STEP {
        x = 0.0;
    }
    return mix(from_key.data, to_key.data, x);
}
